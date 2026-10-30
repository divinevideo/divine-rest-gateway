# Production deployment

## Activation prerequisites (operator-owned)

The workflow is shipped disabled. Merging it does not authorize activation,
provision credentials/resources, move traffic, or enable a product feature.
Keep the repository variable `PRODUCTION_CD_ENABLED` unset until these steps are
complete.

1. Confirm with the serving-service owner that `divine-rest-gateway` in the
   intended Cloudflare account serves `gateway.divine.video`. Read the current
   active deployment/version and routes in Cloudflare, then compare them with
   `wrangler.toml`. Record the account and verification in the activation change.
   Do not infer this from the Worker name or a successful health response alone.
2. Compare the existing KV namespace IDs, Durable Object classes and migrations,
   publish queue, consumer/dead-letter queue, custom domain and relay URL with
   the committed configuration. Reuse them; do not silently create replacements.
   Confirm existing runtime secrets by **name only**, including
   `SHORT_LINK_CREATION_TOKEN`. Wrangler deploy preserves secrets,
   but reapplies committed plain variables and routes. In particular, every
   release applies the committed `SHORT_LINK_CREATION_ENABLED=true`, which turns
   on public short-link creation. Reconcile
   any dashboard-only feature setting before activation, not during a release.
3. Configure the GitHub `production` environment before enabling CD. Restrict it
   to the protected `main` branch and apply the owner's production approval rules
   (required reviewers, prevention of self-review/bypass as appropriate). Reuse
   existing protections if present; the workflow cannot create them. If the
   environment is missing, creating and protecting it is an explicit prerequisite.
4. Set `CLOUDFLARE_ACCOUNT_ID` as a **production environment variable**. Store
   `CLOUDFLARE_API_TOKEN` as a **production environment secret**, not a repository
   secret available to PR jobs. Use a scoped token, not a global API key. Limit
   it to the intended account and required zone. The deploy needs Workers Scripts
   Edit (also permits deployment-version reads), Workers Routes Edit for the
   configured custom domain and Zone Read for zone lookup; the existing KV/queue
   bindings can also require Workers KV Storage Edit and Queues Edit. Confirm the
   minimal permissions for this account's bindings with its owner; do not grant
   all-account permissions or provision new resources to solve a permission error.
   No runtime secret values belong in GitHub variables, config, or logs.
5. Protect `main`, including workflow changes, with required CI checks and the
   repository's normal review rules. After the above verification and approval,
   the operator may set the **repository-level** `PRODUCTION_CD_ENABLED=true`.
   Then perform one controlled main release and inspect its evidence below.
   GitHub evaluates this repository-level switch before entering the environment.

## Release flow and evidence

The `verify` job runs locked Rust library/protocol tests, offline deployment
guard tests, compiles the runtime harness, and performs the pinned
`wrangler@4.50.0 deploy --dry-run` build. It never runs live integration tests or
side-effecting test traffic. Production `deploy` requires that exact workflow's
`verify` success and uses the immutable `github.sha` checkout, not a moving branch.
It uses the repository's existing Wrangler release build command in the
`verify` job, which has no production environment or deployment token. That job
retains only the checked JavaScript and Wasm bundle as the `checked-worker`
artifact for seven days. The production job runs on a fresh runner and downloads
that artifact from this same workflow run, not another run or branch. It does not
install Rust, run cargo or execute build-produced code. It installs the trusted
pinned Wrangler uploader and prepares a root-level upload config from its own
fresh checkout, parsed and compared with
`wrangler.toml`: `[build]` is removed, `main` points at the prebundled
`.wrangler/release/shim.js`, and `find_additional_modules=true` includes its Wasm
module. All runtime bindings, variables and routes must be identical. The upload validates that comparison,
uses cached Wrangler offline with `--no-bundle`, and does not execute the custom
build or Rust dependency build scripts. Wrangler's own installation and cached
runtime dependencies are trusted uploader code and can still reach the upload
credential; this is not isolation from a compromised Wrangler package. Rust follows the existing stable
toolchain policy; Wrangler and checkout are pinned. Offline tests enforce matching
workflow/driver Wrangler pins and the prepared config comparison.

Only main push and main manual events can enter the production job. No arbitrary
SHA input or PR deployment exists. The `production-worker` concurrency lock is
non-cancelling and covers environment approval, deployment and verification.
Its `queue: max` setting retains up to 100 pending deployment jobs instead of
replacing an existing pending release when another queues. GitHub processes that
queue by arrival at the concurrency group, not by source-commit order. The driver
therefore still re-reads current main inside the lock immediately before starting
Wrangler and fails stale runs. An older, slower verification cannot displace a
newer pending release or overwrite a newer active deployment.
When all 100 pending slots are occupied, additional jobs can be cancelled. Approval
delays, expired seven-day bundle artifacts, and failed checks can also prevent
activation. Inspect cancelled/pending runs and compare the verified release
receipt's source SHA with main before considering a release delivered. Running
production jobs are not cancelled by this workflow's concurrency settings;
superseded PR verification runs can be cancelled. Cancelled pending runs are not
release successes. Manual replay on main reruns validation;
if its recorded SHA is stale, start a new main run rather than rerunning it.

The driver first requires an existing active deployment, then records the new
version reported by Wrangler and verifies it is serving 100% through the
[Cloudflare deployments API](https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/deployments/methods/list/).
It GETs only `/health` with an explicit deployment User-Agent (HTTP 200 and body
`ok`) and rechecks the active version. Each post-deployment read or probe has up
to three attempts with 2- and 4-second backoffs. Upload itself is never retried.
This establishes control-plane version state plus endpoint health, not a
per-request version assertion from the public endpoint. There is no automatic
publish/query smoke traffic, rollback, or resource deletion.

Every driver exit writes a temporary `deployment-receipt.json` on the runner,
prints it in the job log and appends it to the workflow summary: full source SHA, environment,
Worker, Wrangler version, previous deployment ID, uploaded version ID and active
deployment ID when available, and `verified` or `failed`. The run's URL provides
the durable source-to-provider-version link, not the runner-local file (which
is not uploaded and disappears with the runner); retain the summary/log under the
repository's Actions retention policy. A disabled/skipped job is **not** evidence
of a production release. A failed upload, stale run, API read, version mismatch,
or health probe fails the job, even if a provider upload already took effect.
Inspect the active version before retrying any failed/cancelled deployment.

## Rollback and migrations

An operator must authorize rollback separately. First disable future automatic
activation by clearing `PRODUCTION_CD_ENABLED`, cancel queued release jobs, and
wait for or explicitly stop the in-flight release before changing production.
Coordinate manual Wrangler/dashboard deploys under the same release ownership;
the GitHub concurrency lock does not serialize out-of-band operations.

Read the current active deployment and identify the known-good **version ID**
from a verified run or Cloudflare history. Review its compatibility with current
Durable Object/storage state. In the verified account and repository checkout,
use `npx --yes wrangler@4.50.0 rollback <version-id>`, then read deployments again
and GET `/health`. Record the rollback version and affected source SHA in the
incident/change record. Do not replay an old workflow to roll back: its stale
SHA guard intentionally refuses that. Resume CD only after main contains the
desired forward fix and its next release is authorized.

Wrangler deploy applies the ordered `v1` and `v2` Durable Object migrations from
`wrangler.toml` during upload; it does not reset their data. Verify the serving
service's migration history before activation. Future migrations must be
backward-compatible and reviewed before deployment. A code rollback does not
undo migrations, KV writes, queued messages or Durable Object data, and Cloudflare
can reject rollback across incompatible migrations. In that case use a reviewed
forward fix rather than deleting resources or editing migration history.
