//! AC-1: every MLS flow the design needs, natively and in WebAssembly (Node.js and Chromium run these same tests through `cargo xtask mls-spike-wasm`).

use bayan_mls_spike::client::{Change, Received};
use bayan_mls_spike::identity::DeviceId;
use bayan_mls_spike::roster::Role;
use bayan_mls_spike::sim::Deployment;
use bayan_mls_spike::suite::Suite;
use bayan_mls_spike::update::{Update, UpdateKind};
use bayan_mls_spike::validator::Rejection;
use wasm_bindgen_test::wasm_bindgen_test;

const SUITE: Suite = Suite::Classical;

/// The payload sizes of CRDT updates the brief names: 100 B to 100 KB.
const PAYLOAD_SIZES: [usize; 4] = [100, 1_000, 10_000, 100_000];

fn payload(size: usize, seed: u8) -> Vec<u8> {
    (0..size)
        .map(|index: usize| index.to_le_bytes()[0].wrapping_mul(31).wrapping_add(seed))
        .collect()
}

/// alice (owner) creates a group and adds bob (editor) and carol (commenter).
fn three_member_group(deployment: &mut Deployment) -> (Vec<u8>, DeviceId, DeviceId, DeviceId) {
    let alice = deployment
        .add_device("alice/laptop", SUITE, 1)
        .expect("set up the three-member group");
    let bob = deployment
        .add_device("bob/laptop", SUITE, 1)
        .expect("set up the three-member group");
    let carol = deployment
        .add_device("carol/phone", SUITE, 1)
        .expect("set up the three-member group");
    let group = deployment
        .create_group(&alice, SUITE)
        .expect("set up the three-member group");
    deployment
        .add_members(
            &alice,
            &group,
            &[
                (bob.clone(), Role::Editor),
                (carol.clone(), Role::Commenter),
            ],
        )
        .expect("set up the three-member group");
    deployment
        .check_agreement(&group)
        .expect("set up the three-member group");
    (group, alice, bob, carol)
}

#[wasm_bindgen_test(unsupported = test)]
fn key_packages_are_checked_when_published_and_used_once() {
    let mut deployment = Deployment::new();
    let alice = deployment.add_device("alice/laptop", SUITE, 2).unwrap();
    let bob = deployment.add_device("bob/laptop", SUITE, 0).unwrap();
    assert_eq!(deployment.directory.available(&alice, SUITE), 2);

    // A device cannot publish key packages for another device.
    let key_package = deployment.client(&alice).key_package(SUITE).unwrap();
    assert!(matches!(
        deployment.directory.publish(&bob, &key_package),
        Err(Rejection::SenderMismatch)
    ));
    // A key package whose signature does not verify is refused.
    let mut tampered = key_package.clone();
    let middle = tampered.len() / 2;
    tampered[middle] ^= 0x01;
    assert!(deployment.directory.publish(&alice, &tampered).is_err());

    // Each key package is handed out once.
    assert!(deployment.directory.fetch(&alice, SUITE).is_some());
    assert!(deployment.directory.fetch(&alice, SUITE).is_some());
    assert!(deployment.directory.fetch(&alice, SUITE).is_none());
    assert!(
        deployment
            .directory
            .fetch(&alice, Suite::ClassicalChaCha)
            .is_none()
    );
}

#[wasm_bindgen_test(unsupported = test)]
fn create_add_update_and_remove() {
    let mut deployment = Deployment::new();
    let (group, alice, bob, carol) = three_member_group(&mut deployment);
    assert_eq!(deployment.validator.epoch(&group), Some(1));
    let view = deployment.validator.view(&group).unwrap().clone();
    assert_eq!(view.role_of(&bob), Some(Role::Editor));
    assert_eq!(view.role_of(&carol), Some(Role::Commenter));

    // Anyone may update their own keys; every member follows.
    deployment.self_update(&carol, &group).unwrap();
    deployment.self_update(&bob, &group).unwrap();
    assert_eq!(deployment.validator.epoch(&group), Some(3));
    deployment.check_agreement(&group).unwrap();

    // The owner removes carol; carol learns she was removed and the others agree on the new state.
    let removed = deployment
        .remove_members(&alice, &group, std::slice::from_ref(&carol))
        .unwrap();
    assert_eq!(removed.removed, vec![carol.clone()]);
    deployment.check_agreement(&group).unwrap();
    assert!(
        !deployment
            .validator
            .view(&group)
            .unwrap()
            .members
            .contains(&carol)
    );

    // Updates after the removal reach bob, and carol cannot read them even if they reach her.
    let update = Update::new(UpdateKind::Content, payload(100, 1));
    let message = deployment.client_mut(&alice).send(&group, &update).unwrap();
    deployment
        .validator
        .check_application(&alice, &message)
        .unwrap();
    assert_eq!(
        deployment
            .client_mut(&bob)
            .receive(&group, &message)
            .unwrap(),
        Received::Update {
            sender: alice,
            update
        }
    );
    assert!(
        deployment
            .client_mut(&carol)
            .receive(&group, &message)
            .is_err()
    );
}

#[wasm_bindgen_test(unsupported = test)]
fn updates_of_every_size_reach_every_member() {
    let mut deployment = Deployment::new();
    let (group, alice, bob, carol) = three_member_group(&mut deployment);
    for (seed, size) in (0u8..).zip(PAYLOAD_SIZES) {
        for sender in [&alice, &bob] {
            let update = Update::new(UpdateKind::Content, payload(size, seed));
            let results = deployment.send(sender, &group, &update).unwrap();
            assert_eq!(results.len(), 2);
            for (_, result) in results {
                assert_eq!(
                    result.unwrap(),
                    Received::Update {
                        sender: sender.clone(),
                        update: update.clone()
                    }
                );
            }
        }
    }
    // The commenter comments.
    let comment = Update::new(UpdateKind::Comment, b"needs a citation".to_vec());
    for (_, result) in deployment.send(&carol, &group, &comment).unwrap() {
        assert_eq!(
            result.unwrap(),
            Received::Update {
                sender: carol.clone(),
                update: comment.clone()
            }
        );
    }
}

#[wasm_bindgen_test(unsupported = test)]
fn a_user_manages_their_own_devices() {
    let mut deployment = Deployment::new();
    let (group, _alice, bob, _carol) = three_member_group(&mut deployment);
    // bob, an editor, adds his phone: no roster change, his user keeps its role.
    let phone = deployment.add_device("bob/phone", SUITE, 1).unwrap();
    let key_package = deployment.directory.fetch(&phone, SUITE).unwrap();
    let change = Change {
        add: vec![key_package],
        remove: vec![],
        roster: None,
    };
    let (accepted, results) = deployment.commit(&bob, &group, &change).unwrap();
    assert_eq!(accepted.added, vec![phone.clone()]);
    assert!(results.into_iter().all(|(_, result)| result.is_ok()));
    deployment.check_agreement(&group).unwrap();
    assert_eq!(
        deployment.validator.view(&group).unwrap().role_of(&phone),
        Some(Role::Editor)
    );

    // The phone removes the laptop.
    let change = Change {
        add: vec![],
        remove: vec![bob.clone()],
        roster: None,
    };
    deployment.commit(&phone, &group, &change).unwrap();
    deployment.check_agreement(&group).unwrap();
    assert!(
        !deployment
            .validator
            .view(&group)
            .unwrap()
            .members
            .contains(&bob)
    );
}

#[wasm_bindgen_test(unsupported = test)]
fn newcomers_join_with_the_servers_tree_and_reject_a_tampered_one() {
    let mut deployment = Deployment::new();
    let (group, alice, _bob, _carol) = three_member_group(&mut deployment);
    let dave = deployment.add_device("dave/tablet", SUITE, 1).unwrap();
    let key_package = deployment.directory.fetch(&dave, SUITE).unwrap();
    let mut roster = deployment.client(&alice).view(&group).unwrap().roster;
    roster.set(dave.user().clone(), Role::Viewer);
    let change = Change {
        add: vec![key_package],
        remove: vec![],
        roster: Some(roster),
    };
    let output = deployment
        .client_mut(&alice)
        .commit(&group, &change)
        .unwrap();
    deployment
        .validator
        .check_handshake(&alice, &output.commit)
        .unwrap();
    deployment
        .client_mut(&alice)
        .confirm_commit(&group)
        .unwrap();
    let welcome = output.welcome.unwrap();
    let tree = deployment.validator.ratchet_tree(&group).unwrap();

    // A server that changes one byte of the tree is caught: the tree no longer matches the hash the signed GroupInfo commits to.
    let mut tampered = tree.clone();
    let middle = tampered.len() / 2;
    tampered[middle] ^= 0x01;
    assert!(
        deployment
            .client_mut(&dave)
            .join(&welcome, &tampered)
            .is_err()
    );
    // A tree from another epoch is caught too.
    let mut other = Deployment::new();
    let (other_group, ..) = three_member_group(&mut other);
    let other_tree = other.validator.ratchet_tree(&other_group).unwrap();
    assert!(
        deployment
            .client_mut(&dave)
            .join(&welcome, &other_tree)
            .is_err()
    );

    // With the real tree, dave joins and reads updates.
    assert_eq!(
        deployment.client_mut(&dave).join(&welcome, &tree).unwrap(),
        group
    );
    let update = Update::new(UpdateKind::Content, payload(1_000, 7));
    let message = deployment.client_mut(&alice).send(&group, &update).unwrap();
    assert_eq!(
        deployment
            .client_mut(&dave)
            .receive(&group, &message)
            .unwrap(),
        Received::Update {
            sender: alice,
            update
        }
    );
}

#[wasm_bindgen_test(unsupported = test)]
fn late_message_from_previous_epoch() {
    let mut deployment = Deployment::new();
    let (group, alice, bob, carol) = three_member_group(&mut deployment);

    // bob encrypts an update, then a commit moves the group to the next epoch before it is relayed.
    let late = deployment
        .client_mut(&bob)
        .send(&group, &Update::new(UpdateKind::Content, payload(100, 3)))
        .unwrap();
    deployment.self_update(&alice, &group).unwrap();
    // Still a writer: the server relays it and clients can still decrypt it (they keep past epochs' keys).
    deployment.validator.check_application(&bob, &late).unwrap();
    assert!(matches!(
        deployment.client_mut(&carol).receive(&group, &late),
        Ok(Received::Update { .. })
    ));

    // After bob is demoted to viewer, his late messages are refused by the server and by clients.
    let late = deployment
        .client_mut(&bob)
        .send(&group, &Update::new(UpdateKind::Content, payload(100, 4)))
        .unwrap();
    let roster = deployment
        .client(&alice)
        .view(&group)
        .unwrap()
        .roster
        .with(bob.user().clone(), Role::Viewer);
    deployment.set_roles(&alice, &group, roster).unwrap();
    assert!(deployment.validator.check_application(&bob, &late).is_err());
    assert!(
        deployment
            .client_mut(&carol)
            .receive(&group, &late)
            .is_err()
    );

    // Messages from two epochs back are refused by the server.
    let old = deployment
        .client_mut(&alice)
        .send(&group, &Update::new(UpdateKind::Content, payload(100, 5)))
        .unwrap();
    deployment.self_update(&alice, &group).unwrap();
    deployment.self_update(&alice, &group).unwrap();
    assert!(matches!(
        deployment.validator.check_application(&alice, &old),
        Err(Rejection::WrongEpoch { .. })
    ));
}

#[wasm_bindgen_test(unsupported = test)]
fn group_of_one_hundred() {
    let mut deployment = Deployment::new();
    let owner = deployment.add_device("owner/laptop", SUITE, 0).unwrap();
    let group = deployment.create_group(&owner, SUITE).unwrap();
    let members: Vec<(DeviceId, Role)> = (1..100)
        .map(|index| {
            (
                deployment
                    .add_device(&format!("user{index}/laptop"), SUITE, 1)
                    .unwrap(),
                Role::Editor,
            )
        })
        .collect();
    deployment.add_members(&owner, &group, &members).unwrap();
    deployment.check_agreement(&group).unwrap();
    assert_eq!(
        deployment.validator.view(&group).unwrap().members.len(),
        100
    );
    let update = Update::new(UpdateKind::Content, payload(1_000, 9));
    let results = deployment.send(&members[42].0, &group, &update).unwrap();
    assert_eq!(results.len(), 99);
    assert!(results.into_iter().all(|(_, result)| result.is_ok()));
    deployment.self_update(&members[7].0, &group).unwrap();
    deployment.check_agreement(&group).unwrap();
}
