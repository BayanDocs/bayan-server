//! Group re-creation: moving a document to a new group with the same members and roles, for example with a new ciphersuite (ADR-0016 Decision 2: crypto agility by re-creation).
//!
//! An owner creates the new group, adds every current device from fresh key packages for the new suite with the same roster, and announces the move in the old group with an update of kind [`crate::update::UpdateKind::GroupMoved`]. MLS authenticates the announcement, and only an owner may send one. Each member checks that the new group has exactly the old group's members and roles before switching ([`verify_move`]), so the owner cannot use a migration to add or drop anyone unnoticed.
//!
//! MLS also defines a standard way to do this, a `ReInit` proposal followed by a new group bound to the old one through a resumption pre-shared key (RFC 9420 §11.2). OpenMLS 0.9 does not implement it; REPORT.md discusses the difference.

use crate::client::{Client, ClientError};
use crate::policy::PolicyViolation;
use crate::suite::Suite;

const FORMAT_VERSION: u8 = 1;

/// Why a move announcement was refused.
#[derive(Debug, thiserror::Error)]
pub enum MoveError {
    /// The announcement is malformed or names a suite the spike does not use.
    #[error("the move announcement is malformed")]
    Malformed,
    /// The new group's suite is not the announced one.
    #[error("the new group's ciphersuite is not the announced one")]
    WrongSuite,
    /// The new group's members or roles differ from the old group's.
    #[error("the new group's members or roles differ from the old group's")]
    MembershipChanged,
    /// The client is not in the new group or the old one.
    #[error(transparent)]
    Client(#[from] ClientError),
}

/// The body of a [`crate::update::UpdateKind::GroupMoved`] update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupMoved {
    /// The new group's ID.
    pub new_group_id: Vec<u8>,
    /// The new group's suite.
    pub suite: Suite,
}

impl GroupMoved {
    /// Encodes the announcement: version, group ID, ciphersuite code point.
    ///
    /// # Errors
    ///
    /// Returns [`MoveError::Malformed`] for a group ID longer than 255 bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, MoveError> {
        let mut bytes = vec![
            FORMAT_VERSION,
            u8::try_from(self.new_group_id.len()).map_err(|_| MoveError::Malformed)?,
        ];
        bytes.extend_from_slice(&self.new_group_id);
        bytes.extend_from_slice(&u16::from(self.suite.ciphersuite()).to_be_bytes());
        Ok(bytes)
    }

    /// Decodes an announcement.
    ///
    /// # Errors
    ///
    /// Returns [`MoveError::Malformed`] unless it is exactly one well-formed announcement of a suite this build knows.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, MoveError> {
        let [version, length, rest @ ..] = bytes else {
            return Err(MoveError::Malformed);
        };
        let length = usize::from(*length);
        if *version != FORMAT_VERSION || rest.len() != length + 2 {
            return Err(MoveError::Malformed);
        }
        let (group_id, code) = rest.split_at(length);
        let code = u16::from_be_bytes([code[0], code[1]]);
        let suite = Suite::all()
            .iter()
            .copied()
            .find(|suite| u16::from(suite.ciphersuite()) == code)
            .ok_or(MoveError::Malformed)?;
        Ok(Self {
            new_group_id: group_id.to_vec(),
            suite,
        })
    }
}

/// A member's check before following a move from `old_group`: the client must be in the new group, which must use the announced suite and have exactly the old group's members and roles.
///
/// # Errors
///
/// Returns why the move must not be followed.
pub fn verify_move(client: &Client, old_group: &[u8], moved: &GroupMoved) -> Result<(), MoveError> {
    let old = client.view(old_group)?;
    let new = client.view(&moved.new_group_id)?;
    if client.suite(&moved.new_group_id)? != moved.suite {
        return Err(MoveError::WrongSuite);
    }
    if old != new {
        return Err(MoveError::MembershipChanged);
    }
    new.check_consistent()
        .map_err(|_: PolicyViolation| MoveError::MembershipChanged)?;
    Ok(())
}
