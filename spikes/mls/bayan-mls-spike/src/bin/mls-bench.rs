//! Prints the measurements of AC-3 as Markdown table rows (REPORT.md has the commands and the results).
//!
//! `cargo run --release --locked --all-features -p bayan-mls-spike --bin mls-bench` measures every suite at 2, 10, 100 and 1,000 members; `--quick` runs the small smoke configuration. OpenMLS spreads parts of a commit over all processor cores natively; set `RAYON_NUM_THREADS=1` to measure on one core, as WebAssembly runs.

#![expect(
    clippy::print_stdout,
    reason = "a benchmark program reports on standard output"
)]
#![expect(
    clippy::print_stderr,
    reason = "a benchmark program reports errors on standard error"
)]

use std::process::ExitCode;

use bayan_mls_spike::bench::{self, BenchConfig};

fn main() -> ExitCode {
    let config = if std::env::args().any(|argument| argument == "--quick") {
        BenchConfig::smoke()
    } else {
        BenchConfig::full()
    };
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let rayon = std::env::var("RAYON_NUM_THREADS").unwrap_or_else(|_| "unset".to_owned());
    println!(
        "Native ({}, {threads} hardware threads, RAYON_NUM_THREADS={rayon})",
        std::env::consts::ARCH
    );
    println!();
    println!("| suite | members | metric | value | unit |");
    println!("|---|---|---|---|---|");
    match bench::run(&config, &mut |measurement| {
        println!("{}", measurement.row());
    }) {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mls-bench: {error}");
            ExitCode::FAILURE
        }
    }
}
