# Spikes

A spike is a time-boxed experiment that answers a design question before production code depends on the answer, for example [SRV-002](https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/SRV-002-spike-mls.md) (MLS for CRDT updates, in [`mls/`](mls/REPORT.md)). Each spike gets its own folder, `spikes/<name>/`, holding its code and its report, `REPORT.md`.

## Spikes are never part of the server

- No crate under `crates/` may depend on spike code, so it can never reach the server binary or the container image. The Docker build copies only `crates/` and `xtask/`.
- A spike with Rust code is its own Cargo workspace with its own `Cargo.lock`. Cargo's lockfile can hold dependencies that no build compiles (Cargo resolves it with every feature of the workspace's members, and counts optional dependencies that a feature names only weakly, `dependency?/feature`, as switched on), so a spike's dependencies could otherwise change the versions the server is built with (for SRV-002, OpenMLS's optional SQLite storage would have pinned an older SQLite into the server).
- Code that proves itself moves into a production crate through a work package, with tests and review. SRV-002's findings feed the `bayan-mls` crate in bayan-core.
- Spike code is AGPL-3.0-or-later, like the rest of this repository, while bayan-core is GPL-3.0-or-later with the BayanDocs App Store Permission. BayanDocs has no contributor license agreement, so contributions cannot be relicensed ([LICENSING.md](https://github.com/BayanDocs/docs/blob/HEAD/LICENSING.md)): production crates are written from a spike's report, not copied from its code, unless the owner decides otherwise.

## Spikes are still checked

Spike code follows the same rules as everything else in this repository: dependencies follow ADR-0017 (exact pins, at least 24 hours old, licenses on the allowlist, justified in the pull request), the lints are the server's, and `unsafe` code is forbidden. `cargo xtask verify` formats, lints, tests, documents and audits each spike workspace as well, with the server's `deny.toml`. Its supply-chain checks, which run before anything is built, find every spike workspace by its `Cargo.lock` and check it like the server's (bayan-core#22): every dependency pinned exactly, and every package version a change adds to the spike's `Cargo.lock` published at least 24 hours before the commit that added it, with the checksum crates.io published. A spike may depend by path only on its own crates and on crates of the server's workspace.
