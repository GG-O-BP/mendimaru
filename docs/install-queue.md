# Installer resume and install queue

Mendimaru downloads large Studio Pro installers through a durable,
integrity-gated pipeline and can queue multiple versions for installation.

## Resumable downloads

Each version owns two private cache files:

```text
<cache>/installers/Mendix-<version>-Setup.exe.partial       payload
<cache>/installers/Mendix-<version>-Setup.exe.partial.json  resumable state
```

The partial state records the schema version, exact installer version,
source URL, verified payload byte count, total size, and the ETag or
Last-Modified validator observed when the transfer started.

A transfer resumes only when **all** of the following hold:

- the state schema, version, and source URL match;
- the payload is a direct, non-symlink regular file whose length equals the
  recorded byte count;
- the byte count is below the 2 GiB installer ceiling and below the recorded
  total; and
- an ETag or Last-Modified validator exists.

The resume request sends `Range: bytes=<partial>-` plus `If-Range` with the
validator. Responses are classified conservatively:

| Response                                                    | Action                                                              |
| ----------------------------------------------------------- | ------------------------------------------------------------------- |
| `206 Partial Content` with a matching `Content-Range` start | Append to the payload                                               |
| `206` with an unusable `Content-Range`                      | Discard the partial and restart from zero                           |
| `200 OK`                                                    | The origin changed or ignores Range; discard and restart from zero  |
| Network error or cancellation                               | Flush, sync, and retain the partial (unless the user chose discard) |

Completed payloads still pass the existing full size, PE-header, and SHA-256
validation before the atomic cache commit. A failed validation discards the
partial. `--force-redownload` always discards any partial before starting.

### Cancellation choices

The queue UI exposes two cancellation actions:

- **Cancel and keep** — stop now and resume the verified partial later.
- **Cancel and discard** — stop now and delete the partial payload and state.

The legacy single-item cancel button keeps the partial.

## Install queue

Catalog installs are queued rather than globally blocking:

- items are persisted to `<config>/install-queue.json` with a bounded,
  schema-versioned, atomic store;
- one worker processes one item at a time, which also serializes Windows
  installer execution;
- pending items can be reordered, cancelled, or discarded individually;
- failed or cancelled items can be retried, and terminal items removed;
- on restart, interrupted items return to `queued` and resume their partials,
  while terminal items keep their final state;
- each item creates the existing persistent Install operation when it starts,
  so the Operations center and operation retry semantics are unchanged.

Project-launch assistance keeps its synchronous contract: it enqueues the
exact required version and waits for that item to reach a terminal state
before detecting and launching the installed Studio Pro.

The queue store contains only opaque item IDs, versions, flags, progress, and
timestamps. It never stores project paths, URLs with credentials, command
payloads, or Windows paths.
