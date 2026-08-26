# WinBoat Studio discovery

Mendimaru discovers Studio Pro through a versioned WinBoat Guest capability.
The Guest-side contract is proposed in
[winboat-org/winboat#859](https://github.com/winboat-org/winboat/pull/859). The
fast path avoids the icon extraction and full application serialization
performed by the legacy `/apps` endpoint while preserving the same exact
version and executable-root checks on the host.

## Negotiation and authentication

Mendimaru reads the bounded `/health` response first. A Guest that supports the
lightweight contract advertises `apps-query-v1` and `authentication: bearer`.
Mendimaru then requests:

```text
GET /apps?includeIcons=false&pathPrefix=<configured-root>\&pathSuffix=\modeler\studiopro.exe&fields=Name,Path,Source&limit=128
```

The bearer token is read only from `guest_token` beside the configured WinBoat
Compose file. On Unix it must be a directly opened, owner-owned regular file
that is not group- or world-writable. The file and header are bounded, the
header is marked sensitive, and neither the token nor configured paths are
written to traces or errors.

The lightweight response must be an array with all three requested fields.
Mendimaru still accepts only executable paths shaped as:

```text
<configured Mendix root>\<exact version folder>\modeler\studiopro.exe
```

The existing version-folder parser, install-root derivation, sorting, cache
lifecycle, and mutation-time authoritative verification are shared with the
legacy path.

## Failure and fallback policy

| Guest response                                                 | Mendimaru behavior                                                                                                                                              |
| -------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `/health` has `apps-query-v1`                                  | Use only the projected query. Authentication, HTTP, timeout, malformed, oversized, and over-count failures are returned; they never downgrade to the full list. |
| Capability is absent                                           | Request the legacy `/apps` endpoint. Send the shared token when present so authenticated pre-capability Guests also work.                                       |
| Legacy unauthenticated Guest                                   | Request `/apps` without an Authorization header.                                                                                                                |
| Authentication is advertised but the token is absent or unsafe | Fail before requesting `/apps`.                                                                                                                                 |
| A token file exists but fails validation                       | Fail closed, including when the current Guest would otherwise allow an unauthenticated legacy request.                                                          |
| Guest rejects the token with 401/403                           | Fail authentication without retrying an unauthenticated request.                                                                                                |

Bounds are deliberately different for the two payload shapes:

| Boundary        |            Projected |     Legacy |
| --------------- | -------------------: | ---------: |
| Request timeout |           12 seconds | 30 seconds |
| Response bytes  |              256 KiB |      8 MiB |
| App records     |                  128 |      4,096 |
| Health response | 16 KiB and 2 seconds |       same |

Every app field is independently bounded and control characters are rejected.
The Guest-side contract additionally limits its PowerShell projection to 10
seconds, 128 results, and 256 KiB, validates local Windows paths and projection
names, and rejects reparse points at every traversed component.

## Same-VM measurements

Measurements were taken on 2026-08-26 against the same Windows VM and host
loopback port. The baseline used the installed WinBoat Guest v0.9.0 full
`/apps`. The after run temporarily stopped that service, bound the final
capability build to the same Guest port, ran ten samples, and restored v0.9.0.
The original `/health` and `/version` were verified after every run. Percentiles
use nearest-rank; no sample was discarded.

### Guest `/apps`

| Route            | Samples (ms)                                                                                                 |         p50 |         p95 |        Mean |   Bytes |
| ---------------- | ------------------------------------------------------------------------------------------------------------ | ----------: | ----------: | ----------: | ------: |
| Legacy full list | 4,176.510, 3,498.849, 3,385.625, 3,366.487, 3,393.500, 3,341.238, 3,389.970, 3,378.316, 3,396.219, 3,339.027 | 3,385.625ms | 4,176.510ms | 3,466.574ms | 231,882 |
| `apps-query-v1`  | 765.962, 654.246, 708.718, 667.346, 640.206, 645.935, 637.754, 665.005, 666.031, 655.624                     |   655.624ms |   765.962ms |   670.683ms |     454 |

The projected route reduced p50 by 80.64%, p95 by 81.66%, and payload size by
99.804%. It returned four Studio Pro records containing exactly `Name`, `Path`,
and `Source`.

### Cold authoritative CLI refresh

Each sample launched a fresh `mendimaru studio list --backend linux-winboat`
process, so the backend in-memory cache could not make the result warm.

| Route           | Samples (ms)                                                                                                 |         p50 |         p95 |        Mean |
| --------------- | ------------------------------------------------------------------------------------------------------------ | ----------: | ----------: | ----------: |
| Legacy Guest    | 4,954.359, 4,592.383, 4,329.309, 5,080.498, 4,547.609, 4,297.068, 4,281.489, 4,423.326, 4,366.784, 4,292.531 | 4,366.784ms | 5,080.498ms | 4,516.536ms |
| Projected Guest | 1,324.091, 1,234.039, 1,241.193, 1,217.820, 1,219.513, 1,219.985, 1,256.908, 1,254.338, 1,206.626, 1,225.058 | 1,225.058ms | 1,324.091ms | 1,239.957ms |

The end-to-end cold p50 improved by 71.95% and p95 by 73.94%, exceeding the
40% target without relaxing executable identity or install-root validation.
