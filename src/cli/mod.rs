//! CLI surface for the `castors` binary.
//!
//! This module only parses user input and routes it to per-command handlers.
//! It intentionally has no knowledge of Docker or the registry yet.

mod add;
mod display;
mod exec;
mod infra;
mod list;
mod mitm;
mod prune;
mod restart;
mod rm;
#[cfg(test)]
mod test_helpers;

use clap::{Parser, Subcommand};

use add::AddArgs;
use exec::ExecArgs;
use infra::InfraArgs;
use mitm::MitmArgs;
use restart::RestartArgs;
use rm::RmArgs;

#[derive(Debug, Parser)]
#[command(name = "castors", about = "Run coding agents in isolated containers")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Add a new castor backed by a container image and a mounted directory.
    Add(AddArgs),
    /// Exec into the shell of an existing castor.
    Exec(ExecArgs),
    /// Shared infrastructure maintenance commands.
    Infra(InfraArgs),
    /// List all registered castors.
    List,
    /// MITM proxy maintenance commands.
    Mitm(MitmArgs),
    /// Recreate a castor from its current config.
    Restart(RestartArgs),
    /// Remove a castor.
    Rm(RmArgs),
    /// Remove all castors.
    Prune,
}

pub fn dispatch(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Add(args) => add::run(args),
        Command::Exec(args) => exec::run(args),
        Command::Infra(args) => infra::run(args),
        Command::List => list::run(),
        Command::Mitm(args) => mitm::run(args),
        Command::Restart(args) => restart::run(args),
        Command::Rm(args) => rm::run(args),
        Command::Prune => prune::run(),
    }
}
