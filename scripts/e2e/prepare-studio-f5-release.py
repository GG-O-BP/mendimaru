import hashlib
import ctypes
import json
import os
import re
import subprocess
import sys
import struct
import time
import urllib.request
import zlib
from pathlib import Path

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
for retained_output in ["studio-f5-parity.json", "release-studio-login.png"]:
    (workspace / "artifacts/e2e" / retained_output).unlink(missing_ok=True)


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

def capture_owned_window(window, output):
    # Capture only the owned RemoteApp, never the user's host desktop.
    # X11 + stdlib avoids depending on unpinned screenshot packages.
    class XImage(ctypes.Structure):
        _fields_ = [
            ("width", ctypes.c_int), ("height", ctypes.c_int),
            ("xoffset", ctypes.c_int), ("format", ctypes.c_int),
            ("data", ctypes.c_void_p), ("byte_order", ctypes.c_int),
            ("bitmap_unit", ctypes.c_int), ("bitmap_bit_order", ctypes.c_int),
            ("bitmap_pad", ctypes.c_int), ("depth", ctypes.c_int),
            ("bytes_per_line", ctypes.c_int), ("bits_per_pixel", ctypes.c_int),
            ("red_mask", ctypes.c_ulong), ("green_mask", ctypes.c_ulong),
            ("blue_mask", ctypes.c_ulong),
        ]
    x11 = ctypes.CDLL("libX11.so.6")
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XGetImage.argtypes = [
        ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
        ctypes.c_uint, ctypes.c_uint, ctypes.c_ulong, ctypes.c_int,
    ]
    x11.XGetImage.restype = ctypes.POINTER(XImage)
    x11.XDestroyImage.argtypes = [ctypes.POINTER(XImage)]
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    geometry = subprocess.check_output(
        ["xdotool", "getwindowgeometry", "--shell", window], text=True, timeout=10,
    )
    dimensions = dict(line.split("=", 1) for line in geometry.splitlines())
    width, height = int(dimensions["WIDTH"]), int(dimensions["HEIGHT"])
    assert 0 < width <= 1920 and 0 < height <= 1080
    display = x11.XOpenDisplay(None)
    assert display, "The owned X11 display is unavailable"
    image = None
    try:
        image = x11.XGetImage(display, int(window), 0, 0, width, height, ctypes.c_ulong(-1), 2)
        assert image
        info = image.contents
        assert info.bits_per_pixel == 32 and info.byte_order == 0
        assert (info.red_mask, info.green_mask, info.blue_mask) == (0xff0000, 0xff00, 0xff)
        pixels = ctypes.string_at(info.data, info.bytes_per_line * height)
        rows = []
        for y in range(height):
            row = pixels[y * info.bytes_per_line:y * info.bytes_per_line + width * 4]
            rgb = bytearray(width * 3)
            rgb[0::3], rgb[1::3], rgb[2::3] = row[2::4], row[1::4], row[0::4]
            rows.append(b"\0" + rgb)
        def chunk(kind, data):
            return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(
            b"\x89PNG\r\n\x1a\n" +
            chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)) +
            chunk(b"IDAT", zlib.compress(b"".join(rows))) + chunk(b"IEND", b"")
        )
        return width, height, b"".join(row[1:] for row in rows)
    finally:
        if image:
            x11.XDestroyImage(image)
        x11.XCloseDisplay(display)

try:
    assert cli("env", "ensure", "--timeout-seconds", "300", timeout=330)["data"]["ready"]
    existing = cli("studio", "status", "--refresh", "--timeout-seconds", "90")
    assert not existing["data"], "The release fixture already has an active Studio owner"
    projects = cli("project", "list")["data"]
    matches = [p for p in projects if p["name"] == "IronCalcSpreadUIShowcase"]
    assert len(matches) == 1, "The configured release project must be unique"
    project = matches[0]
    assert project["requiredVersion"] == "11.12.3", "Unexpected release fixture version"
    assert any(p["version"] == project["requiredVersion"] for p in cli("studio", "list")["data"])
    config = json.loads((Path.home() / ".config/com.ggobp.mendimaru/config.json").read_text())
    assert config["containerName"] == "WinBoat", "Unexpected release VM"
    share = Path(config["sharedDirectory"])
    candidates = []
    for root, dirs, files in os.walk(share):
        depth = len(Path(root).relative_to(share).parts)
        dirs[:] = [
            d for d in dirs if depth < 5
            and d not in [".git", "node_modules", "deployment", "target", ".gradle"]
        ]
        if "IronCalcSpreadUIShowcase.mpr" in files:
            candidates.append(Path(root))
    assert len(candidates) == 1, "The release project's build tree must be unique"
    project_directory = candidates[0]
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
        "studio", "start", "--version", project["requiredVersion"],
        "--project-id", project["projectId"], "--timeout-seconds", "360",
        timeout=390,
    )
    studio_id = launched.get("studioSessionId")
    assert studio_id and re.fullmatch(r"studio-\d+-\d{18}", studio_id)
    print("Owned Studio:", studio_id, flush=True)

    # This exact fixture uses Studio 11.12.3. Its UIA adapter is not enabled,
    # so send the ordinary F5 key to its visible, uniquely named RemoteApp.
    window = None
    login_captured = False
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        p = subprocess.run(
            ["xdotool", "search", "--onlyvisible", "--class", "mendimaru-studio-pro-11-12-3"],
            text=True, capture_output=True, timeout=10,
        )
        observed = []
        for wid in p.stdout.splitlines():
            title = subprocess.check_output(
                ["xdotool", "getwindowname", wid], text=True, timeout=10,
            ).strip()
            observed.append((wid, title))
        print("Owned RemoteApp windows:", json.dumps(observed), flush=True)
        project_windows = [
            (wid, title) for wid, title in observed
            if project["name"] in title
        ]
        if len(project_windows) == 1:
            window, title = project_windows[0]
            print("Studio project window:", title, flush=True)
            break
        login_windows = [
            (wid, title) for wid, title in observed
            if title in ["멘딕스 스튜디오 프로 - 로그인", "Mendix Studio Pro - Sign In", "Mendix Studio Pro - Login"]
        ]
        if len(login_windows) == 1 and not login_captured:
            login_window = login_windows[0][0]
            width, height, pixels = capture_owned_window(
                login_window, workspace / "artifacts/e2e/release-studio-login.png",
            )
            login_captured = True
            # The Korean 11.12.3 offline button was visually reviewed in
            # run 38485731858. Never guess coordinates on another layout:
            # authenticate the button's complete RGB region before input.
            assert (width, height) == (896, 706), "The known offline-entry dialog layout changed"
            button = b"".join(
                pixels[(y * width + 500) * 3:(y * width + 664) * 3]
                for y in range(406, 438)
            )
            assert hashlib.sha256(button).hexdigest() == \
                "42245544b9691ec465eed631d2ed324479598454e3c14e6d21be357e8b7ab4f6", \
                "The reviewed Sign In Later button changed; manual inspection is required"
            subprocess.run(
                ["xdotool", "windowfocus", "--sync", login_window],
                check=True, timeout=15,
            )
            subprocess.run(
                ["xdotool", "mousemove", "--window", login_window, "582", "422", "click", "1"],
                check=True, timeout=15,
            )
            print("Selected the verified Sign In Later button; waiting for the exact project", flush=True)
        print("Waiting for the selected project window", flush=True)
        time.sleep(5)
    assert window, "The exact project's visible RemoteApp window did not appear"
    print("Allowing project load to settle before F5", flush=True)
    time.sleep(45)
    subprocess.run(["xdotool", "windowfocus", "--sync", window], check=True, timeout=15)
    # Acquire native menu focus rather than leaving F5 in a WebView.
    subprocess.run(["xdotool", "key", "--clearmodifiers", "alt+f"], check=True, timeout=15)
    time.sleep(0.5)
    subprocess.run(["xdotool", "key", "--clearmodifiers", "Escape"], check=True, timeout=15)
    subprocess.run(["xdotool", "key", "--clearmodifiers", "F5"], check=True, timeout=15)
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

    previous = None
    stable_since = None
    deadline = time.monotonic() + 240
    while time.monotonic() < deadline:
        assert watcher.poll() is None, "The generated-asset watcher stopped"
        current = build_generation(project_directory)
        if current != previous:
            previous = current
            stable_since = time.monotonic()
        elif time.monotonic() - stable_since >= 20:
            break
        print("Waiting for generated build files to settle", flush=True)
        time.sleep(5)
    else:
        raise RuntimeError("Studio's generated files did not stabilize")
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
        ["node", "scripts/e2e/studio-f5-parity-gate.mjs"],
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
            watcher_output.close()
            print("Generated-asset watcher stopped", flush=True)
        except Exception as error:
            cleanup_failures.append(error)
    for error in cleanup_failures:
        print("Owner cleanup error:", error, flush=True)
    if cleanup_failures and not primary_failure:
        raise cleanup_failures[0]
