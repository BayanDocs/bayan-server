//! AC-4: a member who joins later reads the current document from an encrypted snapshot whose key travels through the group.

use bayan_mls_spike::client::{ClientError, Received};
use bayan_mls_spike::identity::DeviceId;
use bayan_mls_spike::policy::PolicyViolation;
use bayan_mls_spike::roster::Role;
use bayan_mls_spike::sim::{Deliveries, Deployment};
use bayan_mls_spike::snapshot::{self, SnapshotError, SnapshotKey};
use bayan_mls_spike::suite::Suite;
use bayan_mls_spike::update::{Update, UpdateKind};
use openmls_traits::OpenMlsProvider as _;
use wasm_bindgen_test::wasm_bindgen_test;

const SUITE: Suite = Suite::Classical;

/// The document's current state, standing in for a CRDT snapshot (with no history: threat T10).
fn document_state() -> Vec<u8> {
    b"Chapter 1. It was a bright cold day in April. "
        .iter()
        .copied()
        .cycle()
        .take(50_000)
        .collect()
}

/// alice (owner) and bob (editor) have been editing; then alice adds dave (viewer).
fn group_with_newcomer() -> (Deployment, Vec<u8>, DeviceId, DeviceId, DeviceId) {
    let mut deployment = Deployment::new();
    let alice = deployment
        .add_device("alice/laptop", SUITE, 1)
        .expect("set up the group");
    let bob = deployment
        .add_device("bob/laptop", SUITE, 1)
        .expect("set up the group");
    let dave = deployment
        .add_device("dave/tablet", SUITE, 1)
        .expect("set up the group");
    let group = deployment
        .create_group(&alice, SUITE)
        .expect("set up the group");
    deployment
        .add_members(&alice, &group, &[(bob.clone(), Role::Editor)])
        .expect("set up the group");
    deployment
        .send(
            &bob,
            &group,
            &Update::new(
                UpdateKind::Content,
                b"history the newcomer must not see".to_vec(),
            ),
        )
        .expect("set up the group");
    deployment
        .add_members(&alice, &group, &[(dave.clone(), Role::Viewer)])
        .expect("set up the group");
    (deployment, group, alice, bob, dave)
}

/// `sharer` seals the state, uploads the blob and sends the key to the group. Returns the key message as each recipient received it.
fn share_snapshot(
    deployment: &mut Deployment,
    sharer: &DeviceId,
    group: &[u8],
    state: &[u8],
) -> (Vec<u8>, Deliveries) {
    let provider = deployment.client(sharer).provider();
    let (sealed, key) = snapshot::seal(provider.crypto(), provider.rand(), group, SUITE, state)
        .expect("share a snapshot");
    deployment.blobs.insert(key.id.to_vec(), sealed.blob);
    let update = Update::new(
        UpdateKind::SnapshotKey,
        key.to_bytes().expect("share a snapshot"),
    );
    let results = deployment
        .send(sharer, group, &update)
        .expect("share a snapshot");
    (key.id.to_vec(), results)
}

fn received_key(results: &Deliveries, device: &DeviceId) -> SnapshotKey {
    let (_, result) = results
        .iter()
        .find(|(recipient, _)| recipient == device)
        .expect("the key message");
    let Ok(Received::Update { update, .. }) = result else {
        panic!("{device} did not accept the key message: {result:?}")
    };
    assert_eq!(update.kind, UpdateKind::SnapshotKey);
    SnapshotKey::from_bytes(&update.body).expect("the key message")
}

#[wasm_bindgen_test(unsupported = test)]
fn newcomer_reads_the_current_state_from_a_snapshot() {
    let (mut deployment, group, alice, _bob, dave) = group_with_newcomer();
    // dave cannot read anything sent before he joined (forward secrecy)...
    let state = document_state();
    let (id, results) = share_snapshot(&mut deployment, &alice, &group, &state);
    // ...but the key message reaches him, and the blob decrypts to the current state.
    let key = received_key(&results, &dave);
    let blob = deployment.blobs.get(&id).unwrap();
    let crypto = deployment.client(&dave).provider().crypto();
    assert_eq!(snapshot::open(crypto, &key, &group, blob).unwrap(), state);
    // The server holds only ciphertext: the state does not appear in the blob.
    assert!(!blob.windows(16).any(|window| window == &state[..16]));
}

#[wasm_bindgen_test(unsupported = test)]
fn tampered_or_swapped_blobs_are_refused() {
    let (mut deployment, group, alice, bob, dave) = group_with_newcomer();
    let (id, results) = share_snapshot(&mut deployment, &alice, &group, &document_state());
    let key = received_key(&results, &dave);
    let crypto = deployment.client(&dave).provider().crypto();
    let blob = deployment.blobs.get(&id).unwrap().clone();

    // One changed byte anywhere: refused before decryption.
    for position in [0, 16, 20, 40, blob.len() - 1] {
        let mut tampered = blob.clone();
        tampered[position] ^= 0x01;
        assert_eq!(
            snapshot::open(crypto, &key, &group, &tampered),
            Err(SnapshotError::WrongBlob)
        );
    }
    // Another snapshot's blob: refused.
    let (other_id, _) = share_snapshot(&mut deployment, &bob, &group, b"another state");
    let other = deployment.blobs.get(&other_id).unwrap().clone();
    let crypto = deployment.client(&dave).provider().crypto();
    assert_eq!(
        snapshot::open(crypto, &key, &group, &other),
        Err(SnapshotError::WrongBlob)
    );
    // The right blob presented as another document's: the associated data does not match.
    assert_eq!(
        snapshot::open(crypto, &key, b"another-document", &blob),
        Err(SnapshotError::Malformed)
    );
    // A key message whose hash names the blob but whose key is wrong: refused.
    let mut wrong_key = key.clone();
    wrong_key.key[0] ^= 0x01;
    assert_eq!(
        snapshot::open(crypto, &wrong_key, &group, &blob),
        Err(SnapshotError::Malformed)
    );
    // Malformed key messages.
    for length in 0..key.to_bytes().unwrap().len() {
        assert!(SnapshotKey::from_bytes(&key.to_bytes().unwrap()[..length]).is_err());
    }
}

#[wasm_bindgen_test(unsupported = test)]
fn only_members_who_may_edit_can_share_snapshots() {
    let (mut deployment, group, alice, _bob, _dave) = group_with_newcomer();
    // A commenter or viewer could otherwise plant a forged "current state" for newcomers.
    let erin = deployment.add_device("erin/phone", SUITE, 1).unwrap();
    deployment
        .add_members(&alice, &group, &[(erin.clone(), Role::Commenter)])
        .unwrap();
    let provider = deployment.client(&erin).provider();
    let (_, key) =
        snapshot::seal(provider.crypto(), provider.rand(), &group, SUITE, b"forged").unwrap();
    let update = Update::new(UpdateKind::SnapshotKey, key.to_bytes().unwrap());
    assert!(deployment.client_mut(&erin).send(&group, &update).is_err());
    let message = deployment
        .client_mut(&erin)
        .send_unchecked(&group, &update)
        .unwrap();
    // The server relays it (a commenter may send), and every client refuses it.
    for (device, result) in deployment.deliver(&erin, &group, &message).unwrap() {
        assert!(
            matches!(
                result,
                Err(ClientError::Policy(PolicyViolation::NotAllowed { .. }))
            ),
            "{device} accepted a commenter's snapshot"
        );
    }
}

#[wasm_bindgen_test(unsupported = test)]
fn removed_members_get_no_later_snapshot_keys() {
    let (mut deployment, group, alice, bob, dave) = group_with_newcomer();
    deployment
        .remove_members(&alice, &group, std::slice::from_ref(&dave))
        .unwrap();
    let provider = deployment.client(&alice).provider();
    let (_, key) = snapshot::seal(
        provider.crypto(),
        provider.rand(),
        &group,
        SUITE,
        b"after dave left",
    )
    .unwrap();
    let message = deployment
        .client_mut(&alice)
        .send(
            &group,
            &Update::new(UpdateKind::SnapshotKey, key.to_bytes().unwrap()),
        )
        .unwrap();
    // Even if the server hands dave the key message, he cannot decrypt it.
    assert!(
        deployment
            .client_mut(&dave)
            .receive(&group, &message)
            .is_err()
    );
    assert!(matches!(
        deployment.client_mut(&bob).receive(&group, &message),
        Ok(Received::Update { .. })
    ));
}
