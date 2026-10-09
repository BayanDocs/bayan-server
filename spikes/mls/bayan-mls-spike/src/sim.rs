//! A whole deployment in one process: the server (validator, key package directory and fan-out) and any number of clients. Tests, benchmarks and the WebAssembly runs drive MLS through it, and tests reach past it to play a misbehaving client or server.
//!
//! The "network" is direct function calls, and transport authentication is simulated by passing the sending device's name along with each message, as the server would learn it from the authenticated connection.

use std::collections::BTreeMap;

use crate::client::{Change, Client, ClientError, Received};
use crate::directory::KeyPackageDirectory;
use crate::identity::{DeviceId, IdentityError};
use crate::policy::PolicyViolation;
use crate::roster::{Role, Roster};
use crate::suite::Suite;
use crate::update::Update;
use crate::validator::{Accepted, Rejection, Validator};

/// Why a simulated operation failed.
#[derive(Debug, thiserror::Error)]
pub enum SimError {
    /// A client refused.
    #[error("client {device}: {error}")]
    Client {
        /// The device whose client refused.
        device: DeviceId,
        /// Why.
        error: ClientError,
    },
    /// The server refused.
    #[error("server: {0}")]
    Server(#[from] Rejection),
    /// No such device in the deployment.
    #[error("unknown device {0}")]
    UnknownDevice(DeviceId),
    /// The device has no key package left on the server.
    #[error("no key package for {0}")]
    NoKeyPackage(DeviceId),
    /// A device name is invalid.
    #[error(transparent)]
    Identity(#[from] IdentityError),
}

fn client_error(device: &DeviceId) -> impl Fn(ClientError) -> SimError + '_ {
    move |error| SimError::Client {
        device: device.clone(),
        error,
    }
}

/// The server and its clients.
#[derive(Debug, Default)]
pub struct Deployment {
    /// The server's validator.
    pub validator: Validator,
    /// The server's key package directory.
    pub directory: KeyPackageDirectory,
    /// The server's store of opaque blobs (encrypted snapshots), by ID.
    pub blobs: BTreeMap<Vec<u8>, Vec<u8>>,
    clients: BTreeMap<DeviceId, Client>,
}

/// What each recipient made of a delivered message.
pub type Deliveries = Vec<(DeviceId, Result<Received, ClientError>)>;

impl Deployment {
    /// An empty deployment whose server accepts every suite of this build.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a device (for example `"alice/laptop"`) and publishes `key_packages` key packages for `suite`.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid name or if publishing fails.
    pub fn add_device(
        &mut self,
        name: &str,
        suite: Suite,
        key_packages: usize,
    ) -> Result<DeviceId, SimError> {
        let device = DeviceId::parse(name)?;
        let client = Client::new(device.clone()).map_err(client_error(&device))?;
        self.clients.insert(device.clone(), client);
        self.publish_key_packages(&device, suite, key_packages)?;
        Ok(device)
    }

    /// Has `device` publish `count` more key packages for `suite`.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown device or if the server refuses them.
    pub fn publish_key_packages(
        &mut self,
        device: &DeviceId,
        suite: Suite,
        count: usize,
    ) -> Result<(), SimError> {
        let client = self
            .clients
            .get(device)
            .ok_or_else(|| SimError::UnknownDevice(device.clone()))?;
        for _ in 0..count {
            let key_package = client.key_package(suite).map_err(client_error(device))?;
            self.directory.publish(device, &key_package)?;
        }
        Ok(())
    }

    /// The client of `device`.
    ///
    /// # Panics
    ///
    /// Panics for an unknown device; for tests and benchmarks only.
    #[must_use]
    pub fn client(&self, device: &DeviceId) -> &Client {
        self.clients
            .get(device)
            .unwrap_or_else(|| panic!("unknown device {device}"))
    }

    /// The client of `device`, mutably.
    ///
    /// # Panics
    ///
    /// Panics for an unknown device; for tests and benchmarks only.
    pub fn client_mut(&mut self, device: &DeviceId) -> &mut Client {
        self.clients
            .get_mut(device)
            .unwrap_or_else(|| panic!("unknown device {device}"))
    }

    /// `owner` creates a group with `suite` and registers it with the server. Returns its ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the client or the server refuses.
    pub fn create_group(&mut self, owner: &DeviceId, suite: Suite) -> Result<Vec<u8>, SimError> {
        let client = self
            .clients
            .get_mut(owner)
            .ok_or_else(|| SimError::UnknownDevice(owner.clone()))?;
        let group_id = client.create_group(suite).map_err(client_error(owner))?;
        let group_info = client.group_info(&group_id).map_err(client_error(owner))?;
        let tree = client
            .ratchet_tree(&group_id)
            .map_err(client_error(owner))?;
        self.validator.register_group(owner, &group_info, &tree)?;
        Ok(group_id)
    }

    /// `committer` commits `change`; the server validates it; on acceptance the committer applies it, every other member processes it, and the added devices join from the Welcome with the tree the server provides. Each other member's result is returned (an error there means that client refused the commit).
    ///
    /// # Errors
    ///
    /// Returns an error if the committer cannot make the commit or the server refuses it (the committer then discards it).
    pub fn commit(
        &mut self,
        committer: &DeviceId,
        group_id: &[u8],
        change: &Change,
    ) -> Result<(Accepted, Deliveries), SimError> {
        let recipients: Vec<DeviceId> = self
            .validator
            .view(group_id)
            .ok_or(Rejection::UnknownGroup)?
            .members
            .iter()
            .filter(|device| *device != committer)
            .cloned()
            .collect();
        let client = self
            .clients
            .get_mut(committer)
            .ok_or_else(|| SimError::UnknownDevice(committer.clone()))?;
        let output = client
            .commit(group_id, change)
            .map_err(client_error(committer))?;
        let accepted = match self.validator.check_handshake(committer, &output.commit) {
            Ok(accepted) => accepted,
            Err(rejection) => {
                client
                    .discard_commit(group_id)
                    .map_err(client_error(committer))?;
                return Err(rejection.into());
            }
        };
        client
            .confirm_commit(group_id)
            .map_err(client_error(committer))?;
        let mut results = Vec::with_capacity(recipients.len());
        for device in recipients {
            if let Some(client) = self.clients.get_mut(&device) {
                let result = client.receive(group_id, &output.commit);
                results.push((device, result));
            }
        }
        if let Some(welcome) = &output.welcome {
            let tree = self.validator.ratchet_tree(group_id)?;
            for device in &accepted.added {
                if let Some(client) = self.clients.get_mut(device) {
                    client.join(welcome, &tree).map_err(client_error(device))?;
                }
            }
        }
        Ok((accepted, results))
    }

    /// `committer` adds devices with roles: it fetches one key package per device from the server and sets the roles in the roster in the same commit.
    ///
    /// # Errors
    ///
    /// Returns an error if a key package is missing or the commit fails.
    pub fn add_members(
        &mut self,
        committer: &DeviceId,
        group_id: &[u8],
        members: &[(DeviceId, Role)],
    ) -> Result<Accepted, SimError> {
        let suite = self
            .client(committer)
            .suite(group_id)
            .map_err(client_error(committer))?;
        let mut roster = self
            .client(committer)
            .view(group_id)
            .map_err(client_error(committer))?
            .roster;
        let mut change = Change::default();
        for (device, role) in members {
            let key_package = self
                .directory
                .fetch(device, suite)
                .ok_or_else(|| SimError::NoKeyPackage(device.clone()))?;
            change.add.push(key_package);
            roster.set(device.user().clone(), *role);
        }
        change.roster = Some(roster);
        let (accepted, results) = self.commit(committer, group_id, &change)?;
        Self::all_accepted(results)?;
        Ok(accepted)
    }

    /// `committer` removes devices, and the roster entries of users left without a device.
    ///
    /// # Errors
    ///
    /// Returns an error if the commit fails or a member refuses it.
    pub fn remove_members(
        &mut self,
        committer: &DeviceId,
        group_id: &[u8],
        devices: &[DeviceId],
    ) -> Result<Accepted, SimError> {
        let view = self
            .client(committer)
            .view(group_id)
            .map_err(client_error(committer))?;
        let mut roster: Roster = view.roster.clone();
        for device in devices {
            let other_devices = view
                .members
                .iter()
                .any(|member| member.user() == device.user() && !devices.contains(member));
            if !other_devices {
                roster.remove(device.user());
            }
        }
        let change = Change {
            add: Vec::new(),
            remove: devices.to_vec(),
            roster: Some(roster),
        };
        let (accepted, results) = self.commit(committer, group_id, &change)?;
        Self::all_accepted(results)?;
        Ok(accepted)
    }

    /// `committer` changes roles.
    ///
    /// # Errors
    ///
    /// Returns an error if the commit fails or a member refuses it.
    pub fn set_roles(
        &mut self,
        committer: &DeviceId,
        group_id: &[u8],
        roster: Roster,
    ) -> Result<Accepted, SimError> {
        let change = Change {
            add: Vec::new(),
            remove: Vec::new(),
            roster: Some(roster),
        };
        let (accepted, results) = self.commit(committer, group_id, &change)?;
        Self::all_accepted(results)?;
        Ok(accepted)
    }

    /// `committer` updates its own keys (an empty commit with an update path).
    ///
    /// # Errors
    ///
    /// Returns an error if the commit fails or a member refuses it.
    pub fn self_update(
        &mut self,
        committer: &DeviceId,
        group_id: &[u8],
    ) -> Result<Accepted, SimError> {
        let (accepted, results) = self.commit(committer, group_id, &Change::default())?;
        Self::all_accepted(results)?;
        Ok(accepted)
    }

    fn all_accepted(
        results: Vec<(DeviceId, Result<Received, ClientError>)>,
    ) -> Result<(), SimError> {
        for (device, result) in results {
            result.map_err(|error| SimError::Client { device, error })?;
        }
        Ok(())
    }

    /// `sender` sends an update; the server checks it and delivers it to every other member. Returns each recipient's result.
    ///
    /// # Errors
    ///
    /// Returns an error if the sender cannot encrypt it or the server refuses it.
    pub fn send(
        &mut self,
        sender: &DeviceId,
        group_id: &[u8],
        update: &Update,
    ) -> Result<Deliveries, SimError> {
        let client = self
            .clients
            .get_mut(sender)
            .ok_or_else(|| SimError::UnknownDevice(sender.clone()))?;
        let message = client
            .send(group_id, update)
            .map_err(client_error(sender))?;
        self.deliver(sender, group_id, &message)
    }

    /// Delivers an application message that `sender`'s connection submitted, after the server's check. Tests use it directly to submit messages a client would refuse to make.
    ///
    /// # Errors
    ///
    /// Returns an error if the server refuses it.
    pub fn deliver(
        &mut self,
        sender: &DeviceId,
        group_id: &[u8],
        message: &[u8],
    ) -> Result<Deliveries, SimError> {
        self.validator.check_application(sender, message)?;
        Ok(self.deliver_unchecked(sender, group_id, message))
    }

    /// Delivers a message to every member but `sender` without asking the validator: a misbehaving server.
    pub fn deliver_unchecked(
        &mut self,
        sender: &DeviceId,
        group_id: &[u8],
        message: &[u8],
    ) -> Deliveries {
        let recipients: Vec<DeviceId> = self
            .clients
            .iter()
            .filter(|(device, client)| {
                *device != sender && client.group_ids().any(|id| id == group_id)
            })
            .map(|(device, _)| device.clone())
            .collect();
        recipients
            .into_iter()
            .map(|device| {
                let result = self.client_mut(&device).receive(group_id, message);
                (device, result)
            })
            .collect()
    }

    /// `owner` moves the group to a new group with `suite` (group re-creation, see [`crate::recreate`]): it creates and registers the new group, adds every other current device with the same roles from fresh key packages for `suite` (one commit and one Welcome), then announces the move in the old group. Every member checks the announcement against the new group before following it. Returns the new group's ID.
    ///
    /// # Errors
    ///
    /// Returns an error if a device has no key package for `suite`, a commit fails, or a member refuses the move.
    pub fn recreate_group(
        &mut self,
        owner: &DeviceId,
        old_group: &[u8],
        suite: Suite,
    ) -> Result<Vec<u8>, SimError> {
        let view = self
            .validator
            .view(old_group)
            .ok_or(Rejection::UnknownGroup)?
            .clone();
        let new_group = self.create_group(owner, suite)?;
        let mut change = Change {
            add: Vec::new(),
            remove: Vec::new(),
            roster: Some(view.roster.clone()),
        };
        for device in view.members.iter().filter(|device| *device != owner) {
            change.add.push(
                self.directory
                    .fetch(device, suite)
                    .ok_or_else(|| SimError::NoKeyPackage(device.clone()))?,
            );
        }
        let (_, results) = self.commit(owner, &new_group, &change)?;
        Self::all_accepted(results)?;
        let moved = crate::recreate::GroupMoved {
            new_group_id: new_group.clone(),
            suite,
        };
        let body = moved
            .to_bytes()
            .map_err(|_| Rejection::Unsupported("group ID too long"))?;
        let update = Update::new(crate::update::UpdateKind::GroupMoved, body);
        for (device, result) in self.send(owner, old_group, &update)? {
            let accepted = matches!(&result, Ok(Received::Update { update, .. }) if update.kind == crate::update::UpdateKind::GroupMoved);
            let verified = accepted
                && crate::recreate::verify_move(self.client(&device), old_group, &moved).is_ok();
            if !verified {
                return Err(SimError::Client {
                    device,
                    error: result
                        .err()
                        .unwrap_or(ClientError::Unsupported("move announcement")),
                });
            }
        }
        Ok(new_group)
    }

    /// Checks that the validator and every member's client agree on the group's members, roles and epoch.
    ///
    /// # Errors
    ///
    /// Returns [`PolicyViolation::MembershipMismatch`] naming nothing more, if any of them disagree.
    pub fn check_agreement(&self, group_id: &[u8]) -> Result<(), SimError> {
        let server = self
            .validator
            .view(group_id)
            .ok_or(Rejection::UnknownGroup)?;
        let epoch = self
            .validator
            .epoch(group_id)
            .ok_or(Rejection::UnknownGroup)?;
        for device in &server.members {
            let client = self
                .clients
                .get(device)
                .ok_or_else(|| SimError::UnknownDevice(device.clone()))?;
            let view = client.view(group_id).map_err(client_error(device))?;
            let client_epoch = client.epoch(group_id).map_err(client_error(device))?;
            if &view != server || client_epoch != epoch {
                return Err(Rejection::Policy(PolicyViolation::MembershipMismatch).into());
            }
        }
        Ok(())
    }
}
