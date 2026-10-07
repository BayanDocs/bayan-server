//! AC-2: the server's validator refuses unauthorized membership changes and updates from devices without a write role, and clients refuse updates and changes whose authenticated sender lacks the role, even when a misbehaving server relays them.

use bayan_mls_spike::client::{Change, ClientError, Received};
use bayan_mls_spike::identity::DeviceId;
use bayan_mls_spike::policy::PolicyViolation;
use bayan_mls_spike::roster::Role;
use bayan_mls_spike::sim::{Deployment, SimError};
use bayan_mls_spike::suite::Suite;
use bayan_mls_spike::update::{Update, UpdateKind};
use bayan_mls_spike::validator::Rejection;
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
    let mut samples: Vec<Vec<u8>> = vec![vec![], vec![0], vec![0xff; 32], (0..=255).collect()];
    // Every truncation of real messages, and single flipped bits at a spread of positions.
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
        // None of these may panic; garbage must never be accepted as a commit, and anything accepted as an update must still fail at the clients if it was altered.
        let _ = f.deployment.validator.check_handshake(&f.bob, sample);
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
}
