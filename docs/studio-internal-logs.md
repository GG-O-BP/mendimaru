# Studio internal exception diagnostics

On Linux WinBoat, an owned **Studio Pro 11.12.6** session can export its
internal exceptions without opening Explorer or sending a support report.
Mendix documents these as the logs opened by
[Help → Open Log File Directory](https://docs.mendix.com/refguide/help-menu/).
`runtime logs` continues to describe Runtime/relay lifecycle diagnostics.

```sh
mendimaru studio logs --session-id STUDIO_SESSION_ID --json
mendimaru studio logs --session-id STUDIO_SESSION_ID \
  --since 2026-10-20T12:00:00+09:00 --until 2026-10-20T12:05:00+09:00 \
  --max-bytes 65536 --max-lines 200 --json > studio-diagnostics.json
mendimaru studio logs --session-id STUDIO_SESSION_ID --cursor CURSOR --json
```

Start the session with the current CLI. Older keepers require an explicit
close/start; collection never reconnects or replaces a keeper. The request
uses the authenticated, serialized keeper channel and shared VM observation
lease. It does not start/stop the app, change models, databases, configuration,
log levels, or transport forwarding, or upload reports.

The provider verifies the selected process's PID, creation time, current-user
ownership, trusted executable and exact installed version. Only direct
`log.txt`/`log.N.txt` files in the guest user's
`%LOCALAPPDATA%\Mendix\log\11.12.6` directory are candidates. It never searches
project directories, credential stores, Git logs or arbitrary paths. Before
reading content, Windows Restart Manager must identify the selected PID and
creation time as the file's sole external owner. Unowned or concurrently shared
logs fail closed. Handles reject reparse points, hard links, redirected parent
paths and replacement/deletion during collection. Only records at or after the
selected process start time can be returned. Historical files and sessions
whose ownership cannot be proved are unavailable.

Each successful JSON `data` has `source: "studio-internal"`, `sessionId`, exact
`version`, `guestTimeZone`, `lines`, `exceptionCount`, `bytes`, `maxBytes`,
`maxLines`, `initialTimestamp`, `lastTimestamp`, `snapshotBytes`, `scannedBytes`,
`nextCursor`, `truncated`, `incompleteLine`, and
`redactionPolicy: "structural-only"`. An empty page or `exceptionCount: 0` is
**not evidence that a Studio exception was collected**. Relay readiness is
never substituted for a Studio log.

The structural redactor runs in the guest before signing/writing a report.
It preserves original local timestamps, severity, qualified exception types,
numeric exception codes, inner-exception ordering and stack method symbols.
Free-form messages, source paths, argument lists, credentials, connection
strings, headers and multiline secret values are omitted. A small fixed set
of generic system messages is safe to retain. This deliberately limits message
detail; it does not promise raw log fidelity. `initialTimestamp` supplies the
context for a page beginning inside a stack trace. Guest-local timestamps are
interpreted with the guest timezone for RFC3339 filters; ambiguous or invalid
DST timestamps fail explicitly. Original timestamps are never rewritten.

`max-bytes` bounds UTF-8 line content including one newline per line, excluding
the bounded JSON envelope (default 65,536; allowed 256–262,144). `max-lines`
defaults to 200 (allowed 1–2,000). Each read hashes at most a 16 MiB snapshot,
scans at most 4 MiB, and buffers at most 64 KiB per source line. Larger files
or lines exceeding the scan budget fail explicitly. Oversized diagnostic lines
are replaced, never sliced into potentially secret fragments. Incomplete final
lines are withheld and reported through `incompleteLine`.

The signed opaque cursor binds the session, pinned file identity, snapshot
content/length, byte position, timestamp context and time filters. Reusing it
with another session/filter, editing it, rotating/replacing/truncating the file,
or overwriting snapshot bytes fails with `studio-logs-cursor-invalid`.
Appending does not alter an existing snapshot. Keep the original time filters
on continuation; page limits may change. At snapshot EOF the cursor is stable;
omit it to start a new snapshot, including after an incomplete final line.

Safe errors remain distinct on stderr, with no guest exception messages:

| Reason                            | Meaning                                                 |
| --------------------------------- | ------------------------------------------------------- |
| `studio-logs-not-found`           | No supported log directory/files                        |
| `studio-logs-unreadable`          | Read/access/format/budget failure                       |
| `studio-logs-wrong-session`       | Process identity or exclusive file ownership not proved |
| `studio-logs-unsupported-version` | Exact Studio version lacks a validated adapter          |
| `studio-logs-cursor-invalid`      | Invalid, stale or foreign continuation                  |
| `ui-session-unavailable`          | No matching current keeper/channel                      |
| `ui-bridge-untrusted`             | Authentication or local ownership failure               |

Unsupported versions/backends exit 3; invalid options/cursors exit 2;
precondition/read failures exit 1. Successful collection exits 0 with empty
stderr. The capability is `studio.logs`; the closed result contract is
[`studio-logs.schema.json`](../schemas/studio-logs.schema.json), under outer
contract **8.0.0**. Shell redirection exports only the sanitized JSON; no raw
log artifact is persisted by Mendimaru.
