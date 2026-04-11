//! Per-castor container lifecycle (Docker backend).
//!
//! These free functions are the implementation of the castor-related methods
//! on [`crate::engine::Engine`]. The trait `impl` in `engine::docker::mod`
//! delegates here.
//!
//! This is the only place in the crate that spawns `docker` subprocesses
//! against single containers; argv construction lives in [`cmd`] and output
//! parsing lives in [`status`], both of which are pure and unit-tested.

mod cmd;
mod status;

use std::process::ExitStatus;

use crate::config::ResolvedConfig;
use crate::core::domain::CastorName;
use crate::core::registry::CastorEntry;
use crate::engine::docker::process::{
    display_exit, run_capture, run_interactive, stderr_str, stdout_str,
};
use crate::engine::types::{CastorStatus, EngineError, ManagedContainer};

/// Returns the canonical Docker container name for a given castor.
#[must_use]
pub fn container_name(name: &CastorName) -> String {
    format!("castor-{}", name.as_str())
}

pub fn create_and_start(entry: &CastorEntry, config: &ResolvedConfig) -> Result<(), EngineError> {
    let output = run_capture(&cmd::run_args(entry, config))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(EngineError::Backend(format!(
            "docker run failed (exit {}): {}",
            display_exit(&output.status),
            stderr_str(&output).trim()
        )))
    }
}

pub fn exec_shell(name: &CastorName, shell: Option<&str>) -> Result<ExitStatus, EngineError> {
    match inspect_status(name)? {
        CastorStatus::Missing => return Err(EngineError::NotFound(name.clone())),
        CastorStatus::Exited { .. } => {
            let started = run_capture(&cmd::start_args(name))?;
            if !started.status.success() {
                return Err(EngineError::Backend(format!(
                    "docker start failed: {}",
                    stderr_str(&started).trim()
                )));
            }
        }
        CastorStatus::Running => {}
    }
    run_interactive(&cmd::exec_args(name, shell))
}

pub fn stop_and_remove(name: &CastorName) -> Result<(), EngineError> {
    let output = run_capture(&cmd::rm_force_args(name))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = stderr_str(&output);
    // tolerate the case where the container is already gone
    if stderr.contains("No such container") {
        return Ok(());
    }
    Err(EngineError::Backend(format!(
        "docker rm -f failed: {}",
        stderr.trim()
    )))
}

pub fn inspect_status(name: &CastorName) -> Result<CastorStatus, EngineError> {
    let output = run_capture(&cmd::inspect_args(name))?;
    if output.status.success() {
        return Ok(status::parse_inspect_status(&stdout_str(&output)));
    }
    let stderr = stderr_str(&output);
    if stderr.contains("No such object") || stderr.contains("No such container") {
        return Ok(CastorStatus::Missing);
    }
    Err(EngineError::Backend(format!(
        "docker inspect failed: {}",
        stderr.trim()
    )))
}

pub fn list_managed() -> Result<Vec<ManagedContainer>, EngineError> {
    let output = run_capture(&cmd::list_args())?;
    if !output.status.success() {
        return Err(EngineError::Backend(format!(
            "docker ps failed: {}",
            stderr_str(&output).trim()
        )));
    }
    Ok(stdout_str(&output)
        .lines()
        .filter_map(status::parse_list_line)
        .collect())
}
