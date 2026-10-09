//! The server's key package directory: devices publish key packages, and whoever adds them to a group fetches one (each is used once).
//!
//! Key packages are public. The directory checks each one before storing it, so it never hands out garbage: a valid signature and lifetime, a ciphersuite the server accepts, a credential naming exactly the device the transport authenticated, and support for the role roster.

use std::collections::BTreeMap;

use openmls::prelude::{CredentialType, ExtensionType};
use openmls_rust_crypto::RustCrypto;

use crate::identity::DeviceId;
use crate::roster::ROSTER_EXTENSION_TYPE;
use crate::suite::Suite;
use crate::validator::Rejection;
use crate::wire::{self, WireError};

/// How many unused key packages one device may keep on the server.
pub const MAX_KEY_PACKAGES_PER_DEVICE: usize = 100;

/// Key packages by device and ciphersuite.
#[derive(Debug, Default)]
pub struct KeyPackageDirectory {
    crypto: RustCrypto,
    packages: BTreeMap<(DeviceId, u16), Vec<Vec<u8>>>,
}

impl KeyPackageDirectory {
    /// An empty directory.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores a key package that `authenticated` published.
    ///
    /// # Errors
    ///
    /// Returns an error if the key package is invalid, names another device, uses a ciphersuite the spike does not use, lacks roster support, or the device has too many stored.
    pub fn publish(
        &mut self,
        authenticated: &DeviceId,
        key_package: &[u8],
    ) -> Result<(), Rejection> {
        let parsed = wire::decode_key_package(key_package, &self.crypto)?;
        let leaf = parsed.leaf_node();
        if &wire::device_of(leaf.credential())? != authenticated {
            return Err(Rejection::SenderMismatch);
        }
        let suite = Suite::from_ciphersuite(parsed.ciphersuite())
            .ok_or(Rejection::UnsupportedCiphersuite)?;
        let capabilities = leaf.capabilities();
        if !capabilities
            .extensions()
            .contains(&ExtensionType::Unknown(ROSTER_EXTENSION_TYPE))
            || !capabilities.credentials().contains(&CredentialType::Basic)
        {
            return Err(WireError::InvalidKeyPackage.into());
        }
        let stored = self
            .packages
            .entry((authenticated.clone(), u16::from(suite.ciphersuite())))
            .or_default();
        if stored.len() >= MAX_KEY_PACKAGES_PER_DEVICE {
            return Err(Rejection::Unsupported(
                "more key packages than the per-device limit",
            ));
        }
        stored.push(key_package.to_vec());
        Ok(())
    }

    /// Takes one of `device`'s key packages for `suite`, if any is left.
    pub fn fetch(&mut self, device: &DeviceId, suite: Suite) -> Option<Vec<u8>> {
        let stored = self
            .packages
            .get_mut(&(device.clone(), u16::from(suite.ciphersuite())))?;
        stored.pop()
    }

    /// How many key packages `device` has left for `suite`.
    #[must_use]
    pub fn available(&self, device: &DeviceId, suite: Suite) -> usize {
        self.packages
            .get(&(device.clone(), u16::from(suite.ciphersuite())))
            .map_or(0, Vec::len)
    }
}
