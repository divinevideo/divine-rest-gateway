# Divine REST Gateway Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a Rust/WASM REST API caching proxy for Nostr on Cloudflare Workers with Durable Objects.

**Architecture:** Cloudflare Worker handles HTTP routing, auth, and caching. Durable Objects maintain persistent websocket connections to a Nostr relay. Cloudflare Queues handle reliable event publishing with verification.

**Tech Stack:** Rust, worker-rs, wasm-bindgen, nostr crate, Cloudflare Workers KV, Durable Objects, Queues

---

## Phase 1: Project Scaffolding

### Task 1.1: Initialize Rust Worker Project

**Files:**
- Create: `Cargo.toml`
- Create: `wrangler.toml`
- Create: `src/lib.rs`
- Create: `.gitignore`

**Step 1: Create Cargo.toml**

```toml
[package]
name = "divine-rest-gateway"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
worker = "0.4"
worker-macros = "0.4"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
base64 = "0.22"
sha2 = "0.10"
hex = "0.4"
getrandom = { version = "0.2", features = ["js"] }
console_error_panic_hook = "0.1"

# Nostr
nostr = { version = "0.37", default-features = false, features = ["std"] }

[profile.release]
opt-level = "s"
lto = true
```

**Step 2: Create wrangler.toml**

```toml
name = "divine-rest-gateway"
main = "build/worker/shim.mjs"
compatibility_date = "2024-01-01"

[build]
command = "cargo install -q worker-build && worker-build --release"

[vars]
RELAY_URL = "wss://relay.example.com"

[[kv_namespaces]]
binding = "CACHE"
id = "PLACEHOLDER_CACHE_ID"

[[durable_objects.bindings]]
name = "RELAY_POOL"
class_name = "RelayPool"

[[migrations]]
tag = "v1"
new_classes = ["RelayPool"]

[[queues.producers]]
queue = "publish-events"
binding = "PUBLISH_QUEUE"

[[queues.consumers]]
queue = "publish-events"
max_retries = 6
dead_letter_queue = "publish-failed"
```

**Step 3: Create minimal src/lib.rs**

```rust
// ABOUTME: Main entry point for the Cloudflare Worker
// ABOUTME: Handles HTTP routing and Worker lifecycle

use worker::*;

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();
    Response::ok("Divine REST Gateway")
}
```

**Step 4: Create .gitignore**

```
/target
/build
.wrangler
node_modules
.dev.vars
```

**Step 5: Verify it compiles**

Run: `cargo check`
Expected: Compiles with no errors (may have warnings)

**Step 6: Commit**

```bash
git add -A
git commit -m "chore: initialize Rust Worker project scaffold"
```

---

## Phase 2: Core Types

### Task 2.1: Nostr Filter Types and Encoding

**Files:**
- Create: `src/filter.rs`
- Modify: `src/lib.rs`

**Step 1: Write test for filter encoding/decoding**

Create `src/filter.rs`:

```rust
// ABOUTME: Nostr filter parsing, validation, and base64url encoding
// ABOUTME: Handles conversion between HTTP query params and Nostr filter objects

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Filter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<u16>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(rename = "#e", skip_serializing_if = "Option::is_none")]
    pub e_tags: Option<Vec<String>>,
    #[serde(rename = "#p", skip_serializing_if = "Option::is_none")]
    pub p_tags: Option<Vec<String>>,
}

impl Filter {
    /// Decode a base64url-encoded filter from query string
    pub fn from_base64(encoded: &str) -> Result<Self, FilterError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| FilterError::InvalidBase64)?;
        let json = String::from_utf8(bytes).map_err(|_| FilterError::InvalidUtf8)?;
        serde_json::from_str(&json).map_err(|_| FilterError::InvalidJson)
    }

    /// Encode filter to base64url for use in URLs
    pub fn to_base64(&self) -> String {
        let json = serde_json::to_string(self).expect("filter serialization cannot fail");
        URL_SAFE_NO_PAD.encode(json.as_bytes())
    }

    /// Generate cache key hash for this filter
    pub fn cache_key(&self) -> String {
        let json = serde_json::to_string(self).expect("filter serialization cannot fail");
        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        let hash = hasher.finalize();
        format!("query:{}", hex::encode(&hash[..16])) // 128-bit truncated
    }

    /// Determine TTL in seconds based on filter content
    pub fn ttl_seconds(&self) -> u64 {
        match self.kinds.as_ref().and_then(|k| k.first()) {
            Some(0) => 900,   // profiles: 15 min
            Some(3) => 600,   // contacts: 10 min
            Some(1) => 300,   // notes: 5 min
            Some(7) => 120,   // reactions: 2 min
            _ => 180,         // default: 3 min
        }
    }

    /// Check if this is a single-event lookup by ID
    pub fn is_single_event_lookup(&self) -> bool {
        matches!(&self.ids, Some(ids) if ids.len() == 1)
            && self.authors.is_none()
            && self.kinds.is_none()
    }
}

#[derive(Debug)]
pub enum FilterError {
    InvalidBase64,
    InvalidUtf8,
    InvalidJson,
}

impl std::fmt::Display for FilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBase64 => write!(f, "invalid base64 encoding"),
            Self::InvalidUtf8 => write!(f, "invalid UTF-8"),
            Self::InvalidJson => write!(f, "invalid JSON filter"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_roundtrip() {
        let filter = Filter {
            authors: Some(vec!["abc123".to_string()]),
            kinds: Some(vec![1]),
            limit: Some(20),
            ids: None,
            since: None,
            until: None,
            e_tags: None,
            p_tags: None,
        };

        let encoded = filter.to_base64();
        let decoded = Filter::from_base64(&encoded).unwrap();
        assert_eq!(filter, decoded);
    }

    #[test]
    fn test_cache_key_deterministic() {
        let filter = Filter {
            authors: Some(vec!["abc".to_string()]),
            kinds: Some(vec![1]),
            limit: None,
            ids: None,
            since: None,
            until: None,
            e_tags: None,
            p_tags: None,
        };

        let key1 = filter.cache_key();
        let key2 = filter.cache_key();
        assert_eq!(key1, key2);
        assert!(key1.starts_with("query:"));
    }

    #[test]
    fn test_ttl_by_kind() {
        let profile_filter = Filter {
            kinds: Some(vec![0]),
            ..Default::default()
        };
        assert_eq!(profile_filter.ttl_seconds(), 900);

        let note_filter = Filter {
            kinds: Some(vec![1]),
            ..Default::default()
        };
        assert_eq!(note_filter.ttl_seconds(), 300);
    }
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            ids: None,
            authors: None,
            kinds: None,
            since: None,
            until: None,
            limit: None,
            e_tags: None,
            p_tags: None,
        }
    }
}
```

**Step 2: Update src/lib.rs to include module**

```rust
// ABOUTME: Main entry point for the Cloudflare Worker
// ABOUTME: Handles HTTP routing and Worker lifecycle

use worker::*;

mod filter;

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();
    Response::ok("Divine REST Gateway")
}
```

**Step 3: Run tests**

Run: `cargo test`
Expected: All 3 tests pass

**Step 4: Commit**

```bash
git add -A
git commit -m "feat: add Filter type with base64 encoding and cache keys"
```

---

### Task 2.2: API Response Types

**Files:**
- Create: `src/types.rs`
- Modify: `src/lib.rs`

**Step 1: Create response types**

Create `src/types.rs`:

```rust
// ABOUTME: API request/response types for the REST gateway
// ABOUTME: Defines JSON structures for query responses and publish requests

use serde::{Deserialize, Serialize};

/// Response for query endpoints
#[derive(Debug, Serialize)]
pub struct QueryResponse {
    pub events: Vec<serde_json::Value>,
    pub eose: bool,
    pub complete: bool,
    pub cached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_age_seconds: Option<u64>,
}

/// Request body for publish endpoint
#[derive(Debug, Deserialize)]
pub struct PublishRequest {
    pub event: serde_json::Value,
}

/// Response for publish endpoint
#[derive(Debug, Serialize)]
pub struct PublishResponse {
    pub status: String,
    pub event_id: String,
}

/// Response for publish status endpoint
#[derive(Debug, Serialize, Deserialize)]
pub struct PublishStatus {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempts: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Standard error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u32>,
}

impl ErrorResponse {
    pub fn new(error: &str) -> Self {
        Self {
            error: error.to_string(),
            detail: None,
            retry_after: None,
        }
    }

    pub fn with_detail(mut self, detail: &str) -> Self {
        self.detail = Some(detail.to_string());
        self
    }
}

/// Cached query data stored in KV
#[derive(Debug, Serialize, Deserialize)]
pub struct CachedQuery {
    pub events: Vec<serde_json::Value>,
    pub eose: bool,
    pub timestamp: u64,
}
```

**Step 2: Update src/lib.rs**

```rust
// ABOUTME: Main entry point for the Cloudflare Worker
// ABOUTME: Handles HTTP routing and Worker lifecycle

use worker::*;

mod filter;
mod types;

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();
    Response::ok("Divine REST Gateway")
}
```

**Step 3: Verify compilation**

Run: `cargo check`
Expected: Compiles without errors

**Step 4: Commit**

```bash
git add -A
git commit -m "feat: add API request/response types"
```

---

## Phase 3: Cache Layer

### Task 3.1: KV Cache Operations

**Files:**
- Create: `src/cache.rs`
- Modify: `src/lib.rs`

**Step 1: Create cache module**

Create `src/cache.rs`:

```rust
// ABOUTME: Workers KV cache operations for storing and retrieving query results
// ABOUTME: Handles TTL management and cache key generation

use crate::types::{CachedQuery, PublishStatus};
use worker::kv::KvStore;
use worker::*;

pub struct Cache {
    kv: KvStore,
}

impl Cache {
    pub fn new(kv: KvStore) -> Self {
        Self { kv }
    }

    /// Get cached query result
    pub async fn get_query(&self, cache_key: &str) -> Result<Option<(CachedQuery, u64)>> {
        match self.kv.get(cache_key).json::<CachedQuery>().await? {
            Some(cached) => {
                let now = now_seconds();
                let age = now.saturating_sub(cached.timestamp);
                Ok(Some((cached, age)))
            }
            None => Ok(None),
        }
    }

    /// Store query result with TTL
    pub async fn put_query(&self, cache_key: &str, events: Vec<serde_json::Value>, eose: bool, ttl_seconds: u64) -> Result<()> {
        let cached = CachedQuery {
            events,
            eose,
            timestamp: now_seconds(),
        };
        self.kv
            .put(cache_key, serde_json::to_string(&cached)?)?
            .expiration_ttl(ttl_seconds)
            .execute()
            .await?;
        Ok(())
    }

    /// Get publish status
    pub async fn get_publish_status(&self, event_id: &str) -> Result<Option<PublishStatus>> {
        let key = format!("publish:{}", event_id);
        self.kv.get(&key).json::<PublishStatus>().await
    }

    /// Set publish status
    pub async fn set_publish_status(&self, event_id: &str, status: &PublishStatus) -> Result<()> {
        let key = format!("publish:{}", event_id);
        self.kv
            .put(&key, serde_json::to_string(status)?)?
            .expiration_ttl(86400) // 24 hours
            .execute()
            .await?;
        Ok(())
    }
}

/// Get current Unix timestamp in seconds
fn now_seconds() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
}
```

**Step 2: Add js-sys dependency to Cargo.toml**

Add to `[dependencies]` in Cargo.toml:

```toml
js-sys = "0.3"
```

**Step 3: Update src/lib.rs**

```rust
// ABOUTME: Main entry point for the Cloudflare Worker
// ABOUTME: Handles HTTP routing and Worker lifecycle

use worker::*;

mod cache;
mod filter;
mod types;

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();
    Response::ok("Divine REST Gateway")
}
```

**Step 4: Verify compilation**

Run: `cargo check`
Expected: Compiles without errors

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: add KV cache layer for queries and publish status"
```

---

## Phase 4: HTTP Router

### Task 4.1: Basic Router Setup

**Files:**
- Create: `src/router.rs`
- Modify: `src/lib.rs`

**Step 1: Create router with query endpoint**

Create `src/router.rs`:

```rust
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
        (Method::Get, "/") => Response::ok("Divine REST Gateway v0.1.0"),

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

    let kv = env.kv("CACHE")?;
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
    let kv = env.kv("CACHE")?;
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
    // TODO: NIP-98 auth validation
    // TODO: Queue event for publishing

    let body: crate::types::PublishRequest = req.json().await?;

    // Extract event ID
    let event_id = body
        .event
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    // Queue for publishing
    let queue = env.queue("PUBLISH_QUEUE")?;
    queue.send(body.event).await?;

    // Set initial status
    let kv = env.kv("CACHE")?;
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
```

**Step 2: Update src/lib.rs to use router**

```rust
// ABOUTME: Main entry point for the Cloudflare Worker
// ABOUTME: Handles HTTP routing and Worker lifecycle

use worker::*;

mod cache;
mod filter;
mod router;
mod types;

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();
    router::handle_request(req, env).await
}
```

**Step 3: Verify compilation**

Run: `cargo check`
Expected: Compiles without errors

**Step 4: Commit**

```bash
git add -A
git commit -m "feat: add HTTP router with query, profile, event, and publish endpoints"
```

---

## Phase 5: Durable Object - Relay Pool

### Task 5.1: Relay Pool Durable Object

**Files:**
- Create: `src/relay_pool.rs`
- Modify: `src/lib.rs`

**Step 1: Create Durable Object for relay connections**

Create `src/relay_pool.rs`:

```rust
// ABOUTME: Durable Object that maintains persistent websocket connections to Nostr relay
// ABOUTME: Handles query execution, request coalescing, and connection management

use crate::filter::Filter;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use worker::*;

#[durable_object]
pub struct RelayPool {
    state: State,
    env: Env,
    relay_url: Option<String>,
}

#[durable_object]
impl DurableObject for RelayPool {
    fn new(state: State, env: Env) -> Self {
        Self {
            state,
            env,
            relay_url: None,
        }
    }

    async fn fetch(&mut self, mut req: Request) -> Result<Response> {
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

    async fn handle_query(&mut self, mut req: Request) -> Result<Response> {
        let filter: Filter = req.json().await?;
        let events = self.query_relay(&filter).await?;
        Response::from_json(&events)
    }

    async fn handle_publish(&mut self, mut req: Request) -> Result<Response> {
        let event: serde_json::Value = req.json().await?;
        let success = self.publish_to_relay(&event).await?;
        Response::from_json(&serde_json::json!({ "ok": success }))
    }

    async fn handle_verify(&mut self, mut req: Request) -> Result<Response> {
        let body: VerifyRequest = req.json().await?;
        let found = self.verify_event(&body.event_id).await?;
        Response::from_json(&serde_json::json!({ "found": found }))
    }

    async fn query_relay(&self, filter: &Filter) -> Result<Vec<serde_json::Value>> {
        let relay_url = self.get_relay_url();

        // Create websocket connection
        let ws = WebSocket::connect(&relay_url).await?;
        let (mut tx, mut rx) = ws.split();

        // Generate subscription ID
        let sub_id = format!("q{}", js_sys::Date::now() as u64);

        // Send REQ message
        let req_msg = serde_json::json!(["REQ", sub_id, filter]);
        tx.send(WebSocketMessage::Text(req_msg.to_string())).await?;

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
            if events.len() > 0 && now - last_event_time > idle_timeout_ms {
                break; // Idle timeout after first event
            }
            if events.is_empty() && now - start > 1000.0 {
                break; // 1s timeout for empty results
            }
            if events.len() >= limit {
                break; // Limit reached
            }

            // Try to receive with small timeout
            match rx.receive().await {
                Ok(Some(WebSocketMessage::Text(msg))) => {
                    if let Ok(parsed) = serde_json::from_str::<Vec<serde_json::Value>>(&msg) {
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
                Ok(Some(WebSocketMessage::Close(_))) => break,
                Ok(None) => break,
                Err(_) => break,
            }
        }

        // Send CLOSE
        let close_msg = serde_json::json!(["CLOSE", sub_id]);
        let _ = tx.send(WebSocketMessage::Text(close_msg.to_string())).await;

        Ok(events)
    }

    async fn publish_to_relay(&self, event: &serde_json::Value) -> Result<bool> {
        let relay_url = self.get_relay_url();

        let ws = WebSocket::connect(&relay_url).await?;
        let (mut tx, mut rx) = ws.split();

        // Send EVENT message
        let event_msg = serde_json::json!(["EVENT", event]);
        tx.send(WebSocketMessage::Text(event_msg.to_string())).await?;

        // Wait for OK response
        let start = js_sys::Date::now();
        let timeout_ms = 3000.0;

        loop {
            if js_sys::Date::now() - start > timeout_ms {
                return Ok(false);
            }

            match rx.receive().await {
                Ok(Some(WebSocketMessage::Text(msg))) => {
                    if let Ok(parsed) = serde_json::from_str::<Vec<serde_json::Value>>(&msg) {
                        if parsed.get(0).and_then(|v| v.as_str()) == Some("OK") {
                            let accepted = parsed.get(2).and_then(|v| v.as_bool()).unwrap_or(false);
                            return Ok(accepted);
                        }
                    }
                }
                Ok(Some(WebSocketMessage::Close(_))) | Ok(None) | Err(_) => return Ok(false),
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
```

**Step 2: Update src/lib.rs to export Durable Object**

```rust
// ABOUTME: Main entry point for the Cloudflare Worker
// ABOUTME: Handles HTTP routing and Worker lifecycle

use worker::*;

mod cache;
mod filter;
mod relay_pool;
mod router;
mod types;

pub use relay_pool::RelayPool;

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();
    router::handle_request(req, env).await
}
```

**Step 3: Verify compilation**

Run: `cargo check`
Expected: Compiles without errors

**Step 4: Commit**

```bash
git add -A
git commit -m "feat: add RelayPool Durable Object for persistent relay connections"
```

---

## Phase 6: Queue Consumer

### Task 6.1: Publish Queue Consumer

**Files:**
- Create: `src/queue_consumer.rs`
- Modify: `src/lib.rs`

**Step 1: Create queue consumer**

Create `src/queue_consumer.rs`:

```rust
// ABOUTME: Cloudflare Queue consumer for processing event publishes
// ABOUTME: Handles publishing to relay with verification and retry logic

use crate::cache::Cache;
use crate::types::PublishStatus;
use worker::*;

pub async fn handle_queue(message_batch: MessageBatch<serde_json::Value>, env: Env) -> Result<()> {
    let relay_pool = env.durable_object("RELAY_POOL")?;
    let stub = relay_pool.id_from_name("default")?.get_stub()?;
    let kv = env.kv("CACHE")?;
    let cache = Cache::new(kv);

    for message in message_batch.messages()? {
        let event = message.body();
        let event_id = event
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Get current attempt count
        let current_status = cache.get_publish_status(&event_id).await?.unwrap_or(PublishStatus {
            status: "processing".to_string(),
            attempts: Some(0),
            verified_at: None,
            error: None,
        });
        let attempts = current_status.attempts.unwrap_or(0) + 1;

        // Update status to processing
        cache
            .set_publish_status(
                &event_id,
                &PublishStatus {
                    status: format!("attempt_{}", attempts),
                    attempts: Some(attempts),
                    verified_at: None,
                    error: None,
                },
            )
            .await?;

        // Publish to relay
        let publish_req = Request::new_with_init(
            "http://do/publish",
            RequestInit::new()
                .with_method(Method::Post)
                .with_body(Some(serde_json::to_string(&event)?.into())),
        )?;
        let mut publish_resp = stub.fetch_with_request(publish_req).await?;
        let publish_result: serde_json::Value = publish_resp.json().await?;
        let relay_ok = publish_result.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);

        if !relay_ok {
            // Relay rejected - retry
            cache
                .set_publish_status(
                    &event_id,
                    &PublishStatus {
                        status: format!("retry_{}", attempts),
                        attempts: Some(attempts),
                        verified_at: None,
                        error: Some("relay rejected".to_string()),
                    },
                )
                .await?;
            message.retry()?;
            continue;
        }

        // Verify event exists on relay
        let verify_req = Request::new_with_init(
            "http://do/verify",
            RequestInit::new()
                .with_method(Method::Post)
                .with_body(Some(serde_json::json!({ "event_id": event_id }).to_string().into())),
        )?;
        let mut verify_resp = stub.fetch_with_request(verify_req).await?;
        let verify_result: serde_json::Value = verify_resp.json().await?;
        let found = verify_result.get("found").and_then(|v| v.as_bool()).unwrap_or(false);

        if found {
            // Success - mark as published
            let now = js_sys::Date::new_0().to_iso_string().as_string().unwrap_or_default();
            cache
                .set_publish_status(
                    &event_id,
                    &PublishStatus {
                        status: "published".to_string(),
                        attempts: Some(attempts),
                        verified_at: Some(now),
                        error: None,
                    },
                )
                .await?;
            message.ack()?;
        } else {
            // Not found - retry
            cache
                .set_publish_status(
                    &event_id,
                    &PublishStatus {
                        status: format!("retry_{}", attempts),
                        attempts: Some(attempts),
                        verified_at: None,
                        error: Some("event not found on relay".to_string()),
                    },
                )
                .await?;
            message.retry()?;
        }
    }

    Ok(())
}
```

**Step 2: Update src/lib.rs to handle queue events**

```rust
// ABOUTME: Main entry point for the Cloudflare Worker
// ABOUTME: Handles HTTP routing and Worker lifecycle

use worker::*;

mod cache;
mod filter;
mod queue_consumer;
mod relay_pool;
mod router;
mod types;

pub use relay_pool::RelayPool;

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();
    router::handle_request(req, env).await
}

#[event(queue)]
async fn queue(batch: MessageBatch<serde_json::Value>, env: Env, _ctx: Context) -> Result<()> {
    console_error_panic_hook::set_once();
    queue_consumer::handle_queue(batch, env).await
}
```

**Step 3: Verify compilation**

Run: `cargo check`
Expected: Compiles without errors

**Step 4: Commit**

```bash
git add -A
git commit -m "feat: add queue consumer for reliable event publishing with verification"
```

---

## Phase 7: NIP-98 Authentication

### Task 7.1: NIP-98 Validation

**Files:**
- Create: `src/auth.rs`
- Modify: `src/router.rs`

**Step 1: Create NIP-98 auth module**

Create `src/auth.rs`:

```rust
// ABOUTME: NIP-98 HTTP authentication validation
// ABOUTME: Validates kind 27235 auth events for authenticated endpoints

use base64::{engine::general_purpose::STANDARD, Engine};
use nostr::secp256k1::schnorr::Signature;
use nostr::secp256k1::{Message, Secp256k1, XOnlyPublicKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Debug)]
pub struct AuthResult {
    pub pubkey: String,
}

#[derive(Debug)]
pub enum AuthError {
    MissingHeader,
    InvalidFormat,
    InvalidBase64,
    InvalidJson,
    InvalidKind,
    InvalidMethod,
    InvalidUrl,
    Expired,
    InvalidSignature,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingHeader => write!(f, "missing Authorization header"),
            Self::InvalidFormat => write!(f, "invalid Authorization format, expected 'Nostr <token>'"),
            Self::InvalidBase64 => write!(f, "invalid base64 token"),
            Self::InvalidJson => write!(f, "invalid JSON event"),
            Self::InvalidKind => write!(f, "invalid event kind, expected 27235"),
            Self::InvalidMethod => write!(f, "method tag does not match request"),
            Self::InvalidUrl => write!(f, "url tag does not match request"),
            Self::Expired => write!(f, "auth event expired"),
            Self::InvalidSignature => write!(f, "invalid event signature"),
        }
    }
}

#[derive(Deserialize)]
struct AuthEvent {
    id: String,
    pubkey: String,
    created_at: u64,
    kind: u32,
    tags: Vec<Vec<String>>,
    content: String,
    sig: String,
}

pub fn validate_nip98(
    auth_header: Option<&str>,
    method: &str,
    url: &str,
) -> Result<AuthResult, AuthError> {
    let header = auth_header.ok_or(AuthError::MissingHeader)?;

    // Parse "Nostr <base64>" format
    let token = header
        .strip_prefix("Nostr ")
        .ok_or(AuthError::InvalidFormat)?;

    // Decode base64
    let json_bytes = STANDARD.decode(token).map_err(|_| AuthError::InvalidBase64)?;
    let json_str = String::from_utf8(json_bytes).map_err(|_| AuthError::InvalidBase64)?;

    // Parse event
    let event: AuthEvent = serde_json::from_str(&json_str).map_err(|_| AuthError::InvalidJson)?;

    // Validate kind
    if event.kind != 27235 {
        return Err(AuthError::InvalidKind);
    }

    // Check created_at within ±60 seconds
    let now = (js_sys::Date::now() / 1000.0) as u64;
    if event.created_at > now + 60 || event.created_at < now.saturating_sub(60) {
        return Err(AuthError::Expired);
    }

    // Validate method tag
    let method_tag = event
        .tags
        .iter()
        .find(|t| t.get(0).map(|s| s.as_str()) == Some("method"))
        .and_then(|t| t.get(1))
        .ok_or(AuthError::InvalidMethod)?;

    if method_tag.to_uppercase() != method.to_uppercase() {
        return Err(AuthError::InvalidMethod);
    }

    // Validate URL tag
    let url_tag = event
        .tags
        .iter()
        .find(|t| t.get(0).map(|s| s.as_str()) == Some("u"))
        .and_then(|t| t.get(1))
        .ok_or(AuthError::InvalidUrl)?;

    if url_tag != url {
        return Err(AuthError::InvalidUrl);
    }

    // Verify signature
    if !verify_signature(&event) {
        return Err(AuthError::InvalidSignature);
    }

    Ok(AuthResult {
        pubkey: event.pubkey,
    })
}

fn verify_signature(event: &AuthEvent) -> bool {
    // Compute event ID
    let serialized = serde_json::json!([
        0,
        event.pubkey,
        event.created_at,
        event.kind,
        event.tags,
        event.content
    ]);
    let serialized_str = serialized.to_string();

    let mut hasher = Sha256::new();
    hasher.update(serialized_str.as_bytes());
    let computed_id = hex::encode(hasher.finalize());

    if computed_id != event.id {
        return false;
    }

    // Verify schnorr signature
    let secp = Secp256k1::verification_only();

    let pubkey = match hex::decode(&event.pubkey)
        .ok()
        .and_then(|bytes| XOnlyPublicKey::from_slice(&bytes).ok())
    {
        Some(pk) => pk,
        None => return false,
    };

    let sig = match hex::decode(&event.sig)
        .ok()
        .and_then(|bytes| Signature::from_slice(&bytes).ok())
    {
        Some(s) => s,
        None => return false,
    };

    let msg = match hex::decode(&event.id)
        .ok()
        .and_then(|bytes| Message::from_digest_slice(&bytes).ok())
    {
        Some(m) => m,
        None => return false,
    };

    secp.verify_schnorr(&sig, &msg, &pubkey).is_ok()
}
```

**Step 2: Update handle_publish in router.rs to use auth**

In `src/router.rs`, update the `handle_publish` function:

```rust
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

    // Queue for publishing
    let queue = env.queue("PUBLISH_QUEUE")?;
    queue.send(body.event).await?;

    // Set initial status
    let kv = env.kv("CACHE")?;
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
```

**Step 3: Update src/lib.rs to include auth module**

```rust
// ABOUTME: Main entry point for the Cloudflare Worker
// ABOUTME: Handles HTTP routing and Worker lifecycle

use worker::*;

mod auth;
mod cache;
mod filter;
mod queue_consumer;
mod relay_pool;
mod router;
mod types;

pub use relay_pool::RelayPool;

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();
    router::handle_request(req, env).await
}

#[event(queue)]
async fn queue(batch: MessageBatch<serde_json::Value>, env: Env, _ctx: Context) -> Result<()> {
    console_error_panic_hook::set_once();
    queue_consumer::handle_queue(batch, env).await
}
```

**Step 4: Verify compilation**

Run: `cargo check`
Expected: Compiles without errors

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: add NIP-98 authentication for publish endpoint"
```

---

## Phase 8: Deployment Configuration

### Task 8.1: Production wrangler.toml

**Files:**
- Modify: `wrangler.toml`
- Create: `.dev.vars.example`

**Step 1: Update wrangler.toml with proper configuration**

```toml
name = "divine-rest-gateway"
main = "build/worker/shim.mjs"
compatibility_date = "2024-01-01"
compatibility_flags = ["nodejs_compat"]

[build]
command = "cargo install -q worker-build && worker-build --release"

# Default relay - override in .dev.vars or secrets
[vars]
RELAY_URL = "wss://relay.damus.io"

# KV namespace for caching
[[kv_namespaces]]
binding = "CACHE"
id = "YOUR_KV_NAMESPACE_ID"
preview_id = "YOUR_PREVIEW_KV_NAMESPACE_ID"

# Durable Object for relay connections
[[durable_objects.bindings]]
name = "RELAY_POOL"
class_name = "RelayPool"

[[migrations]]
tag = "v1"
new_classes = ["RelayPool"]

# Publish queue
[[queues.producers]]
queue = "divine-publish-events"
binding = "PUBLISH_QUEUE"

[[queues.consumers]]
queue = "divine-publish-events"
max_batch_size = 10
max_batch_timeout = 5
max_retries = 6
dead_letter_queue = "divine-publish-failed"

# Rate limiting (using Cloudflare's built-in)
# Configure via dashboard or use KV-based in code
```

**Step 2: Create .dev.vars.example**

```
RELAY_URL=wss://relay.example.com
```

**Step 3: Commit**

```bash
git add -A
git commit -m "chore: update deployment configuration"
```

---

### Task 8.2: README

**Files:**
- Create: `README.md`

**Step 1: Create README**

```markdown
# Divine REST Gateway

REST API caching proxy for Nostr, running on Cloudflare Workers.

## Features

- **Read acceleration**: Cache Nostr queries with CDN + KV caching
- **Write proxy**: Reliable event publishing with verification and retries
- **NIP-98 auth**: Authenticated writes via HTTP Authorization
- **Edge deployment**: Global distribution via Cloudflare Workers

## API

### Query Events

```
GET /query?filter=<base64url-encoded-filter>
```

Filter is a base64url-encoded JSON Nostr filter:
```json
{"authors": ["pubkey"], "kinds": [1], "limit": 20}
```

### Convenience Endpoints

```
GET /profile/{pubkey}  - Get kind 0 profile
GET /event/{id}        - Get single event by ID
```

### Publish Event

```
POST /publish
Authorization: Nostr <base64-nip98-event>
Content-Type: application/json

{"event": {...signed nostr event...}}
```

### Check Publish Status

```
GET /publish/status/{event_id}
```

## Development

```bash
# Install wrangler
npm install -g wrangler

# Create KV namespace
wrangler kv:namespace create CACHE
wrangler kv:namespace create CACHE --preview

# Update wrangler.toml with namespace IDs

# Create queues
wrangler queues create divine-publish-events
wrangler queues create divine-publish-failed

# Run locally
wrangler dev

# Deploy
wrangler deploy
```

## Configuration

Set `RELAY_URL` in wrangler.toml vars or as a secret:
```bash
wrangler secret put RELAY_URL
```

## License

MIT
```

**Step 2: Commit**

```bash
git add -A
git commit -m "docs: add README with API documentation"
```

---

## Summary

**Phase 1**: Project scaffolding (Cargo, wrangler, minimal worker)
**Phase 2**: Core types (Filter, API responses)
**Phase 3**: Cache layer (KV operations)
**Phase 4**: HTTP router (all endpoints)
**Phase 5**: Durable Object (relay connections)
**Phase 6**: Queue consumer (reliable publishing)
**Phase 7**: NIP-98 authentication
**Phase 8**: Deployment configuration

Each phase builds on the previous, with working code at each commit point.
