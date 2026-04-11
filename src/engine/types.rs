//! Backend-independent types used by the engine API.
//!
//! These types are deliberately *not* Docker-specific. They describe what the
//! CLI needs to know about a managed container, regardless of which runtime
//! actually executes it.

use thiserror::Error;

use crate::core::domain::CastorName;

/// Lifecycle state of a managed container as observed from a backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CastorStatus {
    /// Container exists and is currently running.
    Running,
    /// Container exists but has exited. Carries the last exit code if known.
    Exited { exit_code: Option<i32> },
    /// No container exists for this castor. The registry knows about it but
    /// the runtime does not.
    Missing,
}

/// Snapshot of a runtime container managed by `castors`. Returned by listing
/// queries so the CLI can join with registry entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedContainer {
    pub name: CastorName,
    pub status: CastorStatus,
}

/// Errors surfaced by the engine layer. Intentionally backend-agnostic; the
/// concrete failure message from the backend is captured in [`Self::Backend`].
#[derive(Debug, Error)]
pub enum EngineError {
    #[error("backend executable not available: {0}")]
    BackendUnavailable(String),
    #[error("no container found for castor '{0}'")]
    NotFound(CastorName),
    #[error("backend reported an error: {0}")]
    Backend(String),
}
