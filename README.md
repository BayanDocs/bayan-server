# bayan-server

The collaboration backbone of [BayanDocs](https://github.com/BayanDocs/docs): accounts, devices, sharing and roles, and a relay that stores and forwards end-to-end-encrypted collaboration data. It is **zero-knowledge**: documents are encrypted on users' devices with the IETF Messaging Layer Security standard (MLS), and the server can never read their content, titles or structure.

It ships as a single Rust binary in a single container. By default it needs nothing else (an embedded database and local disk); larger deployments can use PostgreSQL and any S3-compatible object store. It also serves the [web app](https://github.com/BayanDocs/bayan-web), so `docker run` gives a complete, self-hosted BayanDocs.

> **Status: Phase 0 (Foundations).** The server skeleton from work package [SRV-001](https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/SRV-001-server-scaffold.md) is in place: configuration, content-free logging, the SQLite and PostgreSQL metadata database with migrations, health endpoints, static file serving with security headers, the container image and CI. Accounts, document storage, WebSockets and MLS come in later work packages.

## Quick start

Run the server in a hardened container, with its built-in SQLite database on a volume:

```sh
docker build -t bayan-server .
docker run --detach --name bayan-server --read-only --cap-drop ALL --security-opt no-new-privileges \
  --volume bayan-data:/data --publish 127.0.0.1:8080:8080 bayan-server
curl http://127.0.0.1:8080/readyz
```

Or with Docker Compose for development: `docker compose up --build` (add `--profile postgres` for a PostgreSQL setup on port 8081).

- [docs/configuration.md](docs/configuration.md): every setting (environment variables, the TOML file, secrets from files), the database, logs and endpoints.
- [docs/deployment.md](docs/deployment.md): building, running, upgrading and checking the container, including PostgreSQL.

## Development

You need the Rust toolchain pinned in `rust-toolchain.toml` (rustup installs it automatically) and the tools from `scripts/dev-setup.sh`: cargo-deny and grype, and for the MLS spike's WebAssembly tests wasm-bindgen, Node.js, Chromium's headless shell and chromedriver. The script is idempotent, installs every tool at a pinned version and checks every download against a pinned SHA-256 hash; `scripts/dev-setup.sh <tool>…` installs only the tools named.

```sh
cargo xtask verify                 # the full verification gate: format, lint, test, docs, cargo deny
cargo run -p bayan-server          # run locally on 127.0.0.1:8080 with SQLite in ./data
BAYAN_TEST_POSTGRES_URL=postgres://… cargo xtask test-postgres         # PostgreSQL integration test
BAYAN_SQLX_POSTGRES_URL=postgres://… cargo xtask sqlx-prepare         # regenerate query metadata after changing queries or migrations
scripts/container-smoke-test.sh bayan-server                          # check a built image
```

Builds never need a database: query metadata is committed in `crates/bayan-db-*/.sqlx/`. See [AGENTS.md](AGENTS.md) for the rules and workflow.

[spikes/](spikes/README.md) holds time-boxed experiments that are never part of the server, such as the MLS spike of work package SRV-002 ([report](spikes/mls/REPORT.md)): `cargo xtask mls-spike-wasm` runs its tests in WebAssembly in Node.js and Chromium, after `scripts/dev-setup.sh` has installed the pinned tools.

**BayanDocs cloud sessions:** the cloud environment's setup script will run `scripts/dev-setup.sh` once the batch of environment changes collected in [BayanDocs/docs#7](https://github.com/BayanDocs/docs/issues/7) is applied; until then, run it yourself at the start of a session.

## Where things are decided

- Master plan: [docs/plan](https://github.com/BayanDocs/docs/tree/HEAD/plan)
- Server decision: [ADR-0015](https://github.com/BayanDocs/docs/blob/HEAD/adr/0015-server-architecture.md); encryption and identity: [ADR-0016](https://github.com/BayanDocs/docs/blob/HEAD/adr/0016-e2ee-and-identity.md); [threat model](https://github.com/BayanDocs/docs/blob/HEAD/specs/threat-model.md)
- Work packages: [docs/workpackages](https://github.com/BayanDocs/docs/tree/HEAD/workpackages)

Contributors and agents: start with [AGENTS.md](AGENTS.md).

## License

Licensed under the GNU Affero General Public License v3.0 or later ([LICENSE](LICENSE)); SPDX: `AGPL-3.0-or-later`. Exception: [integrations/](integrations/README.md) (API descriptions, SDKs and examples) is licensed Apache-2.0, so companies can integrate without touching AGPL code. Running the unmodified server, connecting clients and building integrations carry no obligations. [REUSE.toml](REUSE.toml) records which license applies to which files, and [LICENSES/](LICENSES) holds the full texts. See the [licensing FAQ](https://github.com/BayanDocs/docs/blob/HEAD/LICENSING.md) and [ADR-0003](https://github.com/BayanDocs/docs/blob/HEAD/adr/0003-licensing-and-contribution-model.md).
