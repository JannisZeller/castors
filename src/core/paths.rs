//! Default filesystem locations for castors state and config.
//!
//! Everything castors reads or writes for a user lives under a single directory:
//! `~/.castors/`. User-authored config sits directly below that directory;
//! generated state sits below `~/.castors/.state/`.
//!
//! The same path applies on Linux, macOS, and Windows
//! (`C:\Users\<name>\.castors\`). This deliberately diverges from XDG
//! (`~/.config/`, `~/.local/state/`) and from Apple's
//! `~/Library/Application Support/`: castors is small and operator-facing,
//! and "the same path everywhere" matters more than strict platform
//! convention when the operator is going to `cat` and `vim` these files.
//!
//! Other CLI tools that take the same call: `gh`, `aws`, `kubectl`, `ssh`,
//! the dotfile parts of `git`.
//!
//! Centralizing the rule here means individual subsystems
//! (`registry`, `config`) only ever build *file* paths, not directory paths,
//! so the policy can move without having to chase down call sites.

use std::path::PathBuf;

/// Subdirectory under the user's home that holds all castors files. Hidden
/// by convention so it does not clutter `ls` in the home directory.
pub const CASTORS_HOME_SUBDIR: &str = ".castors";
pub const STATE_SUBDIR: &str = ".state";
pub const INFRA_SUBDIR: &str = "infra";
pub const REGISTRY_FILENAME: &str = "registry.json";
pub const STATE_LOCK_FILENAME: &str = "state.lock";

/// Returns `~/.castors`, or `None` when no home directory can be determined
/// (extremely unusual; typically only happens in stripped-down test or
/// container environments where `$HOME` is unset).
#[must_use]
pub fn castors_home() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(CASTORS_HOME_SUBDIR))
}

/// Returns the directory for generated castors state.
#[must_use]
pub fn state_dir() -> Option<PathBuf> {
    castors_home().map(|h| h.join(STATE_SUBDIR))
}

/// Returns the registry path for generated castor metadata.
#[must_use]
pub fn registry_path() -> Option<PathBuf> {
    state_dir().map(|s| s.join(REGISTRY_FILENAME))
}

/// Returns the directory where generated shared infra files live.
#[must_use]
pub fn infra_dir() -> Option<PathBuf> {
    state_dir().map(|s| s.join(INFRA_SUBDIR))
}

/// Returns the advisory lock file path for state-changing workflows.
#[must_use]
pub fn state_lock_path() -> Option<PathBuf> {
    state_dir().map(|s| s.join(STATE_LOCK_FILENAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn castors_home_ends_in_dot_castors() {
        // We can't pin the absolute path because it depends on the test
        // environment's HOME, but we can pin the suffix.
        let p = castors_home().expect("test environment should expose a HOME");
        assert!(p.ends_with(CASTORS_HOME_SUBDIR));
    }

    #[test]
    fn state_dir_lives_under_castors_home() {
        let p = state_dir().expect("test environment should expose a HOME");
        assert!(p.ends_with(".castors/.state"));
    }

    #[test]
    fn registry_path_lives_under_state_dir() {
        let p = registry_path().expect("test environment should expose a HOME");
        assert!(p.ends_with(".castors/.state/registry.json"));
    }

    #[test]
    fn infra_dir_lives_under_state_dir() {
        let p = infra_dir().expect("test environment should expose a HOME");
        assert!(p.ends_with(".castors/.state/infra"));
    }

    #[test]
    fn state_lock_path_lives_under_state_dir() {
        let p = state_lock_path().expect("test environment should expose a HOME");
        assert!(p.ends_with(".castors/.state/state.lock"));
    }
}
