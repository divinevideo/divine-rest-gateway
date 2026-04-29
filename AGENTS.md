# Repository Guidelines

## Project Structure & Module Organization
- Worker and library code lives in `src/`.
- Integration coverage lives in `tests/`.
- User-facing documentation lives in `README.md`, `CHANGELOG.md`, and `docs/`.
- Deployment config lives in `wrangler.toml`.
- Keep new Rust modules focused. Prefer separating auth, cache, relay access, routing, and queue handling instead of growing broad catch-all modules.

## Build, Test, and Development Commands
- `cargo test`: run the Rust test suite.
- `cargo check`: run a fast compile-only validation pass.
- `wrangler dev`: run the Worker locally.
- `wrangler deploy`: deploy the Worker.
- If you change API behavior, auth handling, or queue behavior, update the relevant docs and request examples together.

## Coding Style & Naming Conventions
- Use idiomatic Rust with explicit request, response, and error shapes.
- Prefer focused modules and clear boundaries between cache, relay, and publish logic.
- Keep PRs tightly scoped. Do not mix unrelated cleanup, formatting churn, or speculative refactors into the same change.
- Temporary or transitional code must include `TODO(#issue):` with the tracking issue for removal.

## Pull Request Guardrails
- PR titles must use Conventional Commit format: `type(scope): summary` or `type: summary`.
- Set the correct PR title when opening the PR. Do not rely on fixing it afterward.
- If a PR title changes after opening, verify that the semantic PR title check reruns successfully.
- PR descriptions must include a short summary, motivation, linked issue, and manual test plan.
- Changes to public endpoints, NIP-98 auth, caching, or publish retries should include representative requests or rollout notes when helpful.

## Security & Sensitive Information
- Do not commit secrets, relay credentials, private auth material, or sensitive payload samples.
- Public issues, PRs, branch names, screenshots, and descriptions must not mention corporate partners, customers, brands, campaign names, or other sensitive external identities unless a maintainer explicitly approves it. Use generic descriptors instead.
