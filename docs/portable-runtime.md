# Portable Runtime build and execution

Mendimaru can build and run a Mendix web application without Studio Pro or a
Windows VM when the project's exact Mendix version is covered by the documented
Portable Runtime policy. Linux and Windows use the same public Runtime result,
artifact, error, and lifecycle schemas; only the official host launcher differs.

## Supported hosts and versions

Portable execution is enabled only on x86-64 Linux and Windows. Linux runs
`bin/start`; Windows packages must contain both `bin/start.bat` and
`bin/start.ps1`, and Mendimaru runs the PowerShell launcher non-interactively.
Mendix distributes macOS MxBuild only inside `StudioPro.app`, not through the
standalone archive used here. Mendimaru therefore keeps both macOS portable
build and run unsupported until an exact installed-app integration is added;
it never guesses a Linux binary or start script.

The allowlist is intentionally explicit and reflects the Mendix Portable
Runtime support policy reviewed in June 2026:

- Mendix 10.24.19 LTS and later 10.24 patches;
- Mendix 11.6.5 MTS and later 11.6 patches;
- Mendix 11.9 and later Mendix 11 minors.

Mendix 11.7 and 11.8, Mendix 12, incomplete version strings, and all other
versions are rejected with `runtime_version_unsupported`. Mendimaru never opens
the project with a nearby MxBuild, uses `--loose-version-check`, upgrades the
project, or silently changes to Studio Pro Run Locally. A caller can explicitly
select a future Windows/macOS Studio Run Locally backend when that capability is
implemented, but it is not an automatic fallback.

The policy and command behavior are based on the official
[Mx command-line tool](https://docs.mendix.com/refguide/mx-command-line-tool/),
[MxBuild](https://docs.mendix.com/refguide/mxbuild/), and
[Portable App deployment](https://docs.mendix.com/developerportal/deploy/portable-app-distribution-deploy/)
documentation. Update the allowlist and its tests when Mendix changes that
policy; do not infer support from an archive that happens to download.

## Exact toolchain and Java

`runtime build` resolves the one exact version declared by the selected
`project_<sha256>` ID. It obtains the matching official host MxBuild archive,
verifies its size and SHA-256 digest, extracts it without following links or
archive traversal paths, and verifies the project again with `mx show-version`.
For releases whose official archive name contains a fourth build component, the
full version must be available; Mendimaru does not guess it.

The project-selected Java major is checked with `mx show-java-version`. The
resolved `java` executable must report that major (Java 21 for current Mendix 11
projects). Advanced and test installations may set an absolute
`MENDIMARU_MXBUILD_HOME` containing `mendimaru-mxbuild-version`, and an absolute
`MENDIMARU_JAVA_HOME`; both are still executed and exact-version validated.

MxBuild creates a `portable-app-package` with `--write-errors`. Mendimaru never
uses `--export-secrets`. Consistency errors, other build failures, and Runtime
initialization/readiness failures remain distinct:

| Failure                   | Error code                      | Diagnostic artifact     |
| ------------------------- | ------------------------------- | ----------------------- |
| Model consistency         | `consistency_failed`            | JSON consistency report |
| Packaging/tool failure    | `runtime_build_failed`          | MxBuild text log        |
| Launcher/configuration    | `runtime_initialization_failed` | Runtime text log        |
| HTTP readiness deadline   | `runtime_readiness_timeout`     | Runtime text log        |
| Process exits after start | `runtime_exited`                | Runtime text log        |

## CLI lifecycle

```text
mendimaru runtime build --project-id PROJECT_ID [--clean]
mendimaru runtime start --project-id PROJECT_ID [--clean]
mendimaru runtime status --session-id RUNTIME_SESSION_ID
mendimaru runtime wait --session-id RUNTIME_SESSION_ID
mendimaru runtime url --session-id RUNTIME_SESSION_ID
mendimaru runtime logs --session-id RUNTIME_SESSION_ID [--cursor CURSOR]
mendimaru runtime stop --session-id RUNTIME_SESSION_ID
```

`runtime start` builds or integrity-checks the cached package, copies a private
deployment for the new session, allocates separate application and admin ports,
and binds both to `127.0.0.1`. Success is returned only after the admin
`/probes/ready` endpoint is healthy. The documented application
`/health/ready` endpoint and an HTTP 200 application root are compatibility
checks for package variants without the admin probe. `runtime url` returns a URL
only while the session remains ready.

Each session has its own deployment, embedded database, uploaded files, ports,
PID/start-time identity, log, and state record. Linux process groups and Windows
Job Objects contain descendants. Timeout, cancellation, failure, and `runtime
stop` terminate the complete tree; a normal stop gives the Runtime a grace
period before forcing it. State records do not trust a recycled PID.

Logs are returned in bounded batches. When `truncated` is true, pass
`nextCursor` to the next `runtime logs --cursor` request. Paths stay private;
public artifacts use opaque `artifact_<random>` IDs and
`mendimaru-cache://...` locations.

## Cache and clean rebuilds

The cache key includes the project content digest, exact Mendix version,
verified MxBuild archive digest, package target, and host OS. Every cache hit
rechecks the package and diagnostic file sizes and SHA-256 digests before use.
Concurrent builds of one project take a private per-project lock, while Runtime
deployments remain session-isolated.

`--clean` removes that project's existing build entries while holding the lock,
then performs a new consistency check and package build. It does not delete the
source project or global exact-version toolchain download. Interrupted staging
directories are removed before the next build.

## Secrets, accounts, and licenses

Runtime-only values are supplied as a JSON object in the parent process
environment, never as CLI options:

```bash
MENDIMARU_RUNTIME_ENV_JSON='{"RUNTIME_LICENSE_ID":"...","RUNTIME_LICENSE_KEY":"..."}' \
  mendimaru runtime start --project-id project_... --json
```

The object accepts at most 64 uppercase environment names, 64 KiB total, and 3
bytes through 8 KiB per value. Mendimaru-owned names, Java/PATH, admin credentials, loopback
ports, addresses, and ApplicationRootUrl are reserved. After parsing, the JSON
is removed from the command process environment. Values travel to the detached
supervisor over stdin, are never put in arguments or state files, and are
removed from raw and Base64-encoded Runtime output before the bounded private
log is written. The invoking process is still responsible for protecting its
own environment and shell history; use a secret manager or ephemeral process
environment rather than a checked-in script.

Official MxBuild downloads do not require Mendimaru to store a Mendix account.
The application and any private external dependencies may have their own access
requirements. An unlicensed local package can use the Runtime's documented
development/trial behavior and limits. Production or other licensed execution
requires valid Mendix-issued license values and remains subject to Mendix terms;
Mendimaru does not obtain, validate entitlement for, or persist those values.
