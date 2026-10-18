import hashlib
import json
import os
import re
import subprocess
import sys
import time
import urllib.request
from pathlib import Path


def load_fixture(environment):
    files = {}
    for key, variable, suffix in [
        ("project_path", "MENDIMARU_STUDIO_PARITY_PROJECT_PATH", ".mpr"),
        ("suite_path", "MENDIMARU_STUDIO_PARITY_SUITE", ".json"),
    ]:
        value = environment.get(variable, "")
        assert value, f"Set {variable} to an absolute file path on the runner"
        path = Path(value)
        assert path.is_absolute(), f"{variable} must be absolute"
        assert path.suffix.lower() == suffix, f"{variable} has an unexpected file type"
        path = path.resolve(strict=True)
        assert path.is_file(), f"{variable} must identify a file"
        files[key] = path
    version = environment.get("MENDIMARU_STUDIO_PARITY_VERSION", "")
    assert not version or re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version), "Invalid expected Studio version"
    files["expected_version"] = version
    return files


def validate_suite(suite_path):
    # 공통 suite 검증을 재사용하며 VM/Studio를 시작하기 전에 실패시킨다.
    validator = Path(__file__).resolve().with_name("studio-f5-parity-gate.mjs").as_uri()
    subprocess.run(
        ["node", "--input-type=module", "-e",
         "import { readFileSync } from 'node:fs'; "
         f"import {{ validateSuiteShape }} from {json.dumps(validator)}; "
         "validateSuiteShape(JSON.parse(readFileSync(process.argv[1], 'utf8')), process.argv[1]);",
         str(suite_path)], check=True, timeout=30,
    )


def select_project(fixture, projects, installed):
    # CLI와 같은 canonical MPR 경로 digest를 사용한다. 이름이 같은 앱도 구별한다.
    project_id = "project_" + hashlib.sha256(os.fsencode(fixture["project_path"])).hexdigest()
    matches = [project for project in projects if project.get("projectId") == project_id]
    assert len(matches) == 1, "The configured MPR must resolve to exactly one discovered project"
    project = matches[0]
    version = project.get("requiredVersion")
    assert isinstance(version, str) and re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version), "The selected project has no exact Studio version"
    expected = fixture["expected_version"]
    assert not expected or version == expected, "The project's Studio version differs from the configured expectation"
    assert any(item.get("version") == version for item in installed), "Install the selected project's exact Studio version on the runner"
    return project, version


def project_directory_in_share(project_path, config):
    share = Path(config["sharedDirectory"]).resolve(strict=True)
    assert project_path.is_relative_to(share), "The configured project must be inside the configured shared directory"
    return project_path.parent


def studio_window_class(version):
    assert re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version), "Invalid Studio version"
    return "^mendimaru-studio-pro-" + version.replace(".", "-") + "$"


def select_project_window(windows, project_name):
    assert isinstance(project_name, str) and project_name, "The selected project has no name"
    title = re.compile(r"(?<!\w)" + re.escape(project_name) + r"(?!\w)")
    matches = [window for window, name in windows if title.search(name)]
    assert len(matches) <= 1, "The selected project's visible Studio window is ambiguous"
    return matches[0] if matches else None


def cli(*args, timeout=150):
    print("CLI:", " ".join(args), flush=True)
    p = subprocess.run(
        [binary, *args, "--json"],
        text=True, capture_output=True, timeout=timeout,
    )
    try:
        envelope = json.loads(p.stdout or p.stderr)
    except ValueError as error:
        raise RuntimeError(
            f"{args[:2]} returned no JSON: exit={p.returncode}, "
            f"stderr={p.stderr[:1200]}"
        ) from error
    if p.returncode != 0 or not envelope.get("ok"):
        raise RuntimeError(f"{args[:2]} failed: {envelope.get('error')}")
    return envelope


def build_generation(project):
    generation = hashlib.sha256()
    count = 0
    for root in [project / "deployment/web/dist", project / "deployment/model"]:
        if not root.is_dir():
            continue
        for p in sorted(root.rglob("*")):
            if not p.is_file() or p.is_symlink():
                continue
            stat = p.stat()
            generation.update(
                f"{p.relative_to(project)}:{stat.st_size}:{stat.st_mtime_ns}\n".encode()
            )
            count += 1
    assert count > 0, "Studio has no generated build files"
    return generation.hexdigest()


def deliver_f5(window):
    subprocess.run(["xdotool", "windowfocus", "--sync", window], check=True, timeout=15)
    subprocess.run(["xdotool", "key", "--clearmodifiers", "alt+f"], check=True, timeout=15)
    time.sleep(0.5)
    subprocess.run(["xdotool", "key", "--clearmodifiers", "Escape"], check=True, timeout=15)
    subprocess.run(["xdotool", "key", "--clearmodifiers", "F5"], check=True, timeout=15)


def wait_for_generated_build(project, watcher, minimum_seconds=0):
    previous = None
    stable_since = None
    started = time.monotonic()
    deadline = started + 240
    while time.monotonic() < deadline:
        assert watcher.poll() is None, "The generated-asset watcher stopped"
        try:
            current = build_generation(project)
        except (FileNotFoundError, AssertionError):
            # A real Studio rebuild may replace the generated directories.
            previous = stable_since = None
            time.sleep(5)
            continue
        now = time.monotonic()
        if current != previous:
            previous = current
            stable_since = now
        elif now - stable_since >= 20 and now - started >= minimum_seconds:
            return current
        print("Waiting for generated build files to settle", flush=True)
        time.sleep(5)
    raise RuntimeError("Studio's generated files did not stabilize")


def main():
    global binary, workspace
    fixture = load_fixture(os.environ)
    validate_suite(fixture["suite_path"])
    config_root = Path(os.environ.get("MENDIMARU_CONFIG_DIR") or
                       Path.home() / ".config/com.ggobp.mendimaru")
    config = json.loads((config_root / "config.json").read_text())
    project_directory = project_directory_in_share(fixture["project_path"], config)
    engine = config["containerRuntime"]
    assert engine in ("docker", "podman"), "Unsupported configured container runtime"
    subprocess.run([engine, "ps", "--format", "{{.Names}}"], check=True,
                   capture_output=True, text=True, timeout=30)
    if sys.argv[1:] == ["--check-fixture"]:
        print("Parity fixture files and suite validation: OK", flush=True)
        return
    assert not sys.argv[1:], "expected no arguments or --check-fixture"
    binary = os.environ["MENDIMARU_STUDIO_PARITY_BINARY"]
    workspace = Path(os.environ["GITHUB_WORKSPACE"])
    source_commit = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=workspace, text=True,
    ).strip()
    tag = os.environ["RELEASE_TAG"]
    assert re.fullmatch(r"v[0-9]+(?:\.[0-9]+){2}", tag), "Invalid release tag"
    tag_source = subprocess.check_output(
        ["git", "rev-parse", f"{tag}^{{commit}}"], cwd=workspace, text=True,
    ).strip()
    assert source_commit == tag_source, "The release source must match its immutable tag"
    subprocess.run(["git", "diff", "--exit-code", "HEAD", "--"], cwd=workspace, check=True)
    print("Verified release source:", source_commit, flush=True)
    print("Release binary SHA-256:", hashlib.sha256(Path(binary).read_bytes()).hexdigest(), flush=True)
    studio_id = None
    runtime_id = None
    shared_id = None
    watcher = None
    watcher_log = None
    watcher_output = None
    (workspace / "artifacts/e2e/studio-f5-parity.json").unlink(missing_ok=True)

    try:
        assert cli("env", "ensure", "--timeout-seconds", "300", timeout=330)["data"]["ready"]
        # Inspect retained owners before any legacy guest operation can open RDP.
        owners = cli("studio", "status")
        assert not owners["data"], "An existing Studio owner needs to finish"
        existing = cli("studio", "status", "--refresh", "--timeout-seconds", "90")
        assert not existing["data"], "The release fixture already has an active Studio owner"
        project, version = select_project(
            fixture, cli("project", "list")["data"], cli("studio", "list")["data"],
        )
        records = cli("runtime", "list")["data"]["sessions"]
        assert not any(r["state"] not in ["stopped", "failed"] for r in records), "An existing Runtime owner needs to finish"
        prepared = cli(
            "runtime", "start", "--mode", "studio-run-locally",
            "--timeout-seconds", "600", timeout=630,
        )
        runtime_id = prepared["runtimeSessionId"]
        assert prepared["data"]["runtime"]["state"] != "failed"
        print("Owned Runtime forwarding:", runtime_id, flush=True)

        # The documented UNC repair changes only generated bundler inputs. The
        # actual release decision still requires an unmodified Chromium pass.
        watcher_log = (workspace / "artifacts/e2e/release-asset-watcher.ndjson")
        watcher_log.parent.mkdir(parents=True, exist_ok=True)
        watcher_output = watcher_log.open("w")
        watcher = subprocess.Popen(
            [binary, "assets", "watch", "--project-id", project["projectId"],
             "--rewrite-generated-assets"],
            stdout=watcher_output, stderr=subprocess.STDOUT,
        )
        time.sleep(1)
        assert watcher.poll() is None, "The generated-asset watcher failed to start"

        launched = cli(
            "studio", "start", "--version", version,
            "--project-id", project["projectId"], "--timeout-seconds", "360",
            timeout=390,
        )
        studio_id = launched.get("studioSessionId")
        assert studio_id and re.fullmatch(r"studio-\d+-\d{18}", studio_id)
        print("Owned Studio:", studio_id, flush=True)

        # 버전에서 창 class를 유도하고 선택한 프로젝트 창만 사용한다.
        # 로그인/변환 모달은 좌표나 언어별 버튼을 추측해 닫지 않는다.
        window = None
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            p = subprocess.run(
                ["xdotool", "search", "--onlyvisible", "--class", studio_window_class(version)],
                text=True, capture_output=True, timeout=10,
            )
            assert p.returncode in (0, 1), "Studio window discovery failed"
            observed = []
            for wid in p.stdout.splitlines():
                title = subprocess.check_output(
                    ["xdotool", "getwindowname", wid], text=True, timeout=10,
                ).strip()
                observed.append((wid, title))
            window = select_project_window(observed, project["name"])
            if window:
                break
            print("Waiting for the selected project window; visible Studio windows:", len(observed), flush=True)
            time.sleep(5)
        assert window, ("The selected project's visible RemoteApp window did not appear. "
                        "Prepare Studio sign-in and resolve project conversion/update dialogs "
                        "on the runner before retrying; the gate never guesses dialog coordinates.")
        print("Allowing project load to settle before F5", flush=True)
        time.sleep(45)
        # On a cold shared project, Studio can retain the initial UNC bundle
        # failure even after the watcher has repaired inputs and Rspack rebuilt.
        # Warm that same owned session, then issue the F5 used for validation.
        # Neither this warm-up nor an assisted browser run satisfies the gate.
        deliver_f5(window)
        print("Warming generated bundler inputs before release F5", flush=True)
        wait_for_generated_build(project_directory, watcher, minimum_seconds=75)
        deliver_f5(window)
        print("Delivered ordinary Studio F5", flush=True)

        # studio.start already calls link_studio_runtime_session. Starting
        # this Runtime a second time asks the guest for fresh Studio metadata
        # via another RDP operation, disconnecting the RemoteApp we must keep.
        # Status/wait use the existing authenticated keeper, not guest discovery.
        runtime = cli(
            "runtime", "status", "--session-id", runtime_id,
            "--timeout-seconds", "90", timeout=120,
        )["data"]
        if "runtime" in runtime:
            runtime = runtime["runtime"]
        assert runtime["studioSessionId"] == studio_id, "Studio must bind the owned preparation"
        ready = cli(
            "runtime", "wait", "--session-id", runtime_id,
            "--timeout-seconds", "600", timeout=630,
        )["data"]
        # runtime.wait exposes the RuntimeStatus directly.
        if "runtime" in ready:
            ready = ready["runtime"]
        assert ready["state"] == "ready" and ready["httpReady"]
        print("Studio Runtime is HTTP-ready:", runtime_id, flush=True)
        with urllib.request.urlopen(ready["url"], timeout=30) as response:
            assert response.status < 500
            response.read()

        previous = wait_for_generated_build(project_directory, watcher)
        marker = Path(os.environ["RUNNER_TEMP"]) / "mendimaru-release-build-generation"
        marker.write_text(previous + "\n")
        print("Owned Studio keeper status before preparation:", json.dumps(
            cli("studio", "status", "--session-id", studio_id)["data"],
        ), flush=True)
        preparation_deadline = time.monotonic() + 45
        while True:
            try:
                descriptor = cli(
                    "browser", "session", "prepare", "--runtime-session-id", runtime_id,
                    "--build-marker", str(marker),
                )["data"]
                break
            except RuntimeError as error:
                if "no verified environment" not in str(error):
                    raise
                if time.monotonic() < preparation_deadline:
                    print("Waiting for complete owned environment observations:", error, flush=True)
                    time.sleep(5)
                    continue
                # Observe the same real F5 target through the tagged release's
                # standalone browser path. Diagnostic evidence never replaces
                # the required comparable shared preparation or passes this job.
                diagnostic = subprocess.run(
                    [binary, "browser", "test", "--runtime-session-id", runtime_id,
                     "--suite-path", os.environ["MENDIMARU_STUDIO_PARITY_SUITE"],
                     "--build-marker", str(marker), "--asset-mirror", "off",
                     "--fail-on-console-error", "--fail-on-network-failure", "--json"],
                    text=True, capture_output=True, timeout=660,
                )
                try:
                    envelope = json.loads(diagnostic.stdout or diagnostic.stderr)
                    data = envelope.get("data") or {}
                    print("Preparation environment diagnostic:", json.dumps({
                        "exitCode": diagnostic.returncode,
                        "error": envelope.get("error"),
                        "outcome": data.get("outcome"),
                        "environment": data.get("environment"),
                    }), flush=True)
                except ValueError:
                    print("The standalone environment diagnostic returned no JSON", flush=True)
                raise
        shared_id = descriptor["sessionId"]
        assert descriptor["state"] == "ready"
        assert descriptor["preparation"]["comparable"] is True
        print("Prepared comparable shared session:", shared_id, flush=True)
        gate_env = os.environ.copy()
        gate_env["MENDIMARU_STUDIO_PARITY_SHARED_SESSION_ID"] = shared_id
        subprocess.run(
            ["node", str(Path(__file__).with_name("studio-f5-parity-gate.mjs"))],
            env=gate_env, check=True, timeout=1320,
        )
        assert build_generation(project_directory) == previous, "The build changed during parity verification"
    finally:
        primary_failure = sys.exc_info()[0] is not None
        cleanup_failures = []
        if shared_id:
            try:
                cli("browser", "session", "finalize", "--shared-session-id", shared_id)
            except Exception as error:
                cleanup_failures.append(error)
        if studio_id:
            try:
                cli("studio", "stop", "--session-id", studio_id, "--timeout-seconds", "120", timeout=150)
            except Exception as error:
                cleanup_failures.append(error)
        if runtime_id:
            # A Studio stop reply confirms process exit before keeper forwarding
            # cleanup finishes. Join that cleanup before Actions reaps the keeper.
            deadline = time.monotonic() + 240
            while True:
                try:
                    cli("runtime", "stop", "--session-id", runtime_id,
                        "--timeout-seconds", "180", timeout=210)
                    break
                except Exception as error:
                    if time.monotonic() >= deadline:
                        cleanup_failures.append(error)
                        break
                    print("Waiting for owned Runtime cleanup:", error, flush=True)
                    time.sleep(5)
        # A failed forwarding preparation may return no Runtime ID while
        # still rolling back/restarting the configured guest. Join readiness
        # even on that early error, not only after a successfully owned record.
        try:
            assert cli("env", "ensure", "--timeout-seconds", "300", timeout=330)["data"]["ready"]
        except Exception as error:
            cleanup_failures.append(error)
        if watcher is not None:
            try:
                watcher.terminate()
                watcher.wait(timeout=20)
                print("Generated-asset watcher stopped", flush=True)
            except Exception as error:
                cleanup_failures.append(error)
        if watcher_output is not None:
            watcher_output.close()
        for error in cleanup_failures:
            print("Owner cleanup error:", error, flush=True)
        if cleanup_failures and not primary_failure:
            raise cleanup_failures[0]


if __name__ == "__main__":
    main()
