//! The role policy, shared by the server's validator and by every client (ADR-0016 Decision 5).
//!
//! Both sides run exactly these checks. The server enforces them so that unauthorized changes are never relayed; clients enforce them again so that a misbehaving server cannot grant anyone more than their role allows. If the two ever disagree, the client refuses the change and stops following the group, which is detectable and fails closed.
//!
//! The rules:
//!
//! - Only commits change a group; standalone proposals and proposal types the design does not use are refused.
//! - Owners may add and remove anyone and change the roster. Any member may add or remove other devices of their own user (COL-06), and any member may update their own keys (post-compromise security).
//! - After every commit, each user with a device in the group has exactly one role in the roster, the roster lists nobody else, and at least one owner remains.
//! - A member's new leaf (in a commit's update path) keeps its credential identity: a device cannot turn into another device.
//! - Application messages: the server accepts them from devices whose role may send any (owners, editors, commenters); clients then check the decrypted kind against the sender's role ([`check_update`]).

use std::collections::BTreeSet;

use crate::identity::DeviceId;
use crate::roster::{Role, Roster};
use crate::update::UpdateKind;

/// The state the policy judges against: who is in the group, and their roles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupView {
    /// The devices in the group.
    pub members: BTreeSet<DeviceId>,
    /// The role of every user with a device in the group.
    pub roster: Roster,
}

impl GroupView {
    /// The role of `device`'s user, if the device is a member and the roster lists the user.
    #[must_use]
    pub fn role_of(&self, device: &DeviceId) -> Option<Role> {
        if self.members.contains(device) {
            self.roster.role(device.user())
        } else {
            None
        }
    }

    /// Checks that the roster lists exactly the users with devices in the group, and at least one owner.
    ///
    /// # Errors
    ///
    /// Returns [`PolicyViolation::RosterMismatch`] or [`PolicyViolation::NoOwner`].
    pub fn check_consistent(&self) -> Result<(), PolicyViolation> {
        let users: BTreeSet<_> = self.members.iter().map(DeviceId::user).collect();
        let listed: BTreeSet<_> = self.roster.iter().map(|(user, _)| user).collect();
        if users != listed {
            return Err(PolicyViolation::RosterMismatch);
        }
        if !self.roster.has_owner() {
            return Err(PolicyViolation::NoOwner);
        }
        Ok(())
    }
}

/// What a commit does, as read from MLS by the validator or a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitSummary {
    /// The device that made and signed the commit.
    pub committer: DeviceId,
    /// Devices the commit adds.
    pub added: Vec<DeviceId>,
    /// Devices the commit removes.
    pub removed: Vec<DeviceId>,
    /// The roster in the new group context, if the commit carries a group-context-extensions proposal. Only owners may send one, even one that leaves the roster as it is; the new context must hold exactly the roster and its required capabilities ([`Roster::from_group_context`]).
    pub new_roster: Option<Roster>,
    /// The credential identity in the committer's new leaf, if the commit has an update path.
    pub path_identity: Option<DeviceId>,
    /// Proposals of kinds this design never uses (by reference, PSK, re-initialization, external init, custom, …), named for error messages.
    pub unsupported: Vec<String>,
}

/// Why the policy refused a change.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyViolation {
    /// The sender is not a member, or its user has no role.
    #[error("the sender is not a member with a role")]
    NotAMember,
    /// The sender's role does not allow this change.
    #[error("the sender's role ({role:?}) does not allow {action}")]
    NotAllowed {
        /// The sender's role.
        role: Role,
        /// What was attempted.
        action: &'static str,
    },
    /// The commit contains a proposal the design does not use.
    #[error("unsupported proposal: {0}")]
    Unsupported(String),
    /// A device was added twice, or added while already a member, or removed while not a member.
    #[error("the membership change does not match the current members")]
    MembershipMismatch,
    /// After the commit, the roster does not list exactly the users with devices in the group.
    #[error("the roster does not match the group's members")]
    RosterMismatch,
    /// After the commit, no owner would remain.
    #[error("no owner would remain")]
    NoOwner,
    /// The committer's new leaf names a different device.
    #[error("a member's credential identity cannot change")]
    IdentityChanged,
}

/// Checks a commit against the group's state before it, and returns the state after it.
///
/// # Errors
///
/// Returns the first rule the commit breaks.
pub fn check_commit(
    before: &GroupView,
    commit: &CommitSummary,
) -> Result<GroupView, PolicyViolation> {
    let role = before
        .role_of(&commit.committer)
        .ok_or(PolicyViolation::NotAMember)?;
    if let Some(proposal) = commit.unsupported.first() {
        return Err(PolicyViolation::Unsupported(proposal.clone()));
    }
    if commit
        .path_identity
        .as_ref()
        .is_some_and(|identity| identity != &commit.committer)
    {
        return Err(PolicyViolation::IdentityChanged);
    }
    let own_device = |device: &DeviceId| device.user() == commit.committer.user();
    let mut members = before.members.clone();
    for device in &commit.removed {
        if !role.can_manage_members() && !own_device(device) {
            return Err(PolicyViolation::NotAllowed {
                role,
                action: "removing another user's device",
            });
        }
        if !members.remove(device) {
            return Err(PolicyViolation::MembershipMismatch);
        }
    }
    for device in &commit.added {
        if !role.can_manage_members() && !own_device(device) {
            return Err(PolicyViolation::NotAllowed {
                role,
                action: "adding another user's device",
            });
        }
        if !members.insert(device.clone()) {
            return Err(PolicyViolation::MembershipMismatch);
        }
    }
    // Only owners may change the group context at all, even with a commit that keeps the roster: no other role has a reason to, and the context decides who may act on the group (external senders, for example).
    let roster = match &commit.new_roster {
        Some(roster) => {
            if !role.can_manage_members() {
                return Err(PolicyViolation::NotAllowed {
                    role,
                    action: if roster == &before.roster {
                        "changing the group context"
                    } else {
                        "changing roles"
                    },
                });
            }
            roster.clone()
        }
        None => before.roster.clone(),
    };
    let after = GroupView { members, roster };
    after.check_consistent()?;
    Ok(after)
}

/// The server's check of an application message: the authenticated device must be a member whose role may send application messages. Returns that role.
///
/// # Errors
///
/// Returns [`PolicyViolation::NotAMember`] or [`PolicyViolation::NotAllowed`].
pub fn check_application_sender(
    view: &GroupView,
    device: &DeviceId,
) -> Result<Role, PolicyViolation> {
    let role = view.role_of(device).ok_or(PolicyViolation::NotAMember)?;
    if !role.can_send_application_messages() {
        return Err(PolicyViolation::NotAllowed {
            role,
            action: "sending updates",
        });
    }
    Ok(role)
}

/// A client's check of a decrypted update: the sender's role must allow its kind.
///
/// # Errors
///
/// Returns [`PolicyViolation::NotAMember`] or [`PolicyViolation::NotAllowed`].
pub fn check_update(
    view: &GroupView,
    sender: &DeviceId,
    kind: UpdateKind,
) -> Result<Role, PolicyViolation> {
    let role = view.role_of(sender).ok_or(PolicyViolation::NotAMember)?;
    if !kind.allowed_for(role) {
        return Err(PolicyViolation::NotAllowed {
            role,
            action: "sending this kind of update",
        });
    }
    Ok(role)
}

#[cfg(test)]
mod tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;
    use crate::identity::UserId;

    fn device(text: &str) -> DeviceId {
        DeviceId::parse(text).unwrap()
    }

    fn user(name: &str) -> UserId {
        UserId::new(name).unwrap()
    }

    /// alice (owner, one device), bob (editor, laptop), carol (commenter), dave (viewer).
    fn group() -> GroupView {
        GroupView {
            members: ["alice/laptop", "bob/laptop", "carol/phone", "dave/tablet"]
                .into_iter()
                .map(device)
                .collect(),
            roster: Roster::new()
                .with(user("alice"), Role::Owner)
                .with(user("bob"), Role::Editor)
                .with(user("carol"), Role::Commenter)
                .with(user("dave"), Role::Viewer),
        }
    }

    fn commit(committer: &str) -> CommitSummary {
        CommitSummary {
            committer: device(committer),
            added: vec![],
            removed: vec![],
            new_roster: None,
            path_identity: Some(device(committer)),
            unsupported: vec![],
        }
    }

    #[wasm_bindgen_test(unsupported = test)]
    fn owners_manage_members_and_roles() {
        let before = group();
        let mut add = commit("alice/laptop");
        add.added = vec![device("erin/laptop")];
        add.new_roster = Some(before.roster.clone().with(user("erin"), Role::Editor));
        let after = check_commit(&before, &add).unwrap();
        assert_eq!(after.role_of(&device("erin/laptop")), Some(Role::Editor));

        let mut remove = commit("alice/laptop");
        remove.removed = vec![device("bob/laptop")];
        let mut roster = before.roster.clone();
        roster.remove(&user("bob"));
        remove.new_roster = Some(roster);
        assert!(
            !check_commit(&before, &remove)
                .unwrap()
                .members
                .contains(&device("bob/laptop"))
        );

        let mut promote = commit("alice/laptop");
        promote.new_roster = Some(before.roster.clone().with(user("dave"), Role::Editor));
        assert_eq!(
            check_commit(&before, &promote)
                .unwrap()
                .role_of(&device("dave/tablet")),
            Some(Role::Editor)
        );
    }

    #[wasm_bindgen_test(unsupported = test)]
    fn other_roles_cannot_manage_members_or_roles() {
        let before = group();
        for committer in ["bob/laptop", "carol/phone", "dave/tablet"] {
            let mut add = commit(committer);
            add.added = vec![device("erin/laptop")];
            add.new_roster = Some(before.roster.clone().with(user("erin"), Role::Viewer));
            assert!(
                matches!(
                    check_commit(&before, &add),
                    Err(PolicyViolation::NotAllowed { .. })
                ),
                "{committer} added"
            );

            let mut remove = commit(committer);
            remove.removed = vec![device("alice/laptop")];
            assert!(
                matches!(
                    check_commit(&before, &remove),
                    Err(PolicyViolation::NotAllowed { .. })
                ),
                "{committer} removed"
            );

            let mut promote = commit(committer);
            let committer_user = device(committer).user().clone();
            promote.new_roster = Some(before.roster.clone().with(committer_user, Role::Owner));
            assert!(
                matches!(
                    check_commit(&before, &promote),
                    Err(PolicyViolation::NotAllowed { .. })
                ),
                "{committer} promoted"
            );

            // Not even a context change that keeps the roster as it is.
            let mut same_roster = commit(committer);
            same_roster.new_roster = Some(before.roster.clone());
            assert!(
                matches!(
                    check_commit(&before, &same_roster),
                    Err(PolicyViolation::NotAllowed {
                        action: "changing the group context",
                        ..
                    })
                ),
                "{committer} changed the group context"
            );
        }
    }

    #[wasm_bindgen_test(unsupported = test)]
    fn anyone_may_update_their_keys_and_manage_their_own_devices() {
        let before = group();
        for committer in ["alice/laptop", "bob/laptop", "carol/phone", "dave/tablet"] {
            assert_eq!(
                check_commit(&before, &commit(committer)).unwrap(),
                before,
                "{committer} self-update"
            );
        }
        let mut second_device = commit("dave/tablet");
        second_device.added = vec![device("dave/phone")];
        let after = check_commit(&before, &second_device).unwrap();
        assert_eq!(after.role_of(&device("dave/phone")), Some(Role::Viewer));

        let mut drop_device = commit("dave/phone");
        drop_device.removed = vec![device("dave/tablet")];
        assert!(
            !check_commit(&after, &drop_device)
                .unwrap()
                .members
                .contains(&device("dave/tablet"))
        );
    }

    #[wasm_bindgen_test(unsupported = test)]
    fn last_owner_cannot_be_removed_or_demoted() {
        let before = group();
        let mut demote = commit("alice/laptop");
        demote.new_roster = Some(before.roster.clone().with(user("alice"), Role::Editor));
        assert_eq!(
            check_commit(&before, &demote),
            Err(PolicyViolation::NoOwner)
        );

        // A second owner makes demoting the first one fine.
        let mut promote = commit("alice/laptop");
        promote.new_roster = Some(before.roster.clone().with(user("bob"), Role::Owner));
        let two_owners = check_commit(&before, &promote).unwrap();
        let mut demote = commit("bob/laptop");
        demote.new_roster = Some(two_owners.roster.clone().with(user("alice"), Role::Viewer));
        assert!(check_commit(&two_owners, &demote).is_ok());
    }

    #[wasm_bindgen_test(unsupported = test)]
    fn roster_must_match_membership() {
        let before = group();
        // A new user without a role.
        let mut add = commit("alice/laptop");
        add.added = vec![device("erin/laptop")];
        assert_eq!(
            check_commit(&before, &add),
            Err(PolicyViolation::RosterMismatch)
        );
        // A removed user who keeps a role.
        let mut remove = commit("alice/laptop");
        remove.removed = vec![device("bob/laptop")];
        assert_eq!(
            check_commit(&before, &remove),
            Err(PolicyViolation::RosterMismatch)
        );
        // A role for someone who is not in the group.
        let mut ghost = commit("alice/laptop");
        ghost.new_roster = Some(before.roster.clone().with(user("mallory"), Role::Owner));
        assert_eq!(
            check_commit(&before, &ghost),
            Err(PolicyViolation::RosterMismatch)
        );
    }

    #[wasm_bindgen_test(unsupported = test)]
    fn malformed_commits_are_refused() {
        let before = group();
        assert_eq!(
            check_commit(&before, &commit("mallory/laptop")),
            Err(PolicyViolation::NotAMember)
        );
        let mut unsupported = commit("alice/laptop");
        unsupported.unsupported = vec!["re-initialization".to_owned()];
        assert!(matches!(
            check_commit(&before, &unsupported),
            Err(PolicyViolation::Unsupported(_))
        ));
        let mut twice = commit("alice/laptop");
        twice.added = vec![device("bob/laptop")];
        assert_eq!(
            check_commit(&before, &twice),
            Err(PolicyViolation::MembershipMismatch)
        );
        let mut absent = commit("alice/laptop");
        absent.removed = vec![device("erin/laptop")];
        assert_eq!(
            check_commit(&before, &absent),
            Err(PolicyViolation::MembershipMismatch)
        );
        let mut impostor = commit("bob/laptop");
        impostor.path_identity = Some(device("alice/laptop"));
        assert_eq!(
            check_commit(&before, &impostor),
            Err(PolicyViolation::IdentityChanged)
        );
    }

    #[wasm_bindgen_test(unsupported = test)]
    fn application_messages_follow_roles() {
        let view = group();
        assert_eq!(
            check_application_sender(&view, &device("carol/phone")),
            Ok(Role::Commenter)
        );
        assert!(check_application_sender(&view, &device("dave/tablet")).is_err());
        assert!(check_application_sender(&view, &device("mallory/laptop")).is_err());

        assert!(check_update(&view, &device("bob/laptop"), UpdateKind::Content).is_ok());
        assert!(check_update(&view, &device("carol/phone"), UpdateKind::Content).is_err());
        assert!(check_update(&view, &device("carol/phone"), UpdateKind::Comment).is_ok());
        assert!(check_update(&view, &device("dave/tablet"), UpdateKind::Comment).is_err());
        assert!(check_update(&view, &device("bob/laptop"), UpdateKind::GroupMoved).is_err());
        assert!(check_update(&view, &device("alice/laptop"), UpdateKind::GroupMoved).is_ok());
    }
}
