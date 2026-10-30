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
| `POST` | `/api/links` | Create a short link for a Divine video or profile URL (no auth; off unless enabled) |

Every path except `/api/links` adds permissive CORS headers (`Access-Control-Allow-Origin: *`) to its responses and answers `OPTIONS` preflight requests. `/api/links` applies its own strict CORS policy, described below.

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

Writes are authenticated with [NIP-98](https://github.com/nostr-protocol/nips/blob/master/98.md). The `Authorization` header carries a base64-encoded kind 27235 event; the gateway verifies its event ID and the BIP-340 Schnorr signature over that ID, requires exactly one `method` tag and one `u` (URL) tag matching the request, and rejects events whose `created_at` is more than 60 seconds from the server clock. `Authorization` headers longer than 16 KiB are rejected.

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

### Create short links

```
POST https://gateway.divine.video/api/links
Content-Type: application/json

{ "url": "https://divine.video/video/<id>" }
```

Creates a short link for a Divine video or profile URL, or returns the existing one, by calling the Divine link shortener with a server-held bearer token that never reaches the client. `POST` requests get `503` unless the `SHORT_LINK_CREATION_ENABLED` var is exactly `true`. The endpoint is served only at the exact URL `https://gateway.divine.video/api/links`; any other host, or a query string, gets `404`.

- **Body:** at most 4096 bytes of `application/json` containing only a `url` field, either `https://divine.video/video/<id>` or `https://divine.video/profile/<pubkey>` (64 hex characters or an `npub`). IDs may contain ASCII letters, digits, `-`, `.`, `_` and `~`; query strings, fragments and percent-encoding are rejected with `400`.
- **Authentication:** none. Creation is public; no account, signature, or client credential is required. Existing clients may continue sending an Authorization header, which is ignored for this endpoint.
- **Quotas:** 120 requests per minute per trusted client IP, counted in fixed one-minute windows. A request counts once it passes validation, before the shortener is called, so one that returns an existing link or ends in an upstream `503` still counts, and retrying a `503` uses quota. An exhausted quota gets `429` with `Retry-After: 60`; a `429` does not count.
- **CORS:** browsers may call it only from `https://divine.video` and `https://www.divine.video`; other origins get `403`. Requests without an `Origin` header, such as those from native apps, are accepted.

A new link returns `201 Created` and an existing one `200 OK`:

```json
{ "code": "Abc12345", "short_url": "https://divine.video/l/Abc12345", "url": "https://divine.video/video/<id>" }
```

Upstream errors, timeouts and missing configuration return `503`; clients should fall back to the full URL. Responses are never cached (`Cache-Control: no-store`). The design and its local runtime tests are described in `docs/plans/2026-10-27-protected-short-links.md`.

## Architecture

The gateway is a single Cloudflare Worker with these moving parts:

- **Router (`src/router.rs`)** — the Worker `fetch` entry point. It dispatches HTTP requests, applies CORS, checks the cache, and formats responses.
- **Cache (`src/cache.rs`)** — a thin layer over Workers KV (`REST_GATEWAY_CACHE`) storing query results and publish-status records. Query cache keys are a SHA-256 hash of the raw filter JSON, so any change to the filter (including custom tags) produces a distinct entry.
- **RelayPool (`src/relay_pool.rs`)** — a Durable Object that opens a WebSocket to the configured Nostr relay and runs the actual `REQ`/`EVENT` exchange. It races message receipt against explicit timeouts (5 s overall, 1 s for empty results, 300 ms idle after the first event) so queries cannot hang indefinitely, and it returns as soon as the relay sends `EOSE`.
- **Short links (`src/short_links.rs`, `src/short_link_contract.rs`)** — the `POST /api/links` handler: strict CORS, anonymous request and response contract checks, and the bounded call to the link shortener.
- **ShortLinkLimiter (`src/short_link_limiter.rs`)** — a SQLite-backed Durable Object that serializes the per-IP creation quota so concurrent requests cannot exceed it.

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

# Operator-authorized manual deploy only; coordinate with CD (docs/deployment.md)
wrangler deploy
```

The build step is defined in `wrangler.toml` and runs `cargo install -q worker-build --version 0.7.1 --locked && worker-build --release`.

The integration tests in `tests/integration_tests.rs` run against the live `https://gateway.divine.video` deployment, so they require network access. The short-link runtime tests in `tests/short_links_local.rs` run against a local `wrangler dev` and are ignored by default; `docs/plans/2026-10-27-protected-short-links.md` explains how to run them.

Before first deploy, create the backing resources referenced by `wrangler.toml`:

```bash
# KV namespace for the cache
wrangler kv namespace create REST_GATEWAY_CACHE
wrangler kv namespace create REST_GATEWAY_CACHE --preview

# Queues for the publish pipeline
wrangler queues create divine-publish-events
wrangler queues create divine-publish-failed
```

Update the generated namespace IDs in `wrangler.toml` if they differ from the committed values. The `SHORT_LINK_LIMITER` Durable Object is created by the `v2` migration on deploy. Before enabling `POST /api/links`, store the link shortener's bearer token with `wrangler secret put SHORT_LINK_CREATION_TOKEN`.

## Configuration

Configuration lives in `wrangler.toml`:

- **`RELAY_URL`** (var) — the Nostr relay the gateway connects to. Defaults to `wss://relay.divine.video`. Override it locally in `.dev.vars` (see `.dev.vars.example`) or as a secret with `wrangler secret put RELAY_URL`.
- **`REST_GATEWAY_CACHE`** (KV namespace) — stores cached query results and publish-status records.
- **`RELAY_POOL`** (Durable Object, class `RelayPool`) — holds the persistent relay WebSocket connections.
- **`PUBLISH_QUEUE`** (Queue producer → `divine-publish-events`) and its consumer, which retries up to 6 times with `divine-publish-failed` as the dead-letter queue.
- **`SHORT_LINK_CREATION_ENABLED`** (var) — turns on `POST /api/links` when exactly `true`. It is `true` in `wrangler.toml`; set it to `false` to pause creation while preserving existing redirects. A normal deploy applies the committed value.
- **`SHORT_LINK_CREATION_TOKEN`** (secret) — the bearer token the gateway sends to the link shortener. Set it with `wrangler secret put SHORT_LINK_CREATION_TOKEN`; it is never returned to clients.
- **`SHORT_LINK_LIMITER`** (Durable Object, class `ShortLinkLimiter`, SQLite storage from the `v2` migration) — enforces the short-link creation quotas.

Observability logging is enabled with 100% head sampling, and the Worker is bound to the `gateway.divine.video` custom domain.

## Deployment

`.github/workflows/ci.yml` checks each commit before its production deployment job can run. Production automation is **off until an operator completes the activation prerequisites** and sets the repository variable `PRODUCTION_CD_ENABLED=true`. Pull requests only run checks. A manual replay must select `main` and still reruns the checks; old runs cannot deploy over current main. See [the deployment runbook](docs/deployment.md) for account/binding verification, credentials, environment protections, release receipts, and rollback. `.github/workflows/semantic_pr.yml` enforces Conventional Commit PR titles.

See `CHANGELOG.md` for release history and `docs/react-client-usage.md` for a client integration guide.

## License

MIT

---

Part of [Divine](https://divine.video) — your playground for human creativity · [Brand guidelines](https://github.com/divinevideo/brand-guidelines)
