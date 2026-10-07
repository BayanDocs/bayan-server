//! Group re-creation for ciphersuite migration: a new group with the same members and roles, announced by an owner and checked by every member.

use bayan_mls_spike::client::{Change, Received};
use bayan_mls_spike::recreate::{self, GroupMoved, MoveError};
use bayan_mls_spike::roster::Role;
use bayan_mls_spike::sim::Deployment;
use bayan_mls_spike::suite::Suite;
use bayan_mls_spike::update::{Update, UpdateKind};
use wasm_bindgen_test::wasm_bindgen_test;

/// The migrations this build can test: to the second classical suite, and with the `provisional-pq` feature to the provisional hybrid suite.
fn targets() -> Vec<Suite> {
    Suite::all()
        .iter()
        .copied()
        .filter(|suite| *suite != Suite::Classical)
        .collect()
}

#[wasm_bindgen_test(unsupported = test)]
fn a_group_moves_to_a_new_suite_with_the_same_members_and_roles() {
    for target in targets() {
        let mut deployment = Deployment::new();
        let alice = deployment
            .add_device("alice/laptop", Suite::Classical, 1)
            .unwrap();
        let bob = deployment
            .add_device("bob/laptop", Suite::Classical, 1)
            .unwrap();
        let bob_phone = deployment
            .add_device("bob/phone", Suite::Classical, 1)
            .unwrap();
        let carol = deployment
            .add_device("carol/phone", Suite::Classical, 1)
            .unwrap();
        let old = deployment.create_group(&alice, Suite::Classical).unwrap();
        deployment
            .add_members(
                &alice,
                &old,
                &[
                    (bob.clone(), Role::Editor),
                    (bob_phone.clone(), Role::Editor),
                    (carol.clone(), Role::Viewer),
                ],
            )
            .unwrap();
        // Every device publishes key packages for the new suite (its client must support it).
        for device in [&bob, &bob_phone, &carol] {
            deployment.publish_key_packages(device, target, 1).unwrap();
        }
        let new = deployment.recreate_group(&alice, &old, target).unwrap();
        assert_ne!(new, old);
        assert_eq!(
            deployment.validator.view(&new),
            deployment.validator.view(&old)
        );
        deployment.check_agreement(&new).unwrap();
        for device in [&alice, &bob, &bob_phone, &carol] {
            assert_eq!(
                deployment.client(device).suite(&new).unwrap(),
                target,
                "{}",
                target.label()
            );
        }
        // Work continues in the new group, which keys from the old group cannot read.
        let update = Update::new(UpdateKind::Content, b"after the move".to_vec());
        let message = deployment.client_mut(&bob).send(&new, &update).unwrap();
        assert!(
            deployment
                .client_mut(&carol)
                .receive(&old, &message)
                .is_err()
        );
        assert_eq!(
            deployment
                .client_mut(&carol)
                .receive(&new, &message)
                .unwrap(),
            Received::Update {
                sender: bob.clone(),
                update
            }
        );
    }
}

#[wasm_bindgen_test(unsupported = test)]
fn members_refuse_a_move_that_changes_membership() {
    let mut deployment = Deployment::new();
    let alice = deployment
        .add_device("alice/laptop", Suite::Classical, 1)
        .unwrap();
    let bob = deployment
        .add_device("bob/laptop", Suite::Classical, 1)
        .unwrap();
    let mallory = deployment
        .add_device("mallory/laptop", Suite::ClassicalChaCha, 1)
        .unwrap();
    let old = deployment.create_group(&alice, Suite::Classical).unwrap();
    deployment
        .add_members(&alice, &old, &[(bob.clone(), Role::Editor)])
        .unwrap();
    deployment
        .publish_key_packages(&bob, Suite::ClassicalChaCha, 1)
        .unwrap();

    // The owner quietly adds mallory to the new group.
    let new = deployment
        .create_group(&alice, Suite::ClassicalChaCha)
        .unwrap();
    let roster = deployment
        .validator
        .view(&old)
        .unwrap()
        .roster
        .clone()
        .with(mallory.user().clone(), Role::Viewer);
    let change = Change {
        add: vec![
            deployment
                .directory
                .fetch(&bob, Suite::ClassicalChaCha)
                .unwrap(),
            deployment
                .directory
                .fetch(&mallory, Suite::ClassicalChaCha)
                .unwrap(),
        ],
        remove: vec![],
        roster: Some(roster),
    };
    deployment.commit(&alice, &new, &change).unwrap();
    let moved = GroupMoved {
        new_group_id: new.clone(),
        suite: Suite::ClassicalChaCha,
    };
    assert!(matches!(
        recreate::verify_move(deployment.client(&bob), &old, &moved),
        Err(MoveError::MembershipChanged)
    ));
    // An announcement naming the wrong suite is refused too.
    let wrong_suite = GroupMoved {
        new_group_id: new,
        suite: Suite::Classical,
    };
    assert!(matches!(
        recreate::verify_move(deployment.client(&bob), &old, &wrong_suite),
        Err(MoveError::WrongSuite)
    ));
}

#[wasm_bindgen_test(unsupported = test)]
fn move_announcements_are_parsed_strictly() {
    let moved = GroupMoved {
        new_group_id: vec![7; 16],
        suite: Suite::ClassicalChaCha,
    };
    let bytes = moved.to_bytes().unwrap();
    assert_eq!(GroupMoved::from_bytes(&bytes).unwrap(), moved);
    for length in 0..bytes.len() {
        assert!(GroupMoved::from_bytes(&bytes[..length]).is_err());
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(GroupMoved::from_bytes(&trailing).is_err());
    let mut unknown_suite = bytes;
    let last = unknown_suite.len() - 1;
    unknown_suite[last] = 0x99;
    assert!(GroupMoved::from_bytes(&unknown_suite).is_err());
}
