//! Configuration schema, loading, merging, and identity resolution for castors.
//!
//! Two distinct documents live on disk:
//!
//! - **Global** — [`GlobalConfig`] at `~/.config/castors/config.yaml`,
//!   applies to every castor on this host. Carries cross-cutting `defaults`
//!   plus the shared `network` / `env` / `secrets` sections.
//! - **Project** — [`ProjectConfig`] at `<mount-dir>/.castors/config.yaml`,
//!   scoped to one workdir. Carries this castor's `castor` identity block
//!   plus its own copy of the shared sections. Optional.
//!
//! At `castors add` time the two layers feed into two independent
//! resolutions:
//!
//! 1. [`merge::merge`] combines the shared sections into a [`ResolvedConfig`]
//!    that the engine layer consumes.
//! 2. [`identity::resolve_identity`] picks the castor's name and image from
//!    the layer-specific blocks (and the CLI flags), with auto-naming as a
//!    fallback.
//!
//! ## Merge semantics (shared sections)
//! - `network.allowed_hosts`: append + dedupe (additive).
//! - `env`: per-key override, project beats global.
//! - `secrets`: per `(host, header)` override, project beats global.
//!
//! ## Update semantics
//! - MITM: `network.allowed_hosts` and `secrets` refresh for MITM castors when
//!   `castors infra refresh` runs or the registry changes (`castors add` /
//!   `rm` / `prune` rewrites `policy.json`).
//! - Squid: `squid.conf` is regenerated when infra is ensured and when the
//!   registry changes, or when `castors infra refresh` runs. The renderer
//!   includes per-workdir rules for registered Squid castors, and refresh asks a
//!   running Squid container to reconfigure.
//! - `env`, `network.proxy`, and the image are baked into the container at
//!   `castors add` / `castors restart <name>` time.
//!
//! ## Security note
//! Anything in `env` is readable by the agent inside the container; treat it
//! as documentation, not as a secret store.
//! True secrets belong in `secrets`, which proxies inject depending on mode;
//! see `docs/networking.md`.

pub mod identity;
pub mod load;
pub mod merge;
pub mod schema;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use thiserror::Error;

pub use identity::{IdentityError, resolve_identity};
pub use schema::{
    GlobalConfig, GlobalDefaults, Host, HostParseError, NetworkConfig, ProjectCastor,
    ProjectConfig, ProxyMode, SecretInjection, SecretSource, SecretSourceParseError,
};

/// Effective per-castor configuration produced by merging the global and
/// project documents. The engine layer only ever sees this shape.
///
/// Intentionally has no serde derives: a `ResolvedConfig` is an in-memory
/// product of merge, never a thing that should appear on disk.
///
/// Identity (`name`, `image`) is *not* part of `ResolvedConfig`; that lives
/// in [`crate::registry::CastorEntry`] because it's a property of the castor
/// itself, not of its runtime environment.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ResolvedConfig {
    pub allowed_hosts: Vec<Host>,
    pub env: BTreeMap<String, String>,
    pub proxy: ProxyMode,
    pub secrets: Vec<SecretInjection>,
}

/// Errors produced by config loading and merging.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config at {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse config at {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: yaml_serde::Error,
    },
    #[error("could not determine default global config path")]
    NoDefaultPath,
}

/// Load and merge the shared sections of the global and project configs for
/// a castor mounted at `mount_dir`. Either layer is allowed to be missing.
///
/// This does *not* perform identity resolution — call
/// [`resolve_identity`] separately for that.
///
/// # Errors
/// Returns a [`ConfigError`] if either file exists but cannot be read or
/// parsed, or if the global config path cannot be resolved on this platform.
pub fn resolve(mount_dir: &Path) -> Result<ResolvedConfig, ConfigError> {
    let global = load::load_global()?;
    let project = load::load_project(mount_dir)?;
    Ok(merge::merge(&global, &project))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// `resolve` is a thin wrapper over `load_*` + `merge`; both are covered
    /// in their own modules. We just make sure the wiring works end-to-end on
    /// a project-only config (no global file in scope under tempdir).
    #[test]
    fn resolve_picks_up_project_config_under_mount_dir() {
        let dir = tempdir().unwrap();
        let castors_dir = dir.path().join(load::PROJECT_CONFIG_DIR);
        fs::create_dir(&castors_dir).unwrap();
        fs::write(
            castors_dir.join(load::CONFIG_FILENAME),
            "network:\n  allowed_hosts:\n    - api.openai.com\nenv:\n  RUST_LOG: trace\n",
        )
        .unwrap();

        let resolved = resolve(dir.path()).unwrap();

        assert!(
            resolved
                .allowed_hosts
                .iter()
                .any(|h| h.as_str() == "api.openai.com")
        );
        assert_eq!(
            resolved.env.get("RUST_LOG").map(String::as_str),
            Some("trace")
        );
    }
}
