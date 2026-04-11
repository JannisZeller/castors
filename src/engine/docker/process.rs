//! Subprocess helpers shared by the `castors` and `infra` submodules of the
//! Docker backend.
//!
//! Centralizing `docker` / `docker compose` spawning here means we have a
//! single `NotFound -> BackendUnavailable` conversion and a single place to
//! evolve if, e.g., a future flag lets us probe a `podman` fallback at
//! runtime. Nothing outside `engine::docker` should import this module.

use std::io;
use std::process::{Command, ExitStatus, Output, Stdio};

use crate::engine::types::EngineError;

/// Name of the CLI binary. A constant rather than a config knob for now; if
/// we ever need to support a custom path we can swap this for an injected
/// value.
pub const DOCKER_BIN: &str = "docker";

/// Runs `docker <args>` and captures stdout + stderr. Used for every
/// non-interactive docker call in the backend.
pub fn run_capture(args: &[String]) -> Result<Output, EngineError> {
    Command::new(DOCKER_BIN)
        .args(args)
        .output()
        .map_err(spawn_err)
}

/// Runs `docker <args>` with inherited stdio. Used only for interactive
/// subcommands (currently `docker exec -it`).
pub fn run_interactive(args: &[String]) -> Result<ExitStatus, EngineError> {
    Command::new(DOCKER_BIN)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(spawn_err)
}

fn spawn_err(err: io::Error) -> EngineError {
    if err.kind() == io::ErrorKind::NotFound {
        EngineError::BackendUnavailable("docker binary not found in PATH".into())
    } else {
        EngineError::Backend(format!("failed to spawn docker: {err}"))
    }
}

pub fn stdout_str(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr_str(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn display_exit(status: &ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "<signal>".to_owned(), |c| c.to_string())
}
