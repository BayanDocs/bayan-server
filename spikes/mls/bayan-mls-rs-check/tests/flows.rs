//! The fallback check, natively and in WebAssembly (Node.js and Chromium through `cargo xtask mls-spike-wasm`).

use bayan_mls_rs_check::run_flows;
use wasm_bindgen_test::wasm_bindgen_test;

#[wasm_bindgen_test(unsupported = test)]
fn mls_rs_supports_the_same_flows() {
    let observed = run_flows().unwrap();
    // Three commits (add, self-update, remove), each followed by the server: alice (leaf 0) added two members and set the roster (3 proposals), bob (leaf 1) committed no proposals, alice removed carol and set the roster (2 proposals).
    assert_eq!(
        observed.commits_seen_by_server,
        vec![(0, 3), (1, 0), (0, 2)]
    );
    assert_eq!(observed.epoch, 3);
    // 4 sizes × 2 recipients, and one after the removal.
    assert_eq!(observed.updates_delivered, 9);
    assert_eq!(
        observed.server_members,
        vec!["alice".to_owned(), "bob".to_owned()]
    );
}
