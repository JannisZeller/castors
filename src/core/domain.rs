//! Core domain types for `castors`.
//!
//! These newtypes exist so that the rest of the codebase works in terms of
//! validated inputs rather than raw strings. Parsing happens once at the CLI
//! boundary; downstream code can then rely on the invariants encoded here.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A validated castor name, used to identify a castor across commands.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CastorName(String);

impl CastorName {
    /// Returns the validated name as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for CastorName {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if raw.is_empty() {
            return Err("castor name must not be empty".into());
        }
        let ok = raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !ok {
            return Err("castor name may only contain ASCII letters, digits, '-' or '_'".into());
        }
        Ok(Self(raw.to_owned()))
    }
}

impl TryFrom<String> for CastorName {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<CastorName> for String {
    fn from(value: CastorName) -> Self {
        value.0
    }
}

impl std::fmt::Display for CastorName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A container image reference, e.g. `my-image:latest`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ImageTag(String);

impl ImageTag {
    /// Returns the image reference as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ImageTag {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if raw.trim().is_empty() {
            return Err("image tag must not be empty".into());
        }
        Ok(Self(raw.to_owned()))
    }
}

impl TryFrom<String> for ImageTag {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<ImageTag> for String {
    fn from(value: ImageTag) -> Self {
        value.0
    }
}

impl std::fmt::Display for ImageTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A directory path to mount into a castor.
///
/// Only shape is validated here. Filesystem existence and canonicalization
/// are enforced by config loading and the engine, not inside this type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MountDir(PathBuf);

impl MountDir {
    /// Returns the mount directory as a path slice.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Returns an absolutized copy of this mount directory.
    ///
    /// Relative paths are joined with the current working directory. The
    /// result is not canonicalized: symlinks are preserved and the path is
    /// not required to exist on disk.
    ///
    /// # Errors
    /// Returns an error if the current working directory cannot be read.
    pub fn to_absolute(&self) -> std::io::Result<Self> {
        let abs = std::path::absolute(&self.0)?;
        Ok(Self(abs))
    }

    /// Rejects mount sources that would expose broad or obviously sensitive
    /// host state to the agent.
    ///
    /// This is deliberately conservative and complements Docker hardening. A
    /// castor can only protect what it does not mount into the container.
    pub fn validate_safe_source(&self) -> Result<(), String> {
        let path = lexically_normalize(&self.0);

        if is_filesystem_root(&path) {
            return Err("mount directory must not be the filesystem root".into());
        }

        for blocked in docker_control_paths() {
            if path == blocked {
                return Err(format!(
                    "mount directory must not expose Docker control path '{}'",
                    blocked.display()
                ));
            }
        }

        if let Some(home) = dirs::home_dir() {
            let home = lexically_normalize(&home);
            if path == home {
                return Err("mount directory must not be the entire home directory".into());
            }

            for sensitive in sensitive_home_paths(&home) {
                if path.starts_with(&sensitive) {
                    return Err(format!(
                        "mount directory must not expose sensitive host path '{}'",
                        sensitive.display()
                    ));
                }
            }
        }

        Ok(())
    }
}

fn lexically_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }

    normalized
}

fn is_filesystem_root(path: &Path) -> bool {
    path.parent().is_none()
}

fn docker_control_paths() -> [PathBuf; 2] {
    [
        PathBuf::from("/var/run/docker.sock"),
        PathBuf::from("/run/docker.sock"),
    ]
}

fn sensitive_home_paths(home: &Path) -> [PathBuf; 8] {
    [
        home.join(".aws"),
        home.join(".azure"),
        home.join(".castors"),
        home.join(".config"),
        home.join(".docker"),
        home.join(".gnupg"),
        home.join(".kube"),
        home.join(".ssh"),
    ]
}

impl FromStr for MountDir {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if raw.is_empty() {
            return Err("mount directory must not be empty".into());
        }
        Ok(Self(PathBuf::from(raw)))
    }
}

impl TryFrom<String> for MountDir {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<MountDir> for String {
    fn from(value: MountDir) -> Self {
        value.0.to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn castor_name_accepts_valid_identifiers() {
        assert!(CastorName::from_str("my-castor_1").is_ok());
    }

    #[test]
    fn castor_name_rejects_empty_and_spaces() {
        assert!(CastorName::from_str("").is_err());
        assert!(CastorName::from_str("has space").is_err());
    }

    #[test]
    fn image_tag_rejects_empty() {
        assert!(ImageTag::from_str("").is_err());
        assert!(ImageTag::from_str(" ").is_err());
    }

    #[test]
    fn mount_dir_accepts_any_non_empty_path() {
        assert!(MountDir::from_str("./some/path").is_ok());
        assert!(MountDir::from_str("").is_err());
    }

    #[test]
    fn castor_name_roundtrips_through_json() {
        let name = CastorName::from_str("abc").unwrap();
        let json = serde_json::to_string(&name).unwrap();
        assert_eq!(json, "\"abc\"");
        let back: CastorName = serde_json::from_str(&json).unwrap();
        assert_eq!(back, name);
    }

    #[test]
    fn castor_name_deserialize_rejects_invalid() {
        let result: Result<CastorName, _> = serde_json::from_str("\"bad name!\"");
        assert!(result.is_err());
    }

    #[test]
    fn mount_dir_rejects_filesystem_root() {
        let mount = MountDir::from_str("/").unwrap();
        assert!(mount.validate_safe_source().is_err());
    }

    #[test]
    fn mount_dir_rejects_entire_home_directory_when_available() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let mount = MountDir(home);
        let err = mount.validate_safe_source().unwrap_err();
        assert!(err.contains("entire home directory"));
    }

    #[test]
    fn mount_dir_rejects_sensitive_home_paths_when_available() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let mount = MountDir(home.join(".ssh/project"));
        let err = mount.validate_safe_source().unwrap_err();
        assert!(err.contains("sensitive host path"));
    }

    #[test]
    fn mount_dir_rejects_docker_socket() {
        let mount = MountDir::from_str("/var/run/docker.sock").unwrap();
        let err = mount.validate_safe_source().unwrap_err();
        assert!(err.contains("Docker control path"));
    }
}
