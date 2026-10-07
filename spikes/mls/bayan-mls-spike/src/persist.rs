//! Saving a client's MLS state and loading it again (the storage-provider measurement of the brief).
//!
//! OpenMLS keeps all state (keys, ratchet trees, epoch secrets) in a storage provider. The spike uses OpenMLS's in-memory provider and saves its key-value pairs wholesale. A device would keep this file encrypted at rest under a key from the operating system's keychain (SEC-11); the spike's file is not encrypted, so it is test data only.
//!
//! Groups with a provisional ciphersuite are never saved (ADR-0016 Decision 2): [`export`] refuses them.
//!
//! Format (version 1), parsed with limits:
//!
//! ```text
//! 8 bytes  "BAYANMLS"
//! u8       format version = 1
//! u8       device name length, then the device name (user/device)
//! u16      signature public key length, then the key
//! u32      group count, then for each: u8 length and the group ID
//! u32      entry count, then for each, sorted by key: u32 key length, key, u32 value length, value
//! ```

use std::collections::BTreeMap;

use openmls::prelude::{GroupId, MlsGroup};
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::OpenMlsProvider as _;
use openmls_traits::types::SignatureScheme;

use crate::client::Client;
use crate::identity::DeviceId;
use crate::suite::Suite;

const MAGIC: &[u8; 8] = b"BAYANMLS";
const FORMAT_VERSION: u8 = 1;
/// The most storage entries a saved state may hold.
pub const MAX_ENTRIES: usize = 1_000_000;
/// The largest saved state accepted, 256 MiB.
pub const MAX_STATE_LEN: usize = 256 * 1024 * 1024;

/// Why a state could not be saved or loaded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PersistError {
    /// A group uses a provisional ciphersuite, which must never be persisted.
    #[error("groups with a provisional ciphersuite are never saved")]
    ProvisionalSuite,
    /// The saved state is malformed, too large or from another format version.
    #[error("the saved state is malformed")]
    Malformed,
    /// The saved state does not contain the device's signature key or a listed group.
    #[error("the saved state is incomplete: {0}")]
    Incomplete(&'static str),
}

/// Saves everything the client's storage provider holds.
///
/// # Errors
///
/// Returns [`PersistError::ProvisionalSuite`] if any group uses a provisional ciphersuite.
pub fn export(client: &Client) -> Result<Vec<u8>, PersistError> {
    for group_id in client.group_ids() {
        let suite = client
            .suite(group_id)
            .map_err(|_| PersistError::Malformed)?;
        if suite.is_provisional() {
            return Err(PersistError::ProvisionalSuite);
        }
    }
    let values = client
        .storage_snapshot()
        .map_err(|_| PersistError::Malformed)?;
    let entries: BTreeMap<Vec<u8>, Vec<u8>> = values.into_iter().collect();
    let device = client.device().to_string();
    let public_key = client.signer().to_public_vec();
    let groups: Vec<&[u8]> = client.group_ids().collect();

    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.push(FORMAT_VERSION);
    push_len_u8(&mut out, device.as_bytes())?;
    out.extend_from_slice(
        &u16::try_from(public_key.len())
            .map_err(|_| PersistError::Malformed)?
            .to_be_bytes(),
    );
    out.extend_from_slice(&public_key);
    out.extend_from_slice(
        &u32::try_from(groups.len())
            .map_err(|_| PersistError::Malformed)?
            .to_be_bytes(),
    );
    for group in groups {
        push_len_u8(&mut out, group)?;
    }
    out.extend_from_slice(
        &u32::try_from(entries.len())
            .map_err(|_| PersistError::Malformed)?
            .to_be_bytes(),
    );
    for (key, value) in &entries {
        push_len_u32(&mut out, key)?;
        push_len_u32(&mut out, value)?;
    }
    Ok(out)
}

/// Loads a client from a saved state.
///
/// # Errors
///
/// Returns an error if the state is malformed, too large, or lacks the signature key or a group.
pub fn import(bytes: &[u8]) -> Result<Client, PersistError> {
    if bytes.len() > MAX_STATE_LEN {
        return Err(PersistError::Malformed);
    }
    let mut reader = Reader(bytes);
    if reader.take(MAGIC.len())? != MAGIC || reader.u8()? != FORMAT_VERSION {
        return Err(PersistError::Malformed);
    }
    let device_length = usize::from(reader.u8()?);
    let device =
        std::str::from_utf8(reader.take(device_length)?).map_err(|_| PersistError::Malformed)?;
    let device = DeviceId::parse(device).map_err(|_| PersistError::Malformed)?;
    let key_length = usize::from(reader.u16()?);
    let public_key = reader.take(key_length)?.to_vec();
    let group_count = reader.count()?;
    let mut group_ids = Vec::with_capacity(group_count.min(1024));
    for _ in 0..group_count {
        let length = usize::from(reader.u8()?);
        group_ids.push(reader.take(length)?.to_vec());
    }
    let entry_count = reader.count()?;
    let provider = OpenMlsRustCrypto::default();
    {
        let mut values = provider
            .storage()
            .values
            .write()
            .map_err(|_| PersistError::Malformed)?;
        for _ in 0..entry_count {
            let key = reader.bytes_u32()?.to_vec();
            let value = reader.bytes_u32()?.to_vec();
            if values.insert(key, value).is_some() {
                return Err(PersistError::Malformed);
            }
        }
    }
    if !reader.0.is_empty() {
        return Err(PersistError::Malformed);
    }
    let signer = SignatureKeyPair::read(provider.storage(), &public_key, SignatureScheme::ED25519)
        .ok_or(PersistError::Incomplete("signature key"))?;
    let mut groups = BTreeMap::new();
    for group_id in group_ids {
        let group = MlsGroup::load(provider.storage(), &GroupId::from_slice(&group_id))
            .map_err(|_| PersistError::Malformed)?
            .ok_or(PersistError::Incomplete("group"))?;
        if Suite::from_ciphersuite(group.ciphersuite()).is_none_or(Suite::is_provisional) {
            return Err(PersistError::ProvisionalSuite);
        }
        groups.insert(group_id, group);
    }
    Client::from_parts(device, provider, signer, groups).map_err(|_| PersistError::Malformed)
}

fn push_len_u8(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), PersistError> {
    out.push(u8::try_from(bytes.len()).map_err(|_| PersistError::Malformed)?);
    out.extend_from_slice(bytes);
    Ok(())
}

fn push_len_u32(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), PersistError> {
    out.extend_from_slice(
        &u32::try_from(bytes.len())
            .map_err(|_| PersistError::Malformed)?
            .to_be_bytes(),
    );
    out.extend_from_slice(bytes);
    Ok(())
}

/// Reads from the front of a slice, failing instead of panicking when it runs out.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], PersistError> {
        if self.0.len() < length {
            return Err(PersistError::Malformed);
        }
        let (taken, rest) = self.0.split_at(length);
        self.0 = rest;
        Ok(taken)
    }

    fn u8(&mut self) -> Result<u8, PersistError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, PersistError> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, PersistError> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn count(&mut self) -> Result<usize, PersistError> {
        let count = usize::try_from(self.u32()?).map_err(|_| PersistError::Malformed)?;
        if count > MAX_ENTRIES {
            return Err(PersistError::Malformed);
        }
        Ok(count)
    }

    fn bytes_u32(&mut self) -> Result<&'a [u8], PersistError> {
        let length = usize::try_from(self.u32()?).map_err(|_| PersistError::Malformed)?;
        self.take(length)
    }
}
