# Studio Run Locally M2EE authentication (#266)

Linux WinBoat can explicitly bind a host environment password to the M2EE
password of an owned **Studio Pro 11.12.6.0** Run Locally process. This is an
optional feature of `host-service.start`; `host-service.status` reports
`m2eeAuthenticationConfigured=true` when configured. Other exact Studio file
versions refuse the binding. Start Studio using the current binary before
starting this host service; an older keeper cannot implement the new contract.

[Mendix's monitoring documentation](https://docs.mendix.com/refguide/monitoring-mendix-runtime/)
identifies the Runtime process's `M2EE_ADMIN_PASS` as the M2EE credential and
requires its Base64 encoding in `X-M2EE-Authentication`. It is separate from the
application's administrator password.
[The Studio 11.12 release notes](https://docs.mendix.com/releasenotes/studio-pro/11.12/)
require that header for development servlets as well.

## Explicit environment contract

Prepare forwarding, launch the selected exact Studio/project, and link its
Runtime as described in [Run Locally](winboat-run-locally.md). With the designated
nonempty environment variable already set in the calling process:

```sh
mendimaru host-service start --service mysql --allow-host-port 3306 \
  --runtime-session-id RUNTIME_SESSION_ID \
  --m2ee-password-env MXCLI_APP_ADMIN_PASSWORD --json
```

The option accepts only an environment **name**, never a password. The variable
must contain 1–256 UTF-8 bytes without control characters. Its name may contain
ASCII letters, digits and underscores and cannot start with a digit. No variable
is implicitly selected. Despite the example's historical name, the supplied
value acts only as the host authentication alias: this does not set or change
MxAdmin, application security, password policy, Studio settings, or the Runtime's
original M2EE password. Do not print the environment or put its value in arguments.
The existing MySQL greeting prerequisite still applies; no DB credentials are
accepted by this API.

Run the app with Studio's **Run Locally**, then send JSON POST requests to the
returned `adminHostPort`, currently `http://127.0.0.1:18090`. The supported paths
in this mode are `/dev/preview_execute_oql` and `/` (M2EE actions). Construct
`X-M2EE-Authentication` from Base64 of the designated environment value's UTF-8
bytes inside the caller. The guest translates a matching alias to the current
owned Runtime's M2EE header, entirely in memory. Missing authentication stays
missing; an incorrect alias is replaced with a fresh invalid credential, so even
a caller supplying a different Runtime's header cannot bypass the alias check.
Both cases must be rejected by the actual Runtime.

The bridge accepts one HTTP/1.1 POST per connection with explicit Content-Length,
headers up to 16 KiB and body/response up to 1 MiB. Chunked input, duplicate or
folded headers, Expect, other methods/paths, absolute URLs and pipelined input are
refused. Responses close the connection. It disables proxies and redirects and
forwards only the JSON body and the translated authentication header to the fixed
guest IPv4 loopback admin port 8090. Application HTTP on 18080 is unaffected.

A repeated start with an authentication option, or against an existing
configured binding, refuses reuse: values may rotate without changing their
variable name. Finish and stop the owned session before starting a new binding.

## Secret and ownership boundary

The alias crosses same-UID private keeper IPC and the existing authenticated
Studio channel; shared transport files contain an independently IV-encrypted,
HMAC-authenticated value. Neither password is written into scripts, reports,
cache records, process arguments, logs or release artifacts. The alias is retained
only for this helper's lifetime and its comparison buffer is cleared on disposal.
Existing TLS pinning, per-connection authentication, cancellation and lifecycle
cleanup remain in force.

Before each admin request the helper checks the selected Studio PID, creation
time, executable, owner and exact file version. It connects to the fixed admin socket, verifies the PID owning that exact
accepted connection and the IPv4 loopback listener on 8090, and requires `java`/`javaw` with the same owner and
interactive session, and verifies its live, creation-time-ordered ancestry back
to that Studio (at most eight levels). It reads only `M2EE_ADMIN_PASS` from that
process's bounded x64 environment, clears temporary buffers, and rechecks the
listener/process/connection identity. The header travels over that same socket;
there is no reconnection or connection pool that could retarget a replacement
listener between validation and sending. It does not read credential stores, deployment
configuration or project user settings, enumerate arbitrary secrets, return a
credential through an API, or cache a Runtime credential across F5 restarts.
A missing listener, foreign process, stale identity, unsupported architecture,
permission failure or ambiguous listener fails closed with no successful reply.

## Acceptance and shutdown

`state=ready` still means the MySQL transport greeting passed.
`m2eeAuthenticationConfigured` means configured, not authenticated or app-ready.
App HTTP readiness also does not prove authenticated admin access or complete
browser bundle readiness.

The reusable verifier checks a read-only `System.Language` Code SELECT with
missing, incorrect and valid headers, requires successful OQL `data`, and tests
M2EE `runtime_status` in all three modes:

```sh
python3 scripts/e2e/verify-studio-m2ee.py \
  --password-env MXCLI_APP_ADMIN_PASSWORD
```

Missing/incorrect servlet headers must produce HTTP 401/403; valid must produce
HTTP 200 with successful data. For M2EE, HTTP 200 with `result=-4` is authentication
rejection. A successful authenticated action requires HTTP 200 and integer
`result=0`. Errors print only a fixed message; the report includes status codes
and result codes, never headers, passwords or response bodies.

With explicit `--shutdown`, the verifier additionally attempts authenticated
M2EE shutdown. Independently observe Studio's **Stop** UI and the app stopping,
then gracefully stop the owned Studio/host-service/Runtime and confirm the owned
sessions ended and their loopback ports were released. A successful shutdown
response alone is insufficient cleanup evidence. On M2EE rejection, preserve
that failure and use the observed Studio Stop action for graceful recovery.

## Automated coverage

The ordinary Rust suite covers environment-name/value bounds, closed start and
launch requests, old host-service record compatibility and lifecycle/transport
regressions. Contract validation accepts the additive optional fields while
rejecting literal passwords and invalid names. The Python verifier tests reject
HTTP-200 authentication failures, malformed success results, unexpected
unauthenticated success, absent OQL data and shutdown denial.

`scripts/test-studio-m2ee.ps1` compiles the production C# bridge and, on Windows,
creates an isolated StudioPro.exe/Java process-tree fixture. It exercises actual
Windows listener ownership and bounded process-environment reads, UTF-8 alias
translation, missing/incorrect/valid servlet and M2EE authentication, shutdown,
Runtime restart, unsafe HTTP refusal, stale Studio identity and foreign listener
refusal. It does not claim actual Studio application acceptance. Real Studio
acceptance evidence is recorded separately.
