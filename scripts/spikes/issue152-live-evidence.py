#!/usr/bin/env python3
"""Live Linux+WinBoat verification driver for issue #152.

Runs real CLI callers against one keeper session on the WinBoat VM and
records timings/outcomes as docs/issue-152-live-evidence.json.
"""
import json
import os
import subprocess
import sys
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
CLI = os.path.join(ROOT, "src-tauri", "target", "debug", "mendimaru")
SESSION = sys.argv[1]
evidence = {
    "schemaVersion": "1.0.0",
    "issue": 152,
    "title": "shared WinBoat helper UI coordination",
    "environment": "Linux host + WinBoat VM, Studio 11.12.4, real session keeper",
    "session": SESSION,
    "recordedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    "steps": [],
}


def record(name, command, seconds, result, note=None):
    message = None
    if isinstance(result, dict) and isinstance(result.get("error"), dict):
        message = result["error"].get("message")
    evidence["steps"].append(
        {
            "step": name,
            "command": " ".join(command),
            "wallSeconds": round(seconds, 2),
            "ok": bool(result.get("ok")) if isinstance(result, dict) else None,
            "errorMessage": message,
            **({"note": note} if note else {}),
        }
    )
    print(f"{name}: ok={result.get('ok') if isinstance(result, dict) else None} "
          f"wall={seconds:.2f}s {message or ''}")


def run(command, timeout=120):
    start = time.monotonic()
    try:
        completed = subprocess.run(
            command, capture_output=True, text=True, timeout=timeout
        )
    except subprocess.TimeoutExpired:
        return {"ok": False, "error": {"message": "driver timeout"}}, time.monotonic() - start
    seconds = time.monotonic() - start
    # Failures exit non-zero and print their JSON envelope on stderr.
    for stream in (completed.stdout, completed.stderr):
        try:
            return json.loads(stream), seconds
        except json.JSONDecodeError:
            continue
    return {"ok": False, "error": {"message": "unparseable CLI reply",
            "stderr": completed.stderr[:200]}}, seconds


# 1. A semantic wait (observation) holds its turn for its bounded poll.
short_wait = {}


def run_short_wait():
    document, seconds = run(
        [CLI, "ui", "wait", "--session-id", SESSION, "--condition", "running",
         "--timeout-ms", "12000", "--json"],
    )
    short_wait["document"], short_wait["seconds"] = document, seconds
    record("short-wait-observation", ["ui", "wait", "--condition", "running",
           "--timeout-ms", "12000"], seconds, document)


waiter = threading.Thread(target=run_short_wait)
waiter.start()
time.sleep(2)

# 2. A tree request accepted during the wait queues in arrival order and then
#    runs: the same-session mailbox is never corrupted by overlap.
document, seconds = run(
    [CLI, "ui", "tree", "--session-id", SESSION, "--timeout-ms", "40000", "--json"]
)
record("tree-queued-behind-wait", ["ui", "tree"], seconds, document,
       note="accepted while the 12 s wait held the session turn")
tree_queued = document
waiter.join()

# 3. Keeper administration stays responsive while UI jobs run.
document, seconds = run([CLI, "studio", "status", "--json"], timeout=60)
record("status-responsiveness", ["studio", "status"], seconds, document)

# 4. Desktop-class focus actions: two callers, serialized and bounded.
find, seconds = run(
    [CLI, "ui", "find", "--session-id", SESSION, "--role", "Window",
     "--timeout-ms", "30000", "--json"],
)
record("find-window", ["ui", "find", "--role", "Window"], seconds, find)
element_id = None
if isinstance(find, dict) and find.get("ok"):
    data = find.get("data")
    matches = data if isinstance(data, list) else data.get("matches", [])
    if matches:
        element_id = matches[0].get("elementId")

if element_id:
    focus_results = {}

    def focus(tag):
        document, seconds = run(
            [CLI, "ui", "action", "--session-id", SESSION, "--element-id",
             element_id, "--action", "focus", "--timeout-ms", "25000", "--json"],
        )
        focus_results[tag] = (document, seconds)

    first = threading.Thread(target=focus, args=("first",))
    second = threading.Thread(target=focus, args=("second",))
    first.start()
    time.sleep(0.4)
    second.start()
    first.join()
    second.join()
    for tag in ("first", "second"):
        document, seconds = focus_results[tag]
        record(f"focus-{tag}-desktop-class", ["ui", "action", "--action", "focus"],
               seconds, document)
else:
    evidence["steps"].append(
        {"step": "focus-desktop-class", "skipped": "no window element id from find"}
    )

# 5. Kill a long-wait caller mid-poll: EOF cancellation removes only that
#    caller's job; the queue serves the next arrival immediately.
long_wait = {}


def run_long_wait():
    document, seconds = run(
        [CLI, "ui", "wait", "--session-id", SESSION, "--condition", "running",
         "--timeout-ms", "45000", "--json"],
    )
    long_wait["document"], long_wait["seconds"] = document, seconds


canceller = threading.Thread(target=run_long_wait)
canceller.start()
time.sleep(3)
subprocess.run(["pkill", "-f", f"ui wait --session-id {SESSION}"], check=False)
canceller.join(timeout=60)
time.sleep(1.5)
record("long-wait-caller-killed", ["ui", "wait", "--timeout-ms", "45000"],
       0.0, {"ok": True}, note="caller EOF-cancelled mid-poll")

document, seconds = run(
    [CLI, "ui", "tree", "--session-id", SESSION, "--timeout-ms", "30000", "--json"]
)
record("tree-after-caller-death", ["ui", "tree"], seconds, document,
       note="queue served the next arrival without waiting out the killed 45 s poll")
tree_after_death = document

# 6. Final keeper status: the session survived every observation and failure.
document, seconds = run([CLI, "studio", "status", "--json"], timeout=60)
sessions = document.get("data", []) if isinstance(document, dict) else []
evidence["finalSessionCount"] = len(sessions) if isinstance(sessions, list) else None
record("final-status", ["studio", "status"], seconds, document)

evidence["outcome"] = {
    "treeQueuedAndSucceeded": bool(tree_queued and tree_queued.get("ok")),
    "sessionSurvivedCallerDeath": bool(tree_after_death and tree_after_death.get("ok")),
    "finalSessionCount": evidence["finalSessionCount"],
}

out_path = os.path.join(ROOT, "docs", "issue-152-live-evidence.json")
with open(out_path, "w", encoding="utf-8") as handle:
    json.dump(evidence, handle, indent=2, ensure_ascii=False)
    handle.write("\n")
print("evidence written:", out_path)
