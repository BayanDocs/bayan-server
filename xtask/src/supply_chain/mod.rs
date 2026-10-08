//! The supply-chain checks of ADR-0017 that Cargo and cargo-deny do not make themselves (work package X-003):
//!
//! - [`exact_pins`], `cargo xtask check-exact-pins`: every dependency is pinned exactly (rule 5).
//! - [`lockfile_age`], `cargo xtask check-lockfile-age`: Cargo builds exactly what `Cargo.lock` lists, and every package version that a change adds to it was published at least 24 hours before the commit that added it, and before now (rule 4), with the checksum crates.io published.
//!
//! The verification gate runs both in its supply-chain step, before anything is built, so that no code of a dependency that fails them runs first. The other rules are enforced elsewhere: cargo-deny checks advisories, licenses, banned crates and sources (`deny.toml`), and the "Supply chain" workflow in `.github/` fails on update-bot configuration and audits the Python tools of CI.
//!
//! This folder is identical in bayan-core and bayan-server (`xtask/src/supply_chain/`): change both copies together, so that a fix made in one repository reaches the other unchanged. It uses only the standard library, and runs `git`, `curl` and `cargo` as programs.

mod cargo;
mod cargo_lock;
mod crates_io;
pub mod exact_pins;
mod git;
mod json;
pub mod lockfile_age;
mod manifest;
mod time;
