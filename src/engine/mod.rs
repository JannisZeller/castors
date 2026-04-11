//! Backend-agnostic engine API.
//!
//! The rest of the CLI only talks to this module. It defines [`Engine`], the
//! trait every container backend must implement, plus a small factory
//! ([`current`]) that returns the active backend.
//!
//! Per [`docs/docker-backend.md`](../../../docs/docker-backend.md), call sites
//! must not import `std::process::Command` or talk to a runtime binary
//! directly; everything goes through this trait.

mod docker;
#[cfg(test)]
pub mod mock;
mod types;

use std::process::ExitStatus;

use crate::config::ResolvedConfig;
use crate::core::domain::CastorName;
use crate::core::registry::{CastorEntry, Registry};

pub use types::{CastorStatus, EngineError, ManagedContainer};

/// Container backend abstraction.
///
/// Implementors manage two related but distinct concerns: the per-castor
/// containers themselves and the shared infrastructure stack (proxy,
/// future monitoring, ...). Both live behind the same trait so the CLI never
/// has to know which runtime is in play.
pub trait Engine {
    // ---- castor lifecycle ----

    /// Creates the container for a castor and starts it in detached mode.
    ///
    /// `config` carries the resolved per-castor configuration: env vars to
    /// inject, allowed hosts, and secret header-injection rules for whichever
    /// proxy mode (`squid` or `mitm`) the backend is routing through. Backends
    /// consume what they support and ignore what they don't.
    ///
    /// # Errors
    /// Returns [`EngineError`] if the backend cannot create or start the
    /// container.
    fn create_and_start(
        &self,
        entry: &CastorEntry,
        config: &ResolvedConfig,
    ) -> Result<(), EngineError>;

    /// Opens an interactive shell inside the castor container.
    ///
    /// `shell` is an absolute path to a binary inside the container (for
    /// example `/bin/bash`). When `None`, the backend picks the first
    /// executable among `/bin/zsh`, `/bin/bash`, and `/bin/sh`.
    ///
    /// Returns the shell's [`ExitStatus`] so the CLI can propagate the inner
    /// exit code.
    ///
    /// # Errors
    /// Returns [`EngineError`] if no container exists or exec fails.
    fn exec_shell(&self, name: &CastorName, shell: Option<&str>)
    -> Result<ExitStatus, EngineError>;

    /// Stops and removes the container for a castor. Tolerant of missing
    /// containers, since external cleanup may have already removed them.
    ///
    /// # Errors
    /// Returns [`EngineError`] only for real backend failures.
    fn stop_and_remove(&self, name: &CastorName) -> Result<(), EngineError>;

    /// Reports the lifecycle state of a castor's container.
    ///
    /// `Missing` is returned as `Ok`, not as an error: callers branch on it.
    ///
    /// # Errors
    /// Returns [`EngineError`] only if the inspect call itself fails.
    fn inspect_status(&self, name: &CastorName) -> Result<CastorStatus, EngineError>;

    /// Lists all `castors`-managed containers known to the backend.
    ///
    /// # Errors
    /// Returns [`EngineError`] if the backend is unavailable.
    fn list_managed(&self) -> Result<Vec<ManagedContainer>, EngineError>;

    // ---- shared infrastructure lifecycle ----

    /// Ensures the shared infrastructure stack (proxy, ...) is running.
    /// Idempotent.
    ///
    /// # Errors
    /// Returns [`EngineError`] if the infra stack cannot be brought up.
    fn ensure_infra(&self, config: &ResolvedConfig) -> Result<(), EngineError>;

    /// Refreshes live proxy policy from the current registry state.
    ///
    /// # Errors
    /// Returns [`EngineError`] if backend-specific inspection or policy
    /// materialization fails.
    fn refresh_proxy_policy(&self, registry: &Registry) -> Result<(), EngineError>;

    /// Ensures the mitmproxy CA exists and returns the public CA certificate.
    ///
    /// # Errors
    /// Returns [`EngineError`] if the backend cannot initialize the CA volume
    /// or read the generated public certificate.
    fn export_mitm_ca_certificate(&self) -> Result<Vec<u8>, EngineError>;

    /// Tears down shared infra if no castor containers remain. Idempotent.
    ///
    /// # Errors
    /// Returns [`EngineError`] if teardown fails.
    fn teardown_infra_if_idle(&self) -> Result<(), EngineError>;
}

/// Returns the active engine for this process.
///
/// Today this is always the Docker backend. When alternatives land, selection
/// will happen here based on configuration; call sites do not need to change.
#[must_use]
pub fn current() -> Box<dyn Engine> {
    Box::new(docker::DockerEngine::new())
}
