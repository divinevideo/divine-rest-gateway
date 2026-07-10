# Divine REST Gateway

A REST API caching proxy for [Nostr](https://nostr.com), running on Cloudflare Workers. It exposes plain HTTP endpoints in front of a Nostr relay so that web and mobile clients can read events over REST instead of managing WebSocket subscriptions, with multi-layer caching for fast reads. It is written in Rust and compiled to WebAssembly.

The gateway runs at [gateway.divine.video](https://gateway.divine.video), which also serves an HTML landing page documenting the API.

## Endpoints

| Method | Path | Description |
| --- | --- | --- |
| `GET` | `/` | HTML landing page with API documentation |
| `GET` | `/health` | Health check, returns `ok` |
| `GET` | `/query?filter=<base64url>` | Query events with a base64url-encoded Nostr filter |
| `GET` | `/profile/{pubkey}` | Convenience wrapper for a kind 0 profile |
| `GET` | `/event/{id}` | Convenience wrapper for a single event by ID |
| `POST` | `/publish` | Publish a signed event (requires NIP-98 auth) |
| `GET` | `/publish/status/{event_id}` | Look up the recorded status of a publish |

All responses include permissive CORS headers (`Access-Control-Allow-Origin: *`), and `OPTIONS` preflight requests are handled for every path.

### Query events

```
GET /query?filter=<base64url-encoded-filter>
```

The `filter` parameter is a standard [NIP-01](https://github.com/nostr-protocol/nips/blob/master/01.md) filter object, serialized to JSON and base64url-encoded (no padding):

```js
const filter = { authors: ["<pubkey>"], kinds: [1], limit: 20 };
const encoded = btoa(JSON.stringify(filter))
  .replace(/\+/g, "-").replace(/\//g, "_").replace(/=/g, "");
const res = await fetch(`https://gateway.divine.video/query?filter=${encoded}`);
```

The raw filter JSON is preserved end to end, so custom tag fields such as `#platform` and `#t` are passed to the relay and included in the cache key rather than being dropped.

The response wraps the matched events:

```json
{
  "events": [ ... ],
  "eose": true,
  "complete": true,
  "cached": true,
  "cache_age_seconds": 42
}
```

`cached` and `cache_age_seconds` indicate whether the response came from cache and how old it is.

To bypass the cache and force a fresh relay fetch, add `?nocache=1` (or `nocache=true`) to the query string, or send a `Cache-Control: no-cache` request header.

### Profile and event lookups

```
GET /profile/{pubkey}   # kind 0 metadata for a public key
GET /event/{id}         # single event by ID
```

These are convenience endpoints that build the corresponding filter (`{"authors":["<pubkey>"],"kinds":[0],"limit":1}` and `{"ids":["<id>"],"limit":1}`) and run it through the same query and caching path. They return the same response shape as `/query`.

### Publish events

```
POST /publish
Authorization: Nostr <base64-encoded NIP-98 event>
Content-Type: application/json

{ "event": { ...signed Nostr event... } }
```

Writes are authenticated with [NIP-98](https://github.com/nostr-protocol/nips/blob/master/98.md). The `Authorization` header carries a base64-encoded kind 27235 event; the gateway verifies its Schnorr signature and event ID, checks that its `method` and `u` (URL) tags match the request, and rejects events whose `created_at` is more than 60 seconds from the server clock.

On success the endpoint records a `queued` status for the event and returns `202 Accepted`:

```json
{ "status": "queued", "event_id": "<id>" }
```

> **Note:** The queue consumer that publishes to the relay and verifies delivery with retries is implemented (see `src/queue_consumer.rs`), but the `/publish` handler does not yet enqueue events — that step is a `TODO` in `src/router.rs` pending queue-producer support in worker-rs. Today, a successful `POST /publish` authenticates the request and records the status; it does not itself deliver the event to the relay.

### Publish status

```
GET /publish/status/{event_id}
```

Returns the status record for a previously submitted event (for example `queued`, `attempt_N`, `retry_N`, or `published`), or `404` if none is stored. Status records are kept in KV for 24 hours.

## Architecture

The gateway is a single Cloudflare Worker with three moving parts:

- **Router (`src/router.rs`)** — the Worker `fetch` entry point. It dispatches HTTP requests, applies CORS, checks the cache, and formats responses.
- **Cache (`src/cache.rs`)** — a thin layer over Workers KV (`REST_GATEWAY_CACHE`) storing query results and publish-status records. Query cache keys are a SHA-256 hash of the raw filter JSON, so any change to the filter (including custom tags) produces a distinct entry.
- **RelayPool (`src/relay_pool.rs`)** — a Durable Object that opens a WebSocket to the configured Nostr relay and runs the actual `REQ`/`EVENT` exchange. It races message receipt against explicit timeouts (5 s overall, 1 s for empty results, 300 ms idle after the first event) so queries cannot hang indefinitely, and it returns as soon as the relay sends `EOSE`.

On a cache miss the router forwards the raw filter to the RelayPool Durable Object, caches the returned events with a content-aware TTL, and responds. Responses also carry `Cache-Control: public, max-age=<ttl>, s-maxage=<ttl>`, so Cloudflare's edge cache absorbs repeated identical requests in front of KV.

TTLs are chosen from the first `kind` in the filter:

| Content | Kind | TTL |
| --- | --- | --- |
| Profiles | 0 | 15 minutes |
| Contacts | 3 | 10 minutes |
| Notes | 1 | 5 minutes |
| Reactions | 7 | 2 minutes |
| Everything else | — | 5 minutes (default) |

The publish path also defines a Cloudflare Queue (`divine-publish-events`) and a queue consumer (`src/queue_consumer.rs`) that publishes each event to the relay, verifies it can be read back, and retries on failure with a dead-letter queue for exhausted messages. As noted above, the producer side is not yet wired into `/publish`.

NIP-98 signature verification uses `k256` (pure-Rust secp256k1/Schnorr), chosen because the usual `secp256k1` native bindings do not compile to WebAssembly.

This gateway is the REST read/write front end for Nostr data on the Divine platform; it sits in front of `relay.divine.video`, which indexes the platform-specific tags Divine clients rely on.

## Getting started

You need the [Rust toolchain](https://rustup.rs/) with the `wasm32-unknown-unknown` target and [Wrangler](https://developers.cloudflare.com/workers/wrangler/).

```bash
# Run the test suite (unit tests)
cargo test

# Fast compile-only check
cargo check

# Run the Worker locally (builds the WASM via worker-build)
wrangler dev

# Deploy to Cloudflare
wrangler deploy
```

The build step is defined in `wrangler.toml` and runs `cargo install -q worker-build && worker-build --release`.

The integration tests in `tests/integration_tests.rs` run against the live `https://gateway.divine.video` deployment, so they require network access.

Before first deploy, create the backing resources referenced by `wrangler.toml`:

```bash
# KV namespace for the cache
wrangler kv namespace create REST_GATEWAY_CACHE
wrangler kv namespace create REST_GATEWAY_CACHE --preview

# Queues for the publish pipeline
wrangler queues create divine-publish-events
wrangler queues create divine-publish-failed
```

Update the generated namespace IDs in `wrangler.toml` if they differ from the committed values.

## Configuration

Configuration lives in `wrangler.toml`:

- **`RELAY_URL`** (var) — the Nostr relay the gateway connects to. Defaults to `wss://relay.divine.video`. Override it locally in `.dev.vars` (see `.dev.vars.example`) or as a secret with `wrangler secret put RELAY_URL`.
- **`REST_GATEWAY_CACHE`** (KV namespace) — stores cached query results and publish-status records.
- **`RELAY_POOL`** (Durable Object, class `RelayPool`) — holds the persistent relay WebSocket connections.
- **`PUBLISH_QUEUE`** (Queue producer → `divine-publish-events`) and its consumer, which retries up to 6 times with `divine-publish-failed` as the dead-letter queue.

Observability logging is enabled with 100% head sampling, and the Worker is bound to the `gateway.divine.video` custom domain.

## Deployment

Deployment is manual via `wrangler deploy`, which runs the `worker-build` release step and uploads the Worker. There is no automated deploy workflow in this repository; the only GitHub Actions workflow (`.github/workflows/semantic_pr.yml`) enforces Conventional Commit PR titles.

See `CHANGELOG.md` for release history and `docs/react-client-usage.md` for a client integration guide.

## License

MIT

---

Part of [Divine](https://divine.video) — your playground for human creativity · [Brand guidelines](https://github.com/divinevideo/brand-guidelines)
