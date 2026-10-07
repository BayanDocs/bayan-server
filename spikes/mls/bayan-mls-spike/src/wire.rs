//! Turning bytes from the network into MLS messages, and MLS messages into the facts the policy needs. Used by clients and by the server's validator alike, so both read commits the same way.

use std::collections::BTreeSet;

use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::{
    BasicCredential, Credential, KeyPackage, KeyPackageIn, LeafNodeIndex, Member, MlsMessageBodyIn,
    MlsMessageIn, MlsMessageOut, Proposal, ProposalOrRefType, ProtocolVersion, RatchetTreeIn,
    StagedCommit,
};
use openmls::treesync::RatchetTree;
use openmls_traits::crypto::OpenMlsCrypto;

use crate::identity::DeviceId;
use crate::policy::{CommitSummary, GroupView};
use crate::roster::Roster;

/// The largest MLS message accepted, 16 MiB. The largest messages are commits and Welcomes for 1,000 members with the hybrid post-quantum suite, about 1.3 MB (REPORT.md); anything far above that is refused before parsing.
pub const MAX_MESSAGE_LEN: usize = 16 * 1024 * 1024;

/// Why bytes could not be read as the MLS message expected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    /// Too large, not a valid encoding, or a different kind of message than expected.
    #[error("the message is malformed or of the wrong kind")]
    Malformed,
    /// A key package's signature, version or keys are invalid.
    #[error("the key package is invalid")]
    InvalidKeyPackage,
    /// A credential is not a basic credential naming a `user/device`.
    #[error("a credential does not name a valid device")]
    InvalidCredential,
    /// A leaf index does not point at a member.
    #[error("a leaf index does not belong to a member")]
    UnknownLeaf,
    /// The group context's roster is missing or malformed.
    #[error("the group's role roster is missing or malformed")]
    InvalidRoster,
}

/// Decodes one MLS message, refusing anything above [`MAX_MESSAGE_LEN`] or with trailing bytes.
///
/// # Errors
///
/// Returns [`WireError::Malformed`].
pub fn decode(bytes: &[u8]) -> Result<MlsMessageBodyIn, WireError> {
    if bytes.len() > MAX_MESSAGE_LEN {
        return Err(WireError::Malformed);
    }
    let message = MlsMessageIn::tls_deserialize_exact(bytes).map_err(|_| WireError::Malformed)?;
    Ok(message.extract())
}

/// Encodes an MLS message for the network.
///
/// # Errors
///
/// Returns [`WireError::Malformed`] if it cannot be encoded (only possible for impossibly large messages).
pub fn encode(message: &MlsMessageOut) -> Result<Vec<u8>, WireError> {
    message
        .tls_serialize_detached()
        .map_err(|_| WireError::Malformed)
}

/// Encodes a ratchet tree (the group's public state) for the network.
///
/// # Errors
///
/// Returns [`WireError::Malformed`] if it cannot be encoded.
pub fn encode_tree(tree: &RatchetTree) -> Result<Vec<u8>, WireError> {
    tree.tls_serialize_detached()
        .map_err(|_| WireError::Malformed)
}

/// Decodes a ratchet tree. OpenMLS verifies it against the group's tree hash when it is used.
///
/// # Errors
///
/// Returns [`WireError::Malformed`].
pub fn decode_tree(bytes: &[u8]) -> Result<RatchetTreeIn, WireError> {
    if bytes.len() > MAX_MESSAGE_LEN {
        return Err(WireError::Malformed);
    }
    RatchetTreeIn::tls_deserialize_exact(bytes).map_err(|_| WireError::Malformed)
}

/// Decodes a key package and verifies its signature, protocol version, lifetime and keys.
///
/// # Errors
///
/// Returns [`WireError::Malformed`] or [`WireError::InvalidKeyPackage`].
pub fn decode_key_package(
    bytes: &[u8],
    crypto: &impl OpenMlsCrypto,
) -> Result<KeyPackage, WireError> {
    let MlsMessageBodyIn::KeyPackage(key_package) = decode(bytes)? else {
        return Err(WireError::Malformed);
    };
    validate_key_package(key_package, crypto)
}

fn validate_key_package(
    key_package: KeyPackageIn,
    crypto: &impl OpenMlsCrypto,
) -> Result<KeyPackage, WireError> {
    key_package
        .validate(crypto, ProtocolVersion::Mls10)
        .map_err(|_| WireError::InvalidKeyPackage)
}

/// The device a credential names.
///
/// # Errors
///
/// Returns [`WireError::InvalidCredential`] unless it is a basic credential whose identity is a valid `user/device`.
pub fn device_of(credential: &Credential) -> Result<DeviceId, WireError> {
    let basic =
        BasicCredential::try_from(credential.clone()).map_err(|_| WireError::InvalidCredential)?;
    DeviceId::from_credential_identity(basic.identity()).map_err(|_| WireError::InvalidCredential)
}

/// The policy's view of a group: its members' devices and the roster in its group context.
///
/// # Errors
///
/// Returns an error if a member's credential or the roster is invalid.
pub fn view_of(
    members: impl Iterator<Item = Member>,
    roster: Result<Roster, crate::roster::RosterError>,
) -> Result<GroupView, WireError> {
    let members = members
        .map(|member| device_of(&member.credential))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let roster = roster.map_err(|_| WireError::InvalidRoster)?;
    Ok(GroupView { members, roster })
}

/// Reads what a commit does. `committer` is the credential of the commit's sender and `credential_at` looks up the credentials of the group's members before the commit.
///
/// # Errors
///
/// Returns an error if a credential, a leaf index or the new roster is invalid.
pub fn summarize_commit(
    committer: &Credential,
    staged: &StagedCommit,
    credential_at: impl Fn(LeafNodeIndex) -> Option<Credential>,
) -> Result<CommitSummary, WireError> {
    let mut summary = CommitSummary {
        committer: device_of(committer)?,
        added: Vec::new(),
        removed: Vec::new(),
        new_roster: None,
        path_identity: staged
            .update_path_leaf_node()
            .map(|leaf| device_of(leaf.credential()))
            .transpose()?,
        unsupported: Vec::new(),
    };
    let mut changes_extensions = false;
    for queued in staged.queued_proposals() {
        if queued.proposal_or_ref_type() == ProposalOrRefType::Reference {
            summary.unsupported.push("proposal by reference".to_owned());
            continue;
        }
        match queued.proposal() {
            Proposal::Add(add) => summary
                .added
                .push(device_of(add.key_package().leaf_node().credential())?),
            Proposal::Remove(remove) => {
                let credential = credential_at(remove.removed()).ok_or(WireError::UnknownLeaf)?;
                summary.removed.push(device_of(&credential)?);
            }
            Proposal::GroupContextExtensions(_) => changes_extensions = true,
            other => summary
                .unsupported
                .push(format!("{:?}", other.proposal_type())),
        }
    }
    if changes_extensions {
        summary.new_roster = Some(
            Roster::from_group_context(staged.group_context())
                .map_err(|_| WireError::InvalidRoster)?,
        );
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test(unsupported = test)]
    fn hostile_bytes_are_refused_without_panicking() {
        let crypto = openmls_rust_crypto::RustCrypto::default();
        let samples: Vec<Vec<u8>> = vec![
            vec![],
            vec![0],
            vec![0, 1],
            vec![0, 1, 0, 1],
            vec![0, 1, 0, 2, 0xff, 0xff, 0xff, 0xff],
            vec![0xff; 64],
            (0..=255).collect(),
            vec![0; MAX_MESSAGE_LEN + 1],
        ];
        for sample in &samples {
            assert!(decode(sample).is_err(), "{} bytes decoded", sample.len());
            assert!(decode_key_package(sample, &crypto).is_err());
            // A single zero byte is a valid, empty tree; OpenMLS rejects it when it is used.
            if sample.len() > 1 {
                assert!(
                    decode_tree(sample).is_err(),
                    "{} bytes decoded as a tree",
                    sample.len()
                );
            }
        }
    }
}
