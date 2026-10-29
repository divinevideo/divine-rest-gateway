"""Offline release guards; no requests or deployment commands are executed."""

import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import MagicMock, patch

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

    def test_empty_deployment_list(self):
        with patch.object(deploy, "get_json", return_value={"success": True, "result": {"deployments": []}}):
            with self.assertRaisesRegex(RuntimeError, "No active deployment"):
                deploy.active_deployment("account", "synthetic-token")

    def test_health_probe_request_and_failures(self):
        for status, body in ((200, b"ok"), (200, b"not ok"), (503, b"ok")):
            response = MagicMock(status=status)
            response.read.return_value = body
            response.__enter__.return_value = response
            with patch.object(deploy.urllib.request, "urlopen", return_value=response) as urlopen:
                if status == 200 and body == b"ok":
                    deploy.check_health()
                else:
                    with self.assertRaises(RuntimeError):
                        deploy.check_health()
            request = urlopen.call_args.args[0]
            self.assertEqual(request.full_url, deploy.HEALTH)
            self.assertEqual(request.get_header("User-agent"), deploy.USER_AGENT)

    def test_api_error_reports_only_status_and_codes(self):
        error = deploy.urllib.error.HTTPError("https://example.invalid", 400, "Bad Request", {},
                                             io.BytesIO(b'{"errors":[{"code":9106,"message":"private"}]}'))
        with patch.object(deploy.urllib.request, "urlopen", side_effect=error):
            with self.assertRaisesRegex(RuntimeError, r"API HTTP 400; provider error codes \[9106\]") as raised:
                deploy.get_json("https://example.invalid", "synthetic-token")
            self.assertNotIn("private", str(raised.exception))

    def test_read_retries_are_bounded(self):
        operation = MagicMock(side_effect=[OSError("transient"), "ok"])
        with patch.object(deploy.time, "sleep") as sleep:
            self.assertEqual(deploy.retry_read(operation), "ok")
            sleep.assert_called_once_with(2)
        operation = MagicMock(side_effect=RuntimeError("still wrong"))
        with patch.object(deploy.time, "sleep"), self.assertRaises(RuntimeError):
            deploy.retry_read(operation)
        self.assertEqual(operation.call_count, 3)

    def test_prepared_config_preserves_all_nonbuild_settings(self):
        source = Path(__file__).parents[1] / "wrangler.toml"
        with tempfile.TemporaryDirectory() as directory:
            prepared = Path(directory) / "upload.toml"
            with patch.object(deploy, "UPLOAD_CONFIG", prepared), \
                    patch.object(deploy, "Path", return_value=source):
                deploy.prepare_upload_config()
                actual = deploy.tomllib.loads(prepared.read_text())
                expected = deploy.tomllib.loads(source.read_text())
                expected.pop("build")
                expected["main"] = ".wrangler/release/shim.js"
                expected["find_additional_modules"] = True
                self.assertEqual(actual, expected)
                prepared.write_text(source.read_text())
                with self.assertRaisesRegex(RuntimeError, "removed custom build"):
                    deploy.require_upload_config()

    def test_upload_streams_output_and_never_builds_or_resolves_packages(self):
        process = MagicMock()
        process.__enter__.return_value = process
        process.stdout = iter(["build line\n", "Current Version ID: new\n"])
        process.wait.return_value = 0
        with patch.object(deploy.subprocess, "Popen", return_value=process) as popen, patch("builtins.print") as output:
            self.assertIn("Current Version ID: new", deploy.deploy_worker())
        command = popen.call_args.args[0]
        self.assertIn("--offline", command)
        self.assertIn("--no-bundle", command)
        self.assertIn(str(deploy.UPLOAD_CONFIG), command)
        self.assertEqual(output.call_count, 2)
        process.wait.return_value = 1
        process.stdout = iter(["upload failed\n"])
        with patch.object(deploy.subprocess, "Popen", return_value=process), patch("builtins.print"), self.assertRaises(RuntimeError):
            deploy.deploy_worker()

    def test_workflow_build_precedes_token_and_pins_match(self):
        workflow = (Path(__file__).parents[1] / ".github/workflows/ci.yml").read_text()
        verify, upload = workflow.split("\n  deploy:", 1)
        self.assertNotIn("CLOUDFLARE_API_TOKEN", verify)
        self.assertIn("actions/upload-artifact@", verify)
        self.assertIn("actions/download-artifact@", upload)
        self.assertNotIn("rustup", upload)
        self.assertNotIn("--outdir", upload)
        self.assertLess(upload.index("name: Prepare the trusted uploader"), upload.index("CLOUDFLARE_API_TOKEN:"))
        pins = deploy.re.findall(r"wrangler@([0-9.]+)", workflow)
        self.assertTrue(pins)
        self.assertEqual(set(pins), {deploy.WRANGLER_VERSION})

    def run_driver(self, *, stale=False, upload_failure=False, health_failure=False):
        version = "00000000-0000-4000-8000-000000000001"
        active = {"id": "deployment", "versions": [{"version_id": version, "percentage": 100}]}
        env = self.env | {"CLOUDFLARE_ACCOUNT_ID": "0" * 32,
                          "CLOUDFLARE_API_TOKEN": "synthetic-token", "GH_TOKEN": "synthetic-token"}
        calls = MagicMock()
        with tempfile.TemporaryDirectory() as directory:
            receipt = Path(directory) / "receipt.json"
            with patch.object(deploy.os, "environ", env), \
                    patch.object(deploy, "Path", return_value=receipt), \
                    patch.object(deploy, "active_deployment", return_value=active) as lookup, \
                    patch.object(deploy, "get_json", return_value={"sha": "b" * 40 if stale else self.sha}), \
                    patch.object(deploy.subprocess, "check_output", return_value=self.sha + "\n"), \
                    patch.object(deploy, "require_upload_config"), \
                    patch.object(deploy, "deploy_worker", return_value=f"Current Version ID: {version}\n",
                                 side_effect=RuntimeError("upload failed") if upload_failure else None) as upload, \
                    patch.object(deploy.time, "sleep"), \
                    patch.object(deploy, "check_health", side_effect=RuntimeError("unhealthy") if health_failure else None) as health, \
                    patch("builtins.print"):
                calls.attach_mock(lookup, "lookup")
                calls.attach_mock(upload, "upload")
                calls.attach_mock(health, "health")
                if stale or upload_failure or health_failure:
                    with self.assertRaises(RuntimeError):
                        deploy.main()
                else:
                    deploy.main()
                self.assertEqual(upload.call_count, 0 if stale else 1)
                self.calls = [name for name, _, _ in calls.mock_calls]
                return json.loads(receipt.read_text())

    def test_success_receipt(self):
        receipt = self.run_driver()
        self.assertEqual(receipt["result"], "verified")
        self.assertEqual(receipt["source_sha"], self.sha)
        self.assertEqual(receipt["version_id"], "00000000-0000-4000-8000-000000000001")
        self.assertEqual(receipt["deployment_id"], "deployment")

    def test_success_rechecks_active_version_after_health_probe(self):
        self.run_driver()
        self.assertEqual(self.calls, ["lookup", "upload", "lookup", "health", "lookup"])

    def test_stale_run_never_uploads(self):
        self.assertEqual(self.run_driver(stale=True)["result"], "failed")

    def test_upload_and_health_failures_are_not_success(self):
        self.assertEqual(self.run_driver(upload_failure=True)["result"], "failed")
        self.assertEqual(self.run_driver(health_failure=True)["result"], "failed")


if __name__ == "__main__":
    unittest.main()
