# Divine REST Gateway - Design Document

## Overview

A REST API caching proxy for Nostr that runs on Cloudflare Workers (Rust→WASM), providing fast HTTP access to Nostr content for web and mobile clients (especially Flutter). This augments rather than replaces existing websocket-based Nostr clients.

## Goals

- **Read acceleration**: Cache and serve Nostr queries via REST with CDN-level caching
- **Write proxy**: Accept signed events, queue for reliable publishing with verification
- **NIP-98 auth**: Validate HTTP auth headers for authenticated endpoints
- **Horizontal scalability**: Handle 100k+ concurrent users via caching and sharding

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                      Cloudflare Edge                            │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────┐    ┌─────────────────┐    ┌──────────────────┐   │
│  │   CDN    │───▶│     Worker      │───▶│  Durable Object  │   │
│  │  Cache   │    │  (router/auth)  │    │  (relay pool)    │   │
│  └──────────┘    └────────┬────────┘    └────────┬─────────┘   │
│       ▲                   │                      │              │
│       │                   ▼                      ▼              │
│       │            ┌──────────┐          ┌─────────────┐       │
│       └────────────│ Workers  │          │  WebSocket  │       │
│                    │    KV    │          │  to Relay   │       │
│                    └──────────┘          └─────────────┘       │
│                                                                 │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐        │
│  │  Publish    │───▶│  Consumer   │───▶│   Retry     │        │
│  │  Queue      │    │  Worker     │    │   Queue     │        │
│  └─────────────┘    └─────────────┘    └─────────────┘        │
└─────────────────────────────────────────────────────────────────┘
```

### Components

1. **CDN Cache**: Edge-level caching of GET responses (~60s TTL)
2. **Worker**: HTTP routing, auth validation, cache lookup, request coordination
3. **Workers KV**: Persistent cache layer (5-15min TTL by content type)
4. **Durable Objects**: Maintain persistent websocket connections to relay, handle query coalescing
5. **Cloudflare Queues**: Reliable publish queue with retry/backoff

## API Design

### Read Endpoints

Primary query endpoint uses base64url-encoded Nostr filters for CDN cacheability:

```
GET /query?filter=<base64url-encoded-filter-json>
```

Example:
```
GET /query?filter=eyJhdXRob3JzIjpbImFiYyJdLCJraW5kcyI6WzFdLCJsaW1pdCI6MjB9
```

Response:
```json
{
  "events": [...],
  "eose": true,
  "complete": true,
  "cached": true,
  "cache_age_seconds": 45
}
```

Convenience endpoints (internally construct filters):
```
GET /profile/{pubkey}           # kind 0
GET /contacts/{pubkey}          # kind 3
GET /notes/{pubkey}?limit=20    # kind 1
GET /event/{id}                 # single event by ID
```

### Write Endpoint

```
POST /publish
Authorization: Nostr <base64-encoded-kind-27235-event>
Content-Type: application/json

{"event": {...signed nostr event...}}
```

Response (immediate):
```json
{
  "status": "queued",
  "event_id": "abc123..."
}
```

Status check:
```
GET /publish/status/{event_id}

{"status": "published", "verified_at": "2025-12-01T10:30:00Z"}
{"status": "retry_2", "attempts": 2, "error": "relay timeout"}
{"status": "failed", "attempts": 6, "error": "event not found after retries"}
```

## Caching Strategy

### Three-Layer Cache

| Layer | TTL | Latency | Cache Key |
|-------|-----|---------|-----------|
| CDN (edge) | 60s | ~5ms | URL (includes filter) |
| KV | 5-15min | ~20ms | `query:<filter-hash>` |
| DO local | Request duration | ~1ms | In-memory |

### TTLs by Content Type

| Content | KV TTL | Rationale |
|---------|--------|-----------|
| Profiles (kind 0) | 15 min | Changes rarely |
| Contacts (kind 3) | 10 min | Changes occasionally |
| Notes (kind 1) | 5 min | Fresh-ish feed |
| Reactions (kind 7) | 2 min | Counts change fast |
| Single event by ID | 1 hour | Immutable |
| Generic queries | 3 min | Default |

### KV Key Structure

```
query:<sha256-filter-hash>  →  {events: [...], timestamp, eose}
event:<event-id>            →  {event: {...}, timestamp}
profile:<pubkey>            →  {event: {...}, timestamp}
publish:<event-id>          →  {status, attempts, verified_at, error}
```

## Durable Objects Design

### Scaling Strategy

For 100k concurrent users with single relay:

```
[100k requests]
     │
     ▼ 95% hit
[CDN Cache] → 95k responses
     │
     ▼ 80% hit
[KV Cache] → 4k responses
     │
     ▼ coalesced
[Sharded DOs] → 1k responses
     │
     ▼
[Relay]
```

### DO Sharding

Shard by filter hash to distribute load:
```rust
let shard = filter_hash % NUM_SHARDS;  // 4-16 shards
let do_id = env.durable_object("RelayPool").id_from_name(&format!("shard-{}", shard));
```

### Request Coalescing

Deduplicate concurrent identical queries:
```rust
if let Some(pending) = self.in_flight.get(&filter_hash) {
    return pending.subscribe().await;  // wait for existing query
}
// else start new query
```

### Connection Pooling

Each DO maintains multiple websocket connections:
```rust
struct RelayPool {
    connections: Vec<WebSocket>,  // 5-10 connections
    query_semaphore: Semaphore,   // limit concurrent queries per connection
}
```

### Query Completion (Hybrid Strategy)

Without guaranteed EOSE, use hybrid completion:
```rust
// Done when ANY of:
// - EOSE received
// - limit reached
// - idle for 300ms after first event
// - max timeout (5s) exceeded
// - zero events + idle 1s (empty result)
```

## Publishing Flow

### Queue-Based Reliable Publishing

```
Client → Worker (validate) → Publish Queue → Consumer Worker → DO → Relay
                                   │                            │
                                   │                            ▼ verify
                                   │                       event exists?
                                   │                            │
                                   ▼ no                         ▼ yes
                             Retry Queue ◀──────────────   KV (published)
                             (backoff)
                                   │
                                   ▼ max retries
                             Dead Letter Queue
```

### Queue Configuration

```toml
[[queues.producers]]
queue = "publish-events"

[[queues.consumers]]
queue = "publish-events"
max_retries = 6
max_batch_size = 10
retry_delay = "exponential"  # 1s, 2s, 4s, 8s, 16s, 32s
dead_letter_queue = "publish-failed"
```

### Consumer Logic

```rust
async fn handle_batch(messages: Vec<Message>) {
    for msg in messages {
        let event: NostrEvent = msg.body();

        // 1. Publish via DO
        do_stub.publish(&event).await;

        // 2. Verify it landed
        let verified = do_stub.query_event(&event.id).await;

        if verified {
            msg.ack();
            kv.put(&format!("publish:{}", event.id), Status::Published).await;
        } else {
            msg.retry();  // back to queue with backoff
        }
    }
}
```

## NIP-98 Authentication

### Validation Flow

1. Decode base64 auth token → kind 27235 event
2. Verify event signature
3. Check `u` tag matches request URL
4. Check `method` tag matches HTTP method
5. Check `created_at` within ±60 seconds
6. Optionally verify pubkey in auth matches event being published

### Request Format

```
POST /publish
Authorization: Nostr <base64-encoded-kind-27235-event>
```

## Error Handling

### HTTP Status Codes

| Scenario | Status | Response |
|----------|--------|----------|
| Cache hit | 200 | `{"events": [...], "cached": true}` |
| Cache miss, success | 200 | `{"events": [...], "cached": false}` |
| Invalid filter | 400 | `{"error": "invalid_filter", "detail": "..."}` |
| Invalid NIP-98 | 401 | `{"error": "auth_failed", "detail": "..."}` |
| Rate limited | 429 | `{"error": "rate_limited", "retry_after": 30}` |
| Relay timeout | 504 | `{"error": "relay_timeout"}` |
| Relay unreachable | 502 | `{"error": "relay_unavailable"}` |
| Event queued | 202 | `{"status": "queued", "event_id": "..."}` |

### DO/Relay Error Handling

- Connection dropped → reconnect with exponential backoff
- Query timeout → return partial results with `"complete": false`
- Relay NOTICE → log and continue

## Rate Limiting

### Unauthenticated (per-IP)

- 60 queries/min
- 10 publishes/min

### Authenticated (per-pubkey via NIP-98)

- 300 queries/min
- 60 publishes/min

Implementation: Cloudflare Rate Limiting rules or KV-based sliding window counters.

## Project Structure

```
divine-rest-gateway/
├── Cargo.toml
├── wrangler.toml
├── src/
│   ├── lib.rs              # Worker entrypoint
│   ├── router.rs           # HTTP routing
│   ├── auth.rs             # NIP-98 validation
│   ├── cache.rs            # KV operations
│   ├── filter.rs           # Nostr filter parsing/hashing
│   ├── relay_pool.rs       # Durable Object
│   └── queue_consumer.rs   # Publish queue handler
└── docs/
    └── plans/
```

## Configuration

### wrangler.toml

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
id = "..."

[[durable_objects.bindings]]
name = "RELAY_POOL"
class_name = "RelayPool"

[[queues.producers]]
queue = "publish-events"
binding = "PUBLISH_QUEUE"

[[queues.consumers]]
queue = "publish-events"
max_retries = 6
dead_letter_queue = "publish-failed"
```

## Decisions Log

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Operations scope | Read + Write + NIP-98 | Full API surface for clients |
| Cache invalidation | Time-based TTL only | Simple, predictable, no live subscriptions |
| Relay configuration | Fixed relay list | Simpler caching, controlled environment |
| Deployment target | Cloudflare Workers | Edge performance, global distribution |
| Language | Rust → WASM | Performance, Nostr ecosystem |
| Architecture | Durable Objects | Connection reuse, request coalescing |
| Query format | GET with base64 filter | CDN cacheability |
| Publishing | Queue with verification | Reliability, backpressure handling |
