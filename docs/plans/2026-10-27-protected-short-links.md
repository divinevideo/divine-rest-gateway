# Protected short-link creation

The existing REST gateway owns authenticated client creation at https://gateway.divine.video/api/links. The Fastly creator remains server-only; its bearer is a Cloudflare secret and never reaches a client. Public resolution stays at https://divine.video/api/links/:code.

The endpoint is disabled unless SHORT_LINK_CREATION_ENABLED is exactly true. Only the canonical gateway origin serves it, including when workers.dev can reach this Worker. POST accepts at most 4096 bytes of application/json containing one url. Only the existing shortener's canonical video/profile target grammar is accepted. No query, fragments, credentials, alternate origins, nested aliases, or identifier truncation.

NIP-98 authentication binds kind27235, the exact canonical endpoint, exact POST method, the SHA-256 of the exact body, and a timestamp within60seconds. Reject missing/duplicate/ambiguous binding tags, oversized authorization, invalid event IDs and invalid BIP340 signatures. Nostr signs the event ID directly; do not hash it a second time. Replays within the short validity window are permitted for this idempotent operation, charge quota again, and can only create the same body-bound mapping.

A SQLite Durable Object enforces fixed one-minute quotas before any upstream request:30 per authenticated pubkey and120 per trusted CF-Connecting-IP. Missing IP or unavailable limits fail closed. Counters store domain-separated hashes, no raw identifiers; expire old windows. One named object serializes checks synchronously, preventing concurrent requests from exceeding either quota. Initial capacity is intentionally conservative; shard only with measured need and equivalent limits.

The endpoint owns CORS before the legacy router's wildcard handling. Only https://divine.video and https://www.divine.video browser origins are allowed; native requests may omit Origin. Errors, preflight and successful responses are no-store, nosniff, and vary by Origin. Authentication never depends on CORS. No credential, pubkey, IP or authorization header is logged.

Upstream is fixed to https://divine.video/api/links, redirects disabled, body/output bounded, with a bounded deadline. Forward only JSON and the server bearer. Validate the complete returned code/short_url/url mapping against the requested target before responding; upstream failures become controlled503. Quota exhaustion returns429 with Retry-After. Disabled feature returns503. Caller timeout/failure falls back to the full canonical URL.

Implementation and verification:

1. Signed synthetic Nostr regression, then correct raw event-ID verification; native auth tests pin binding/expiry/body/signature failure.
2. Pure creation contract tests pin canonical URLs, JSON shape, origin rules and returned mapping.
3. Add Worker handler and limiter, preserve existing routes, add a v2 SQLite migration and default-off variable.
4. Build Wasm and run local Wrangler HTTP tests for auth, CORS, gate, concurrency/quota, failure and positive creation against an injected fixed local upstream. Inspect production bindings before deployment; provision only the new secret/object. Default gate stays off until smoke checks pass.
5. Independent current-diff review and normal CI/merge. Sender gates remain off until receiving and device compatibility gates pass.
