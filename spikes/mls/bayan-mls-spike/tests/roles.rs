//! AC-2: the server's validator refuses unauthorized membership changes and updates from devices without a write role, and clients refuse updates and changes whose authenticated sender lacks the role, even when a misbehaving server relays them.

use bayan_mls_spike::client::{Change, ClientError, Received};
use bayan_mls_spike::identity::DeviceId;
use bayan_mls_spike::policy::PolicyViolation;
use bayan_mls_spike::roster::{Role, Roster, RosterError};
use bayan_mls_spike::sim::{Deployment, SimError};
use bayan_mls_spike::suite::Suite;
use bayan_mls_spike::update::{Update, UpdateKind};
use bayan_mls_spike::validator::Rejection;
use bayan_mls_spike::wire::WireError;
use openmls::prelude::{
    BasicCredential, Extension, Extensions, ExternalSender, GroupContext, SignaturePublicKey,
};
use wasm_bindgen_test::wasm_bindgen_test;

const SUITE: Suite = Suite::Classical;

struct Fixture {
    deployment: Deployment,
    group: Vec<u8>,
    alice: DeviceId,
    bob: DeviceId,
    carol: DeviceId,
    dave: DeviceId,
}

/// alice (owner), bob (editor), carol (commenter), dave (viewer); erin has published a key package but is not a member.
fn fixture() -> Fixture {
    let mut deployment = Deployment::new();
    let alice = deployment
        .add_device("alice/laptop", SUITE, 1)
        .expect("set up the group");
    let bob = deployment
        .add_device("bob/laptop", SUITE, 1)
        .expect("set up the group");
    let carol = deployment
        .add_device("carol/phone", SUITE, 1)
        .expect("set up the group");
    let dave = deployment
        .add_device("dave/tablet", SUITE, 1)
        .expect("set up the group");
    deployment
        .add_device("erin/laptop", SUITE, 4)
        .expect("set up the group");
    let group = deployment
        .create_group(&alice, SUITE)
        .expect("set up the group");
    deployment
        .add_members(
            &alice,
            &group,
            &[
                (bob.clone(), Role::Editor),
                (carol.clone(), Role::Commenter),
                (dave.clone(), Role::Viewer),
            ],
        )
        .expect("set up the group");
    deployment
        .check_agreement(&group)
        .expect("set up the group");
    Fixture {
        deployment,
        group,
        alice,
        bob,
        carol,
        dave,
    }
}

fn erin() -> DeviceId {
    DeviceId::parse("erin/laptop").expect("a valid device name")
}

fn is_not_allowed(error: &Rejection) -> bool {
    matches!(error, Rejection::Policy(PolicyViolation::NotAllowed { .. }))
}

fn client_refused(result: &Result<Received, ClientError>) -> bool {
    matches!(result, Err(ClientError::Policy(_)))
}

/// The group context the roster builds, plus an external sender: a key with which someone outside the group (here, the server's operator) could propose changes, such as adding a member.
fn with_external_sender(roster: &Roster) -> Extensions<GroupContext> {
    let mut extensions: Vec<Extension> = roster
        .group_context_extensions()
        .expect("the roster's extensions")
        .iter()
        .cloned()
        .collect();
    extensions.push(Extension::ExternalSenders(vec![ExternalSender::new(
        SignaturePublicKey::from(vec![0x42; 32]),
        BasicCredential::new(b"server/operator".to_vec()).into(),
    )]));
    Extensions::try_from(extensions).expect("valid extensions")
}

fn unexpected_context(error: &WireError) -> bool {
    matches!(
        error,
        WireError::InvalidRoster(RosterError::UnexpectedExtensions)
    )
}

/// The size of the two MAC tags at the end of a commit in public framing with a SHA-256 suite: the confirmation tag and the membership tag, each a length byte and 32 bytes. They are keyed with the epoch's secrets, so the validator cannot check them (see `known_limitation_a_member_can_stall_the_group_with_a_commit_only_members_can_check`); everything before them is covered by the committer's signature.
const MAC_TAGS: usize = 2 * (1 + 32);

/// A change adding erin as an editor, built by `committer` without its client's own policy check (a misbehaving client).
fn add_erin(fixture: &mut Fixture, committer: &DeviceId) -> bayan_mls_spike::client::CommitOutput {
    let key_package = fixture
        .deployment
        .directory
        .fetch(&erin(), SUITE)
        .expect("build the commit adding erin");
    let roster = fixture
        .deployment
        .client(committer)
        .view(&fixture.group)
        .expect("build the commit adding erin")
        .roster
        .with(erin().user().clone(), Role::Editor);
    let change = Change {
        add: vec![key_package],
        remove: vec![],
        roster: Some(roster),
    };
    fixture
        .deployment
        .client_mut(committer)
        .commit_unchecked(&fixture.group, &change)
        .expect("build the commit adding erin")
}

#[wasm_bindgen_test(unsupported = test)]
fn honest_clients_refuse_to_make_changes_their_role_forbids() {
    let mut f = fixture();
    let key_package = f.deployment.directory.fetch(&erin(), SUITE).unwrap();
    let roster = f
        .deployment
        .client(&f.bob)
        .view(&f.group)
        .unwrap()
        .roster
        .with(erin().user().clone(), Role::Editor);
    let change = Change {
        add: vec![key_package],
        remove: vec![],
        roster: Some(roster),
    };
    let error = f
        .deployment
        .commit(&f.bob.clone(), &f.group.clone(), &change)
        .unwrap_err();
    assert!(
        matches!(
            error,
            SimError::Client {
                error: ClientError::Policy(PolicyViolation::NotAllowed { .. }),
                ..
            }
        ),
        "{error}"
    );
    let update = Update::new(UpdateKind::Content, b"edit".to_vec());
    assert!(
        f.deployment
            .client_mut(&f.carol)
            .send(&f.group, &update)
            .is_err()
    );
}

#[wasm_bindgen_test(unsupported = test)]
fn validator_rejects_membership_changes_by_non_owners_and_clients_reject_them_too() {
    let mut f = fixture();
    for committer in [f.bob.clone(), f.carol.clone(), f.dave.clone()] {
        let output = add_erin(&mut f, &committer);
        let error = f
            .deployment
            .validator
            .check_handshake(&committer, &output.commit)
            .unwrap_err();
        assert!(is_not_allowed(&error), "{committer}: {error}");
        // A misbehaving server relays it anyway: every honest member refuses it and stays in the same epoch.
        let epoch = f.deployment.validator.epoch(&f.group).unwrap();
        for (device, result) in
            f.deployment
                .deliver_unchecked(&committer, &f.group.clone(), &output.commit)
        {
            assert!(
                client_refused(&result),
                "{device} accepted {committer}'s commit: {result:?}"
            );
            assert_eq!(f.deployment.client(&device).epoch(&f.group).unwrap(), epoch);
        }
        f.deployment
            .client_mut(&committer)
            .discard_commit(&f.group)
            .unwrap();
    }

    // Removing someone else is refused the same way.
    let change = Change {
        add: vec![],
        remove: vec![f.alice.clone()],
        roster: None,
    };
    let output = f
        .deployment
        .client_mut(&f.bob)
        .commit_unchecked(&f.group, &change)
        .unwrap();
    assert!(is_not_allowed(
        &f.deployment
            .validator
            .check_handshake(&f.bob, &output.commit)
            .unwrap_err()
    ));
    for (device, result) in
        f.deployment
            .deliver_unchecked(&f.bob.clone(), &f.group.clone(), &output.commit)
    {
        assert!(
            client_refused(&result),
            "{device} accepted bob's removal of alice"
        );
    }
}

#[wasm_bindgen_test(unsupported = test)]
fn newcomer_rejects_a_welcome_from_a_non_owner() {
    let mut f = fixture();
    let bob = f.bob.clone();
    let output = add_erin(&mut f, &bob);
    // The server would refuse the commit; a misbehaving server forwards the Welcome with the tree as bob's client sees it after the commit.
    f.deployment
        .client_mut(&f.bob)
        .confirm_commit(&f.group)
        .unwrap();
    let tree = f.deployment.client(&f.bob).ratchet_tree(&f.group).unwrap();
    let error = f
        .deployment
        .client_mut(&erin())
        .join(&output.welcome.unwrap(), &tree)
        .unwrap_err();
    assert!(
        matches!(
            error,
            ClientError::Policy(PolicyViolation::NotAllowed { .. })
        ),
        "{error}"
    );
}

#[wasm_bindgen_test(unsupported = test)]
fn validator_rejects_role_changes_by_non_owners() {
    let mut f = fixture();
    for committer in [f.bob.clone(), f.carol.clone(), f.dave.clone()] {
        let roster = f
            .deployment
            .client(&committer)
            .view(&f.group)
            .unwrap()
            .roster
            .with(committer.user().clone(), Role::Owner);
        let change = Change {
            add: vec![],
            remove: vec![],
            roster: Some(roster),
        };
        let output = f
            .deployment
            .client_mut(&committer)
            .commit_unchecked(&f.group, &change)
            .unwrap();
        assert!(is_not_allowed(
            &f.deployment
                .validator
                .check_handshake(&committer, &output.commit)
                .unwrap_err()
        ));
        for (device, result) in
            f.deployment
                .deliver_unchecked(&committer, &f.group.clone(), &output.commit)
        {
            assert!(
                client_refused(&result),
                "{device} accepted {committer}'s promotion"
            );
        }
        f.deployment
            .client_mut(&committer)
            .discard_commit(&f.group)
            .unwrap();
    }
    // The owner can.
    let roster = f
        .deployment
        .client(&f.alice)
        .view(&f.group)
        .unwrap()
        .roster
        .with(f.dave.user().clone(), Role::Editor);
    f.deployment
        .set_roles(&f.alice.clone(), &f.group.clone(), roster)
        .unwrap();
    f.deployment.check_agreement(&f.group).unwrap();
}

#[wasm_bindgen_test(unsupported = test)]
fn last_owner_cannot_be_removed_or_demoted() {
    let mut f = fixture();
    let roster = f
        .deployment
        .client(&f.alice)
        .view(&f.group)
        .unwrap()
        .roster
        .with(f.alice.user().clone(), Role::Editor);
    let change = Change {
        add: vec![],
        remove: vec![],
        roster: Some(roster),
    };
    assert!(matches!(
        f.deployment.client_mut(&f.alice).commit(&f.group, &change),
        Err(ClientError::Policy(PolicyViolation::NoOwner))
    ));
    let output = f
        .deployment
        .client_mut(&f.alice)
        .commit_unchecked(&f.group, &change)
        .unwrap();
    assert!(matches!(
        f.deployment
            .validator
            .check_handshake(&f.alice, &output.commit),
        Err(Rejection::Policy(PolicyViolation::NoOwner))
    ));
}

#[wasm_bindgen_test(unsupported = test)]
fn validator_rejects_updates_from_viewers_and_non_members() {
    let mut f = fixture();
    // dave, a viewer, has no write role: the server refuses whatever he sends.
    let message = f
        .deployment
        .client_mut(&f.dave)
        .send_unchecked(&f.group, &Update::new(UpdateKind::Comment, b"hi".to_vec()))
        .unwrap();
    assert!(is_not_allowed(
        &f.deployment
            .validator
            .check_application(&f.dave, &message)
            .unwrap_err()
    ));
    // erin is not a member: the server refuses even a valid member's message submitted over her connection.
    let message = f
        .deployment
        .client_mut(&f.bob)
        .send(
            &f.group,
            &Update::new(UpdateKind::Content, b"edit".to_vec()),
        )
        .unwrap();
    assert!(matches!(
        f.deployment.validator.check_application(&erin(), &message),
        Err(Rejection::Policy(PolicyViolation::NotAMember))
    ));
    // A commenter may send application messages (comments are writes); the server cannot see which kind.
    let message = f
        .deployment
        .client_mut(&f.carol)
        .send(&f.group, &Update::new(UpdateKind::Comment, b"ok".to_vec()))
        .unwrap();
    f.deployment
        .validator
        .check_application(&f.carol, &message)
        .unwrap();
}

#[wasm_bindgen_test(unsupported = test)]
fn clients_reject_updates_whose_sender_lacks_the_role() {
    let mut f = fixture();
    // carol, a commenter, sends a content edit. The server must relay it (it cannot see the kind), but every client refuses it.
    let edit = Update::new(UpdateKind::Content, b"rewrite everything".to_vec());
    let message = f
        .deployment
        .client_mut(&f.carol)
        .send_unchecked(&f.group, &edit)
        .unwrap();
    let results = f
        .deployment
        .deliver(&f.carol.clone(), &f.group.clone(), &message)
        .unwrap();
    assert_eq!(results.len(), 3);
    for (device, result) in results {
        assert!(
            client_refused(&result),
            "{device} accepted a commenter's edit"
        );
    }
    // dave, a viewer, comments, and a misbehaving server relays it: every client refuses it.
    let comment = Update::new(UpdateKind::Comment, b"psst".to_vec());
    let message = f
        .deployment
        .client_mut(&f.dave)
        .send_unchecked(&f.group, &comment)
        .unwrap();
    for (device, result) in
        f.deployment
            .deliver_unchecked(&f.dave.clone(), &f.group.clone(), &message)
    {
        assert!(
            client_refused(&result),
            "{device} accepted a viewer's comment"
        );
    }
    // An editor cannot claim the owner's privileges either.
    let moved = Update::new(UpdateKind::GroupMoved, vec![0; 16]);
    let message = f
        .deployment
        .client_mut(&f.bob)
        .send_unchecked(&f.group, &moved)
        .unwrap();
    for (device, result) in f
        .deployment
        .deliver(&f.bob.clone(), &f.group.clone(), &message)
        .unwrap()
    {
        assert!(
            client_refused(&result),
            "{device} accepted an editor's group move"
        );
    }
}

#[wasm_bindgen_test(unsupported = test)]
fn commit_from_another_devices_connection_is_rejected() {
    let mut f = fixture();
    // bob makes a valid self-update; carol's connection submits it.
    let output = f
        .deployment
        .client_mut(&f.bob)
        .commit(&f.group, &Change::default())
        .unwrap();
    assert!(matches!(
        f.deployment
            .validator
            .check_handshake(&f.carol, &output.commit),
        Err(Rejection::SenderMismatch)
    ));
    // Over bob's own connection it is accepted.
    f.deployment
        .validator
        .check_handshake(&f.bob, &output.commit)
        .unwrap();
}

#[wasm_bindgen_test(unsupported = test)]
fn wrong_framing_is_refused() {
    let mut f = fixture();
    let message = f
        .deployment
        .client_mut(&f.bob)
        .send(&f.group, &Update::new(UpdateKind::Content, b"x".to_vec()))
        .unwrap();
    assert!(matches!(
        f.deployment.validator.check_handshake(&f.bob, &message),
        Err(Rejection::WrongFraming)
    ));
    let output = f
        .deployment
        .client_mut(&f.bob)
        .commit(&f.group, &Change::default())
        .unwrap();
    assert!(matches!(
        f.deployment
            .validator
            .check_application(&f.bob, &output.commit),
        Err(Rejection::WrongFraming)
    ));
}

#[wasm_bindgen_test(unsupported = test)]
fn only_owners_change_the_group_context_and_only_to_the_roster_and_its_requirement() {
    let mut f = fixture();
    let group = f.group.clone();
    let roster = f.deployment.client(&f.alice).view(&group).unwrap().roster;
    let exact = roster.group_context_extensions().unwrap();
    // (committer, new group context, whether it holds more than the roster and its requirement)
    let attempts = [
        // Every role, the owner included, tries to keep the roster but add an external sender.
        (f.dave.clone(), with_external_sender(&roster), true),
        (f.carol.clone(), with_external_sender(&roster), true),
        (f.bob.clone(), with_external_sender(&roster), true),
        (f.alice.clone(), with_external_sender(&roster), true),
        // No role but the owner may even propose the context as it is.
        (f.dave.clone(), exact.clone(), false),
        (f.bob.clone(), exact, false),
    ];
    for (committer, extensions, extra) in attempts {
        let epoch = f.deployment.validator.epoch(&group).unwrap();
        let output = f
            .deployment
            .client_mut(&committer)
            .commit_context_unchecked(&group, extensions)
            .unwrap();
        let error = f
            .deployment
            .validator
            .check_handshake(&committer, &output.commit)
            .unwrap_err();
        if extra {
            assert!(
                matches!(&error, Rejection::Wire(wire) if unexpected_context(wire)),
                "{committer}: {error}"
            );
        } else {
            assert!(is_not_allowed(&error), "{committer}: {error}");
        }
        assert_eq!(f.deployment.validator.epoch(&group).unwrap(), epoch);
        // A misbehaving server relays it anyway: every member refuses it and stays in its epoch.
        for (device, result) in f
            .deployment
            .deliver_unchecked(&committer, &group, &output.commit)
        {
            let refused = match &result {
                Err(ClientError::Wire(wire)) => extra && unexpected_context(wire),
                Err(ClientError::Policy(PolicyViolation::NotAllowed { .. })) => !extra,
                _ => false,
            };
            assert!(
                refused,
                "{device} accepted {committer}'s context change: {result:?}"
            );
            assert_eq!(f.deployment.client(&device).epoch(&group).unwrap(), epoch);
        }
        f.deployment
            .client_mut(&committer)
            .discard_commit(&group)
            .unwrap();
    }
    // The group carries on as before.
    f.deployment.self_update(&f.dave, &group).unwrap();
    f.deployment.check_agreement(&group).unwrap();
}

#[wasm_bindgen_test(unsupported = test)]
fn a_group_whose_context_holds_more_is_refused_by_the_server_and_by_newcomers() {
    let mut deployment = Deployment::new();
    let alice = deployment.add_device("alice/laptop", SUITE, 0).unwrap();
    let phone = deployment.add_device("alice/phone", SUITE, 1).unwrap();
    let roster = Roster::new().with(alice.user().clone(), Role::Owner);
    let group = deployment
        .client_mut(&alice)
        .create_group_with_context_unchecked(SUITE, with_external_sender(&roster))
        .unwrap();
    let client = deployment.client(&alice);
    let (group_info, tree) = (
        client.group_info(&group).unwrap(),
        client.ratchet_tree(&group).unwrap(),
    );
    let error = deployment
        .validator
        .register_group(&alice, &group_info, &tree)
        .unwrap_err();
    assert!(
        matches!(&error, Rejection::Wire(wire) if unexpected_context(wire)),
        "{error}"
    );

    // alice adds her own second device, which leaves the roster as it is; a misbehaving server forwards the Welcome, and the newcomer refuses to join.
    let key_package = deployment.directory.fetch(&phone, SUITE).unwrap();
    let change = Change {
        add: vec![key_package],
        remove: vec![],
        roster: None,
    };
    let output = deployment
        .client_mut(&alice)
        .commit_unchecked(&group, &change)
        .unwrap();
    deployment
        .client_mut(&alice)
        .confirm_commit(&group)
        .unwrap();
    let tree = deployment.client(&alice).ratchet_tree(&group).unwrap();
    let welcome = output.welcome.unwrap();
    let error = deployment
        .client_mut(&phone)
        .join(&welcome, &tree)
        .unwrap_err();
    assert!(
        matches!(&error, ClientError::Wire(wire) if unexpected_context(wire)),
        "{error}"
    );
    assert_eq!(deployment.client(&phone).group_ids().count(), 0);
}

#[wasm_bindgen_test(unsupported = test)]
fn a_role_granted_by_a_commit_does_not_cover_updates_sent_before_it() {
    let mut f = fixture();
    let group = f.group.clone();
    // carol, a commenter, writes an edit, which her own client would refuse to send …
    let edit = Update::new(UpdateKind::Content, b"made while commenting".to_vec());
    let message = f
        .deployment
        .client_mut(&f.carol)
        .send_unchecked(&group, &edit)
        .unwrap();
    // … and alice promotes her to editor before it arrives.
    let roster = f
        .deployment
        .client(&f.alice)
        .view(&group)
        .unwrap()
        .roster
        .with(f.carol.user().clone(), Role::Editor);
    f.deployment.set_roles(&f.alice, &group, roster).unwrap();
    // The server cannot tell an edit from a comment, and carol could send comments then and edits now, so it relays the late message; every member refuses it, judging it by the roles of the epoch it was sent in too.
    for (device, result) in f.deployment.deliver(&f.carol, &group, &message).unwrap() {
        assert!(
            matches!(
                result,
                Err(ClientError::Policy(PolicyViolation::NotAllowed { .. }))
            ),
            "{device} accepted an edit carol made before her promotion: {result:?}"
        );
    }
    // Edits she makes now are accepted.
    for (device, result) in f.deployment.send(&f.carol, &group, &edit).unwrap() {
        assert!(
            result.is_ok(),
            "{device} refused carol's new edit: {result:?}"
        );
    }
}

/// Known limitation, described in REPORT.md: the validator cannot check a commit's confirmation tag or membership tag (MACs keyed with the epoch's secrets), nor that its encrypted path secrets decrypt for every member, so it can accept a commit that every member then refuses. The server is then an epoch ahead of the members and refuses all their later commits, including the owner's removal of the member who caused it. Any role can do this, because every member may update its own keys. When a design from SRV-003 or SRV-103 removes the limitation, this test must change with it.
#[wasm_bindgen_test(unsupported = test)]
fn known_limitation_a_member_can_stall_the_group_with_a_commit_only_members_can_check() {
    let mut f = fixture();
    let group = f.group.clone();
    let epoch = f.deployment.validator.epoch(&group).unwrap();
    // dave, a viewer, makes an honest self-update and corrupts its membership tag (its last byte).
    let mut commit = f
        .deployment
        .client_mut(&f.dave)
        .commit(&group, &Change::default())
        .unwrap()
        .commit;
    *commit.last_mut().unwrap() ^= 0x01;
    f.deployment
        .validator
        .check_handshake(&f.dave, &commit)
        .unwrap();
    assert_eq!(f.deployment.validator.epoch(&group).unwrap(), epoch + 1);
    for (device, result) in f.deployment.deliver_unchecked(&f.dave, &group, &commit) {
        assert!(
            result.is_err(),
            "{device} accepted the corrupted commit: {result:?}"
        );
        assert_eq!(f.deployment.client(&device).epoch(&group).unwrap(), epoch);
    }
    // From now on the server refuses every commit the members make.
    let removal = f
        .deployment
        .remove_members(&f.alice, &group, &[f.dave.clone()])
        .unwrap_err();
    assert!(
        matches!(removal, SimError::Server(Rejection::WrongEpoch { .. })),
        "{removal}"
    );
    let update = f.deployment.self_update(&f.bob, &group).unwrap_err();
    assert!(
        matches!(update, SimError::Server(Rejection::WrongEpoch { .. })),
        "{update}"
    );
}

#[wasm_bindgen_test(unsupported = test)]
fn hostile_bytes_are_rejected_without_panic() {
    let mut f = fixture();
    let valid_commit = f
        .deployment
        .client_mut(&f.bob)
        .commit(&f.group, &Change::default())
        .unwrap()
        .commit;
    let valid_update = f
        .deployment
        .client_mut(&f.alice)
        .send(&f.group, &Update::new(UpdateKind::Content, vec![7; 300]))
        .unwrap();

    // The validator refuses every truncation of a real commit, and the commit with any bit of its signed part flipped, and stays in its epoch. Flips in the two MAC tags at the end are the known limitation above.
    let epoch = f.deployment.validator.epoch(&f.group).unwrap();
    let mut mutations: Vec<(String, Vec<u8>)> = (0..valid_commit.len())
        .map(|length| {
            (
                format!("truncated to {length}"),
                valid_commit[..length].to_vec(),
            )
        })
        .collect();
    for position in 0..valid_commit.len() - MAC_TAGS {
        let mut flipped = valid_commit.clone();
        flipped[position] ^= 0x80;
        mutations.push((format!("bit flipped at {position}"), flipped));
    }
    for (what, sample) in &mutations {
        let result = f.deployment.validator.check_handshake(&f.bob, sample);
        assert!(result.is_err(), "the validator accepted a commit {what}");
        assert_eq!(
            f.deployment.validator.epoch(&f.group).unwrap(),
            epoch,
            "{what}"
        );
    }

    let mut samples: Vec<Vec<u8>> = vec![vec![], vec![0], vec![0xff; 32], (0..=255).collect()];
    // Truncations of real messages, and single flipped bits at a spread of positions.
    for message in [&valid_commit, &valid_update] {
        for length in (0..message.len()).step_by(7) {
            samples.push(message[..length].to_vec());
        }
        for position in (0..message.len()).step_by(11) {
            let mut flipped = message.clone();
            flipped[position] ^= 0x80;
            samples.push(flipped);
        }
    }
    for sample in &samples {
        // None of these may panic. The validator cannot check what an update decrypts to, so an altered update it relays must still fail at every client.
        let _ = f.deployment.validator.check_application(&f.alice, sample);
        assert!(
            f.deployment
                .validator
                .register_group(&f.alice, sample, sample)
                .is_err()
        );
        assert!(
            f.deployment
                .client_mut(&f.carol)
                .join(sample, sample)
                .is_err()
        );
        if sample != &valid_update {
            assert!(
                f.deployment
                    .client_mut(&f.carol)
                    .receive(&f.group, sample)
                    .is_err()
            );
        }
    }
    assert_eq!(f.deployment.validator.epoch(&f.group).unwrap(), epoch);
}
