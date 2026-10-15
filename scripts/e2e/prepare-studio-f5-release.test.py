"""Orchestration fixtures; these do not claim a live Studio/browser pass."""
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location(
    "studio_release", Path(__file__).with_name("prepare-studio-f5-release.py"),
)
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class GenericReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.project_path = self.root / "arbitrary app" / "애플리케이션.mpr"
        self.project_path.parent.mkdir()
        self.project_path.write_text("model fixture")
        self.suite_path = self.root / "acceptance.browser.json"
        self.suite_path.write_text(json.dumps({
            "schemaVersion": "1.0.0", "name": "generic-app",
            "tests": [{"name": "edit", "steps": [
                {"action": "click", "locator": {"by": "role", "role": "button", "name": "Save"}},
                {"action": "expectText", "locator": {"by": "label", "value": "Result"}, "value": "Saved"},
            ]}],
        }))
        self.environment = {
            "MENDIMARU_STUDIO_PARITY_PROJECT_PATH": str(self.project_path),
            "MENDIMARU_STUDIO_PARITY_SUITE": str(self.suite_path),
        }
        self.project_id = "project_" + hashlib.sha256(os.fsencode(self.project_path)).hexdigest()

    def project(self, version="11.12.6", project_id=None):
        return {"projectId": project_id or self.project_id,
                "name": self.project_path.stem, "requiredVersion": version}

    def test_configuration_requires_explicit_existing_files(self):
        self.assertEqual(gate.load_fixture(self.environment)["project_path"], self.project_path)
        for key in self.environment:
            with self.subTest(key=key):
                with self.assertRaisesRegex(AssertionError, "Set .* absolute"):
                    gate.load_fixture({k: v for k, v in self.environment.items() if k != key})
                with self.assertRaisesRegex(AssertionError, "absolute"):
                    gate.load_fixture(dict(self.environment, **{key: "relative.mpr"}))
        with self.assertRaises(FileNotFoundError):
            gate.load_fixture(dict(self.environment, MENDIMARU_STUDIO_PARITY_PROJECT_PATH=str(self.root / "missing.mpr")))
        with self.assertRaisesRegex(AssertionError, "Invalid expected"):
            gate.load_fixture(dict(self.environment, MENDIMARU_STUDIO_PARITY_VERSION="latest"))

    def test_same_named_apps_resolve_by_canonical_path_not_name(self):
        other = self.root / "another" / self.project_path.name
        other.parent.mkdir()
        other.write_text("another model fixture")
        other_id = "project_" + hashlib.sha256(os.fsencode(other)).hexdigest()
        fixture = gate.load_fixture(self.environment)
        selected, version = gate.select_project(
            fixture, [self.project("10.24.26", other_id), self.project()],
            [{"version": "10.24.26"}, {"version": "11.12.6"}],
        )
        self.assertEqual(selected["projectId"], self.project_id)
        self.assertEqual(version, "11.12.6")
        alias = self.root / "alias.mpr"
        alias.symlink_to(self.project_path)
        self.assertEqual(gate.load_fixture(dict(self.environment, MENDIMARU_STUDIO_PARITY_PROJECT_PATH=str(alias)))["project_path"], self.project_path)
        with self.assertRaisesRegex(AssertionError, "exactly one"):
            gate.select_project(fixture, [self.project(project_id=other_id)], [{"version": version}])
        with self.assertRaisesRegex(AssertionError, "exactly one"):
            gate.select_project(fixture, [self.project(), self.project()], [{"version": version}])

    def test_version_is_project_metadata_with_optional_expectation(self):
        fixture = gate.load_fixture(self.environment)
        for version in ["10.24.26", "11.12.6", "12.1.7"]:
            self.assertEqual(gate.select_project(fixture, [self.project(version)], [{"version": version}])[1], version)
        with self.assertRaisesRegex(AssertionError, "Install.*exact"):
            gate.select_project(fixture, [self.project()], [{"version": "12.1.7"}])
        for unknown in [None, "", "latest"]:
            with self.assertRaisesRegex(AssertionError, "no exact"):
                gate.select_project(fixture, [self.project(unknown)], [{"version": "11.12.6"}])
        expected = gate.load_fixture(dict(self.environment, MENDIMARU_STUDIO_PARITY_VERSION="10.24.26"))
        with self.assertRaisesRegex(AssertionError, "differs"):
            gate.select_project(expected, [self.project()], [{"version": "11.12.6"}])

    def test_project_scope_rejects_outside_share_including_symlink(self):
        config = {"sharedDirectory": str(self.project_path.parent)}
        self.assertEqual(gate.project_directory_in_share(self.project_path, config), self.project_path.parent)
        alias = self.project_path.parent / "outside.mpr"
        target = self.root / "outside.mpr"
        target.write_text("fixture")
        alias.symlink_to(target)
        with self.assertRaisesRegex(AssertionError, "inside.*shared"):
            gate.project_directory_in_share(alias.resolve(), config)

    def test_window_selection_uses_version_and_unambiguous_project_title(self):
        for version in ["10.24.26", "11.12.6", "12.1.7"]:
            self.assertEqual(gate.studio_window_class(version), "^mendimaru-studio-pro-" + version.replace(".", "-") + "$")
        with self.assertRaises(AssertionError):
            gate.studio_window_class("11.12.6|.*")
        self.assertEqual(gate.select_project_window([("1", "Application - Studio"), ("2", "App.mpr - Studio")], "App"), "2")
        self.assertIsNone(gate.select_project_window([("1", "Studio - Sign In")], "App"))
        with self.assertRaisesRegex(AssertionError, "ambiguous"):
            gate.select_project_window([("1", "App - Studio"), ("2", "App.mpr - Studio")], "App")

    def test_suite_preflight_keeps_state_change_and_value_assertion_requirement(self):
        gate.validate_suite(self.suite_path)
        self.suite_path.write_text(json.dumps({
            "schemaVersion": "1.0.0", "tests": [{"name": "navigation only", "steps": [{"action": "goto", "path": "/"}]}],
        }))
        with self.assertRaises(subprocess.CalledProcessError):
            gate.validate_suite(self.suite_path)

    def test_preflight_never_launches_vm_and_uses_configured_share(self):
        config_root = self.root / "config"
        config_root.mkdir()
        (config_root / "config.json").write_text(json.dumps({"sharedDirectory": str(self.root), "containerRuntime": "docker"}))
        with patch.dict(os.environ, dict(self.environment, MENDIMARU_CONFIG_DIR=str(config_root)), clear=True), \
             patch.object(gate.sys, "argv", ["prepare", "--check-fixture"]), \
             patch.object(gate, "validate_suite"), patch.object(gate.subprocess, "run") as run, \
             patch.object(gate, "cli") as cli, \
             patch("sys.stdout", new=io.StringIO()):
            gate.main()
            cli.assert_not_called()
            self.assertEqual(run.call_args.args[0], ["docker", "ps", "--format", "{{.Names}}"])

    def test_full_orchestration_uses_selected_app_version_and_cleans_up_after_browser_failure(self):
        for version, browser_failure in [("10.24.26", False), ("11.12.6", True)]:
            with self.subTest(version=version, browser_failure=browser_failure):
                self.run_orchestration(version, browser_failure)

    def run_orchestration(self, version, browser_failure):
        config_root = self.root / "config"
        config_root.mkdir(exist_ok=True)
        (config_root / "config.json").write_text(json.dumps({"sharedDirectory": str(self.root), "containerName": "AnyConfiguredVM", "containerRuntime": "podman"}))
        binary = self.root / "release-binary"
        binary.write_text("release binary fixture")
        generated = self.project_path.parent / "deployment/model"
        generated.mkdir(parents=True, exist_ok=True)
        (generated / "model.json").write_text("generated model fixture")
        studio_id = "studio-42-123456789012345678"
        runtime_id = "runtime_" + "1" * 32
        shared_id = "shared_" + "2" * 32
        runtime = {"studioSessionId": studio_id, "state": "ready", "httpReady": True, "url": "http://127.0.0.1:8080"}
        cli_calls, run_calls = [], []

        def cli(*args, **kwargs):
            cli_calls.append(args)
            pair = args[:2]
            if pair == ("env", "ensure"): return {"data": {"ready": True}}
            if pair == ("studio", "status"): return {"data": []}
            if pair == ("project", "list"): return {"data": [self.project(version)]}
            if pair == ("studio", "list"): return {"data": [{"version": version}]}
            if pair == ("runtime", "list"): return {"data": {"sessions": []}}
            if pair == ("runtime", "start"): return {"runtimeSessionId": runtime_id, "data": {"runtime": runtime}}
            if pair == ("studio", "start"): return {"studioSessionId": studio_id}
            if pair in [("runtime", "status"), ("runtime", "wait")]: return {"data": runtime}
            if args[:3] == ("browser", "session", "prepare"):
                return {"data": {"sessionId": shared_id, "state": "ready", "preparation": {"comparable": True}}}
            if pair in [("studio", "stop"), ("runtime", "stop")] or args[:3] == ("browser", "session", "finalize"):
                return {"data": {}}
            raise AssertionError(args)

        def run(args, **kwargs):
            run_calls.append(args)
            if args[:2] == ["xdotool", "search"]:
                self.assertEqual(args[-1], gate.studio_window_class(version))
                return subprocess.CompletedProcess(args, 0, stdout="123\n")
            if args[:2] == ["node", "scripts/e2e/studio-f5-parity-gate.mjs"]:
                self.assertEqual(kwargs["env"]["MENDIMARU_STUDIO_PARITY_SHARED_SESSION_ID"], shared_id)
                if browser_failure: raise subprocess.CalledProcessError(1, args)
            return subprocess.CompletedProcess(args, 0)

        def output(args, **kwargs):
            return ("a" * 40) if args[0] == "git" else self.project_path.stem + " - Studio"

        watcher = Mock()
        watcher.poll.return_value = None
        http = Mock()
        http.__enter__ = Mock(return_value=Mock(status=200))
        http.__exit__ = Mock(return_value=False)
        environment = dict(self.environment, MENDIMARU_CONFIG_DIR=str(config_root),
                           MENDIMARU_STUDIO_PARITY_BINARY=str(binary), GITHUB_WORKSPACE=str(self.root),
                           RELEASE_TAG="v1.2.3", RUNNER_TEMP=str(self.root))
        with patch.dict(os.environ, environment, clear=True), patch.object(gate.sys, "argv", ["prepare"]), \
             patch.object(gate, "cli", side_effect=cli), patch.object(gate, "validate_suite"), \
             patch.object(gate.subprocess, "run", side_effect=run), patch.object(gate.subprocess, "check_output", side_effect=output), \
             patch.object(gate.subprocess, "Popen", return_value=watcher), \
             patch.object(gate.urllib.request, "urlopen", return_value=http), \
             patch.object(gate.time, "sleep"), patch.object(gate.time, "monotonic", side_effect=range(0, 2000, 10)), \
             patch("sys.stdout", new=io.StringIO()):
            if browser_failure:
                with self.assertRaises(subprocess.CalledProcessError): gate.main()
            else:
                gate.main()
        start = next(args for args in cli_calls if args[:2] == ("studio", "start"))
        self.assertEqual(start[start.index("--version") + 1], version)
        self.assertEqual(start[start.index("--project-id") + 1], self.project_id)
        self.assertIn(["xdotool", "key", "--clearmodifiers", "F5"], run_calls)
        self.assertIn(["podman", "ps", "--format", "{{.Names}}"], run_calls)
        self.assertEqual(sum(args[:2] == ("runtime", "start") for args in cli_calls), 1)
        for prefix in [("browser", "session", "finalize"), ("studio", "stop"), ("runtime", "stop")]:
            self.assertTrue(any(args[:len(prefix)] == prefix for args in cli_calls))
        watcher.terminate.assert_called_once()
        watcher.wait.assert_called_once_with(timeout=20)


if __name__ == "__main__":
    unittest.main()
