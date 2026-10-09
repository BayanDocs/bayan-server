//! The plaintext inside an MLS application message: what kind of update it is, and its bytes.
//!
//! The server never sees this: it is encrypted in MLS private framing. Clients read the kind after decrypting and check it against the sender's role (see [`crate::policy::check_update`]). In the product the body is a CRDT update (ADR-0008); the spike carries opaque bytes of the sizes CRDT updates have (100 B to 100 KB).

use crate::roster::Role;

/// The largest update body the spike accepts, 1 MiB. Larger state travels as an encrypted snapshot (see [`crate::snapshot`]).
pub const MAX_UPDATE_LEN: usize = 1024 * 1024;

/// What an application message carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateKind {
    /// A change to the document's content (a CRDT update).
    Content,
    /// A comment.
    Comment,
    /// The key of an encrypted snapshot, for members who joined later (see [`crate::snapshot`]).
    SnapshotKey,
    /// Notice that the document moved to a new group (see [`crate::recreate`]).
    GroupMoved,
}

impl UpdateKind {
    /// Whether a user with `role` may send this kind of update.
    #[must_use]
    pub fn allowed_for(self, role: Role) -> bool {
        match self {
            Self::Content | Self::SnapshotKey => role.can_edit(),
            Self::Comment => role.can_comment(),
            Self::GroupMoved => role.can_manage_members(),
        }
    }

    fn to_byte(self) -> u8 {
        match self {
            Self::Content => 1,
            Self::Comment => 2,
            Self::SnapshotKey => 3,
            Self::GroupMoved => 4,
        }
    }

    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Content),
            2 => Some(Self::Comment),
            3 => Some(Self::SnapshotKey),
            4 => Some(Self::GroupMoved),
            _ => None,
        }
    }
}

/// Why an update's plaintext was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UpdateError {
    /// Empty, an unknown kind, or a body above [`MAX_UPDATE_LEN`].
    #[error("the update is malformed or too large")]
    Malformed,
}

/// One application message's plaintext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    /// What the update is.
    pub kind: UpdateKind,
    /// Its bytes, opaque to MLS.
    pub body: Vec<u8>,
}

impl Update {
    /// An update of `kind` carrying `body`.
    #[must_use]
    pub fn new(kind: UpdateKind, body: Vec<u8>) -> Self {
        Self { kind, body }
    }

    /// Encodes the update: one byte for the kind, then the body.
    ///
    /// # Errors
    ///
    /// Returns [`UpdateError::Malformed`] if the body is larger than [`MAX_UPDATE_LEN`].
    pub fn to_bytes(&self) -> Result<Vec<u8>, UpdateError> {
        if self.body.len() > MAX_UPDATE_LEN {
            return Err(UpdateError::Malformed);
        }
        let mut bytes = Vec::with_capacity(1 + self.body.len());
        bytes.push(self.kind.to_byte());
        bytes.extend_from_slice(&self.body);
        Ok(bytes)
    }

    /// Decodes an update.
    ///
    /// # Errors
    ///
    /// Returns [`UpdateError::Malformed`] for an empty message, an unknown kind or a body above [`MAX_UPDATE_LEN`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, UpdateError> {
        let (&kind, body) = bytes.split_first().ok_or(UpdateError::Malformed)?;
        let kind = UpdateKind::from_byte(kind).ok_or(UpdateError::Malformed)?;
        if body.len() > MAX_UPDATE_LEN {
            return Err(UpdateError::Malformed);
        }
        Ok(Self {
            kind,
            body: body.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test(unsupported = test)]
    fn updates_round_trip_and_bad_ones_are_refused() {
        let update = Update::new(UpdateKind::Comment, b"nice paragraph".to_vec());
        assert_eq!(
            Update::from_bytes(&update.to_bytes().unwrap()).unwrap(),
            update
        );
        assert_eq!(Update::from_bytes(&[]), Err(UpdateError::Malformed));
        assert_eq!(Update::from_bytes(&[0, 1, 2]), Err(UpdateError::Malformed));
        assert_eq!(Update::from_bytes(&[9]), Err(UpdateError::Malformed));
        let too_large = vec![1; MAX_UPDATE_LEN + 2];
        assert_eq!(Update::from_bytes(&too_large), Err(UpdateError::Malformed));
        assert_eq!(
            Update::new(UpdateKind::Content, vec![0; MAX_UPDATE_LEN + 1]).to_bytes(),
            Err(UpdateError::Malformed)
        );
    }

    #[wasm_bindgen_test(unsupported = test)]
    fn kinds_follow_roles() {
        assert!(UpdateKind::Content.allowed_for(Role::Editor));
        assert!(!UpdateKind::Content.allowed_for(Role::Commenter));
        assert!(UpdateKind::Comment.allowed_for(Role::Commenter));
        assert!(!UpdateKind::Comment.allowed_for(Role::Viewer));
        assert!(UpdateKind::SnapshotKey.allowed_for(Role::Editor));
        assert!(!UpdateKind::SnapshotKey.allowed_for(Role::Commenter));
        assert!(UpdateKind::GroupMoved.allowed_for(Role::Owner));
        assert!(!UpdateKind::GroupMoved.allowed_for(Role::Editor));
    }
}
