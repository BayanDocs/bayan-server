//! Spike SRV-002, time-boxed fallback check (ADR-0016 Decision 1): do the flows the design needs work with mls-rs, the fallback library, natively and in WebAssembly?
//!
//! [`run_flows`] does with mls-rs 0.56 and its pure-Rust crypto provider (`mls-rs-crypto-rustcrypto`) what `bayan-mls-spike` does with OpenMLS: a group whose context carries a custom, required extension (the role roster's place), adding members, Welcomes processed with the ratchet tree fetched separately, updates of 100 B to 100 KB, a self-update, a removal, and a server that follows every commit in public framing with mls-rs's external client, seeing who committed and what. It does not port the role policy; REPORT.md says what porting it would take.

#![forbid(unsafe_code)]

use mls_rs::client_builder::MlsConfig;
use mls_rs::error::{IntoAnyError as _, MlsError};
use mls_rs::extension::ExtensionType;
use mls_rs::extension::built_in::RequiredCapabilitiesExt;
use mls_rs::external_client::builder::MlsConfig as ExternalMlsConfig;
use mls_rs::external_client::{ExternalClient, ExternalReceivedMessage};
use mls_rs::group::{CommitEffect, ReceivedMessage};
use mls_rs::identity::SigningIdentity;
use mls_rs::identity::basic::{BasicCredential, BasicIdentityProvider};
use mls_rs::{
    CipherSuite, CipherSuiteProvider, Client, CryptoProvider, Extension, ExtensionList, MlsMessage,
};
use mls_rs_crypto_rustcrypto::RustCryptoProvider;

/// The classical suite ADR-0016 names (0x0001). mls-rs's post-quantum suites need its AWS-LC provider, which is native code (ADR-0006) and does not build for WebAssembly.
pub const SUITE: CipherSuite = CipherSuite::CURVE25519_AES128;

/// The private-use extension type the OpenMLS spike uses for the role roster.
pub const ROSTER_EXTENSION: u16 = 0xF0BD;

/// The update sizes the brief names.
pub const PAYLOAD_SIZES: [usize; 4] = [100, 1_000, 10_000, 100_000];

/// What the check observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    /// The group's epoch at the end.
    pub epoch: u64,
    /// For each commit the server followed: the committer's leaf index and how many proposals it applied.
    pub commits_seen_by_server: Vec<(u32, usize)>,
    /// How many updates were decrypted and attributed to the right sender.
    pub updates_delivered: usize,
    /// The members at the end, as the server's external group lists them.
    pub server_members: Vec<String>,
}

fn identity(name: &str) -> Result<(mls_rs::crypto::SignatureSecretKey, SigningIdentity), MlsError> {
    let provider = RustCryptoProvider::default()
        .cipher_suite_provider(SUITE)
        .ok_or(MlsError::UnsupportedCipherSuite(SUITE))?;
    let (secret, public) = provider
        .signature_key_generate()
        .map_err(|error| MlsError::CryptoProviderError(error.into_any_error()))?;
    let credential = BasicCredential::new(name.as_bytes().to_vec()).into_credential();
    Ok((secret, SigningIdentity::new(credential, public)))
}

/// A client for `name`, announcing support for the roster extension.
///
/// # Errors
///
/// Returns an error if key generation fails.
pub fn client(name: &str) -> Result<Client<impl MlsConfig>, MlsError> {
    let (secret, signing_identity) = identity(name)?;
    Ok(Client::builder()
        .identity_provider(BasicIdentityProvider)
        .crypto_provider(RustCryptoProvider::default())
        .extension_type(ExtensionType::new(ROSTER_EXTENSION))
        .signing_identity(signing_identity, secret, SUITE)
        .build())
}

/// The server's view of groups: mls-rs's external client, which holds no keys.
#[must_use]
pub fn server() -> ExternalClient<impl ExternalMlsConfig> {
    ExternalClient::builder()
        .identity_provider(BasicIdentityProvider)
        .crypto_provider(RustCryptoProvider::default())
        .extension_type(ExtensionType::new(ROSTER_EXTENSION))
        .build()
}

/// Group-context extensions with a stand-in roster that every member must support.
fn roster_extensions(roster: &[u8]) -> Result<ExtensionList, MlsError> {
    let mut extensions = ExtensionList::new();
    extensions
        .set_from(RequiredCapabilitiesExt {
            extensions: vec![ExtensionType::new(ROSTER_EXTENSION)],
            proposals: vec![],
            credentials: vec![],
        })
        .map_err(MlsError::from)?;
    extensions.set(Extension::new(
        ExtensionType::new(ROSTER_EXTENSION),
        roster.to_vec(),
    ));
    Ok(extensions)
}

fn name_of(signing_identity: &SigningIdentity) -> String {
    signing_identity.credential.as_basic().map_or_else(
        || "?".to_owned(),
        |basic| String::from_utf8_lossy(&basic.identifier).into_owned(),
    )
}

/// Runs every flow once and reports what it saw.
///
/// # Errors
///
/// Returns the first error mls-rs reports.
pub fn run_flows() -> Result<Observed, MlsError> {
    let alice = client("alice")?;
    let bob = client("bob")?;
    let carol = client("carol")?;
    let server_client = server();
    let mut commits_seen_by_server = Vec::new();
    let mut updates_delivered = 0;

    // alice creates the group and registers it with the server (GroupInfo and tree).
    let mut alice_group = alice.create_group(
        roster_extensions(b"alice:owner")?,
        ExtensionList::default(),
        None,
    )?;
    let group_info = alice_group.group_info_message(false)?;
    let mut server_group = server_client.observe_group(
        group_info,
        Some(alice_group.export_tree().into_owned()),
        None,
    )?;

    // alice adds bob and carol and updates the roster in the same commit; the server follows it before alice applies it.
    let bob_key_package =
        bob.generate_key_package_message(ExtensionList::default(), ExtensionList::default(), None)?;
    let carol_key_package = carol.generate_key_package_message(
        ExtensionList::default(),
        ExtensionList::default(),
        None,
    )?;
    let output = alice_group
        .commit_builder()
        .add_member(bob_key_package)?
        .add_member(carol_key_package)?
        .set_group_context_ext(roster_extensions(b"alice:owner,bob:editor,carol:viewer")?)?
        .build()?;
    let commit = MlsMessage::from_bytes(&output.commit_message.to_bytes()?)?;
    commits_seen_by_server.push(server_saw(server_group.process_incoming_message(commit)?)?);
    alice_group.apply_pending_commit()?;
    let welcome = output
        .welcome_messages
        .first()
        .ok_or(MlsError::WelcomeKeyPackageNotFound)?;
    // The newcomers fetch the tree from the server, as in the OpenMLS design.
    let tree = server_group.exported_tree().into_owned();
    let (mut bob_group, _) = bob.join_group(Some(tree.clone()), welcome, None)?;
    let (mut carol_group, _) = carol.join_group(Some(tree), welcome, None)?;

    // Updates of every size from alice reach bob and carol, attributed to alice.
    for size in PAYLOAD_SIZES {
        let body = vec![0x5a; size];
        let message = alice_group.encrypt_application_message(&body, Vec::new())?;
        for group in [&mut bob_group, &mut carol_group] {
            if let ReceivedMessage::ApplicationMessage(update) =
                group.process_incoming_message(message.clone())?
            {
                let sender = group
                    .member_at_index(update.sender_index)
                    .map(|member| name_of(&member.signing_identity));
                if update.data() == body.as_slice() && sender.as_deref() == Some("alice") {
                    updates_delivered += 1;
                }
            }
        }
    }

    // bob updates his keys (an empty commit); the server and the others follow.
    let output = bob_group.commit(Vec::new())?;
    commits_seen_by_server.push(server_saw(
        server_group.process_incoming_message(output.commit_message.clone())?,
    )?);
    bob_group.apply_pending_commit()?;
    alice_group.process_incoming_message(output.commit_message.clone())?;
    carol_group.process_incoming_message(output.commit_message)?;

    // alice removes carol.
    let carol_index = carol_group.current_member_index();
    let output = alice_group
        .commit_builder()
        .remove_member(carol_index)?
        .set_group_context_ext(roster_extensions(b"alice:owner,bob:editor")?)?
        .build()?;
    commits_seen_by_server.push(server_saw(
        server_group.process_incoming_message(output.commit_message.clone())?,
    )?);
    alice_group.apply_pending_commit()?;
    bob_group.process_incoming_message(output.commit_message.clone())?;
    if let ReceivedMessage::Commit(description) =
        carol_group.process_incoming_message(output.commit_message)?
        && !matches!(description.effect, CommitEffect::Removed { .. })
    {
        return Err(MlsError::InvalidCommitSelfUpdate);
    }

    // alice and bob are in the same epoch and still talk.
    let message = bob_group.encrypt_application_message(b"after the removal", Vec::new())?;
    if let ReceivedMessage::ApplicationMessage(update) =
        alice_group.process_incoming_message(message)?
        && update.data() == b"after the removal"
    {
        updates_delivered += 1;
    }
    let server_members = server_group
        .roster()
        .members()
        .iter()
        .map(|member| name_of(&member.signing_identity))
        .collect();
    Ok(Observed {
        epoch: alice_group.current_epoch(),
        commits_seen_by_server,
        updates_delivered,
        server_members,
    })
}

/// What the server learned from a commit: the committer and the number of proposals applied.
fn server_saw(received: ExternalReceivedMessage) -> Result<(u32, usize), MlsError> {
    match received {
        ExternalReceivedMessage::Commit(description) => {
            let applied = match &description.effect {
                CommitEffect::NewEpoch(epoch)
                | CommitEffect::Removed {
                    new_epoch: epoch, ..
                } => epoch.applied_proposals.len(),
                CommitEffect::ReInit(_) => 0,
            };
            Ok((description.committer, applied))
        }
        _ => Err(MlsError::UnexpectedMessageType),
    }
}
