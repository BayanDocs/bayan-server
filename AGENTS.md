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
- **Rust conventions** match bayan-core: pinned toolchain, edition 2024, `forbid(unsafe_code)`, strict lints, exact pins. SQLite (C) is permitted only as the embedded metadata database (ADR-0006 §3); no other C dependencies without an ADR amendment.
- **Container rules:** minimal base image pinned by digest, non-root user, read-only root filesystem, data only under the mounted volume, signed images with SBOM and provenance (from X-101).
- **Single binary, single container by default (ADR-0015):** optional services (PostgreSQL, S3-compatible storage) must never become mandatory for small deployments.

## Verification gate

`cargo xtask verify` (created by SRV-001), plus the container build and smoke test in CI. Until SRV-001 has landed there is no code and no gate.

## Dependency mechanisms

Exact `=x.y.z` requirements in `[workspace.dependencies]`; `Cargo.lock` committed and builds run with `--locked`; `.cargo/config.toml` sets `global-min-publish-age = "1 day"`; a lockfile-age check and `cargo deny` run in CI (X-003). Base images are pinned by digest and updated only in the monthly dependency session.

In BayanDocs cloud sessions the tools are preinstalled at pinned versions by `docs/scripts/cloud-environment-setup.sh`; run `bayandocs-tools` to list them. If a tool is missing, install the version pinned there (never a newer one) and mention it in the pull request.
