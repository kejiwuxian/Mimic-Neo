//! sai-recorder — opt-in record-and-replay workflow capture with token-efficient
//! compression, MimicCLI-compatible output (`types/actions.d.ts`), for the
//! Simular Sai agent and computer-use dataset collection.
//!
//! Pipeline: capture(ring buffer) → state machine → before/after captures →
//! compress → export. An on-screen overlay controls start/stop.

mod actions;
mod capture;
mod cli;
mod compress;
mod export;
mod overlay;
mod recorder;
mod review;
mod telegram;

use clap::Parser;

use cli::{Cli, Command};

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Record(args) => recorder::run(args.into_options()),
    };
    if let Err(e) = result {
        eprintln!("error: {e:?}");
        std::process::exit(1);
    }
}
