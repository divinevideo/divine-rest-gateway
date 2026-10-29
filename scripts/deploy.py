"""Main-only release driver. Never invoked by offline tests without mocks."""

import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import tomllib
import urllib.error
import urllib.request


WORKER = "divine-rest-gateway"
HEALTH = "https://gateway.divine.video/health"
WRANGLER_VERSION = "4.50.0"
WRANGLER = ["npx", "--offline", "--yes", f"wrangler@{WRANGLER_VERSION}"]
USER_AGENT = "divine-rest-gateway-deploy"
UPLOAD_CONFIG = Path(".wrangler-release.toml")


def prepare_upload_config():
    """Keep config at repository root so relative paths retain their meaning."""
    source = Path("wrangler.toml").read_text()
    # Remove only the top-level custom-build section; verify the parsed result below.
    prepared = re.sub(r"(?ms)^\[build\]\s*\n.*?(?=^\[|\Z)", "", source)
    prepared = re.sub(r'(?m)^main\s*=.*$', 'main = ".wrangler/release/shim.js"', prepared)
    prepared = "find_additional_modules = true\n" + prepared
    UPLOAD_CONFIG.write_text(prepared)
    require_upload_config()


def require_upload_config():
    source = tomllib.loads(Path("wrangler.toml").read_text())
    source.pop("build", None)
    source["main"] = ".wrangler/release/shim.js"
    source["find_additional_modules"] = True
    prepared = tomllib.loads(UPLOAD_CONFIG.read_text())
    if prepared != source or "build" in prepared:
        raise RuntimeError("Upload config must match source except removed custom build and prebundled module entry")
    domains = [route.get("pattern") for route in source.get("routes", []) if route.get("custom_domain")]
    if source.get("name") != WORKER or len(domains) != 1 or HEALTH != f"https://{domains[0]}/health":
        raise RuntimeError("Release identity constants do not match the configured Worker and domain")


def deploy_worker():
    # The package was resolved and the Worker built in a secret-free step. Do not
    # resolve packages, run the custom build, or bundle code in the upload step.
    command = WRANGLER + ["deploy", "--no-bundle", "--config", str(UPLOAD_CONFIG)]
    output = []
    with subprocess.Popen(command, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as process:
        for line in process.stdout:
            print(line, end="", flush=True)
            output.append(line)
        if process.wait():
            raise RuntimeError("Wrangler deployment failed; inspect the streamed output")
    return "".join(output)


def retry_read(operation):
    """Retry only bounded post-deployment observations, never upload/activation."""
    for attempt in range(3):
        try:
            return operation()
        except (OSError, ValueError, RuntimeError):
            if attempt == 2:
                raise
            time.sleep(2 ** (attempt + 1))


def get_json(url, token):
    request = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}", "User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        # Report structured codes, not arbitrary remote bodies that could contain
        # private details or reflected authentication material.
        try:
            body = json.loads(error.read(16384))
            codes = [item["code"] for item in body.get("errors", [])
                     if isinstance(item.get("code"), int)]
        except (ValueError, AttributeError, TypeError):
            codes = []
        raise RuntimeError(f"API HTTP {error.code}; provider error codes {codes}") from None


def require_main(env, current_main, checkout):
    if (env["GITHUB_REF"] != "refs/heads/main"
            or env["GITHUB_EVENT_NAME"] not in ("push", "workflow_dispatch")
            or env["GITHUB_REPOSITORY"] != "divinevideo/divine-rest-gateway"):
        raise RuntimeError("Production requires a main push or main manual run in this repository")
    if not re.fullmatch(r"[0-9a-f]{40}", env["GITHUB_SHA"]):
        raise RuntimeError("Invalid source SHA")
    if current_main != env["GITHUB_SHA"] or checkout != env["GITHUB_SHA"]:
        raise RuntimeError("Stale run or mismatched checkout; replay the current main run instead")


def active_deployment(account, token):
    result = get_json(
        f"https://api.cloudflare.com/client/v4/accounts/{account}/workers/scripts/{WORKER}/deployments",
        token,
    )
    if result.get("success") is not True:
        raise RuntimeError("Cloudflare deployment lookup failed")
    # Cloudflare defines the first result as the deployment actively serving traffic.
    deployments = result["result"]["deployments"]
    if not deployments:
        raise RuntimeError("No active deployment exists; release automation cannot bootstrap the Worker")
    return deployments[0]


def require_version(deployment, version):
    if deployment["versions"] != [{"version_id": version, "percentage": 100}]:
        raise RuntimeError("Uploaded version is not serving 100% of production")


def check_health():
    request = urllib.request.Request(HEALTH, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=30) as response:
        if response.status != 200 or response.read(32) != b"ok":
            raise RuntimeError("Production health check failed")


def main():
    env = os.environ
    receipt = {"source_sha": env.get("GITHUB_SHA"), "environment": "production",
               "worker": WORKER, "wrangler": WRANGLER_VERSION, "result": "failed"}
    try:
        account = env["CLOUDFLARE_ACCOUNT_ID"]
        token = env["CLOUDFLARE_API_TOKEN"]
        if not re.fullmatch(r"[0-9a-f]{32}", account) or not token:
            raise RuntimeError("Production account or deployment credential is missing")
        # Also require the existing service to have an active deployment. Never bootstrap it.
        receipt["previous_deployment_id"] = active_deployment(account, token)["id"]
        checkout = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
        current_main = get_json(
            "https://api.github.com/repos/divinevideo/divine-rest-gateway/commits/main",
            env["GH_TOKEN"],
        )["sha"]
        require_main(env, current_main, checkout)
        require_upload_config()
        # This check runs inside the production concurrency lock, after environment approval.
        output = deploy_worker()
        versions = re.findall(r"Current Version ID:\s*([0-9a-f-]{36})", output)
        if len(versions) != 1:
            raise RuntimeError("Wrangler did not report exactly one uploaded version")
        receipt["version_id"] = versions[0]
        def verified_active():
            deployment = active_deployment(account, token)
            require_version(deployment, versions[0])
            return deployment

        active = retry_read(verified_active)
        receipt["deployment_id"] = active["id"]
        retry_read(check_health)
        # Recheck after the public probe so a concurrent out-of-band release is not success.
        retry_read(verified_active)
        receipt["result"] = "verified"
    finally:
        payload = json.dumps(receipt, indent=2)
        Path("deployment-receipt.json").write_text(payload + "\n")
        if env.get("GITHUB_STEP_SUMMARY"):
            with open(env["GITHUB_STEP_SUMMARY"], "a") as summary:
                summary.write("## Production deployment\n```json\n" + payload + "\n```\n")
        print(payload)


if __name__ == "__main__":
    if sys.argv[1:] == ["--prepare"]:
        prepare_upload_config()
    elif not sys.argv[1:]:
        main()
    else:
        raise SystemExit("Usage: deploy.py [--prepare]")
