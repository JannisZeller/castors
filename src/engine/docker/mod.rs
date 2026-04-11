//! Docker backend for the engine API.
//!
//! All docker-touching code in the codebase lives under this module. Nothing
//! else should import `std::process::Command` or shell out to `docker` /
//! `docker compose`.
//!
//! The module is split by lifecycle concern:
//!
//! - [`castors`] — per-castor container lifecycle (run, exec, stop, inspect, list)
//! - [`infra`] — shared infrastructure stack (proxy, future monitoring) via compose
//! - [`labels`] — label keys shared by both submodules

mod castors;
mod infra;
mod labels;
mod process;

use std::process::ExitStatus;

use crate::config::ResolvedConfig;
use crate::core::domain::CastorName;
use crate::core::registry::CastorEntry;
use crate::engine::Engine;
use crate::engine::types::{CastorStatus, EngineError, ManagedContainer};

/// Fixed network name declared by the shared infra Compose template.
const SHARED_NETWORK_NAME: &str = "castors-shared";

/// Docker-backed implementation of [`Engine`].
///
/// Currently stateless. Will gain configuration (binary path, compose project
/// name, ...) when those become user-facing.
#[derive(Debug, Default)]
pub struct DockerEngine;

impl DockerEngine {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Engine for DockerEngine {
    fn create_and_start(
        &self,
        entry: &CastorEntry,
        config: &ResolvedConfig,
    ) -> Result<(), EngineError> {
        castors::create_and_start(entry, config)
    }

    fn exec_shell(
        &self,
        name: &CastorName,
        shell: Option<&str>,
    ) -> Result<ExitStatus, EngineError> {
        castors::exec_shell(name, shell)
    }

    fn stop_and_remove(&self, name: &CastorName) -> Result<(), EngineError> {
        castors::stop_and_remove(name)
    }

    fn inspect_status(&self, name: &CastorName) -> Result<CastorStatus, EngineError> {
        castors::inspect_status(name)
    }

    fn list_managed(&self) -> Result<Vec<ManagedContainer>, EngineError> {
        castors::list_managed()
    }

    fn ensure_infra(&self, config: &ResolvedConfig) -> Result<(), EngineError> {
        infra::ensure_running(config.proxy)
    }

    fn refresh_proxy_policy(
        &self,
        registry: &crate::core::registry::Registry,
    ) -> Result<(), EngineError> {
        infra::refresh_proxy_policy(registry)
    }

    fn export_mitm_ca_certificate(&self) -> Result<Vec<u8>, EngineError> {
        infra::export_mitm_ca_certificate()
    }

    /// Queries `list_managed` and, if it comes back empty, delegates to
    /// [`infra::teardown`]. The "if idle" check lives here (not inside
    /// `infra`) so the `infra` submodule stays oblivious to castor-level
    /// state — it only knows how to bring its own stack up and down.
    fn teardown_infra_if_idle(&self) -> Result<(), EngineError> {
        if self.list_managed()?.is_empty() {
            infra::teardown()
        } else {
            Ok(())
        }
    }
}
