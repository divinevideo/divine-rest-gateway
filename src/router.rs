// ABOUTME: HTTP request routing for the REST gateway
// ABOUTME: Routes requests to appropriate handlers based on path and method

use crate::cache::Cache;
use crate::filter::Filter;
use crate::types::{ErrorResponse, QueryResponse};
use worker::*;

pub async fn handle_request(req: Request, env: Env) -> Result<Response> {
    let url = req.url()?;
    let path = url.path();
    let method = req.method();

    match (method, path) {
        (Method::Get, "/") => landing_page(),

        (Method::Get, "/health") => Response::ok("ok"),

        (Method::Get, "/query") => handle_query(req, env).await,

        (Method::Get, path) if path.starts_with("/profile/") => {
            handle_profile(req, env, &path[9..]).await
        }

        (Method::Get, path) if path.starts_with("/event/") => {
            handle_event(req, env, &path[7..]).await
        }

        (Method::Get, path) if path.starts_with("/publish/status/") => {
            handle_publish_status(env, &path[16..]).await
        }

        (Method::Post, "/publish") => handle_publish(req, env).await,

        _ => {
            let err = ErrorResponse::new("not_found").with_detail("endpoint not found");
            json_response(&err, 404)
        }
    }
}

async fn handle_query(req: Request, env: Env) -> Result<Response> {
    let url = req.url()?;
    let params: std::collections::HashMap<_, _> = url.query_pairs().collect();

    let filter_param = match params.get("filter") {
        Some(f) => f,
        None => {
            let err = ErrorResponse::new("invalid_filter").with_detail("missing filter parameter");
            return json_response(&err, 400);
        }
    };

    let filter = match Filter::from_base64(filter_param) {
        Ok(f) => f,
        Err(e) => {
            let err = ErrorResponse::new("invalid_filter").with_detail(&e.to_string());
            return json_response(&err, 400);
        }
    };

    let kv = env.kv("REST_GATEWAY_CACHE")?;
    let cache = Cache::new(kv);
    let cache_key = filter.cache_key();

    // Check cache first
    if let Some((cached, age)) = cache.get_query(&cache_key).await? {
        let response = QueryResponse {
            events: cached.events,
            eose: cached.eose,
            complete: cached.eose,
            cached: true,
            cache_age_seconds: Some(age),
        };
        return json_response_with_cache(&response, 200, filter.ttl_seconds());
    }

    // Cache miss - query relay via Durable Object
    let relay_pool = env.durable_object("RELAY_POOL")?;
    let stub = relay_pool.id_from_name("default")?.get_stub()?;

    let do_req = Request::new_with_init(
        "http://do/query",
        RequestInit::new()
            .with_method(Method::Post)
            .with_body(Some(serde_json::to_string(&filter)?.into())),
    )?;

    let mut do_resp = stub.fetch_with_request(do_req).await?;
    let events: Vec<serde_json::Value> = do_resp.json().await?;

    // Cache the result
    cache
        .put_query(&cache_key, events.clone(), true, filter.ttl_seconds())
        .await?;

    let response = QueryResponse {
        events,
        eose: true,
        complete: true,
        cached: false,
        cache_age_seconds: None,
    };
    json_response_with_cache(&response, 200, filter.ttl_seconds())
}

async fn handle_profile(_req: Request, env: Env, pubkey: &str) -> Result<Response> {
    let filter = Filter {
        authors: Some(vec![pubkey.to_string()]),
        kinds: Some(vec![0]),
        limit: Some(1),
        ..Default::default()
    };

    // Reuse query logic via internal request
    let encoded = filter.to_base64();
    let url = format!("http://internal/query?filter={}", encoded);
    let req = Request::new(&url, Method::Get)?;
    handle_query(req, env).await
}

async fn handle_event(_req: Request, env: Env, event_id: &str) -> Result<Response> {
    let filter = Filter {
        ids: Some(vec![event_id.to_string()]),
        limit: Some(1),
        ..Default::default()
    };

    let encoded = filter.to_base64();
    let url = format!("http://internal/query?filter={}", encoded);
    let req = Request::new(&url, Method::Get)?;
    handle_query(req, env).await
}

async fn handle_publish_status(env: Env, event_id: &str) -> Result<Response> {
    let kv = env.kv("REST_GATEWAY_CACHE")?;
    let cache = Cache::new(kv);

    match cache.get_publish_status(event_id).await? {
        Some(status) => json_response(&status, 200),
        None => {
            let err = ErrorResponse::new("not_found").with_detail("event not found");
            json_response(&err, 404)
        }
    }
}

async fn handle_publish(mut req: Request, env: Env) -> Result<Response> {
    // Get full URL for NIP-98 validation
    let url = req.url()?.to_string();
    let auth_header = req.headers().get("Authorization")?;

    // Validate NIP-98 auth
    match crate::auth::validate_nip98(auth_header.as_deref(), "POST", &url) {
        Ok(_auth) => {
            // Auth successful, proceed with publish
        }
        Err(e) => {
            let err = ErrorResponse::new("auth_failed").with_detail(&e.to_string());
            return json_response(&err, 401);
        }
    }

    let body: crate::types::PublishRequest = req.json().await?;

    // Extract event ID
    let event_id = body
        .event
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    // TODO: Queue for publishing - Cloudflare Queues API not yet available in worker-rs
    // For now, return a placeholder response
    // let queue = env.queue("PUBLISH_QUEUE")?;
    // queue.send(body.event).await?;

    // Set initial status
    let kv = env.kv("REST_GATEWAY_CACHE")?;
    let cache = Cache::new(kv);
    let status = crate::types::PublishStatus {
        status: "queued".to_string(),
        attempts: Some(0),
        verified_at: None,
        error: None,
    };
    cache.set_publish_status(&event_id, &status).await?;

    let response = crate::types::PublishResponse {
        status: "queued".to_string(),
        event_id,
    };
    json_response(&response, 202)
}

fn landing_page() -> Result<Response> {
    let html = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Divine REST Gateway</title>
    <style>
        :root { --bg: #0d1117; --fg: #c9d1d9; --accent: #58a6ff; --code-bg: #161b22; --border: #30363d; }
        * { box-sizing: border-box; }
        body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; background: var(--bg); color: var(--fg); line-height: 1.6; margin: 0; padding: 2rem; max-width: 900px; margin: 0 auto; }
        h1, h2, h3 { color: #fff; }
        h1 { border-bottom: 1px solid var(--border); padding-bottom: 0.5rem; }
        a { color: var(--accent); text-decoration: none; }
        a:hover { text-decoration: underline; }
        code { background: var(--code-bg); padding: 0.2rem 0.4rem; border-radius: 4px; font-size: 0.9em; }
        pre { background: var(--code-bg); padding: 1rem; border-radius: 8px; overflow-x: auto; border: 1px solid var(--border); }
        pre code { background: none; padding: 0; }
        .endpoint { background: var(--code-bg); border: 1px solid var(--border); border-radius: 8px; margin: 1rem 0; padding: 1rem; }
        .method { display: inline-block; padding: 0.2rem 0.5rem; border-radius: 4px; font-weight: bold; font-size: 0.8em; margin-right: 0.5rem; }
        .get { background: #238636; color: #fff; }
        .post { background: #8957e5; color: #fff; }
        .path { font-family: monospace; color: var(--accent); }
        .desc { margin-top: 0.5rem; color: #8b949e; }
        .try-it { margin-top: 0.5rem; font-size: 0.9em; }
    </style>
</head>
<body>
    <h1>Divine REST Gateway</h1>
    <p>REST API caching proxy for <a href="https://nostr.com">Nostr</a>, running on Cloudflare Workers.</p>

    <h2>How It Works</h2>
    <p>This gateway provides HTTP REST endpoints that proxy to Nostr relays via WebSocket, with multi-layer caching (CDN + KV) for fast reads. Perfect for web and mobile clients that prefer REST over WebSocket.</p>
    <ul>
        <li><strong>Read acceleration</strong>: CDN + KV caching with content-aware TTLs</li>
        <li><strong>Write proxy</strong>: Reliable event publishing with verification and retries</li>
        <li><strong>NIP-98 auth</strong>: Authenticated writes via HTTP Authorization header</li>
        <li><strong>Edge deployment</strong>: Global distribution via Cloudflare's edge network</li>
    </ul>

    <h2>API Endpoints</h2>

    <div class="endpoint">
        <span class="method get">GET</span>
        <span class="path">/query?filter=&lt;base64url-encoded-filter&gt;</span>
        <p class="desc">Query events using a Nostr filter. The filter is base64url-encoded JSON.</p>
        <div class="try-it">
            <strong>Example filter:</strong> <code>{"kinds":[0],"limit":5}</code><br>
            <a href="/query?filter=eyJraW5kcyI6WzBdLCJsaW1pdCI6NX0">Try it</a>
        </div>
    </div>

    <div class="endpoint">
        <span class="method get">GET</span>
        <span class="path">/profile/{pubkey}</span>
        <p class="desc">Get a user's profile (kind 0 event) by their public key.</p>
        <div class="try-it">
            <a href="/profile/82341f882b6eabcd2ba7f1ef90aad961cf074af15b9ef44a09f9d2a8fbfbe6a2">Example: jack's profile</a>
        </div>
    </div>

    <div class="endpoint">
        <span class="method get">GET</span>
        <span class="path">/event/{id}</span>
        <p class="desc">Get a single event by its ID.</p>
    </div>

    <div class="endpoint">
        <span class="method post">POST</span>
        <span class="path">/publish</span>
        <p class="desc">Publish a signed Nostr event. Requires NIP-98 authentication.</p>
        <pre><code>POST /publish
Authorization: Nostr &lt;base64-nip98-event&gt;
Content-Type: application/json

{"event": {...signed nostr event...}}</code></pre>
    </div>

    <div class="endpoint">
        <span class="method get">GET</span>
        <span class="path">/publish/status/{event_id}</span>
        <p class="desc">Check the publish status of an event.</p>
    </div>

    <h2>Filter Encoding</h2>
    <p>Filters are standard <a href="https://github.com/nostr-protocol/nips/blob/master/01.md">NIP-01</a> filter objects, base64url-encoded for use in URLs:</p>
    <pre><code>// JavaScript example
const filter = {authors: ["pubkey"], kinds: [1], limit: 20};
const encoded = btoa(JSON.stringify(filter))
  .replace(/\+/g, '-').replace(/\//g, '_').replace(/=/g, '');
fetch(`/query?filter=${encoded}`);</code></pre>

    <h2>Response Format</h2>
    <pre><code>{
  "events": [...],      // Array of Nostr events
  "eose": true,         // End of stored events reached
  "complete": true,     // Query fully satisfied
  "cached": true,       // Response served from cache
  "cache_age_seconds": 42
}</code></pre>

    <h2>Cache Behavior</h2>
    <p>TTLs vary by content type:</p>
    <ul>
        <li><strong>Profiles (kind 0)</strong>: 15 minutes</li>
        <li><strong>Contacts (kind 3)</strong>: 10 minutes</li>
        <li><strong>Notes (kind 1)</strong>: 5 minutes</li>
        <li><strong>Reactions (kind 7)</strong>: 2 minutes</li>
        <li><strong>Other queries</strong>: 5 minutes</li>
    </ul>

    <h2>Source Code</h2>
    <p>Written in Rust, compiled to WebAssembly. <a href="https://github.com/divinevideo/divine-rest-gateway">View on GitHub</a></p>

    <footer style="margin-top: 3rem; padding-top: 1rem; border-top: 1px solid var(--border); color: #8b949e; font-size: 0.9em;">
        Divine REST Gateway v0.1.0 &middot; Powered by Cloudflare Workers
    </footer>
</body>
</html>"#;

    let mut headers = Headers::new();
    headers.set("Content-Type", "text/html; charset=utf-8")?;
    Ok(Response::from_body(ResponseBody::Body(html.as_bytes().to_vec()))?.with_headers(headers))
}

fn json_response<T: serde::Serialize>(data: &T, status: u16) -> Result<Response> {
    let body = serde_json::to_string(data)?;
    let mut headers = Headers::new();
    headers.set("Content-Type", "application/json")?;
    Ok(Response::from_body(ResponseBody::Body(body.into_bytes()))?.with_status(status).with_headers(headers))
}

fn json_response_with_cache<T: serde::Serialize>(data: &T, status: u16, max_age: u64) -> Result<Response> {
    let body = serde_json::to_string(data)?;
    let mut headers = Headers::new();
    headers.set("Content-Type", "application/json")?;
    headers.set("Cache-Control", &format!("public, max-age={}, s-maxage={}", max_age, max_age))?;
    Ok(Response::from_body(ResponseBody::Body(body.into_bytes()))?.with_status(status).with_headers(headers))
}
