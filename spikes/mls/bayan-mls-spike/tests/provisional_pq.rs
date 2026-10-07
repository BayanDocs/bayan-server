//! The flows with the PROVISIONAL hybrid post-quantum suite (draft-ietf-mls-pq-ciphersuites, ML-KEM-768 + X25519, code point 0x004F). Measurement only: groups with it are never persisted (see tests/persistence.rs).

#![cfg(feature = "provisional-pq")]

use bayan_mls_spike::client::Received;
use bayan_mls_spike::roster::Role;
use bayan_mls_spike::sim::Deployment;
use bayan_mls_spike::suite::Suite;
use bayan_mls_spike::update::{Update, UpdateKind};
use wasm_bindgen_test::wasm_bindgen_test;

const SUITE: Suite = Suite::ProvisionalHybridPq;

#[wasm_bindgen_test(unsupported = test)]
fn every_flow_works_with_the_provisional_hybrid_suite() {
    assert!(SUITE.is_provisional());
    let mut deployment = Deployment::new();
    let alice = deployment.add_device("alice/laptop", SUITE, 1).unwrap();
    let bob = deployment.add_device("bob/laptop", SUITE, 1).unwrap();
    let carol = deployment.add_device("carol/phone", SUITE, 1).unwrap();
    let group = deployment.create_group(&alice, SUITE).unwrap();
    deployment
        .add_members(
            &alice,
            &group,
            &[
                (bob.clone(), Role::Editor),
                (carol.clone(), Role::Commenter),
            ],
        )
        .unwrap();
    deployment.self_update(&bob, &group).unwrap();
    for size in [100, 1_000, 10_000, 100_000] {
        let update = Update::new(UpdateKind::Content, vec![0x5a; size]);
        for (_, result) in deployment.send(&bob, &group, &update).unwrap() {
            assert_eq!(
                result.unwrap(),
                Received::Update {
                    sender: bob.clone(),
                    update: update.clone()
                }
            );
        }
    }
    deployment.remove_members(&alice, &group, &[carol]).unwrap();
    deployment.check_agreement(&group).unwrap();
    assert_eq!(deployment.client(&alice).suite(&group).unwrap(), SUITE);
}
