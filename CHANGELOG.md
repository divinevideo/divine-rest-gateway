# Changelog

All notable changes to this project will be documented in this file.

## [0.1.1] - 2025-12-01

### Fixed

- **Critical**: Custom tag filters (`#platform`, `#t`, etc.) were being dropped from cache keys and relay queries. Filters now preserve raw JSON to include ALL fields.
- **Critical**: WebSocket queries could hang indefinitely. Added proper timeout handling using `futures::select` to race message receive against setTimeout.
- Switch default relay from `relay.damus.io` to `relay.divine.video` (indexes `#platform` and other Divine-specific tags)

### Added

- CORS headers on all responses (`Access-Control-Allow-Origin: *`)
- Comprehensive unit tests for auth, filter, and types modules
- Integration tests against live gateway

## [0.1.0] - 2024-12-01

### Added

- Initial release of Divine REST Gateway
- **Query API**: `GET /query?filter=<base64url>` for Nostr filter queries
- **Profile endpoint**: `GET /profile/{pubkey}` for user metadata
- **Event endpoint**: `GET /event/{id}` for single event lookup
- **Publish API**: `POST /publish` with NIP-98 authentication
- **Status endpoint**: `GET /publish/status/{event_id}` for publish tracking
- **Landing page**: HTML documentation at root path
- **Multi-layer caching**: CDN + Cloudflare KV with content-aware TTLs
  - Profiles (kind 0): 15 minutes
  - Contacts (kind 3): 10 minutes
  - Notes (kind 1): 5 minutes
  - Reactions (kind 7): 2 minutes
  - Default: 5 minutes
- **Durable Objects**: Persistent WebSocket connections to Nostr relays
- **Cloudflare Queues**: Reliable event publishing with retry support
- **NIP-98 authentication**: HTTP auth using kind 27235 events
- **Schnorr signature verification**: Pure Rust via k256 (WASM compatible)
- **Observability**: Request logging with 100% sampling
- **Custom domain**: gateway.divine.video

### Technical

- Built with Rust compiled to WebAssembly
- Runs on Cloudflare Workers edge network
- Uses k256 for WASM-compatible secp256k1 operations
- worker-rs 0.7 for Cloudflare Workers bindings
