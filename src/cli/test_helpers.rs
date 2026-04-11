//! Shared fixtures for CLI handler tests.
//!
//! Each `cli/*.rs` file owns its own `mod tests`; this module exists so the
//! arrange-step of those tests does not have to duplicate temp-registry and
//! sample-entry boilerplate.

use chrono::{DateTime, Utc};
use std::str::FromStr;
use tempfile::TempDir;

use crate::core::domain::{CastorName, ImageTag, MountDir};
use crate::core::registry::{CastorEntry, Registry};

/// Returns an empty registry backed by a fresh temp directory.
///
/// The [`TempDir`] must be kept alive (e.g. bound to `_tmp`) for the duration
/// of the test, otherwise the backing files are removed mid-run.
pub fn fresh_registry() -> (TempDir, Registry) {
    let tmp = TempDir::new().expect("create tempdir");
    let path = tmp.path().join("registry.json");
    let registry = Registry::load_from(path).expect("load empty registry");
    (tmp, registry)
}

/// Builds a [`CastorEntry`] with a deterministic timestamp for assertions.
pub fn sample_entry(name: &str, image: &str, dir: &str) -> CastorEntry {
    CastorEntry {
        name: cn(name),
        image: ImageTag::from_str(image).expect("valid image tag"),
        mount_dir: MountDir::from_str(dir).expect("valid mount dir"),
        created_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
    }
}

/// Short alias for `CastorName::from_str(s).unwrap()`, used heavily in tests.
pub fn cn(s: &str) -> CastorName {
    CastorName::from_str(s).expect("valid castor name")
}
