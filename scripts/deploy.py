"""Main-only release driver. Never invoked by offline tests without mocks."""

import json
import os
from pathlib import Path
import re
import subprocess
import urllib.request


WORKER = "divine-rest-gateway"
HEALTH = "https://gateway.divine.video/health"
WRANGLER = ["npx", "--yes", "wrangler@4.50.0"]


def get_json(url, token):
    request = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


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
    return result["result"]["deployments"][0]


def require_version(deployment, version):
    if deployment["versions"] != [{"version_id": version, "percentage": 100}]:
        raise RuntimeError("Uploaded version is not serving 100% of production")


def check_health():
    with urllib.request.urlopen(HEALTH, timeout=30) as response:
        if response.status != 200 or response.read(32) != b"ok":
            raise RuntimeError("Production health check failed")


def main():
    env = os.environ
    receipt = {"source_sha": env.get("GITHUB_SHA"), "environment": "production",
               "worker": WORKER, "wrangler": "4.50.0", "result": "failed"}
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
        # This check runs inside the production concurrency lock, after environment approval.
        deployed = subprocess.run(WRANGLER + ["deploy"], text=True,
                                  stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        print(deployed.stdout, flush=True)
        deployed.check_returncode()
        versions = re.findall(r"Current Version ID:\s*([0-9a-f-]{36})", deployed.stdout)
        if len(versions) != 1:
            raise RuntimeError("Wrangler did not report exactly one uploaded version")
        receipt["version_id"] = versions[0]
        active = active_deployment(account, token)
        require_version(active, versions[0])
        receipt["deployment_id"] = active["id"]
        check_health()
        # Recheck after the public probe so a concurrent out-of-band release is not success.
        require_version(active_deployment(account, token), versions[0])
        receipt["result"] = "verified"
    finally:
        payload = json.dumps(receipt, indent=2)
        Path("deployment-receipt.json").write_text(payload + "\n")
        if env.get("GITHUB_STEP_SUMMARY"):
            with open(env["GITHUB_STEP_SUMMARY"], "a") as summary:
                summary.write("## Production deployment\n```json\n" + payload + "\n```\n")
        print(payload)


if __name__ == "__main__":
    main()
