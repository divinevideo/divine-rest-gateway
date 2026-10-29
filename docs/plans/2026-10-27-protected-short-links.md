# Public short-link creation

The REST gateway serves public creation at https://gateway.divine.video/api/links. No account, signature or client credential is required. The Fastly creator remains server-only; its bearer is a Cloudflare secret and never reaches clients. Public resolution stays at https://divine.video/api/links/:code.

POST accepts at most 4096 bytes of application/json containing one url. Only canonical Divine video/profile targets are accepted, with full identifiers. The endpoint requires the canonical gateway URL; it rejects query strings and disallowed browser origins. Browser origins https://divine.video and https://www.divine.video are allowed; native and command-line requests may omit Origin. Existing clients may continue sending Authorization headers, but creation ignores them.

The existing SQLite Durable Object enforces 120 requests per trusted CF-Connecting-IP per fixed minute before upstream work. It stores only a domain-separated IP hash, expires old windows and updates counters synchronously so concurrent requests cannot exceed the limit. No per-account quota is needed. Missing IP or unavailable limits returns a controlled503. Exhaustion returns429 with Retry-After:60.

Upstream is fixed to https://divine.video/api/links and uses only the server-held bearer. Redirects are disabled, request/response sizes bounded, and the deadline is1500ms. The complete returned code/short_url/url mapping must match the requested target. All responses are no-store. SHORT_LINK_CREATION_ENABLED=true enables creation; false pauses creation while existing links keep resolving.

## Local runtime verification

Use a synthetic destination and an isolated local state directory. Put the server-only SHORT_LINK_CREATION_TOKEN in ignored .dev.vars with mode0600, never in a command argument. The local-upstream setting preserves the canonical URL in the Worker.

```sh
wrangler deploy --dry-run
wrangler dev --local --port 9785 --inspector-port 9786 --local-upstream gateway.divine.video --upstream-protocol https --var SHORT_LINK_CREATION_ENABLED:true --persist-to .wrangler/anonymous-short-links-smoke
cargo test --locked --test short_links_local -- --ignored --test-threads=1
```

Tests exercise anonymous HTTP creation/reuse, CORS, invalid destinations, compatibility with old Authorization headers, and125 concurrent anonymous calls with exactly120 quota admits. The quota test uses an unused synthetic IP window; use fresh local state when repeating within a minute. Admitted upstream calls may return controlled503 if the deadline expires.

Default cargo test does not start a Worker or use a production bearer. CI runs unit/contract tests, compiles the runtime harness, and builds the Worker without deploying. Production deploys use the repository's checked-main release workflow.
