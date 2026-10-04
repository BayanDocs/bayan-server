# bayan-server

The collaboration backbone of [BayanDocs](https://github.com/BayanDocs/docs): accounts, devices, sharing and roles, and a relay that stores and forwards end-to-end-encrypted collaboration data. It is **zero-knowledge**: documents are encrypted on users' devices with the IETF Messaging Layer Security standard (MLS), and the server can never read their content, titles or structure.

It ships as a single Rust binary in a single container. By default it needs nothing else (an embedded database and local disk); larger deployments can use PostgreSQL and any S3-compatible object store. It also serves the [web app](https://github.com/BayanDocs/bayan-web), so `docker run` gives a complete, self-hosted BayanDocs.

> **Status: Phase 0 (Foundations).** No code yet. The first work package is [SRV-001](https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/SRV-001-server-scaffold.md).

## Where things are decided

- Master plan: [docs/plan](https://github.com/BayanDocs/docs/tree/HEAD/plan)
- Server decision: [ADR-0015](https://github.com/BayanDocs/docs/blob/HEAD/adr/0015-server-architecture.md); encryption and identity: [ADR-0016](https://github.com/BayanDocs/docs/blob/HEAD/adr/0016-e2ee-and-identity.md); [threat model](https://github.com/BayanDocs/docs/blob/HEAD/specs/threat-model.md)
- Work packages: [docs/workpackages](https://github.com/BayanDocs/docs/tree/HEAD/workpackages)

Contributors and agents: start with [AGENTS.md](AGENTS.md).

## License

Licensed under the GNU Affero General Public License v3.0 or later ([LICENSE](LICENSE)); SPDX: `AGPL-3.0-or-later`. Exception: [integrations/](integrations/README.md) (API descriptions, SDKs and examples) is licensed Apache-2.0, so companies can integrate without touching AGPL code. Running the unmodified server, connecting clients and building integrations carry no obligations. [REUSE.toml](REUSE.toml) records which license applies to which files, and [LICENSES/](LICENSES) holds the full texts. See the [licensing FAQ](https://github.com/BayanDocs/docs/blob/HEAD/LICENSING.md) and [ADR-0003](https://github.com/BayanDocs/docs/blob/HEAD/adr/0003-licensing-and-contribution-model.md).
