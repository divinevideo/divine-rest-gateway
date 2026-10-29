"""Offline release guards; no requests or deployment commands are executed."""

import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("deploy", Path(__file__).parents[1] / "scripts/deploy.py")
deploy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(deploy)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.sha = "a" * 40
        self.env = {"GITHUB_REF": "refs/heads/main", "GITHUB_EVENT_NAME": "push",
                    "GITHUB_SHA": self.sha, "GITHUB_REPOSITORY": "divinevideo/divine-rest-gateway"}

    def test_push_and_manual_main(self):
        for event in ("push", "workflow_dispatch"):
            self.env["GITHUB_EVENT_NAME"] = event
            deploy.require_main(self.env, self.sha, self.sha)

    def test_reject_untrusted_or_stale_runs(self):
        for key, value in (("GITHUB_REF", "refs/heads/topic"),
                           ("GITHUB_EVENT_NAME", "pull_request"),
                           ("GITHUB_EVENT_NAME", "pull_request_target"),
                           ("GITHUB_REPOSITORY", "someone/fork"),
                           ("GITHUB_SHA", "not-a-sha")):
            with self.subTest(key=key, value=value), self.assertRaises(RuntimeError):
                deploy.require_main(self.env | {key: value}, self.sha, self.sha)
        for main, checkout in (("b" * 40, self.sha), (self.sha, "b" * 40)):
            with self.assertRaises(RuntimeError):
                deploy.require_main(self.env, main, checkout)

    def test_require_full_active_version(self):
        deploy.require_version({"versions": [{"version_id": "new", "percentage": 100}]}, "new")
        for versions in ([], [{"version_id": "old", "percentage": 100}],
                         [{"version_id": "new", "percentage": 50},
                          {"version_id": "old", "percentage": 50}]):
            with self.assertRaises(RuntimeError):
                deploy.require_version({"versions": versions}, "new")

    def test_failed_provider_lookup(self):
        with patch.object(deploy, "get_json", return_value={"success": False}):
            with self.assertRaises(RuntimeError):
                deploy.active_deployment("account", "synthetic-token")

    def run_driver(self, *, stale=False, upload_failure=False, health_failure=False):
        version = "00000000-0000-4000-8000-000000000001"
        active = {"id": "deployment", "versions": [{"version_id": version, "percentage": 100}]}
        env = self.env | {"CLOUDFLARE_ACCOUNT_ID": "0" * 32,
                          "CLOUDFLARE_API_TOKEN": "synthetic-token", "GH_TOKEN": "synthetic-token"}
        result = subprocess.CompletedProcess([], 1 if upload_failure else 0,
                                             stdout=f"Current Version ID: {version}\n")
        with tempfile.TemporaryDirectory() as directory:
            receipt = Path(directory) / "receipt.json"
            with patch.object(deploy.os, "environ", env), \
                    patch.object(deploy, "Path", return_value=receipt), \
                    patch.object(deploy, "active_deployment", return_value=active), \
                    patch.object(deploy, "get_json", return_value={"sha": "b" * 40 if stale else self.sha}), \
                    patch.object(deploy.subprocess, "check_output", return_value=self.sha + "\n"), \
                    patch.object(deploy.subprocess, "run", return_value=result) as upload, \
                    patch.object(deploy, "check_health", side_effect=RuntimeError("unhealthy") if health_failure else None), \
                    patch("builtins.print"):
                if stale or upload_failure or health_failure:
                    with self.assertRaises((RuntimeError, subprocess.CalledProcessError)):
                        deploy.main()
                else:
                    deploy.main()
                self.assertEqual(upload.call_count, 0 if stale else 1)
                return json.loads(receipt.read_text())

    def test_success_receipt(self):
        receipt = self.run_driver()
        self.assertEqual(receipt["result"], "verified")
        self.assertEqual(receipt["source_sha"], self.sha)
        self.assertIn("version_id", receipt)
        self.assertIn("deployment_id", receipt)

    def test_stale_run_never_uploads(self):
        self.assertEqual(self.run_driver(stale=True)["result"], "failed")

    def test_upload_and_health_failures_are_not_success(self):
        self.assertEqual(self.run_driver(upload_failure=True)["result"], "failed")
        self.assertEqual(self.run_driver(health_failure=True)["result"], "failed")


if __name__ == "__main__":
    unittest.main()
