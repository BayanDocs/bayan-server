//! Roles and the role roster, a custom group-context extension (ADR-0016 Decision 5).
//!
//! The roster maps every user with a device in the group to one role. It lives in the MLS group context, so all members agree on it in each epoch: it is covered by the signature on every `GroupInfo`, bound into the key schedule through the group context, and can only change through a commit, which its committer signs. Changing it is an owner's privilege, enforced by the server's validator and independently by every client (see [`crate::policy`]).
//!
//! Wire format (version 1), parsed with limits because it arrives from the network:
//!
//! ```text
//! u8  version = 1
//! u16 entry count (big-endian), at most MAX_ROSTER_ENTRIES
//! entries, sorted by user name with no duplicates:
//!   u8  user name length, then the name (rules of crate::identity)
//!   u8  role: 1 owner, 2 editor, 3 commenter, 4 viewer
//! ```

use std::collections::BTreeMap;

use openmls::prelude::{
    CredentialType, Extension, ExtensionType, Extensions, GroupContext,
    RequiredCapabilitiesExtension, UnknownExtension,
};

use crate::identity::UserId;

/// The extension type of the roster, from the range RFC 9420 §17.3 reserves for private use (0xF000–0xFFFF). Production would register a type, or use the MLS extensions framework once it is published.
pub const ROSTER_EXTENSION_TYPE: u16 = 0xF0BD;

/// The most users a roster may hold. Far above the 1,000 members the spike measures, and small enough that parsing stays cheap.
pub const MAX_ROSTER_ENTRIES: usize = 10_000;

const FORMAT_VERSION: u8 = 1;

/// A user's role in one document (COL-05).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    /// Manages membership and roles, edits and comments.
    Owner,
    /// Edits and comments.
    Editor,
    /// Comments.
    Commenter,
    /// Reads only.
    Viewer,
}

impl Role {
    /// Adding and removing members and changing roles.
    #[must_use]
    pub fn can_manage_members(self) -> bool {
        self == Self::Owner
    }

    /// Changing the document's content.
    #[must_use]
    pub fn can_edit(self) -> bool {
        matches!(self, Self::Owner | Self::Editor)
    }

    /// Adding comments.
    #[must_use]
    pub fn can_comment(self) -> bool {
        matches!(self, Self::Owner | Self::Editor | Self::Commenter)
    }

    /// Sending application messages at all. This is all the server can check: application messages are encrypted, so it cannot tell a comment from an edit, and clients enforce the difference.
    #[must_use]
    pub fn can_send_application_messages(self) -> bool {
        self.can_comment()
    }

    fn to_byte(self) -> u8 {
        match self {
            Self::Owner => 1,
            Self::Editor => 2,
            Self::Commenter => 3,
            Self::Viewer => 4,
        }
    }

    fn from_byte(byte: u8) -> Result<Self, RosterError> {
        match byte {
            1 => Ok(Self::Owner),
            2 => Ok(Self::Editor),
            3 => Ok(Self::Commenter),
            4 => Ok(Self::Viewer),
            _ => Err(RosterError::Malformed),
        }
    }
}

/// Why a roster was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RosterError {
    /// The group context has no roster extension.
    #[error("the group context has no role roster")]
    Missing,
    /// The extension's bytes are not a valid roster.
    #[error("the role roster is malformed")]
    Malformed,
    /// The roster has more than [`MAX_ROSTER_ENTRIES`] entries.
    #[error("the role roster has more than {MAX_ROSTER_ENTRIES} entries")]
    TooLarge,
}

/// The role of every user in a group.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Roster {
    roles: BTreeMap<UserId, Role>,
}

impl Roster {
    /// An empty roster.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the roster with `user` given `role`, replacing any earlier role.
    #[must_use]
    pub fn with(mut self, user: UserId, role: Role) -> Self {
        self.roles.insert(user, role);
        self
    }

    /// Gives `user` the role `role`, replacing any earlier role.
    pub fn set(&mut self, user: UserId, role: Role) {
        self.roles.insert(user, role);
    }

    /// Removes `user` from the roster.
    pub fn remove(&mut self, user: &UserId) {
        self.roles.remove(user);
    }

    /// The role of `user`, if the roster lists them.
    #[must_use]
    pub fn role(&self, user: &UserId) -> Option<Role> {
        self.roles.get(user).copied()
    }

    /// The users and their roles, sorted by user name.
    pub fn iter(&self) -> impl Iterator<Item = (&UserId, Role)> {
        self.roles.iter().map(|(user, role)| (user, *role))
    }

    /// The number of users.
    #[must_use]
    pub fn len(&self) -> usize {
        self.roles.len()
    }

    /// Whether the roster lists nobody.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.roles.is_empty()
    }

    /// Whether at least one user is an owner.
    #[must_use]
    pub fn has_owner(&self) -> bool {
        self.roles.values().any(|role| role.can_manage_members())
    }

    /// Encodes the roster in its wire format.
    ///
    /// # Errors
    ///
    /// Returns [`RosterError::TooLarge`] above [`MAX_ROSTER_ENTRIES`] entries.
    pub fn to_bytes(&self) -> Result<Vec<u8>, RosterError> {
        let count = u16::try_from(self.roles.len()).map_err(|_| RosterError::TooLarge)?;
        if self.roles.len() > MAX_ROSTER_ENTRIES {
            return Err(RosterError::TooLarge);
        }
        let mut bytes = Vec::with_capacity(3 + self.roles.len() * 16);
        bytes.push(FORMAT_VERSION);
        bytes.extend_from_slice(&count.to_be_bytes());
        for (user, role) in &self.roles {
            let name = user.as_str().as_bytes();
            // UserId guarantees 1..=64 bytes.
            bytes.push(u8::try_from(name.len()).map_err(|_| RosterError::Malformed)?);
            bytes.extend_from_slice(name);
            bytes.push(role.to_byte());
        }
        Ok(bytes)
    }

    /// Decodes a roster from its wire format, refusing anything that is not exactly one valid, canonically sorted roster.
    ///
    /// # Errors
    ///
    /// Returns [`RosterError::Malformed`] or [`RosterError::TooLarge`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, RosterError> {
        let mut reader = Reader(bytes);
        if reader.byte()? != FORMAT_VERSION {
            return Err(RosterError::Malformed);
        }
        let count = usize::from(u16::from_be_bytes([reader.byte()?, reader.byte()?]));
        if count > MAX_ROSTER_ENTRIES {
            return Err(RosterError::TooLarge);
        }
        let mut roles = BTreeMap::new();
        let mut previous: Option<UserId> = None;
        for _ in 0..count {
            let length = usize::from(reader.byte()?);
            let name =
                std::str::from_utf8(reader.take(length)?).map_err(|_| RosterError::Malformed)?;
            let user = UserId::new(name).map_err(|_| RosterError::Malformed)?;
            // Strictly increasing order: one encoding per roster, and no duplicates.
            if previous.as_ref().is_some_and(|previous| previous >= &user) {
                return Err(RosterError::Malformed);
            }
            let role = Role::from_byte(reader.byte()?)?;
            previous = Some(user.clone());
            roles.insert(user, role);
        }
        if !reader.0.is_empty() {
            return Err(RosterError::Malformed);
        }
        Ok(Self { roles })
    }

    /// The group-context extensions that carry this roster: the roster itself, and a required-capabilities extension that makes every member's client declare support for it (a client that could not read the roster could not check roles).
    ///
    /// # Errors
    ///
    /// Returns an error if the roster is too large to encode.
    pub fn group_context_extensions(&self) -> Result<Extensions<GroupContext>, RosterError> {
        Extensions::try_from(vec![
            Extension::RequiredCapabilities(RequiredCapabilitiesExtension::new(
                &[ExtensionType::Unknown(ROSTER_EXTENSION_TYPE)],
                &[],
                &[CredentialType::Basic],
            )),
            Extension::Unknown(ROSTER_EXTENSION_TYPE, UnknownExtension(self.to_bytes()?)),
        ])
        .map_err(|_| RosterError::Malformed)
    }

    /// Reads the roster from a group context.
    ///
    /// # Errors
    ///
    /// Returns [`RosterError::Missing`] if there is no roster, or a decoding error.
    pub fn from_group_context(context: &GroupContext) -> Result<Self, RosterError> {
        Self::from_extensions(context.extensions())
    }

    /// Reads the roster from a group context's extensions.
    ///
    /// # Errors
    ///
    /// Returns [`RosterError::Missing`] if there is no roster, or a decoding error.
    pub fn from_extensions(extensions: &Extensions<GroupContext>) -> Result<Self, RosterError> {
        let extension = extensions
            .unknown(ROSTER_EXTENSION_TYPE)
            .ok_or(RosterError::Missing)?;
        Self::from_bytes(&extension.0)
    }
}

/// Reads bytes from the front of a slice, failing instead of panicking when they run out.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Result<u8, RosterError> {
        let (&first, rest) = self.0.split_first().ok_or(RosterError::Malformed)?;
        self.0 = rest;
        Ok(first)
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], RosterError> {
        if self.0.len() < length {
            return Err(RosterError::Malformed);
        }
        let (taken, rest) = self.0.split_at(length);
        self.0 = rest;
        Ok(taken)
    }
}

#[cfg(test)]
mod tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    fn user(name: &str) -> UserId {
        UserId::new(name).unwrap()
    }

    #[wasm_bindgen_test(unsupported = test)]
    fn rosters_round_trip() {
        let roster = Roster::new()
            .with(user("alice"), Role::Owner)
            .with(user("bob"), Role::Editor)
            .with(user("carol"), Role::Commenter)
            .with(user("dave"), Role::Viewer);
        let bytes = roster.to_bytes().unwrap();
        assert_eq!(Roster::from_bytes(&bytes).unwrap(), roster);
        assert!(roster.has_owner());
        assert!(!Roster::new().with(user("bob"), Role::Editor).has_owner());
    }

    #[wasm_bindgen_test(unsupported = test)]
    fn role_permissions() {
        assert!(
            Role::Owner.can_manage_members() && Role::Owner.can_edit() && Role::Owner.can_comment()
        );
        assert!(
            !Role::Editor.can_manage_members()
                && Role::Editor.can_edit()
                && Role::Editor.can_comment()
        );
        assert!(!Role::Commenter.can_edit() && Role::Commenter.can_comment());
        assert!(Role::Commenter.can_send_application_messages());
        assert!(!Role::Viewer.can_comment() && !Role::Viewer.can_send_application_messages());
    }

    #[wasm_bindgen_test(unsupported = test)]
    fn malformed_rosters_are_refused() {
        let valid = Roster::new()
            .with(user("alice"), Role::Owner)
            .with(user("bob"), Role::Viewer)
            .to_bytes()
            .unwrap();
        // Every truncation, and trailing bytes.
        for length in 0..valid.len() {
            assert_eq!(
                Roster::from_bytes(&valid[..length]),
                Err(RosterError::Malformed),
                "truncated to {length}"
            );
        }
        let mut trailing = valid.clone();
        trailing.push(0);
        assert_eq!(Roster::from_bytes(&trailing), Err(RosterError::Malformed));
        // Unknown version and role.
        let mut version = valid.clone();
        version[0] = 2;
        assert_eq!(Roster::from_bytes(&version), Err(RosterError::Malformed));
        let mut role = valid.clone();
        *role.last_mut().unwrap() = 9;
        assert_eq!(Roster::from_bytes(&role), Err(RosterError::Malformed));
        // Duplicate and unsorted users.
        let duplicate = [1, 0, 2, 1, b'a', 1, 1, b'a', 2];
        assert_eq!(Roster::from_bytes(&duplicate), Err(RosterError::Malformed));
        let unsorted = [1, 0, 2, 1, b'b', 1, 1, b'a', 2];
        assert_eq!(Roster::from_bytes(&unsorted), Err(RosterError::Malformed));
        // Invalid names.
        assert_eq!(
            Roster::from_bytes(&[1, 0, 1, 1, b'A', 1]),
            Err(RosterError::Malformed)
        );
        assert_eq!(
            Roster::from_bytes(&[1, 0, 1, 0, 1]),
            Err(RosterError::Malformed)
        );
        // A count above the limit is refused before reading entries.
        assert_eq!(
            Roster::from_bytes(&[1, 0xff, 0xff]),
            Err(RosterError::TooLarge)
        );
    }
}
