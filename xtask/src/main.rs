//! Repository automation: `cargo xtask <command>`.

mod dist;
mod perf;

use anyhow::{Result, bail};

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("dist") => dist::dist(dist::Options::parse(args)?),
        Some("perf") => perf::perf(perf::Options::parse(args)?),
        Some("-h" | "--help") | None => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}

const USAGE: &str = "usage: cargo xtask <command>

commands:
  dist [--target <triple>] [--version <version>]
      Build `nexus` in release mode and package it into dist/ (archive,
      checksums and the native installers the host OS can build).
  perf [--runs <n>] [--out <dir>] [--no-build] [--no-xvfb]
      Measure cold start, frame time, idle CPU and memory of `nexus` in release
      mode under Xvfb and fail when a budget in perf-budgets.toml is exceeded.";
