# Protected short-link creation

The existing REST gateway owns authenticated client creation at https://gateway.divine.video/api/links. The Fastly creator remains server-only; its bearer is a Cloudflare secret and never reaches a client. Public resolution stays at https://divine.video/api/links/:code.

The endpoint is disabled unless SHORT_LINK_CREATION_ENABLED is exactly true. Only the canonical gateway origin serves it, including when workers.dev can reach this Worker. POST accepts at most 4096 bytes of application/json containing one url. Only the existing shortener's canonical video/profile target grammar is accepted. No query, fragments, credentials, alternate origins, nested aliases, or identifier truncation.

NIP-98 authentication binds kind 27235, the exact canonical endpoint, exact POST method, the SHA-256 of the exact body, and a timestamp within 60 seconds. Reject missing/duplicate/ambiguous binding tags, oversized authorization, invalid event IDs and invalid BIP340 signatures. Nostr signs the event ID directly; do not hash it a second time. Replays within the short validity window are permitted for this idempotent operation, charge quota again, and can only create the same body-bound mapping.

A SQLite Durable Object enforces fixed one-minute quotas before any upstream request: 30 per authenticated pubkey and 120 per trusted CF-Connecting-IP. Missing IP or unavailable limits fail closed. Counters store domain-separated hashes, no raw identifiers; expire old windows. One named object serializes checks synchronously, preventing concurrent requests from exceeding either quota. Initial capacity is intentionally conservative; shard only with measured need and equivalent limits.

The endpoint owns CORS before the legacy router's wildcard handling. Only https://divine.video and https://www.divine.video browser origins are allowed; native requests may omit Origin. Errors, preflight and successful responses are no-store, nosniff, and vary by Origin. Authentication never depends on CORS. No credential, pubkey, IP or authorization header is logged.

Upstream is fixed to https://divine.video/api/links, redirects disabled, body/output bounded, with a bounded deadline. Forward only JSON and the server bearer. Validate the complete returned code/short_url/url mapping against the requested target before responding; upstream failures become controlled 503. Quota exhaustion returns 429 with Retry-After. Disabled feature returns 503. Caller timeout/failure falls back to the full canonical URL.

Implementation and verification:

1. Signed synthetic Nostr regression, then correct raw event-ID verification; native auth tests pin binding/expiry/body/signature failure.
2. Pure creation contract tests pin canonical URLs, JSON shape, origin rules and returned mapping.
3. Add Worker handler and limiter, preserve existing routes, add a v2 SQLite migration and default-off variable.
4. Build Wasm and run local Wrangler HTTP tests for auth, CORS, gate, concurrency/quota, failure and positive creation against the fixed production shortener using only synthetic verification targets. Inspect production bindings before deployment; provision only the new secret/object. Default gate stays off until smoke checks pass. SHORT_LINK_CREATION_ENABLED is declared in wrangler.toml and keep_vars is not set, so every wrangler deploy applies the committed value and reverts a dashboard-only change; open or close the gate by changing the committed value.
5. Independent current-diff review and normal CI/merge. Sender gates remain off until receiving and device compatibility gates pass.

## Local runtime verification

Use only a synthetic target and an isolated local state directory. Put the server-only SHORT_LINK_CREATION_TOKEN in ignored .dev.vars with mode 0600; never put the credential in a command argument. A valid bearer calls the fixed production shortener and creates/reuses only the synthetic verification mapping. The local Worker keeps the gateway's canonical URL through the documented local-upstream override.

```sh
wrangler deploy --dry-run
wrangler dev --local --port 9785 --inspector-port 9786 --local-upstream gateway.divine.video --upstream-protocol https --var SHORT_LINK_CREATION_ENABLED:true --persist-to .wrangler/short-links-smoke
cargo test --locked --test short_links_local -- --ignored --test-threads=1
```

Run the test command in a second terminal. The tests require unused synthetic quota windows; do not reuse state within the same minute. They test the actual Wasm handler and SQLite object, including 35 concurrent calls with exactly 30 identity admits and 125 calls across five keys with exactly 120 IP admits. Upstream availability and local clock boundaries are environment-dependent; the IP test permits controlled 503 for an admitted request when the upstream deadline expires.

The default cargo test run intentionally does not start a Worker or use a production bearer. CI compiles this harness, runs pure protocol/contract tests, and builds the Wasm package without deploying. Local runtime evidence is required separately.
