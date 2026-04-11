//! Core library for the `castors` CLI.

pub mod cli;
pub mod config;
pub mod core;
pub mod engine;
pub mod proxy;

/// Entry point for the CLI. Parses arguments and dispatches to handlers.
///
/// # Errors
/// Returns any error surfaced by a command handler.
pub fn run() -> anyhow::Result<()> {
    let parsed = <cli::Cli as clap::Parser>::parse();
    cli::dispatch(parsed)
}
