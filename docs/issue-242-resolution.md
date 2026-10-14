# Studio launch recovery after loss of its previous owner (#242)

The installed Linux application refused Studio Pro 11.12.6 with
`a WinBoat Runtime precondition was not satisfied` on 2026-10-15. All environment
readiness checks passed. A Runtime from the previous day retained a Studio link
and `starting` state even though its guest report later recorded Studio closure.
After application restart, neither an in-memory owner nor a keeper could observe
that link. Launch converted missing ownership into `unknown` and rejected every
subsequent attempt before starting Studio.

Explicit launch now revalidates an orphan through an authenticated, read-only
Windows probe of the exact PID/start-time identity. A confirmed ended identity
permits clearing the stale link and reusing the existing Runtime and forwarding.
A retained owner prevents a second RDP connection. A live guest identity remains
a conflict; observation, transport and authentication failures retain the record
with an actionable retryable diagnostic. A targeted probe never discards unrelated
protected-project metadata.

The guest probe distinguishes process-not-found from access/refresh failures and
PID reuse. It does not use the general session enumeration's best-effort filtering
as proof of absence. No public schema or persisted record fields change. Normal
Runtime/browser observation still uses owner metadata and host HTTP only.

See the #242 section in [the regression matrix](winboat-regression-matrix.md) for
the automated boundaries and [Run Locally recovery](winboat-run-locally.md) for
operator behavior.
