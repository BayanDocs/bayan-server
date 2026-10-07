//! The server-side validator (ADR-0015 Decision 5, ADR-0016 Decisions 3 and 5): the part of the MLS Delivery Service that decides what to relay.
//!
//! It holds no keys and sees no plaintext. For each group it tracks the public state with OpenMLS's `PublicGroup` (the ratchet tree and group context), built from the creator's signed `GroupInfo` and updated from commits, which arrive in public framing. Every message comes with the device the transport authenticated (simulated here: the caller passes it), and:
//!
//! - a commit is relayed only if that device is the member that signed it, OpenMLS verifies it against the public state, and the role policy allows it ([`crate::policy::check_commit`]);
//! - an application message is relayed only if it is in private framing, for the current or previous epoch, from a device that is a member whose role may send updates. The sender inside is encrypted, so this check relies on the transport's authentication; clients check the decrypted sender's role again;
//! - standalone proposals, external commits and anything else are refused.
//!
//! It also gives newcomers the public ratchet tree, so Welcomes need not carry it.

use std::collections::BTreeMap;

use openmls::prelude::{
    ContentType, MlsMessageBodyIn, ProcessedMessageContent, ProposalStore, ProtocolMessage,
    PublicGroup, Sender,
};
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::OpenMlsProvider as _;

use crate::identity::DeviceId;
use crate::policy::{self, GroupView, PolicyViolation};
use crate::roster::Roster;
use crate::suite::Suite;
use crate::wire::{self, WireError};

/// Why the validator refused a message.
#[derive(Debug, thiserror::Error)]
pub enum Rejection {
    /// The bytes are not the MLS message expected.
    #[error(transparent)]
    Wire(#[from] WireError),
    /// The change breaks the role policy.
    #[error(transparent)]
    Policy(#[from] PolicyViolation),
    /// The validator does not track this group.
    #[error("unknown group")]
    UnknownGroup,
    /// A group with this ID is already registered.
    #[error("the group already exists")]
    GroupExists,
    /// A commit or proposal in private framing, or an update in public framing.
    #[error("wrong framing: commits must be public and updates private")]
    WrongFraming,
    /// A message for an epoch the validator does not accept.
    #[error("the message is for epoch {got}, the group is at epoch {current}")]
    WrongEpoch {
        /// The message's epoch.
        got: u64,
        /// The group's epoch.
        current: u64,
    },
    /// The authenticated device is not the member that signed the commit.
    #[error("the commit was not sent by the device that signed it")]
    SenderMismatch,
    /// A new group must start with only its creator, whose user owns it.
    #[error("a new group must contain only its creator, as owner")]
    InvalidNewGroup,
    /// The group's ciphersuite is not accepted.
    #[error("the group's ciphersuite is not accepted")]
    UnsupportedCiphersuite,
    /// A standalone proposal, external commit or other message the design does not use.
    #[error("unsupported message: {0}")]
    Unsupported(&'static str),
    /// OpenMLS refused the message (the text names OpenMLS's error type, never message content).
    #[error("MLS error: {0}")]
    Mls(String),
}

fn mls(error: impl std::fmt::Debug) -> Rejection {
    Rejection::Mls(format!("{error:?}"))
}

/// What an accepted commit changed, for routing: the Welcome goes to `added`, the commit to everyone else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    /// The group's epoch after the commit.
    pub epoch: u64,
    /// Devices the commit added.
    pub added: Vec<DeviceId>,
    /// Devices the commit removed.
    pub removed: Vec<DeviceId>,
}

#[derive(Debug)]
struct TrackedGroup {
    public: PublicGroup,
    view: GroupView,
    /// The view before the last commit, to judge late application messages from the previous epoch.
    previous: Option<GroupView>,
}

/// The validator for every group on one server.
#[derive(Debug)]
pub struct Validator {
    /// Crypto for verifying signatures, and storage for the public group state. The validator never generates or holds a secret.
    provider: OpenMlsRustCrypto,
    groups: BTreeMap<Vec<u8>, TrackedGroup>,
    accepted: Vec<Suite>,
}

impl Default for Validator {
    fn default() -> Self {
        Self::new(Suite::all())
    }
}

impl Validator {
    /// A validator that accepts groups with the given ciphersuites.
    #[must_use]
    pub fn new(accepted: &[Suite]) -> Self {
        Self {
            provider: OpenMlsRustCrypto::default(),
            groups: BTreeMap::new(),
            accepted: accepted.to_vec(),
        }
    }

    /// Starts tracking a new group from its creator's signed `GroupInfo` and ratchet tree. The authenticated device must be the group's only member and its user the only owner in the roster. Returns the group's ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the `GroupInfo` or tree is invalid, the group exists, or it is not a fresh group of its creator.
    pub fn register_group(
        &mut self,
        authenticated: &DeviceId,
        group_info: &[u8],
        ratchet_tree: &[u8],
    ) -> Result<Vec<u8>, Rejection> {
        let MlsMessageBodyIn::GroupInfo(group_info) = wire::decode(group_info)? else {
            return Err(WireError::Malformed.into());
        };
        let tree = wire::decode_tree(ratchet_tree)?;
        let (public, _) = PublicGroup::from_external(
            self.provider.crypto(),
            self.provider.storage(),
            tree,
            group_info,
            ProposalStore::new(),
        )
        .map_err(mls)?;
        let group_id = public.group_id().as_slice().to_vec();
        if self.groups.contains_key(&group_id) {
            return Err(Rejection::GroupExists);
        }
        if !Suite::from_ciphersuite(public.ciphersuite())
            .is_some_and(|suite| self.accepted.contains(&suite))
        {
            return Err(Rejection::UnsupportedCiphersuite);
        }
        let view = wire::view_of(
            public.members(),
            Roster::from_group_context(public.group_context()),
        )?;
        let creator_only = view.members.len() == 1 && view.members.contains(authenticated);
        if !creator_only || view.check_consistent().is_err() {
            return Err(Rejection::InvalidNewGroup);
        }
        self.groups.insert(
            group_id.clone(),
            TrackedGroup {
                public,
                view,
                previous: None,
            },
        );
        Ok(group_id)
    }

    /// Validates a commit from `authenticated` and, if it is allowed, applies it to the tracked state.
    ///
    /// # Errors
    ///
    /// Returns why the commit must not be relayed.
    pub fn check_handshake(
        &mut self,
        authenticated: &DeviceId,
        message: &[u8],
    ) -> Result<Accepted, Rejection> {
        let message = match wire::decode(message)? {
            MlsMessageBodyIn::PublicMessage(message) => message,
            MlsMessageBodyIn::PrivateMessage(_) => return Err(Rejection::WrongFraming),
            _ => return Err(WireError::Malformed.into()),
        };
        let group = self
            .groups
            .get_mut(message.group_id().as_slice())
            .ok_or(Rejection::UnknownGroup)?;
        let current = group.public.group_context().epoch().as_u64();
        if message.epoch().as_u64() != current {
            return Err(Rejection::WrongEpoch {
                got: message.epoch().as_u64(),
                current,
            });
        }
        if message.content_type() != ContentType::Commit {
            return Err(Rejection::Unsupported("standalone proposals"));
        }
        let Sender::Member(index) = *message.sender() else {
            return Err(Rejection::Unsupported("commits from outside the group"));
        };
        let leaf = group
            .public
            .leaf(index)
            .ok_or(PolicyViolation::NotAMember)?;
        if &wire::device_of(leaf.credential())? != authenticated {
            return Err(Rejection::SenderMismatch);
        }
        let processed = group
            .public
            .process_message(self.provider.crypto(), ProtocolMessage::from(message))
            .map_err(mls)?;
        let committer = processed.credential().clone();
        let ProcessedMessageContent::StagedCommitMessage(staged) = processed.into_content() else {
            return Err(Rejection::Unsupported(
                "anything but commits in public framing",
            ));
        };
        let public = &group.public;
        let summary = wire::summarize_commit(&committer, &staged, |index| {
            public.leaf(index).map(|leaf| leaf.credential().clone())
        })?;
        let after = policy::check_commit(&group.view, &summary)?;
        group
            .public
            .merge_commit(self.provider.storage(), *staged)
            .map_err(mls)?;
        // The state OpenMLS tracks and the state the policy predicted must agree.
        let tracked = wire::view_of(
            group.public.members(),
            Roster::from_group_context(group.public.group_context()),
        )?;
        if tracked != after {
            return Err(PolicyViolation::MembershipMismatch.into());
        }
        group.previous = Some(std::mem::replace(&mut group.view, after));
        Ok(Accepted {
            epoch: group.public.group_context().epoch().as_u64(),
            added: summary.added,
            removed: summary.removed,
        })
    }

    /// Checks an application message from `authenticated`: private framing, the current or previous epoch, and a sender whose role may send updates (in the previous epoch's state as well as the current one, so a writer demoted by the last commit cannot slip in late messages). The validator cannot see who encrypted the message; clients check that.
    ///
    /// # Errors
    ///
    /// Returns why the message must not be relayed.
    pub fn check_application(
        &self,
        authenticated: &DeviceId,
        message: &[u8],
    ) -> Result<(), Rejection> {
        let message = match wire::decode(message)? {
            MlsMessageBodyIn::PrivateMessage(message) => message,
            MlsMessageBodyIn::PublicMessage(_) => return Err(Rejection::WrongFraming),
            _ => return Err(WireError::Malformed.into()),
        };
        if message.content_type() != ContentType::Application {
            return Err(Rejection::WrongFraming);
        }
        let group = self
            .groups
            .get(message.group_id().as_slice())
            .ok_or(Rejection::UnknownGroup)?;
        let current = group.public.group_context().epoch().as_u64();
        let epoch = message.epoch().as_u64();
        policy::check_application_sender(&group.view, authenticated)?;
        if epoch == current {
            Ok(())
        } else if epoch.checked_add(1) == Some(current) {
            let previous = group.previous.as_ref().ok_or(Rejection::WrongEpoch {
                got: epoch,
                current,
            })?;
            policy::check_application_sender(previous, authenticated)?;
            Ok(())
        } else {
            Err(Rejection::WrongEpoch {
                got: epoch,
                current,
            })
        }
    }

    /// The group's public ratchet tree, for newcomers.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown group.
    pub fn ratchet_tree(&self, group_id: &[u8]) -> Result<Vec<u8>, Rejection> {
        let group = self.groups.get(group_id).ok_or(Rejection::UnknownGroup)?;
        Ok(wire::encode_tree(&group.public.export_ratchet_tree())?)
    }

    /// The validator's view of a group's members and roles.
    #[must_use]
    pub fn view(&self, group_id: &[u8]) -> Option<&GroupView> {
        self.groups.get(group_id).map(|group| &group.view)
    }

    /// The group's current epoch.
    #[must_use]
    pub fn epoch(&self, group_id: &[u8]) -> Option<u64> {
        self.groups
            .get(group_id)
            .map(|group| group.public.group_context().epoch().as_u64())
    }
}
