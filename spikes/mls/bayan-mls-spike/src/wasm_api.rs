//! What a web client would call, exported to JavaScript with wasm-bindgen. It exists so that `cargo xtask mls-spike-wasm-size` measures the size of a realistic client: everything these functions reach is in the WebAssembly file, and nothing else (the server's validator, the simulation and the benchmarks are left out).

use wasm_bindgen::prelude::{JsError, wasm_bindgen};

use crate::client::{Change, Client, Received};
use crate::identity::DeviceId;
use crate::suite::Suite;
use crate::update::{Update, UpdateKind};

/// One device's MLS client, for JavaScript.
#[wasm_bindgen]
#[derive(Debug)]
pub struct WebClient {
    inner: Client,
}

fn js(error: impl std::fmt::Display) -> JsError {
    JsError::new(&error.to_string())
}

#[wasm_bindgen]
impl WebClient {
    /// A new device named `user/device`.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid name.
    #[wasm_bindgen(constructor)]
    pub fn new(device: &str) -> Result<WebClient, JsError> {
        let device = DeviceId::parse(device).map_err(js)?;
        Ok(Self {
            inner: Client::new(device).map_err(js)?,
        })
    }

    /// A key package for the classical suite.
    ///
    /// # Errors
    ///
    /// Returns an error if OpenMLS fails.
    #[wasm_bindgen(js_name = keyPackage)]
    pub fn key_package(&self) -> Result<Vec<u8>, JsError> {
        self.inner.key_package(Suite::Classical).map_err(js)
    }

    /// Creates a group owned by this device; returns its ID.
    ///
    /// # Errors
    ///
    /// Returns an error if OpenMLS fails.
    #[wasm_bindgen(js_name = createGroup)]
    pub fn create_group(&mut self) -> Result<Vec<u8>, JsError> {
        self.inner.create_group(Suite::Classical).map_err(js)
    }

    /// Joins from a Welcome and the server's ratchet tree; returns the group's ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the Welcome or tree is invalid or the roles do not check out.
    pub fn join(&mut self, welcome: &[u8], ratchet_tree: &[u8]) -> Result<Vec<u8>, JsError> {
        self.inner.join(welcome, ratchet_tree).map_err(js)
    }

    /// Prepares a commit adding the devices of these key packages (with roles set by the caller in later versions) or, with none, updating this device's keys; returns the commit.
    ///
    /// # Errors
    ///
    /// Returns an error if the change is not allowed or OpenMLS fails.
    pub fn commit(
        &mut self,
        group: &[u8],
        key_package: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, JsError> {
        let change = Change {
            add: key_package.into_iter().collect(),
            ..Change::default()
        };
        Ok(self.inner.commit(group, &change).map_err(js)?.commit)
    }

    /// Applies this device's pending commit after the server accepted it.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no pending commit.
    #[wasm_bindgen(js_name = confirmCommit)]
    pub fn confirm_commit(&mut self, group: &[u8]) -> Result<(), JsError> {
        self.inner.confirm_commit(group).map_err(js)
    }

    /// Encrypts a content update.
    ///
    /// # Errors
    ///
    /// Returns an error if this device's role does not allow editing or OpenMLS fails.
    pub fn send(&mut self, group: &[u8], body: &[u8]) -> Result<Vec<u8>, JsError> {
        self.inner
            .send(group, &Update::new(UpdateKind::Content, body.to_vec()))
            .map_err(js)
    }

    /// Processes a message from the server; returns the update's body, or nothing for a commit.
    ///
    /// # Errors
    ///
    /// Returns an error if the message is invalid or its sender lacks the role.
    pub fn receive(&mut self, group: &[u8], message: &[u8]) -> Result<Option<Vec<u8>>, JsError> {
        Ok(match self.inner.receive(group, message).map_err(js)? {
            Received::Update { update, .. } => Some(update.body),
            Received::Commit { .. } => None,
        })
    }
}
