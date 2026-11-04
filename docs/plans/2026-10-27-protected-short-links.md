# Public short-link creation

The REST gateway serves public creation at https://gateway.divine.video/api/links. No account, signature or client credential is required. The Fastly creator remains server-only; its bearer is a Cloudflare secret and never reaches clients. Public resolution stays at https://divine.video/api/links/:code.

POST accepts at most 4096 bytes of application/json containing one url. Only canonical Divine video/profile targets are accepted, with full identifiers. The endpoint requires the canonical gateway URL; it rejects query strings and disallowed browser origins. Browser origins https://divine.video and https://www.divine.video are allowed; native and command-line requests may omit Origin. Existing clients may continue sending Authorization headers, but creation ignores them.

The existing SQLite Durable Object enforces 120 requests per trusted client IPv4 address or IPv6 /64 per fixed minute, 300 per network per UTC day, and 1,000 across the service per UTC day. Equivalent addresses normalize before hashing; invalid or missing trusted addresses fail closed. All three counters are checked synchronously and charged in one SQLite statement. Indexed cleanup expires minute and daily rows. Existing canonical IPv4 minute counters survive deployment; historical individual-IPv6 counters cannot be retroactively aggregated into /64 counters. No per-account quota or login is needed. Minute exhaustion returns 429 with Retry-After: 60; daily exhaustion returns the seconds until midnight UTC. Clients fall back to full URLs if a cap is exhausted. A caller can exhaust the shared cap, pausing new short links for everyone until the next day. These initial rollout budgets bound request volume, not the eventual size of permanent storage.

Upstream is fixed to https://divine.video/api/links and uses only the server-held bearer. Redirects are disabled, request/response sizes bounded, and one 1500 ms deadline covers request streaming, the limiter and upstream response. A limiter call already in flight can charge quota after the caller times out. Missing upstream configuration is checked before quota is charged. Additive upstream metadata is ignored while required mapping fields remain validated. The complete returned code/short_url/url mapping must match the requested target. All responses are no-store. SHORT_LINK_CREATION_ENABLED=true enables creation; false pauses creation while existing links keep resolving.

## Rollout scope

Public creation, app sharing and HTTP redirect verification are the authorized release scope. Recurring backups, alert delivery and physical-device testing are not prerequisites for this release. This records the maintainer's scope decision; it does not claim those operations are active or tested. This anonymous API supersedes the authenticated rollout proposal in PR #9. Existing redirects and `/publish` authentication are unaffected.

## Isolated runtime verification

```sh
worker-build --release
npm ci --ignore-scripts
npm test
```

Tests execute the real Wasm handler and SQLite limiter in fresh local state. They intercept every outbound request, use only synthetic fixtures and fail on unexpected network access. No production bearer or physical device is needed. Coverage includes concurrent minute and daily budgets, expiry and existing-counter preservation, anonymous creation, IPv6 network normalization, compatibility with old Authorization headers, CORS, missing configuration and body/limiter deadlines. CI builds the Worker and runs these tests before retaining its checked bundle.

`cargo test` also includes the existing read/auth-rejection integration tests against the live gateway; the optional creation tests in `tests/short_links_local.rs` stay ignored by default. Those optional tests need a separately started local Wrangler and upstream access. They are not part of the isolated suite or a release prerequisite. Production deployment follows the checked-main release runbook.
