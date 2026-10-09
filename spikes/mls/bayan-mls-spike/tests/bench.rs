//! The measurements of AC-3 in WebAssembly. `cargo xtask mls-spike-bench node` and `cargo xtask mls-spike-bench chromium` run `bench_full`, which takes minutes; natively the `mls-bench` program measures the same.

#![cfg(target_arch = "wasm32")]

use bayan_mls_spike::bench::{self, BenchConfig};
use wasm_bindgen_test::{console_log, wasm_bindgen_test};

/// Where the measurement runs: Node.js's version, or the browser's user agent.
fn environment() -> String {
    let global = js_sys::global();
    let get = |object: &wasm_bindgen::JsValue, key: &str| {
        js_sys::Reflect::get(object, &key.into())
            .ok()
            .filter(|value| !value.is_undefined())
    };
    if let Some(version) = get(&global, "process")
        .and_then(|process| get(&process, "version"))
        .and_then(|value| value.as_string())
    {
        return format!("Node.js {version}");
    }
    get(&global, "navigator")
        .and_then(|navigator| get(&navigator, "userAgent"))
        .and_then(|value| value.as_string())
        .unwrap_or_else(|| "unknown host".to_owned())
}

#[wasm_bindgen_test]
#[ignore = "takes minutes; run it with `cargo xtask mls-spike-bench`"]
fn bench_full() {
    console_log!("WebAssembly in {}", environment());
    console_log!("| suite | members | metric | value | unit |");
    console_log!("|---|---|---|---|---|");
    bench::run(&BenchConfig::full(), &mut |measurement| {
        console_log!("{}", measurement.row())
    })
    .unwrap();
}
