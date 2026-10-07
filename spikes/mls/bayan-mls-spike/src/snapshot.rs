//! Encrypted snapshots, so that members who join later can read the current document (ADR-0016 Decision 4, AC-4).
//!
//! MLS gives forward secrecy: a newcomer cannot decrypt anything sent before they joined. A member with the right to share content (an owner or editor) therefore encrypts a snapshot of the document state under a fresh random key, uploads the ciphertext to the server as an opaque blob, and sends the key to the group in an MLS application message of kind [`crate::update::UpdateKind::SnapshotKey`]. A newcomer receives that message after joining, fetches the blob, checks it and decrypts it.
//!
//! Everything comes from MLS and its provider; nothing here is new cryptography (ADR-0016 §12):
//!
//! - the AEAD is the group's own (AES-128-GCM in the suites the plan uses), through the OpenMLS provider, with a fresh random key and nonce per snapshot, so a key is used exactly once;
//! - the associated data binds the ciphertext to the document's group and the snapshot's ID, so a blob cannot be passed off as another document's or another snapshot's;
//! - the key message also carries the SHA-256 hash of the exact blob, so the server cannot swap in a different blob, and a sender cannot make one blob decrypt differently for different members (AES-GCM alone is not key-committing);
//! - the key message itself is protected and authenticated by MLS, and recipients accept it only from a sender whose role may share content.
//!
//! The security properties and their limits are discussed in REPORT.md.

use openmls_traits::crypto::OpenMlsCrypto;
use openmls_traits::random::OpenMlsRand;
use openmls_traits::types::{AeadType, HashType};

use crate::suite::Suite;

/// Bound into every snapshot's associated data, so these ciphertexts can never be confused with any other use of the same AEAD.
const LABEL: &[u8] = b"BayanDocs snapshot v1";
const KEY_FORMAT_VERSION: u8 = 1;
/// The length of a snapshot ID.
pub const ID_LEN: usize = 16;
/// The largest snapshot accepted, 256 MiB.
pub const MAX_SNAPSHOT_LEN: usize = 256 * 1024 * 1024;

/// Why a snapshot could not be made or opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotError {
    /// The blob does not match the hash in the key message: the server (or someone) changed or swapped it.
    #[error("the snapshot does not match its key message")]
    WrongBlob,
    /// The blob or key message is malformed, or decryption failed.
    #[error("the snapshot cannot be decrypted")]
    Malformed,
    /// The crypto provider failed.
    #[error("crypto provider error")]
    Crypto,
}

/// What the server stores: opaque to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedSnapshot {
    /// The blob: the snapshot ID, the nonce and the ciphertext.
    pub blob: Vec<u8>,
}

/// What the group receives in an MLS application message: everything a member needs to fetch, check and decrypt one snapshot.
#[derive(Clone, PartialEq, Eq)]
pub struct SnapshotKey {
    /// The snapshot's ID, under which the server stores the blob.
    pub id: [u8; ID_LEN],
    /// The AEAD (the group's).
    pub aead: AeadType,
    /// The snapshot's key, used for this snapshot only.
    pub key: Vec<u8>,
    /// SHA-256 of the blob.
    pub blob_hash: Vec<u8>,
}

impl std::fmt::Debug for SnapshotKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print key material.
        formatter
            .debug_struct("SnapshotKey")
            .field("id", &self.id)
            .field("aead", &self.aead)
            .finish_non_exhaustive()
    }
}

fn associated_data(group_id: &[u8], id: &[u8; ID_LEN]) -> Result<Vec<u8>, SnapshotError> {
    let mut aad = Vec::with_capacity(LABEL.len() + 1 + group_id.len() + ID_LEN);
    aad.extend_from_slice(LABEL);
    aad.push(u8::try_from(group_id.len()).map_err(|_| SnapshotError::Malformed)?);
    aad.extend_from_slice(group_id);
    aad.extend_from_slice(id);
    Ok(aad)
}

fn aead_code(aead: AeadType) -> u16 {
    match aead {
        AeadType::Aes128Gcm => 1,
        AeadType::Aes256Gcm => 2,
        AeadType::ChaCha20Poly1305 => 3,
    }
}

fn aead_from_code(code: u16) -> Option<AeadType> {
    match code {
        1 => Some(AeadType::Aes128Gcm),
        2 => Some(AeadType::Aes256Gcm),
        3 => Some(AeadType::ChaCha20Poly1305),
        _ => None,
    }
}

/// Encrypts `state` as a snapshot of the group `group_id`, which uses `suite`.
///
/// # Errors
///
/// Returns an error if the state is too large or the provider fails.
pub fn seal(
    crypto: &impl OpenMlsCrypto,
    rand: &impl OpenMlsRand,
    group_id: &[u8],
    suite: Suite,
    state: &[u8],
) -> Result<(EncryptedSnapshot, SnapshotKey), SnapshotError> {
    if state.len() > MAX_SNAPSHOT_LEN {
        return Err(SnapshotError::Malformed);
    }
    let aead = suite.ciphersuite().aead_algorithm();
    let id: [u8; ID_LEN] = rand.random_array().map_err(|_| SnapshotError::Crypto)?;
    let key = rand
        .random_vec(aead.key_size())
        .map_err(|_| SnapshotError::Crypto)?;
    let nonce = rand
        .random_vec(aead.nonce_size())
        .map_err(|_| SnapshotError::Crypto)?;
    let ciphertext = crypto
        .aead_encrypt(aead, &key, state, &nonce, &associated_data(group_id, &id)?)
        .map_err(|_| SnapshotError::Crypto)?;
    let mut blob = Vec::with_capacity(ID_LEN + nonce.len() + ciphertext.len());
    blob.extend_from_slice(&id);
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ciphertext);
    let blob_hash = crypto
        .hash(HashType::Sha2_256, &blob)
        .map_err(|_| SnapshotError::Crypto)?;
    Ok((
        EncryptedSnapshot { blob },
        SnapshotKey {
            id,
            aead,
            key,
            blob_hash,
        },
    ))
}

/// Checks a blob against its key message and decrypts it, for the group `group_id`.
///
/// # Errors
///
/// Returns [`SnapshotError::WrongBlob`] if the blob is not the one the key message names, or [`SnapshotError::Malformed`] if it does not decrypt.
pub fn open(
    crypto: &impl OpenMlsCrypto,
    key: &SnapshotKey,
    group_id: &[u8],
    blob: &[u8],
) -> Result<Vec<u8>, SnapshotError> {
    if blob.len() > MAX_SNAPSHOT_LEN + 1024 {
        return Err(SnapshotError::Malformed);
    }
    let hash = crypto
        .hash(HashType::Sha2_256, blob)
        .map_err(|_| SnapshotError::Crypto)?;
    if hash != key.blob_hash {
        return Err(SnapshotError::WrongBlob);
    }
    let nonce_end = ID_LEN + key.aead.nonce_size();
    if blob.len() < nonce_end || blob[..ID_LEN] != key.id {
        return Err(SnapshotError::Malformed);
    }
    crypto
        .aead_decrypt(
            key.aead,
            &key.key,
            &blob[nonce_end..],
            &blob[ID_LEN..nonce_end],
            &associated_data(group_id, &key.id)?,
        )
        .map_err(|_| SnapshotError::Malformed)
}

impl SnapshotKey {
    /// Encodes the key message's body: version, ID, AEAD, key and blob hash.
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotError::Malformed`] for impossible lengths.
    pub fn to_bytes(&self) -> Result<Vec<u8>, SnapshotError> {
        let mut bytes = vec![KEY_FORMAT_VERSION];
        bytes.extend_from_slice(&self.id);
        bytes.extend_from_slice(&aead_code(self.aead).to_be_bytes());
        bytes.push(u8::try_from(self.key.len()).map_err(|_| SnapshotError::Malformed)?);
        bytes.extend_from_slice(&self.key);
        bytes.push(u8::try_from(self.blob_hash.len()).map_err(|_| SnapshotError::Malformed)?);
        bytes.extend_from_slice(&self.blob_hash);
        Ok(bytes)
    }

    /// Decodes a key message's body.
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotError::Malformed`] unless it is exactly one well-formed key message.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SnapshotError> {
        let (&version, rest) = bytes.split_first().ok_or(SnapshotError::Malformed)?;
        if version != KEY_FORMAT_VERSION || rest.len() < ID_LEN + 3 {
            return Err(SnapshotError::Malformed);
        }
        let (id, rest) = rest.split_at(ID_LEN);
        let (aead, rest) = rest.split_at(2);
        let aead = aead_from_code(u16::from_be_bytes([aead[0], aead[1]]))
            .ok_or(SnapshotError::Malformed)?;
        let (&key_length, rest) = rest.split_first().ok_or(SnapshotError::Malformed)?;
        let key_length = usize::from(key_length);
        if key_length != aead.key_size() || rest.len() < key_length + 1 {
            return Err(SnapshotError::Malformed);
        }
        let (key, rest) = rest.split_at(key_length);
        let (&hash_length, hash) = rest.split_first().ok_or(SnapshotError::Malformed)?;
        if usize::from(hash_length) != 32 || hash.len() != 32 {
            return Err(SnapshotError::Malformed);
        }
        let id = <[u8; ID_LEN]>::try_from(id).map_err(|_| SnapshotError::Malformed)?;
        Ok(Self {
            id,
            aead,
            key: key.to_vec(),
            blob_hash: hash.to_vec(),
        })
    }
}
