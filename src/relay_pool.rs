// ABOUTME: Durable Object that maintains persistent websocket connections to Nostr relay
// ABOUTME: Handles query execution, request coalescing, and connection management

use crate::filter::Filter;
use futures_util::StreamExt;
use serde::Deserialize;
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
        let filter: Filter = req.json().await?;
        let events = self.query_relay(&filter).await?;
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

    async fn query_relay(&self, filter: &Filter) -> Result<Vec<serde_json::Value>> {
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

        // Send REQ message
        let req_msg = serde_json::json!(["REQ", sub_id, filter]);
        ws.send_with_str(&req_msg.to_string())?;

        let mut events = Vec::new();
        let limit = filter.limit.unwrap_or(100);
        let start = js_sys::Date::now();
        let timeout_ms = 5000.0; // 5 second max
        let idle_timeout_ms = 300.0; // 300ms idle timeout
        let mut last_event_time = start;

        // Collect events until done
        loop {
            let now = js_sys::Date::now();

            // Check timeouts
            if now - start > timeout_ms {
                break; // Max timeout
            }
            if !events.is_empty() && now - last_event_time > idle_timeout_ms {
                break; // Idle timeout after first event
            }
            if events.is_empty() && now - start > 1000.0 {
                break; // 1s timeout for empty results
            }
            if events.len() >= limit {
                break; // Limit reached
            }

            // Try to receive next event
            match event_stream.next().await {
                Some(Ok(WebsocketEvent::Message(msg))) => {
                    if let Some(text) = msg.text() {
                        if let Ok(parsed) = serde_json::from_str::<Vec<serde_json::Value>>(&text) {
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

        // Send CLOSE
        let close_msg = serde_json::json!(["CLOSE", sub_id]);
        let _ = ws.send_with_str(&close_msg.to_string());

        Ok(events)
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
        let filter = Filter {
            ids: Some(vec![event_id.to_string()]),
            limit: Some(1),
            ..Default::default()
        };

        let events = self.query_relay(&filter).await?;
        Ok(!events.is_empty())
    }
}

#[derive(Deserialize)]
struct VerifyRequest {
    event_id: String,
}
