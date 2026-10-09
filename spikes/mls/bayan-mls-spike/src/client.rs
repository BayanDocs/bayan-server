//! An MLS client: one device, holding its signature key and the MLS state of every group it is in.
//!
//! Group settings follow ADR-0016: handshake messages (commits) use public framing so the server can validate them, application messages (updates) private framing; the role roster sits in the group context; Welcomes carry no ratchet tree, because newcomers fetch the public tree from the server, which tracks it anyway (see [`crate::validator`]); and every incoming commit, Welcome and update is checked against [`crate::policy`] before it is accepted.

use std::cell::Cell;
use std::collections::BTreeMap;

use openmls::prelude::{
    Capabilities, CredentialType, CredentialWithKey, ExtensionType, Extensions, GroupContext,
    GroupId, KeyPackage, MlsGroup, MlsGroupCreateConfig, MlsGroupJoinConfig, MlsMessageBodyIn,
    MlsMessageOut, PURE_PLAINTEXT_WIRE_FORMAT_POLICY, ProcessedMessageContent, StagedWelcome,
};
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::OpenMlsProvider as _;
use openmls_traits::random::OpenMlsRand as _;

use crate::identity::DeviceId;
use crate::policy::{self, CommitSummary, GroupView, PolicyViolation};
use crate::roster::{ROSTER_EXTENSION_TYPE, Roster, RosterError};
use crate::suite::Suite;
use crate::update::{Update, UpdateError};
use crate::wire::{self, WireError};

/// How many past epochs a client keeps decryption keys for, by default, so that an update sent just before a commit can still be read after it. One covers that race. OpenMLS keeps every member's leaf for each past epoch and stores it again after every message, so each one costs time per message and state in large groups (REPORT.md); [`Client::with_past_epochs`] chooses another number.
pub const DEFAULT_PAST_EPOCHS: usize = 1;

/// Why a client operation failed.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The bytes are not the MLS message expected.
    #[error(transparent)]
    Wire(#[from] WireError),
    /// The change or message breaks the role policy.
    #[error(transparent)]
    Policy(#[from] PolicyViolation),
    /// The roster is malformed.
    #[error(transparent)]
    Roster(#[from] RosterError),
    /// The update's plaintext is malformed.
    #[error(transparent)]
    Update(#[from] UpdateError),
    /// The client is not in this group.
    #[error("unknown group")]
    UnknownGroup,
    /// The message belongs to another group than the one named.
    #[error("the message belongs to another group")]
    WrongGroup,
    /// A key package is for another ciphersuite than the group's.
    #[error("the key package's ciphersuite does not match the group's")]
    WrongCiphersuite,
    /// The message is a standalone proposal or another kind the design does not use.
    #[error("unsupported message: {0}")]
    Unsupported(&'static str),
    /// OpenMLS refused the operation (the text names OpenMLS's error type, never message content).
    #[error("MLS error: {0}")]
    Mls(String),
}

/// Converts an OpenMLS error into [`ClientError::Mls`]. OpenMLS errors describe the failure, never message content.
fn mls(error: impl std::fmt::Debug) -> ClientError {
    ClientError::Mls(format!("{error:?}"))
}

/// A requested change to a group's membership or roles. All fields empty means a self-update: new keys for the committer (post-compromise security).
#[derive(Debug, Clone, Default)]
pub struct Change {
    /// Key packages (as fetched from the server) of the devices to add.
    pub add: Vec<Vec<u8>>,
    /// Devices to remove.
    pub remove: Vec<DeviceId>,
    /// The roster after the change, if roles change. Adding a new user or removing a user's last device always changes it.
    pub roster: Option<Roster>,
}

impl Change {
    /// Whether the change only updates the committer's keys.
    #[must_use]
    pub fn is_self_update(&self) -> bool {
        self.add.is_empty() && self.remove.is_empty() && self.roster.is_none()
    }
}

/// A commit ready for the server: the commit itself (public framing) and, if it adds devices, the Welcome for them.
#[derive(Debug, Clone)]
pub struct CommitOutput {
    /// The commit, an MLS message in public framing.
    pub commit: Vec<u8>,
    /// The Welcome for added devices, without the ratchet tree.
    pub welcome: Option<Vec<u8>>,
}

/// What a client accepted from an incoming message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Received {
    /// An update, from a sender whose role allows its kind.
    Update {
        /// The device that sent it, as authenticated by MLS.
        sender: DeviceId,
        /// The decrypted update.
        update: Update,
    },
    /// A commit that passed the policy and was merged.
    Commit {
        /// The device that made the commit.
        committer: DeviceId,
        /// The group's epoch after the commit.
        epoch: u64,
        /// Whether this client was removed by the commit.
        removed_self: bool,
    },
}

/// One device's MLS client.
#[derive(Debug)]
pub struct Client {
    device: DeviceId,
    provider: OpenMlsRustCrypto,
    signer: SignatureKeyPair,
    credential: CredentialWithKey,
    groups: BTreeMap<Vec<u8>, MlsGroup>,
    /// The members and roles of each group's current epoch, read once per epoch rather than once per message.
    views: BTreeMap<Vec<u8>, GroupView>,
    /// The members and roles of each group's previous epoch: a late update is judged by the roles of the epoch it was sent in as well as the current one, as the validator does.
    previous_views: BTreeMap<Vec<u8>, GroupView>,
    past_epochs: usize,
    /// Whether this device ever created key material for a provisional ciphersuite, such as a key package; [`crate::persist`] then refuses to save it.
    provisional_key_material: Cell<bool>,
}

/// The capabilities every client announces in its leaf: basic credentials and the roster extension (the group context requires both).
fn capabilities() -> Capabilities {
    Capabilities::builder()
        .extensions(vec![ExtensionType::Unknown(ROSTER_EXTENSION_TYPE)])
        .credentials(vec![CredentialType::Basic])
        .build()
}

fn join_config(past_epochs: usize) -> MlsGroupJoinConfig {
    MlsGroupJoinConfig::builder()
        .wire_format_policy(PURE_PLAINTEXT_WIRE_FORMAT_POLICY)
        .use_ratchet_tree_extension(false)
        .max_past_epochs(past_epochs)
        .build()
}

impl Client {
    /// A new device with a fresh Ed25519 signature key (all suites the spike uses sign with Ed25519).
    ///
    /// # Errors
    ///
    /// Returns an error if the key cannot be generated or stored.
    pub fn new(device: DeviceId) -> Result<Self, ClientError> {
        Self::with_past_epochs(device, DEFAULT_PAST_EPOCHS)
    }

    /// A new device that keeps decryption keys for `past_epochs` past epochs in the groups it creates or joins.
    ///
    /// # Errors
    ///
    /// Returns an error if the key cannot be generated or stored.
    pub fn with_past_epochs(device: DeviceId, past_epochs: usize) -> Result<Self, ClientError> {
        let provider = OpenMlsRustCrypto::default();
        let signer =
            SignatureKeyPair::new(openmls_traits::types::SignatureScheme::ED25519).map_err(mls)?;
        signer.store(provider.storage()).map_err(mls)?;
        let credential = CredentialWithKey {
            credential: openmls::prelude::BasicCredential::new(device.to_credential_identity())
                .into(),
            signature_key: signer.to_public_vec().into(),
        };
        Ok(Self {
            device,
            provider,
            signer,
            credential,
            groups: BTreeMap::new(),
            views: BTreeMap::new(),
            previous_views: BTreeMap::new(),
            past_epochs,
            provisional_key_material: Cell::new(false),
        })
    }

    /// Rebuilds a client from its parts (see [`crate::persist`]).
    pub(crate) fn from_parts(
        device: DeviceId,
        provider: OpenMlsRustCrypto,
        signer: SignatureKeyPair,
        groups: BTreeMap<Vec<u8>, MlsGroup>,
    ) -> Result<Self, ClientError> {
        let credential = CredentialWithKey {
            credential: openmls::prelude::BasicCredential::new(device.to_credential_identity())
                .into(),
            signature_key: signer.to_public_vec().into(),
        };
        let mut client = Self {
            device,
            provider,
            signer,
            credential,
            groups,
            views: BTreeMap::new(),
            previous_views: BTreeMap::new(),
            past_epochs: DEFAULT_PAST_EPOCHS,
            provisional_key_material: Cell::new(false),
        };
        let group_ids: Vec<Vec<u8>> = client.groups.keys().cloned().collect();
        for group_id in group_ids {
            client.refresh_view(&group_id)?;
        }
        Ok(client)
    }

    /// Reads the members and roles of the group's current epoch into the cache. Called whenever the epoch changes. The group's context was checked when this client accepted it (see [`Roster::from_group_context`]), so the roster is only looked up here.
    fn refresh_view(&mut self, group_id: &[u8]) -> Result<(), ClientError> {
        let group = self.group(group_id)?;
        let view = wire::view_of(group.members(), Roster::find_in(group.extensions()))?;
        self.views.insert(group_id.to_vec(), view);
        Ok(())
    }

    /// Keeps the current epoch's members and roles as the previous epoch's, after a commit was merged, and reads the new ones.
    fn advance_view(&mut self, group_id: &[u8]) -> Result<(), ClientError> {
        if let Some(view) = self.views.remove(group_id) {
            self.previous_views.insert(group_id.to_vec(), view);
        }
        self.refresh_view(group_id)
    }

    fn cached_view(&self, group_id: &[u8]) -> Result<&GroupView, ClientError> {
        self.views.get(group_id).ok_or(ClientError::UnknownGroup)
    }

    /// The device this client is.
    #[must_use]
    pub fn device(&self) -> &DeviceId {
        &self.device
    }

    /// The OpenMLS provider (crypto, randomness, storage).
    #[must_use]
    pub fn provider(&self) -> &OpenMlsRustCrypto {
        &self.provider
    }

    /// Whether this device ever created key material for a provisional ciphersuite, such as a key package (see [`crate::persist`]).
    #[must_use]
    pub fn holds_provisional_key_material(&self) -> bool {
        self.provisional_key_material.get()
    }

    pub(crate) fn signer(&self) -> &SignatureKeyPair {
        &self.signer
    }

    /// A fresh key package for `suite`, encoded as an MLS message for the server's directory. Each key package can be used once.
    ///
    /// # Errors
    ///
    /// Returns an error if OpenMLS cannot build it.
    pub fn key_package(&self, suite: Suite) -> Result<Vec<u8>, ClientError> {
        if suite.is_provisional() {
            self.provisional_key_material.set(true);
        }
        let bundle = KeyPackage::builder()
            .leaf_node_capabilities(capabilities())
            .build(
                suite.ciphersuite(),
                &self.provider,
                &self.signer,
                self.credential.clone(),
            )
            .map_err(mls)?;
        Ok(wire::encode(&MlsMessageOut::from(
            bundle.key_package().clone(),
        ))?)
    }

    /// Creates a group in which this device is the only member and its user the owner. Returns the group's ID, 16 random bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the roster is malformed or OpenMLS fails.
    pub fn create_group(&mut self, suite: Suite) -> Result<Vec<u8>, ClientError> {
        let roster = Roster::new().with(self.device.user().clone(), crate::roster::Role::Owner);
        self.create_group_with(suite, roster.group_context_extensions()?)
    }

    /// Creates a group whose context holds `extensions`, whatever they are: only for tests that play a misbehaving creator (for example one that adds an external sender), whose group the server and newcomers must refuse.
    ///
    /// # Errors
    ///
    /// Returns an error if OpenMLS refuses the extensions or the roster cannot be found in them.
    pub fn create_group_with_context_unchecked(
        &mut self,
        suite: Suite,
        extensions: Extensions<GroupContext>,
    ) -> Result<Vec<u8>, ClientError> {
        self.create_group_with(suite, extensions)
    }

    fn create_group_with(
        &mut self,
        suite: Suite,
        extensions: Extensions<GroupContext>,
    ) -> Result<Vec<u8>, ClientError> {
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(suite.ciphersuite())
            .wire_format_policy(PURE_PLAINTEXT_WIRE_FORMAT_POLICY)
            .use_ratchet_tree_extension(false)
            .max_past_epochs(self.past_epochs)
            .capabilities(capabilities())
            .with_group_context_extensions(extensions)
            .build();
        let group_id = self.provider.rand().random_vec(16).map_err(mls)?;
        let group = MlsGroup::new_with_group_id(
            &self.provider,
            &self.signer,
            &config,
            GroupId::from_slice(&group_id),
            self.credential.clone(),
        )
        .map_err(mls)?;
        self.groups.insert(group_id.clone(), group);
        self.refresh_view(&group_id)?;
        Ok(group_id)
    }

    fn group(&self, group_id: &[u8]) -> Result<&MlsGroup, ClientError> {
        self.groups.get(group_id).ok_or(ClientError::UnknownGroup)
    }

    /// The IDs of the groups this client is in.
    pub fn group_ids(&self) -> impl Iterator<Item = &[u8]> {
        self.groups.keys().map(Vec::as_slice)
    }

    /// Who is in the group and their roles, as this client sees it.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown group or invalid state.
    pub fn view(&self, group_id: &[u8]) -> Result<GroupView, ClientError> {
        self.cached_view(group_id).cloned()
    }

    /// The group's current epoch.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown group.
    pub fn epoch(&self, group_id: &[u8]) -> Result<u64, ClientError> {
        Ok(self.group(group_id)?.epoch().as_u64())
    }

    /// The group's ciphersuite.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown group or a ciphersuite the spike does not use.
    pub fn suite(&self, group_id: &[u8]) -> Result<Suite, ClientError> {
        Suite::from_ciphersuite(self.group(group_id)?.ciphersuite())
            .ok_or(ClientError::WrongCiphersuite)
    }

    /// The signed `GroupInfo` of the group's current epoch (without the tree), for registering a new group with the server.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown group or if OpenMLS fails.
    pub fn group_info(&self, group_id: &[u8]) -> Result<Vec<u8>, ClientError> {
        let group = self.group(group_id)?;
        let group_info = group
            .export_group_info(self.provider.crypto(), &self.signer, false)
            .map_err(mls)?;
        Ok(wire::encode(&group_info)?)
    }

    /// The group's public ratchet tree.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown group.
    pub fn ratchet_tree(&self, group_id: &[u8]) -> Result<Vec<u8>, ClientError> {
        Ok(wire::encode_tree(
            &self.group(group_id)?.export_ratchet_tree(),
        )?)
    }

    /// Prepares a commit. It is pending until the server accepts it ([`Client::confirm_commit`]) or refuses it ([`Client::discard_commit`]). The client refuses to prepare a commit that breaks the policy, so honest clients never send one.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid key packages, devices that are not members, policy violations, or OpenMLS failures.
    pub fn commit(
        &mut self,
        group_id: &[u8],
        change: &Change,
    ) -> Result<CommitOutput, ClientError> {
        self.build_commit(group_id, change, true)
    }

    /// Prepares a commit without checking the policy first: only for tests that play a misbehaving client.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid key packages, devices that are not members, or OpenMLS failures.
    pub fn commit_unchecked(
        &mut self,
        group_id: &[u8],
        change: &Change,
    ) -> Result<CommitOutput, ClientError> {
        self.build_commit(group_id, change, false)
    }

    fn build_commit(
        &mut self,
        group_id: &[u8],
        change: &Change,
        check_policy: bool,
    ) -> Result<CommitOutput, ClientError> {
        let before = self.view(group_id)?;
        let crypto = self.provider.crypto();
        let group = self.group(group_id)?;
        let mut key_packages = Vec::with_capacity(change.add.len());
        let mut added = Vec::with_capacity(change.add.len());
        for bytes in &change.add {
            let key_package = wire::decode_key_package(bytes, crypto)?;
            if key_package.ciphersuite() != group.ciphersuite() {
                return Err(ClientError::WrongCiphersuite);
            }
            added.push(wire::device_of(key_package.leaf_node().credential())?);
            key_packages.push(key_package);
        }
        let mut removed = Vec::with_capacity(change.remove.len());
        for device in &change.remove {
            let index = group
                .members()
                .find(|member| wire::device_of(&member.credential).as_ref() == Ok(device))
                .map(|member| member.index)
                .ok_or(PolicyViolation::MembershipMismatch)?;
            removed.push(index);
        }
        // The roster is proposed only when it changes, and then in exactly the form every member checks for.
        let new_roster = change
            .roster
            .clone()
            .filter(|roster| *roster != before.roster);
        if check_policy {
            policy::check_commit(
                &before,
                &CommitSummary {
                    committer: self.device.clone(),
                    added,
                    removed: change.remove.clone(),
                    new_roster: new_roster.clone(),
                    path_identity: None,
                    unsupported: Vec::new(),
                },
            )?;
        }
        let extensions = new_roster
            .map(|roster| roster.group_context_extensions())
            .transpose()?;
        self.stage_commit(
            group_id,
            key_packages,
            removed,
            extensions,
            change.is_self_update(),
        )
    }

    /// Prepares a commit whose only proposal replaces the group-context extensions with `extensions`, without checking the policy: only for tests that play a misbehaving client, for example one that keeps the roster but adds an external sender.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown group or if OpenMLS refuses the extensions.
    pub fn commit_context_unchecked(
        &mut self,
        group_id: &[u8],
        extensions: Extensions<GroupContext>,
    ) -> Result<CommitOutput, ClientError> {
        self.stage_commit(group_id, Vec::new(), Vec::new(), Some(extensions), false)
    }

    fn stage_commit(
        &mut self,
        group_id: &[u8],
        key_packages: Vec<KeyPackage>,
        removed: Vec<openmls::prelude::LeafNodeIndex>,
        extensions: Option<Extensions<GroupContext>>,
        self_update: bool,
    ) -> Result<CommitOutput, ClientError> {
        let provider = &self.provider;
        let signer = &self.signer;
        let group = self
            .groups
            .get_mut(group_id)
            .ok_or(ClientError::UnknownGroup)?;
        let mut builder = group
            .commit_builder()
            .propose_adds(key_packages)
            .propose_removals(removed);
        if let Some(extensions) = extensions {
            builder = builder
                .propose_group_context_extensions(extensions)
                .map_err(mls)?;
        }
        if self_update {
            builder = builder.force_self_update(true);
        }
        let bundle = builder
            .load_psks(provider.storage())
            .map_err(mls)?
            .build(provider.rand(), provider.crypto(), signer, |_| true)
            .map_err(mls)?
            .stage_commit(provider)
            .map_err(mls)?;
        let (commit, welcome, _group_info) = bundle.into_messages();
        Ok(CommitOutput {
            commit: wire::encode(&commit)?,
            welcome: welcome.as_ref().map(wire::encode).transpose()?,
        })
    }

    /// Applies this client's pending commit after the server accepted it.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown group or if there is no pending commit.
    pub fn confirm_commit(&mut self, group_id: &[u8]) -> Result<(), ClientError> {
        let provider = &self.provider;
        let group = self
            .groups
            .get_mut(group_id)
            .ok_or(ClientError::UnknownGroup)?;
        group.merge_pending_commit(provider).map_err(mls)?;
        self.advance_view(group_id)
    }

    /// Drops this client's pending commit after the server refused it.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown group or a storage failure.
    pub fn discard_commit(&mut self, group_id: &[u8]) -> Result<(), ClientError> {
        let provider = &self.provider;
        let group = self
            .groups
            .get_mut(group_id)
            .ok_or(ClientError::UnknownGroup)?;
        group.clear_pending_commit(provider.storage()).map_err(mls)
    }

    /// Joins a group from a Welcome and the group's ratchet tree (fetched from the server; OpenMLS checks it against the tree hash the Welcome's signed `GroupInfo` commits to). The client checks the roster: it must list exactly the members' users with an owner among them, give this device's user a role, and the device that added this one must be an owner or one of this user's own devices. Returns the group's ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the Welcome or tree is invalid, or the policy refuses the group.
    pub fn join(&mut self, welcome: &[u8], ratchet_tree: &[u8]) -> Result<Vec<u8>, ClientError> {
        // OpenMLS deletes the key package's private keys as soon as it finds them for a Welcome, before it checks the group info and the tree, so a failed join would make the Welcome unusable for good (a server could block newcomers by sending a bad tree once). Joining is therefore a transaction: on failure the client's storage is restored. A production storage provider must offer the same (REPORT.md).
        let saved = self.storage_snapshot()?;
        let result = self.join_inner(welcome, ratchet_tree);
        if result.is_err() {
            self.storage_restore(saved)?;
        }
        result
    }

    fn join_inner(&mut self, welcome: &[u8], ratchet_tree: &[u8]) -> Result<Vec<u8>, ClientError> {
        let MlsMessageBodyIn::Welcome(welcome) = wire::decode(welcome)? else {
            return Err(WireError::Malformed.into());
        };
        let tree = wire::decode_tree(ratchet_tree)?;
        let staged = StagedWelcome::new_from_welcome(
            &self.provider,
            &join_config(self.past_epochs),
            welcome,
            Some(tree),
        )
        .map_err(mls)?;
        let view = wire::view_of(
            staged.members(),
            Roster::from_group_context(staged.group_context()),
        )?;
        view.check_consistent()?;
        view.role_of(&self.device)
            .ok_or(PolicyViolation::NotAMember)?;
        let sender = wire::device_of(staged.welcome_sender().map_err(mls)?.credential())?;
        let sender_role = view.role_of(&sender).ok_or(PolicyViolation::NotAMember)?;
        if !sender_role.can_manage_members() && sender.user() != self.device.user() {
            return Err(PolicyViolation::NotAllowed {
                role: sender_role,
                action: "adding another user's device",
            }
            .into());
        }
        let group = staged.into_group(&self.provider).map_err(mls)?;
        let group_id = group.group_id().as_slice().to_vec();
        self.groups.insert(group_id.clone(), group);
        self.views.insert(group_id.clone(), view);
        self.previous_views.remove(&group_id);
        Ok(group_id)
    }

    /// The bytes the client's storage provider holds (keys and the state of all its groups), counting keys and values: what a device must keep, for the measurements.
    ///
    /// # Errors
    ///
    /// Returns an error if the storage lock is poisoned.
    pub fn state_size(&self) -> Result<usize, ClientError> {
        let values = self
            .provider
            .storage()
            .values
            .read()
            .map_err(|_| ClientError::Mls("storage lock poisoned".to_owned()))?;
        Ok(values
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum())
    }

    /// A copy of everything in the client's storage provider (keys and MLS state).
    pub(crate) fn storage_snapshot(
        &self,
    ) -> Result<std::collections::HashMap<Vec<u8>, Vec<u8>>, ClientError> {
        let values = self
            .provider
            .storage()
            .values
            .read()
            .map_err(|_| ClientError::Mls("storage lock poisoned".to_owned()))?;
        Ok(values.clone())
    }

    fn storage_restore(
        &self,
        saved: std::collections::HashMap<Vec<u8>, Vec<u8>>,
    ) -> Result<(), ClientError> {
        let mut values = self
            .provider
            .storage()
            .values
            .write()
            .map_err(|_| ClientError::Mls("storage lock poisoned".to_owned()))?;
        *values = saved;
        Ok(())
    }

    /// Encrypts an update for the group (private framing). The client refuses to send what its role does not allow, since every other client would discard it.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown group, a forbidden kind or an OpenMLS failure.
    pub fn send(&mut self, group_id: &[u8], update: &Update) -> Result<Vec<u8>, ClientError> {
        policy::check_update(self.cached_view(group_id)?, &self.device, update.kind)?;
        self.send_unchecked(group_id, update)
    }

    /// Encrypts an update without checking this client's own role: only for tests that play a misbehaving client.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown group or an OpenMLS failure.
    pub fn send_unchecked(
        &mut self,
        group_id: &[u8],
        update: &Update,
    ) -> Result<Vec<u8>, ClientError> {
        let provider = &self.provider;
        let signer = &self.signer;
        let group = self
            .groups
            .get_mut(group_id)
            .ok_or(ClientError::UnknownGroup)?;
        let message = group
            .create_message(provider, signer, &update.to_bytes()?)
            .map_err(mls)?;
        Ok(wire::encode(&message)?)
    }

    /// Processes a message the server delivered for the group: decrypts and checks an update against its sender's role, or checks a commit against the policy and merges it. A commit the policy refuses is not merged; the client then no longer follows the group (fail closed).
    ///
    /// # Errors
    ///
    /// Returns an error if the message is malformed, fails MLS verification, or breaks the policy.
    pub fn receive(&mut self, group_id: &[u8], bytes: &[u8]) -> Result<Received, ClientError> {
        let message = match wire::decode(bytes)? {
            MlsMessageBodyIn::PublicMessage(message) => {
                openmls::prelude::ProtocolMessage::from(message)
            }
            MlsMessageBodyIn::PrivateMessage(message) => {
                openmls::prelude::ProtocolMessage::from(message)
            }
            _ => return Err(WireError::Malformed.into()),
        };
        if message.group_id().as_slice() != group_id {
            return Err(ClientError::WrongGroup);
        }
        let before = self.views.get(group_id).ok_or(ClientError::UnknownGroup)?;
        let provider = &self.provider;
        let group = self
            .groups
            .get_mut(group_id)
            .ok_or(ClientError::UnknownGroup)?;
        let processed = group.process_message(provider, message).map_err(mls)?;
        let sender_credential = processed.credential().clone();
        let sender = wire::device_of(&sender_credential)?;
        let late = processed.epoch().as_u64() < group.epoch().as_u64();
        let received = match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(message) => {
                let update = Update::from_bytes(&message.into_bytes())?;
                policy::check_update(before, &sender, update.kind)?;
                // An update sent before the last commit must also have been allowed then, as the validator requires: a role granted by that commit does not cover what was sent before it.
                if late {
                    let previous =
                        self.previous_views
                            .get(group_id)
                            .ok_or(ClientError::Unsupported(
                                "updates from before this device joined",
                            ))?;
                    policy::check_update(previous, &sender, update.kind)?;
                }
                return Ok(Received::Update { sender, update });
            }
            ProcessedMessageContent::StagedCommitMessage(staged) => {
                let summary = wire::summarize_commit(&sender_credential, &staged, |index| {
                    group.member(index).cloned()
                })?;
                policy::check_commit(before, &summary)?;
                let removed_self = staged.self_removed();
                group.merge_staged_commit(provider, *staged).map_err(mls)?;
                Received::Commit {
                    committer: sender,
                    epoch: group.epoch().as_u64(),
                    removed_self,
                }
            }
            ProcessedMessageContent::ProposalMessage(_)
            | ProcessedMessageContent::ExternalJoinProposalMessage(_) => {
                return Err(ClientError::Unsupported("standalone proposals"));
            }
            _ => return Err(ClientError::Unsupported("this client's own messages")),
        };
        // The commit was merged: the group is in a new epoch.
        if matches!(
            received,
            Received::Commit {
                removed_self: true,
                ..
            }
        ) {
            self.views.remove(group_id);
            self.previous_views.remove(group_id);
        } else {
            self.advance_view(group_id)?;
        }
        Ok(received)
    }

    /// Exports a secret from the group's current epoch (RFC 9420 §8.5), for application keys bound to the epoch.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown group or an OpenMLS failure.
    pub fn export_secret(
        &self,
        group_id: &[u8],
        label: &str,
        context: &[u8],
        length: usize,
    ) -> Result<Vec<u8>, ClientError> {
        self.group(group_id)?
            .export_secret(self.provider.crypto(), label, context, length)
            .map_err(mls)
    }
}
