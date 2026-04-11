//! Persisted registry of known castors.
//!
//! The registry is a single JSON file on disk holding one entry per castor.
//! It is intentionally simple: no migrations, no indexes, just a map keyed
//! by [`CastorName`]. The file lives under the user's castors state directory
//! by default, with an override for tests.
//!
//! The type owns its on-disk path so that `save()` does not need the caller
//! to thread the path through every call site.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::core::domain::{CastorName, ImageTag, MountDir};

/// Metadata persisted for each castor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CastorEntry {
    pub name: CastorName,
    pub image: ImageTag,
    pub mount_dir: MountDir,
    pub created_at: DateTime<Utc>,
}

/// Errors produced by registry operations.
#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("castor {0} already exists")]
    AlreadyExists(CastorName),
    #[error("castor {0} not found")]
    NotFound(CastorName),
    #[error("failed to read registry at {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to write registry at {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to parse registry at {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to serialize registry")]
    Serialize(#[source] serde_json::Error),
    #[error("could not determine default registry path")]
    NoDefaultPath,
}

/// Persistent map of known castors, backed by a JSON file on disk.
#[derive(Debug)]
pub struct Registry {
    path: PathBuf,
    state: RegistryState,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryState {
    #[serde(default)]
    castors: BTreeMap<CastorName, CastorEntry>,
}

impl Registry {
    /// Loads the registry from the default generated state path.
    ///
    /// Creates an empty registry in memory if no file exists yet. The file
    /// itself is not created until [`save`](Self::save) is called.
    ///
    /// # Errors
    /// Returns an error if the file exists but cannot be read or parsed.
    pub fn load() -> Result<Self, RegistryError> {
        let path = default_path()?;
        Self::load_from(path)
    }

    /// Loads the registry from a specific path. Intended for tests and for
    /// callers who manage the storage location explicitly.
    ///
    /// # Errors
    /// Returns an error if the file exists but cannot be read or parsed.
    pub fn load_from(path: impl Into<PathBuf>) -> Result<Self, RegistryError> {
        let path = path.into();
        let state = match fs::read_to_string(&path) {
            Ok(contents) => {
                serde_json::from_str(&contents).map_err(|source| RegistryError::Parse {
                    path: path.clone(),
                    source,
                })?
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => RegistryState::default(),
            Err(source) => {
                return Err(RegistryError::Read {
                    path: path.clone(),
                    source,
                });
            }
        };
        Ok(Self { path, state })
    }

    /// Persists the registry to its backing file, creating parent directories
    /// as needed. Writes atomically via a temporary file to avoid leaving a
    /// corrupt registry on disk if the process is interrupted.
    ///
    /// # Errors
    /// Returns an error if serialization or any filesystem operation fails.
    pub fn save(&self) -> Result<(), RegistryError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| RegistryError::Write {
                path: self.path.clone(),
                source,
            })?;
        }

        let serialized =
            serde_json::to_vec_pretty(&self.state).map_err(RegistryError::Serialize)?;

        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, &serialized).map_err(|source| RegistryError::Write {
            path: tmp.clone(),
            source,
        })?;
        fs::rename(&tmp, &self.path).map_err(|source| RegistryError::Write {
            path: self.path.clone(),
            source,
        })?;
        Ok(())
    }

    /// Inserts a new castor, failing if the name is already taken.
    ///
    /// # Errors
    /// Returns [`RegistryError::AlreadyExists`] if the name is present.
    pub fn insert(&mut self, entry: CastorEntry) -> Result<(), RegistryError> {
        if self.state.castors.contains_key(&entry.name) {
            return Err(RegistryError::AlreadyExists(entry.name));
        }
        self.state.castors.insert(entry.name.clone(), entry);
        Ok(())
    }

    /// Looks up a castor by name.
    #[must_use]
    pub fn get(&self, name: &CastorName) -> Option<&CastorEntry> {
        self.state.castors.get(name)
    }

    /// Removes a castor by name and returns its metadata.
    ///
    /// # Errors
    /// Returns [`RegistryError::NotFound`] if the name is absent.
    pub fn remove(&mut self, name: &CastorName) -> Result<CastorEntry, RegistryError> {
        self.state
            .castors
            .remove(name)
            .ok_or_else(|| RegistryError::NotFound(name.clone()))
    }

    /// Iterates over all entries in deterministic (name-sorted) order.
    pub fn list(&self) -> impl Iterator<Item = &CastorEntry> {
        self.state.castors.values()
    }

    /// Removes all entries.
    pub fn clear(&mut self) {
        self.state.castors.clear();
    }

    /// Returns the path this registry is backed by.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the number of registered castors.
    #[must_use]
    pub fn len(&self) -> usize {
        self.state.castors.len()
    }

    /// Returns `true` if no castors are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.state.castors.is_empty()
    }
}

/// Returns the default registry path: `~/.castors/.state/registry.json` on every
/// platform. See [`crate::paths`] for the rationale behind the unified
/// location.
fn default_path() -> Result<PathBuf, RegistryError> {
    crate::core::paths::registry_path().ok_or(RegistryError::NoDefaultPath)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    use tempfile::tempdir;

    fn sample_entry(name: &str) -> CastorEntry {
        CastorEntry {
            name: CastorName::from_str(name).unwrap(),
            image: ImageTag::from_str("example:latest").unwrap(),
            mount_dir: MountDir::from_str("./proj").unwrap(),
            created_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
        }
    }

    #[test]
    fn default_path_lives_under_castors_state_dir() {
        // Pin the policy from `crate::paths`: generated registry state is
        // `~/.castors/.state/registry.json` on every platform.
        let p = default_path().expect("test environment should expose a HOME");
        assert!(p.ends_with(".castors/.state/registry.json"));
    }

    #[test]
    fn load_from_missing_path_yields_empty_registry() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("registry.json");

        let registry = Registry::load_from(&path).unwrap();

        assert!(registry.is_empty());
        assert_eq!(registry.path(), path);
    }

    #[test]
    fn insert_and_get_roundtrip() {
        let dir = tempdir().unwrap();
        let mut registry = Registry::load_from(dir.path().join("registry.json")).unwrap();

        let entry = sample_entry("alpha");
        registry.insert(entry.clone()).unwrap();

        let fetched = registry.get(&entry.name).unwrap();
        assert_eq!(fetched, &entry);
    }

    #[test]
    fn insert_rejects_duplicate_names() {
        let dir = tempdir().unwrap();
        let mut registry = Registry::load_from(dir.path().join("registry.json")).unwrap();

        registry.insert(sample_entry("alpha")).unwrap();
        let err = registry.insert(sample_entry("alpha")).unwrap_err();

        assert!(matches!(err, RegistryError::AlreadyExists(_)));
    }

    #[test]
    fn remove_returns_entry_and_absents_it() {
        let dir = tempdir().unwrap();
        let mut registry = Registry::load_from(dir.path().join("registry.json")).unwrap();
        let entry = sample_entry("alpha");
        registry.insert(entry.clone()).unwrap();

        let removed = registry.remove(&entry.name).unwrap();

        assert_eq!(removed, entry);
        assert!(registry.get(&entry.name).is_none());
    }

    #[test]
    fn remove_errors_when_name_missing() {
        let dir = tempdir().unwrap();
        let mut registry = Registry::load_from(dir.path().join("registry.json")).unwrap();

        let err = registry
            .remove(&CastorName::from_str("ghost").unwrap())
            .unwrap_err();

        assert!(matches!(err, RegistryError::NotFound(_)));
    }

    #[test]
    fn save_then_load_preserves_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("registry.json");

        let mut registry = Registry::load_from(&path).unwrap();
        registry.insert(sample_entry("alpha")).unwrap();
        registry.insert(sample_entry("beta")).unwrap();
        registry.save().unwrap();

        let reloaded = Registry::load_from(&path).unwrap();
        let names: Vec<_> = reloaded
            .list()
            .map(|e| e.name.as_str().to_owned())
            .collect();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn list_is_sorted_by_name() {
        let dir = tempdir().unwrap();
        let mut registry = Registry::load_from(dir.path().join("registry.json")).unwrap();

        registry.insert(sample_entry("charlie")).unwrap();
        registry.insert(sample_entry("alpha")).unwrap();
        registry.insert(sample_entry("bravo")).unwrap();

        let names: Vec<_> = registry
            .list()
            .map(|e| e.name.as_str().to_owned())
            .collect();
        assert_eq!(names, vec!["alpha", "bravo", "charlie"]);
    }

    #[test]
    fn clear_removes_everything() {
        let dir = tempdir().unwrap();
        let mut registry = Registry::load_from(dir.path().join("registry.json")).unwrap();
        registry.insert(sample_entry("alpha")).unwrap();
        registry.insert(sample_entry("beta")).unwrap();

        registry.clear();

        assert!(registry.is_empty());
    }
}
