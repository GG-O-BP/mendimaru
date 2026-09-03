# Contributing to Mendimaru

## WinBoat contract and cache upgrades

WinBoat Runtime state can outlive an application process or survive an upgrade.
Before changing a contract schema, persisted record layout, Compose transaction,
Studio launch recovery path, or Linux session keeper socket policy, read
[`docs/winboat-regression-matrix.md`](docs/winboat-regression-matrix.md) and run
the ordinary Rust test suite.

When `CONTRACT_SCHEMA_VERSION` or `schemas/runtime.schema.json` changes, complete
the upgrade checklist in that document in the same PR. In particular, add a
fixture for the prior public record shape and prove that current-session
creation, discovery, explicit invalidation, post-success recovery, Compose
rollback, lock cleanup, and keeper cleanup all remain coherent. Do not remove a
security or compatibility assertion merely to make a legacy fixture parse.
