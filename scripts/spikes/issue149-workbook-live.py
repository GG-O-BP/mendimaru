#!/usr/bin/env python3
"""Run the disposable IronCalc fixture under the cooperating VM/data locks.

Requires the installed package, a verified snapshot, a real prepared Studio F5
session, and a new private evidence directory. It never starts/stops the VM.
"""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess

assert os.uname().sysname == "Linux"
assert os.environ.get("MENDIMARU_E2E_ALLOW_MUTATION") == "1"
assert os.environ.get("MENDIMARU_E2E_DISPOSABLE_SNAPSHOT")
for name in ("MENDIMARU_E2E_VERSION", "MENDIMARU_BROWSER_RUNNER_PATH", "MENDIMARU_NODE_BINARY"):
    assert not os.environ.get(name), "no fixture/runner/Node override"
binary = Path(os.environ["MENDIMARU_E2E_BINARY"])
assert binary.is_absolute() and "/target/" not in str(binary)
config = json.loads((Path(os.environ["MENDIMARU_CONFIG_DIR"]) / "config.json").read_text())
assert config["containerName"].startswith("Mendimaru149")
shared = os.environ["MENDIMARU_E2E_SHARED_SESSION_ID"]
status = subprocess.run(
    [str(binary), "browser", "session", "status", "--shared-session-id", shared, "--json"],
    capture_output=True, text=True, timeout=30, check=True,
)
prepared = json.loads(status.stdout)["data"]
assert prepared["state"] == "ready" and prepared["liveParticipants"] == 0
assert prepared["identity"]["runtimeMode"] == "studio-run-locally"
assert prepared["identity"]["studioSessionId"] and prepared["preparation"]["comparable"]
key = hashlib.sha256(("vm-use-v1\0" + config["containerRuntime"] + "\0" + config["containerName"]).encode()).hexdigest()
assert key == prepared["vmKey"]
work = Path(os.environ["MENDIMARU_E2E_WORKBOOK_EVIDENCE"])
assert work.is_absolute()
work.mkdir(mode=0o700, parents=False, exist_ok=False)
(work / "evidence").mkdir(mode=0o700)
env = dict(os.environ, MENDIMARU_WORKBOOK_LEASED="1",
           MENDIMARU_E2E_BUILD_MARKER=prepared["buildMarker"],
           MENDIMARU_WORKBOOK_BASE_URL=prepared["identity"]["baseUrl"])
handles = []
try:
    for filename, mode in [
        (f"/tmp/mendimaru-vm-use-{os.getuid()}/{key}.lock", fcntl.LOCK_SH),
        (f"/tmp/mendimaru-test-sessions-{os.getuid()}/app-{key}.lock", fcntl.LOCK_EX),
    ]:
        fd = os.open(filename, os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC)
        handles.append(fd)
        metadata = os.fstat(fd)
        assert stat.S_ISREG(metadata.st_mode) and metadata.st_uid == os.getuid()
        assert metadata.st_nlink == 1 and not metadata.st_mode & 0o077
        fcntl.flock(fd, mode | fcntl.LOCK_NB)
    # The browser driver retains ownership even if this supervisor is killed.
    env["MENDIMARU_WORKBOOK_LOCK_FDS"] = json.dumps(handles)
    with (work / "driver.log").open("w") as log:
        result = subprocess.run(
            ["node", str(Path(__file__).with_suffix(".mjs").resolve())],
            cwd=work, env=env, timeout=300, stdout=log, stderr=subprocess.STDOUT,
            pass_fds=tuple(handles),
        )
    raise SystemExit(result.returncode)
finally:
    for fd in reversed(handles):
        os.close(fd)
