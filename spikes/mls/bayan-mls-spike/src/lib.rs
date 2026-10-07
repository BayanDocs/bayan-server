//! Spike SRV-002: MLS for CRDT updates, natively and in WebAssembly (ADR-0016's validation gate).
//!
//! **Experimental code.** It answers the questions in `spikes/mls/REPORT.md` and is never part of the server binary; findings move into the `bayan-mls` crate in bayan-core through later work packages.
//!
//! What is here:
//!
//! - [`identity`]: users and devices, and the device name inside each MLS credential.
//! - [`roster`]: roles and the role roster, a custom group-context extension.
//! - [`update`]: the plaintext of an application message (a CRDT update, a comment, a snapshot key, …).
//! - [`policy`]: the role policy that the server's validator and every client both enforce.
//! - [`suite`]: the ciphersuites, including the provisional post-quantum one behind the `provisional-pq` feature.
//! - [`wire`]: decoding network bytes into MLS messages with limits, and reading what a commit does.
//! - [`client`]: a device's MLS client.
//! - [`directory`] and [`validator`]: the server side, which stores key packages and validates commits and updates without any key.
//! - [`snapshot`]: encrypted snapshots so that newcomers can read the current document.
//! - [`recreate`]: moving a document to a new group (ciphersuite migration).
//! - [`persist`]: saving and loading a client's MLS state.
//! - [`sim`]: a whole deployment in one process, for tests, benchmarks and the WebAssembly runs.
//! - [`mod@bench`]: the measurements of the report.
//! - [`wasm_api`]: the functions a web client would call, for measuring the WebAssembly size.

#![forbid(unsafe_code)]

pub mod bench;
pub mod client;
pub mod directory;
pub mod identity;
pub mod persist;
pub mod policy;
pub mod recreate;
pub mod roster;
pub mod sim;
pub mod snapshot;
pub mod suite;
pub mod update;
pub mod validator;
pub mod wasm_api;
pub mod wire;
