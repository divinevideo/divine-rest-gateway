// ABOUTME: Durable Object that maintains persistent websocket connections to Nostr relay
// ABOUTME: Handles query execution, request coalescing, and connection management

use futures_util::StreamExt;
use serde::Deserialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use worker::*;

#[durable_object]
pub struct RelayPool {
    state: State,
    env: Env,
    relay_url: Option<String>,
}

impl DurableObject for RelayPool {
    fn new(state: State, env: Env) -> Self {
        Self {
            state,
            env,
            relay_url: None,
        }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        let url = req.url()?;
        let path = url.path();

        match path {
            "/query" => self.handle_query(req).await,
            "/publish" => self.handle_publish(req).await,
            "/verify" => self.handle_verify(req).await,
            _ => Response::error("not found", 404),
        }
    }
}

impl RelayPool {
    fn get_relay_url(&self) -> String {
        self.relay_url
            .clone()
            .or_else(|| self.env.var("RELAY_URL").ok().map(|v| v.to_string()))
            .unwrap_or_else(|| "wss://relay.damus.io".to_string())
    }

    async fn handle_query(&self, mut req: Request) -> Result<Response> {
        // Get raw filter string - pass directly to relay without parsing
        let filter_str = req.text().await?;
        let events = self.query_relay_raw(&filter_str).await?;
        Response::from_json(&events)
    }

    async fn handle_publish(&self, mut req: Request) -> Result<Response> {
        let event: serde_json::Value = req.json().await?;
        let success = self.publish_to_relay(&event).await?;
        Response::from_json(&serde_json::json!({ "ok": success }))
    }

    async fn handle_verify(&self, mut req: Request) -> Result<Response> {
        let body: VerifyRequest = req.json().await?;
        let found = self.verify_event(&body.event_id).await?;
        Response::from_json(&serde_json::json!({ "found": found }))
    }

    /// Query relay with raw filter string - NO PARSING, preserves ALL fields
    async fn query_relay_raw(&self, filter_json: &str) -> Result<Vec<serde_json::Value>> {
        let relay_url = self.get_relay_url();

        // Parse URL for WebSocket connection
        let url = relay_url.parse().map_err(|_| "Invalid relay URL")?;

        // Create websocket connection
        let ws = WebSocket::connect(url).await?;
        ws.accept()?;

        // Create event stream
        let mut event_stream = ws.events()?;

        // Generate subscription ID
        let sub_id = format!("q{}", js_sys::Date::now() as u64);

        // Send REQ message - embed raw filter string directly into JSON array
        let req_msg = format!(r#"["REQ","{}",{}]"#, sub_id, filter_json);
        ws.send_with_str(&req_msg)?;

        let mut events = Vec::new();
        let limit = 500; // Max events to collect before giving up
        let start = js_sys::Date::now();
        let max_timeout_ms = 5000.0; // 5 second max
        let idle_timeout_ms = 300.0; // 300ms idle timeout
        let empty_timeout_ms = 1000.0; // 1s timeout for empty results
        let mut last_event_time = start;

        // Collect events until done
        loop {
            let now = js_sys::Date::now();
            let elapsed = now - start;

            // Check timeouts BEFORE waiting
            if elapsed > max_timeout_ms {
                break; // Max timeout
            }
            if !events.is_empty() && (now - last_event_time) > idle_timeout_ms {
                break; // Idle timeout after first event
            }
            if events.is_empty() && elapsed > empty_timeout_ms {
                break; // 1s timeout for empty results
            }
            if events.len() >= limit {
                break; // Limit reached
            }

            // Calculate remaining time for this iteration
            let remaining = if events.is_empty() {
                empty_timeout_ms - elapsed
            } else {
                idle_timeout_ms.min(max_timeout_ms - elapsed)
            };

            if remaining <= 0.0 {
                break;
            }

            // Race between next message and timeout
            let next_msg = event_stream.next();
            let timeout = Self::sleep_ms(remaining.min(500.0) as u32); // Check every 500ms max

            // Use select to race timeout vs message
            let result = futures_util::future::select(
                Box::pin(next_msg),
                Box::pin(timeout),
            )
            .await;

            match result {
                futures_util::future::Either::Left((msg_result, _)) => {
                    // Got a message
                    match msg_result {
                        Some(Ok(WebsocketEvent::Message(msg))) => {
                            if let Some(text) = msg.text() {
                                if let Ok(parsed) =
                                    serde_json::from_str::<Vec<serde_json::Value>>(&text)
                                {
                                    if parsed.len() >= 2 {
                                        match parsed[0].as_str() {
                                            Some("EVENT") if parsed.len() >= 3 => {
                                                events.push(parsed[2].clone());
                                                last_event_time = js_sys::Date::now();
                                            }
                                            Some("EOSE") => break,
                                            Some("NOTICE") => {
                                                console_log!("Relay notice: {:?}", parsed.get(1));
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                        Some(Ok(WebsocketEvent::Close(_))) => break,
                        Some(Err(_)) => break,
                        None => break,
                    }
                }
                futures_util::future::Either::Right((_, _)) => {
                    // Timeout - continue loop to re-check timeouts
                    continue;
                }
            }
        }

        // Send CLOSE
        let close_msg = serde_json::json!(["CLOSE", sub_id]);
        let _ = ws.send_with_str(&close_msg.to_string());

        Ok(events)
    }

    /// Sleep for specified milliseconds using JS setTimeout
    async fn sleep_ms(ms: u32) {
        let promise = js_sys::Promise::new(&mut |resolve, _| {
            let global = js_sys::global();
            if let Ok(set_timeout) = js_sys::Reflect::get(&global, &JsValue::from_str("setTimeout")) {
                if let Ok(set_timeout_fn) = set_timeout.dyn_into::<js_sys::Function>() {
                    let _ = set_timeout_fn.call2(&JsValue::NULL, &resolve, &JsValue::from(ms));
                }
            }
        });
        let _ = JsFuture::from(promise).await;
    }

    async fn publish_to_relay(&self, event: &serde_json::Value) -> Result<bool> {
        let relay_url = self.get_relay_url();

        // Parse URL for WebSocket connection
        let url = relay_url.parse().map_err(|_| "Invalid relay URL")?;

        let ws = WebSocket::connect(url).await?;
        ws.accept()?;

        let mut event_stream = ws.events()?;

        // Send EVENT message
        let event_msg = serde_json::json!(["EVENT", event]);
        ws.send_with_str(&event_msg.to_string())?;

        // Wait for OK response
        let start = js_sys::Date::now();
        let timeout_ms = 3000.0;

        loop {
            if js_sys::Date::now() - start > timeout_ms {
                return Ok(false);
            }

            match event_stream.next().await {
                Some(Ok(WebsocketEvent::Message(msg))) => {
                    if let Some(text) = msg.text() {
                        if let Ok(parsed) = serde_json::from_str::<Vec<serde_json::Value>>(&text) {
                            if parsed.get(0).and_then(|v| v.as_str()) == Some("OK") {
                                let accepted = parsed.get(2).and_then(|v| v.as_bool()).unwrap_or(false);
                                return Ok(accepted);
                            }
                        }
                    }
                }
                Some(Ok(WebsocketEvent::Close(_))) | Some(Err(_)) | None => return Ok(false),
            }
        }
    }

    async fn verify_event(&self, event_id: &str) -> Result<bool> {
        let filter = format!(r#"{{"ids":["{}"],"limit":1}}"#, event_id);
        let events = self.query_relay_raw(&filter).await?;
        Ok(!events.is_empty())
    }
}

#[derive(Deserialize)]
struct VerifyRequest {
    event_id: String,
}
