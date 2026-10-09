//! Saving a client's MLS state and loading it again, and the rule that nothing made with a provisional ciphersuite is ever saved.

use bayan_mls_spike::client::Received;
use bayan_mls_spike::persist::{self, PersistError};
use bayan_mls_spike::roster::Role;
use bayan_mls_spike::sim::Deployment;
use bayan_mls_spike::suite::Suite;
use bayan_mls_spike::update::{Update, UpdateKind};
use wasm_bindgen_test::wasm_bindgen_test;

#[wasm_bindgen_test(unsupported = test)]
fn a_restored_client_continues_in_its_groups() {
    let mut deployment = Deployment::new();
    let alice = deployment
        .add_device("alice/laptop", Suite::Classical, 1)
        .unwrap();
    let bob = deployment
        .add_device("bob/laptop", Suite::Classical, 1)
        .unwrap();
    let group = deployment.create_group(&alice, Suite::Classical).unwrap();
    deployment
        .add_members(&alice, &group, &[(bob.clone(), Role::Editor)])
        .unwrap();

    // bob's device saves its state and starts again from the file.
    let saved = persist::export(deployment.client(&bob)).unwrap();
    let restored = persist::import(&saved).unwrap();
    assert_eq!(restored.device(), &bob);
    assert_eq!(
        restored.view(&group).unwrap(),
        deployment.client(&bob).view(&group).unwrap()
    );
    *deployment.client_mut(&bob) = restored;

    // It receives, sends and follows commits as before.
    let update = Update::new(UpdateKind::Content, b"after restart".to_vec());
    let results = deployment.send(&alice, &group, &update).unwrap();
    assert_eq!(
        results[0].1.as_ref().unwrap(),
        &Received::Update {
            sender: alice.clone(),
            update
        }
    );
    deployment
        .send(
            &bob,
            &group,
            &Update::new(UpdateKind::Content, b"from the restored device".to_vec()),
        )
        .unwrap();
    deployment.self_update(&bob, &group).unwrap();
    deployment.self_update(&alice, &group).unwrap();
    deployment.check_agreement(&group).unwrap();
}

#[wasm_bindgen_test(unsupported = test)]
fn malformed_saved_states_are_refused() {
    let mut deployment = Deployment::new();
    let alice = deployment
        .add_device("alice/laptop", Suite::Classical, 1)
        .unwrap();
    deployment.create_group(&alice, Suite::Classical).unwrap();
    let saved = persist::export(deployment.client(&alice)).unwrap();
    for length in (0..saved.len()).step_by(97) {
        assert!(
            persist::import(&saved[..length]).is_err(),
            "a truncated state loaded"
        );
    }
    let mut trailing = saved.clone();
    trailing.push(0);
    assert_eq!(
        persist::import(&trailing).err(),
        Some(PersistError::Malformed)
    );
    let mut version = saved;
    version[8] = 2;
    assert_eq!(
        persist::import(&version).err(),
        Some(PersistError::Malformed)
    );
}

#[cfg(feature = "provisional-pq")]
#[wasm_bindgen_test(unsupported = test)]
fn provisional_suites_are_never_saved() {
    let mut deployment = Deployment::new();
    let alice = deployment
        .add_device("alice/laptop", Suite::ProvisionalHybridPq, 0)
        .unwrap();
    deployment
        .create_group(&alice, Suite::ProvisionalHybridPq)
        .unwrap();
    assert_eq!(
        persist::export(deployment.client(&alice)).err(),
        Some(PersistError::ProvisionalSuite)
    );
}

#[cfg(feature = "provisional-pq")]
#[wasm_bindgen_test(unsupported = test)]
fn provisional_key_packages_are_never_saved() {
    // alice's only group uses the classical suite, but she has also published a key package for the provisional suite, whose private keys sit in her storage.
    let mut deployment = Deployment::new();
    let alice = deployment
        .add_device("alice/laptop", Suite::Classical, 1)
        .unwrap();
    deployment
        .publish_key_packages(&alice, Suite::ProvisionalHybridPq, 1)
        .unwrap();
    deployment.create_group(&alice, Suite::Classical).unwrap();
    assert_eq!(
        persist::export(deployment.client(&alice)).err(),
        Some(PersistError::ProvisionalSuite)
    );
    // A device without provisional key material is saved as before.
    let bob = deployment
        .add_device("bob/laptop", Suite::Classical, 1)
        .unwrap();
    assert!(persist::export(deployment.client(&bob)).is_ok());
}
