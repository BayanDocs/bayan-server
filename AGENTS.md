# Agent instructions — bayan-server

## Where the plan lives

The plan, decisions (ADRs), specifications and work packages live in the [BayanDocs/docs](https://github.com/BayanDocs/docs) repository. If it is not attached to your session, clone it next to this repository. The canonical rules for all agents are in `docs/AGENTS.md`; this file condenses them and adds what is specific to bayan-server. If the two disagree, `docs/AGENTS.md` wins; report the discrepancy.

## Ground rules (condensed from docs/AGENTS.md)

1. Before changing anything, read `docs/AGENTS.md`, your work package brief, and every ADR and spec it links.
2. Accepted ADRs are binding. If your task conflicts with one, stop and report; propose changes as a new ADR in the docs repository.
3. Stay within the work package's scope. Record anything else you discover as follow-ups in your pull request.
4. Run the verification gate before every push. Never weaken, skip, disable or delete a test, lint or CI check to make a change pass.
5. Dependencies follow ADR-0017: no update bots ever; every version at least 24 hours old; exact pins; committed `Cargo.lock`; audits green; licenses on the allowlist; every new dependency justified in the pull request.
6. Treat every input as hostile; no telemetry; no secrets; never log document content, titles, file names, tokens or request bodies.
7. Pull requests use the hand-off template in `docs/plan/06-agent-workflow.md`; Conventional Commits; DCO rules from `CONTRIBUTING.md` once enabled (agents never sign off themselves; the human submitter certifies, per ADR-0003).
8. Clean room: never copy code under an incompatible license.
9. Stop and ask, with options and a recommendation, when the brief is ambiguous, when you need a new decision, or when work touches cryptography, authentication, authorization, licensing or the owner's accounts. Never design cryptography yourself (ADR-0016 §12).
10. Explain your work in plain language for the owner; text meant for the owner to copy is written as flowing paragraphs without hard line breaks.

## Rules specific to bayan-server

- **Licensing (ADR-0003):** the server is AGPL-3.0-or-later, except `integrations/` (machine-readable API descriptions, SDKs and examples), which is Apache-2.0 so companies can integrate without touching AGPL code. `integrations/` must never contain, copy from or depend on AGPL server code or GPL core code; it is written against the published specifications in `docs/specs/protocols/`. The server shows a "Source code" link to the exact source of the running version (OPS-08); keep it working. `REUSE.toml` records which license applies to which files and `LICENSES/` holds the full texts; keep `reuse lint` passing, and add a new license text only with `reuse download <SPDX-ID>`.
- **Zero-knowledge invariant (ADR-0015, ADR-0016):** no code path receives, stores, derives or logs document plaintext or document keys. Document data is opaque ciphertext to the server. Never add a feature that requires reading content (server-side search, previews, content in notifications); such features are built on clients.
- **Authorization is enforced twice:** the server enforces roles from the authenticated connection and public group state; clients independently verify. Server changes must not assume clients will catch server mistakes, and vice versa.
- **Metadata minimization:** store and log only what is needed to route ciphertext; every stored field is justified in the threat model's metadata inventory.
- **Rust conventions** match bayan-core: pinned toolchain, edition 2024, `forbid(unsafe_code)`, strict lints, exact pins. SQLite (C) is permitted only as the embedded metadata database (ADR-0006 §3), and AWS-LC only as rustls's crypto provider for TLS (ADR-0028, ADR-0006 amendment 2026-10-04); no other C dependencies without an ADR amendment.
- **Container rules:** minimal base image pinned by digest, non-root user, read-only root filesystem, data only under the mounted volume, signed images with SBOM and provenance (from X-101).
- **Single binary, single container by default (ADR-0015):** optional services (PostgreSQL, S3-compatible storage) must never become mandatory for small deployments.

## Verification gate

Run `cargo xtask verify` before every push. In order it runs `cargo fmt --check`, Clippy with warnings denied, all tests with `--locked`, `cargo doc` with warnings denied, `cargo deny check`, and the hook where X-003 adds its supply-chain checks. CI (`.github/workflows/ci.yml`) also runs:

- `cargo xtask test-postgres` with `BAYAN_TEST_POSTGRES_URL` set: the PostgreSQL integration test (ignored in plain `cargo test` because it needs a server);
- `cargo xtask sqlx-prepare --check` with `BAYAN_SQLX_POSTGRES_URL` set: the committed sqlx query metadata (`crates/bayan-db-*/.sqlx/`) must match the migrations and queries;
- the container build, `scripts/container-smoke-test.sh` (read-only root filesystem, non-root user, health check, clean shutdown) and the grype vulnerability scan of the image and of `Cargo.lock`.

`scripts/dev-setup.sh` installs the pinned non-Rust tools (cargo-deny, grype) with checksum verification.

## Working in this repository

- **Layout:** `crates/bayan-server` (binary and library: configuration, logging, HTTP, lifecycle), `crates/bayan-db-sqlite` and `crates/bayan-db-postgres` (one crate per database, each with its migrations and compile-time-checked queries), `xtask` (automation, standard library only), `docs/` (configuration and deployment), `scripts/`.
- **Database changes:** add a migration to *both* backend crates (never edit a released one), write queries with `sqlx::query!` in the backend crates, then run `cargo xtask sqlx-prepare` with `BAYAN_SQLX_POSTGRES_URL` pointing at a PostgreSQL server whose user may create databases, and commit the regenerated `.sqlx/` files. It needs `python3` and `psql`. A throwaway server with the image CI uses: `docker run --rm -d --name bayan-pg -e POSTGRES_USER=bayan -e POSTGRES_PASSWORD=dev-only -p 127.0.0.1:5432:5432 docker.io/library/postgres:18.6-trixie@sha256:5a5a84b19854a9ffaa54082c166ff4ec27473a361e496e5ea167f298f2da9722`, then `BAYAN_SQLX_POSTGRES_URL=postgres://bayan:dev-only@127.0.0.1:5432/postgres cargo xtask sqlx-prepare`. Builds use the committed metadata (`SQLX_OFFLINE=true` in `.cargo/config.toml`), so they never need a database.
- **Logging:** log only content-free facts. Never log request bodies, header values, query strings, raw paths, document content, titles, file names, user identifiers or secrets; `tests/logging_*.rs` must keep passing. Secrets are wrapped in `config::Secret`, whose `Debug` output is redacted.
- **Configuration:** every new setting gets an environment variable, a TOML key, validation with a clear error, a test, and a row in `docs/configuration.md`.
- **Security headers** for every response are in `crates/bayan-server/src/http/headers.rs` (ADR-0014).
- **Connections** are served by `crates/bayan-server/src/http/serve.rs` with hyper's HTTP/1 implementation, a timer and a header-read timeout, plus graceful shutdown. Do not switch to `axum::serve`: it gives hyper no timer, which silently disables the header-read timeout and lets idle or half-sent connections pile up. `tests/connections.rs` guards this.
- **Request IDs** are keyed hashes of a counter (`http/request_id.rs`), so they reveal nothing about traffic volume; keep them opaque.
- **Container:** the final image is built `FROM scratch` and holds only the binary, `/data`, `/etc/passwd` and `/etc/group`. Every file in it must have a license on the ADR-0017 allowlist, so `scripts/container-smoke-test.sh` fails on any other file: check a new file's license before adding it to that list. The builder and PostgreSQL images are pinned by digest in `Dockerfile`, `compose.yaml` and the CI workflow; the builder's Rust version must equal `rust-toolchain.toml` (the build checks this).

## Dependency mechanisms

Exact `=x.y.z` requirements in `[workspace.dependencies]`; `Cargo.lock` committed and builds run with `--locked`; `.cargo/config.toml` sets `global-min-publish-age = "1 day"`; `cargo deny` runs in `cargo xtask verify`, and a lockfile-age check joins it in CI with X-003. Base images, the PostgreSQL service image and CI actions are pinned by digest or commit SHA and updated only in the monthly dependency session.

The pinned toolchain (Rust 1.99) ignores `global-min-publish-age` and prints a warning about it on every Cargo command; Cargo enforces it from Rust 1.100. Until the workspace moves to 1.100, resolve the lockfile with the pinned nightly from the cloud environment, which already enforces it: `cargo +nightly-2026-10-02 update` (or `generate-lockfile`) reports "as of 24 hours ago" and skips younger versions. Then build and test with the pinned stable toolchain as usual.

`deny.toml` bans crates that bundle C or C++ code. Today the only native code in the build is SQLite, compiled from source by `libsqlite3-sys` (ADR-0006 §3). ADR-0028 also allows AWS-LC as rustls's crypto provider: the work package that adds TLS unbans `aws-lc-sys`, lets it use `cc`, and adds the `webpki-roots` license exception. Any other native code needs an ADR amendment.

In BayanDocs cloud sessions the tools are preinstalled at pinned versions by `docs/scripts/cloud-environment-setup.sh`; run `bayandocs-tools` to list them. If a tool is missing, install the version pinned there (never a newer one) and mention it in the pull request.
