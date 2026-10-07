//! Users and devices, and how a device is named inside its MLS credential.
//!
//! MLS members are devices (ADR-0016 §6): each device has its own signature key and leaf in the group. Roles belong to users, so every device of a user has that user's role. The spike puts `user/device` into an MLS basic credential. Production replaces this with credentials that the user's long-term identity key cross-signs (designed in SRV-003, built in Phase 3); the server's view of which devices belong to which user is simulated here by the authenticated connection.

use std::fmt;

/// The longest user or device name, in bytes.
pub const MAX_NAME_LEN: usize = 64;

/// Why a name or credential identity was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    /// A name was empty, too long, or used a character outside `a-z`, `0-9`, `-`, `_` and `.`.
    #[error(
        "a user or device name must be 1 to {MAX_NAME_LEN} characters from a-z, 0-9, '-', '_' and '.'"
    )]
    InvalidName,
    /// A credential identity was not `user/device`.
    #[error("a credential identity must have the form user/device")]
    InvalidIdentity,
}

/// Checks a user or device name: 1 to [`MAX_NAME_LEN`] bytes of lowercase ASCII letters, digits, `-`, `_` and `.`.
fn check_name(name: &str) -> Result<(), IdentityError> {
    let allowed = |byte: u8| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
    };
    if name.is_empty() || name.len() > MAX_NAME_LEN || !name.bytes().all(allowed) {
        return Err(IdentityError::InvalidName);
    }
    Ok(())
}

/// A user account, the unit that holds a role.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UserId(String);

impl UserId {
    /// A user name.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::InvalidName`] unless the name is 1 to [`MAX_NAME_LEN`] characters from `a-z`, `0-9`, `-`, `_` and `.`.
    pub fn new(name: &str) -> Result<Self, IdentityError> {
        check_name(name)?;
        Ok(Self(name.to_owned()))
    }

    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One device of a user: an MLS member.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId {
    user: UserId,
    device: String,
}

impl DeviceId {
    /// Parses `user/device`, the form used in tests and in credentials.
    ///
    /// # Errors
    ///
    /// Returns an error unless the text is two valid names separated by one `/`.
    pub fn parse(text: &str) -> Result<Self, IdentityError> {
        let (user, device) = text.split_once('/').ok_or(IdentityError::InvalidIdentity)?;
        check_name(device)?;
        Ok(Self {
            user: UserId::new(user)?,
            device: device.to_owned(),
        })
    }

    /// The user this device belongs to.
    #[must_use]
    pub fn user(&self) -> &UserId {
        &self.user
    }

    /// The device's name, unique per user.
    #[must_use]
    pub fn device(&self) -> &str {
        &self.device
    }

    /// The bytes stored as the identity of the device's MLS basic credential.
    #[must_use]
    pub fn to_credential_identity(&self) -> Vec<u8> {
        self.to_string().into_bytes()
    }

    /// Reads a device from the identity of an MLS basic credential. The bytes come from the network, so anything but a valid `user/device` is refused.
    ///
    /// # Errors
    ///
    /// Returns an error if the bytes are not UTF-8 text of the form `user/device` with valid names.
    pub fn from_credential_identity(identity: &[u8]) -> Result<Self, IdentityError> {
        if identity.len() > 2 * MAX_NAME_LEN + 1 {
            return Err(IdentityError::InvalidIdentity);
        }
        let text = std::str::from_utf8(identity).map_err(|_| IdentityError::InvalidIdentity)?;
        Self::parse(text)
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.user, self.device)
    }
}

#[cfg(test)]
mod tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test(unsupported = test)]
    fn devices_round_trip_through_credential_identities() {
        let device = DeviceId::parse("alice/laptop").unwrap();
        assert_eq!(device.user().as_str(), "alice");
        assert_eq!(device.device(), "laptop");
        assert_eq!(
            DeviceId::from_credential_identity(&device.to_credential_identity()).unwrap(),
            device
        );
    }

    #[wasm_bindgen_test(unsupported = test)]
    fn malformed_identities_are_refused() {
        for bad in [
            "",
            "alice",
            "alice/",
            "/laptop",
            "alice/laptop/2",
            "Alice/laptop",
            "alice/lap top",
            "alice/läptop",
        ] {
            assert!(DeviceId::parse(bad).is_err(), "{bad:?} was accepted");
        }
        assert!(DeviceId::from_credential_identity(&[0xff, b'/', b'a']).is_err());
        let long = format!("{}/{}", "a".repeat(MAX_NAME_LEN + 1), "b");
        assert!(DeviceId::from_credential_identity(long.as_bytes()).is_err());
        assert!(DeviceId::from_credential_identity(&vec![b'a'; 10_000]).is_err());
    }
}
