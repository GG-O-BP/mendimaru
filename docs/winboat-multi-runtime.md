# Multi-project Runtime ownership on one WinBoat VM

Linux WinBoat Runtime sessions for **different projects** can run on the same
VM at the same time, each on its own Studio-configured port. This guide defines
who owns a port, when the shared Compose file and the VM may change, and what
isolation you get — and do not get. It builds on the [shared VM use
leases](winboat-vm-use.md); the parallel browser-suite runner (#155) stays
separate work.

## Port ownership registry

Within one Linux UID and host filesystem namespace, the management identity of
the VM is `(container runtime, explicit Compose container_name)` — the same
identity the VM use locks use. A Runtime session reserves its guest port in a
host-wide registry under the fixed `/tmp/mendimaru-vm-runtime-<uid>` directory,
named by that identity. Cache directories, `MENDIMARU_CACHE_DIR` overrides,
Compose paths, and `TMPDIR` deliberately do **not** enter the key: two CLI
processes with different caches but the same VM see each other's reservations.

Each registry entry records the session id, guest and host port, the absolute
path of the session record, the forwarding that existed on the port before
Mendimaru took it over, and a stop timestamp once the session ended. The
registry is mutated only under the exclusive VM use lease plus a per-VM file
lock, so concurrent reservations of the same port cannot interleave: the second
starter receives the same retryable "already owned by another active Mendimaru
Runtime session" refusal as a same-cache duplicate.

Records created before the registry existed never reserved a port. They are
still honored through the local cache scan, so an upgraded Mendimaru keeps
refusing duplicates it can see; a live session recorded in **another** cache by
an older build remains invisible to it. Stop and start reconcile the difference
as soon as both sides run the current version.

### Crash and record handling

The registry never trusts its own state over the records:

- An entry whose record file disappeared (crash, explicit forget, quarantined
  incompatible record) is evicted on the next read.
- An entry whose record says `stopped` is marked stopped even if the writer
  died between the record write and the registry update.
- Corrupt, foreign, or future-schema registry content is refused loudly
  (`PreconditionFailed`) — never silently reset — because a wrong answer here
  would let two Runtimes collide on one loopback port.

## Compose ownership and stop semantics

The Compose file is owned by the VM administrator (the user's WinBoat setup).
Mendimaru adds exactly one `127.0.0.1:<port>:<port>/tcp` mapping per live
Runtime port and restores what was there before when ownership ends. External
edits elsewhere in the file — other mappings, volumes, labels — are preserved;
rollback and stop rewrites are revision-checked and refuse to clobber a
concurrent external edit.

Runtime shutdown and VM forwarding removal are **separate events**:

- Stopping a session while other Runtime sessions are live on the same VM ends
  only that session: its record becomes `stopped`, its port ownership is
  released for reuse, and its log states that forwarding removal is deferred.
  The other sessions keep their mappings, containers, Studio windows, and app
  state; the VM is not restarted.
- The forwarding of stopped-but-deferred sessions is removed by the **last**
  stopping session, or folded into the next start that legitimately recreates
  the VM. Cleanup replaces each owned port's mapping with the forwarding that
  existed before Mendimaru took the port over (per the earliest recorded
  owner), leaving everything else untouched.
- A stop of the only live session restores the user's original Compose bytes
  exactly when the file is still byte-for-byte the one Mendimaru wrote. Any
  other content — external edits included — goes through the scoped,
  semantics-preserving path instead.

### No implicit VM recreation

Preparing a new port rewrites Compose and restarts the VM, which would destroy
every other live Runtime on it. Therefore:

- A start or Studio launch preparation that needs a port change while another
  Runtime session is live fails with a retryable
  `RuntimePortConflict` that names the port and tells the user to stop the
  other session first. Mendimaru never recreates the VM silently.
- A port whose exact `127.0.0.1:P:P/tcp` mapping is already in place is adopted
  without rewriting the file or restarting the VM, so reusing a prepared port
  is safe next to live sessions.
- Initial port preparation for a Studio launch happens before the Studio worker
  joins (`runtime start --mode studio-run-locally` preparation), keeping worker
  participation out of VM lifecycle changes.

## Project isolation contract

Per project, on the same VM:

- **Ports**: each project uses the Studio-configured `MXCONSOLE_RUNTIME_PORT`
  and gets its own loopback `localhost:P:P` forwarding. Two projects must not
  configure the same Runtime port; Mendimaru refuses the duplicate rather than
  sharing the URL.
- **Project copies and deployment output**: each `.mpr` and its `deployment`
  output live in separate directories under the shared workspace, as Studio
  itself lays them out. Mendimaru never merges or aliases them.
- **Application data, accounts, and import/export files**: Mendimaru provides
  no server-side data isolation. The Windows VM, its Mendix installation, the
  Windows user account, and the databases of concurrently running apps are
  shared VM state. A browser context is a client-side convenience only — it
  does not isolate application data, and tests must not present it as such.
  Concurrent writes need independently prepared data by the operator.
- **What is shared**: the Windows VM, Studio Pro installations, RDP transport,
  Guest API, and the Compose file. A VM restart or UEFI recovery affects every
  project on the VM; that is why recreation is refused — not queued silently —
  while other sessions are live.

## Unsupported configurations are reported, not hidden

These situations return explicit errors instead of a parallel success:

- Same Runtime port for two projects (retryable conflict; stop the other
  session or change the port).
- Port preparation requiring a VM recreation while other sessions are live
  (retryable refusal naming the port).
- Registry or Compose content that cannot be verified (hard failure with a
  safe-recovery hint, never a best-effort guess).
- Studio-linked starts still require the port forwarding to be live-prepared;
  an unprepared port is never satisfied by recreating the VM behind a joined
  worker.

Manual, external editing of the Compose ports while sessions are live is an
external control operation: Mendimaru neither undoes it silently nor treats it
as its own state.
