//! Generated state locking for cross-process CLI coordination.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::PathBuf;

use fs2::FileExt;
use thiserror::Error;

/// Errors produced while acquiring or releasing the generated state lock.
#[derive(Debug, Error)]
pub enum StateLockError {
    #[error("could not determine default castors state path")]
    NoDefaultPath,
    #[error("failed to create state directory at {path}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to open state lock at {path}")]
    Open {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to acquire state lock at {path}")]
    Lock {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// RAII guard for an exclusive advisory lock over generated castors state.
#[derive(Debug)]
pub struct StateLock {
    file: File,
    path: PathBuf,
}

impl StateLock {
    /// Blocks until the process owns the generated state lock.
    ///
    /// # Errors
    /// Returns an error if the state directory cannot be created, the lock file
    /// cannot be opened, or the platform lock operation fails.
    pub fn acquire() -> Result<Self, StateLockError> {
        let state_dir = crate::core::paths::state_dir().ok_or(StateLockError::NoDefaultPath)?;
        Self::acquire_in_state_dir(state_dir)
    }

    fn acquire_in_state_dir(state_dir: PathBuf) -> Result<Self, StateLockError> {
        fs::create_dir_all(&state_dir).map_err(|source| StateLockError::CreateDir {
            path: state_dir.clone(),
            source,
        })?;

        Self::acquire_at(state_dir.join(crate::core::paths::STATE_LOCK_FILENAME))
    }

    fn acquire_at(path: PathBuf) -> Result<Self, StateLockError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| StateLockError::Open {
                path: path.clone(),
                source,
            })?;
        file.lock_exclusive()
            .map_err(|source| StateLockError::Lock {
                path: path.clone(),
                source,
            })?;
        Ok(Self { file, path })
    }

    /// Returns the lock file path held by this guard.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_creates_state_directory_and_lock_file() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join(".castors").join(".state");

        let lock = StateLock::acquire_in_state_dir(state_dir).unwrap();

        assert!(lock.path().exists());
        assert!(lock.path().ends_with(".castors/.state/state.lock"));
    }

    #[test]
    fn acquire_at_creates_lock_file_at_explicit_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("state.lock");
        fs::create_dir_all(path.parent().unwrap()).unwrap();

        let lock = StateLock::acquire_at(path.clone()).unwrap();

        assert_eq!(lock.path(), path);
        assert!(lock.path().exists());
    }
}
