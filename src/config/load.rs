//! File-system loaders for the global and project config documents.
//!
//! Both loaders return `T::default()` when the file does not exist, so a
//! fresh install with no config files at all still produces a valid
//! [`ResolvedConfig`]. A file that exists but cannot be read or parsed is an
//! error: silently dropping a typo'd config would defeat the point of having
//! one.
//!
//! [`ResolvedConfig`]: crate::config::ResolvedConfig

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

use super::ConfigError;
use super::schema::{GlobalConfig, ProjectConfig};

/// Filename used for both the global and project config files.
pub const CONFIG_FILENAME: &str = "config.yaml";

/// Subdirectory within the mount dir that holds the project-level config.
pub const PROJECT_CONFIG_DIR: &str = ".castors";

/// Returns the canonical path of the global config file:
/// `~/.castors/config.yaml` on every platform. See [`crate::core::paths`] for
/// the rationale behind the unified location.
///
/// # Errors
/// Returns [`ConfigError::NoDefaultPath`] when no home directory can be
/// determined (extremely unusual; typically only happens in stripped-down
/// test or container environments where `$HOME` is unset).
pub fn global_path() -> Result<PathBuf, ConfigError> {
    crate::core::paths::castors_home()
        .map(|h| h.join(CONFIG_FILENAME))
        .ok_or(ConfigError::NoDefaultPath)
}

/// Returns the project config path for a castor mounted at `mount_dir`.
///
/// The path is purely derived: the file may or may not exist on disk.
#[must_use]
pub fn project_path(mount_dir: &Path) -> PathBuf {
    mount_dir.join(PROJECT_CONFIG_DIR).join(CONFIG_FILENAME)
}

/// Loads the global config from its default location, or returns
/// [`GlobalConfig::default()`] if the file does not exist.
///
/// # Errors
/// Returns [`ConfigError::Read`] / [`ConfigError::Parse`] if the file exists
/// but cannot be read or parsed.
pub fn load_global() -> Result<GlobalConfig, ConfigError> {
    let path = global_path()?;
    load_global_at(&path)
}

/// Loads the project config for a castor mounted at `mount_dir`, or returns
/// [`ProjectConfig::default()`] if no `<mount_dir>/.castors/config.yaml`
/// exists.
///
/// # Errors
/// Returns [`ConfigError::Read`] / [`ConfigError::Parse`] if the file exists
/// but cannot be read or parsed.
pub fn load_project(mount_dir: &Path) -> Result<ProjectConfig, ConfigError> {
    let path = project_path(mount_dir);
    load_project_at(&path)
}

fn load_global_at(path: &Path) -> Result<GlobalConfig, ConfigError> {
    let mut config: GlobalConfig = load_at(path)?;
    config.normalize_relative_secret_files(path);
    Ok(config)
}

fn load_project_at(path: &Path) -> Result<ProjectConfig, ConfigError> {
    let mut config: ProjectConfig = load_at(path)?;
    config.normalize_relative_secret_files(path);
    Ok(config)
}

/// Loads any `T: Default + DeserializeOwned` from an explicit path. Missing
/// files are not an error — they yield `T::default()`.
///
/// # Errors
/// Returns [`ConfigError::Read`] / [`ConfigError::Parse`] if the file exists
/// but cannot be read or parsed.
pub fn load_at<T>(path: &Path) -> Result<T, ConfigError>
where
    T: Default + DeserializeOwned,
{
    let contents = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(T::default());
        }
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    yaml_serde::from_str(&contents).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn project_path_appends_castors_config_yaml() {
        let p = project_path(Path::new("/work/proj"));
        assert!(p.ends_with(".castors/config.yaml"));
    }

    #[test]
    fn global_path_lives_under_dot_castors_in_home() {
        // Pin the policy from `crate::paths`: global config is
        // `~/.castors/config.yaml` on every platform.
        let p = global_path().expect("test environment should expose a HOME");
        assert!(p.ends_with(".castors/config.yaml"));
    }

    #[test]
    fn load_at_returns_default_when_file_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("missing.yaml");

        let cfg: GlobalConfig = load_at(&path).unwrap();

        assert_eq!(cfg, GlobalConfig::default());
    }

    #[test]
    fn load_at_parses_global_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        fs::write(
            &path,
            "defaults:\n  image: my-default:latest\nnetwork:\n  allowed_hosts:\n    - api.openai.com\n",
        )
        .unwrap();

        let cfg: GlobalConfig = load_at(&path).unwrap();

        assert_eq!(cfg.defaults.image.unwrap().as_str(), "my-default:latest");
        assert_eq!(cfg.network.allowed_hosts[0].as_str(), "api.openai.com");
    }

    #[test]
    fn load_at_reports_parse_error_with_path() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        fs::write(
            &path,
            "network:\n  allowed_hosts:\n    - https://github.com\n",
        )
        .unwrap();

        let err = load_at::<GlobalConfig>(&path).unwrap_err();

        match err {
            ConfigError::Parse { path: p, .. } => assert_eq!(p, path),
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn load_project_resolves_under_mount_dir() {
        let dir = tempdir().unwrap();
        let castors_dir = dir.path().join(PROJECT_CONFIG_DIR);
        fs::create_dir(&castors_dir).unwrap();
        fs::write(
            castors_dir.join(CONFIG_FILENAME),
            "castor:\n  name: my-agent\nenv:\n  RUST_LOG: trace\n",
        )
        .unwrap();

        let cfg = load_project(dir.path()).unwrap();

        assert_eq!(cfg.castor.name.unwrap().as_str(), "my-agent");
        assert_eq!(cfg.env["RUST_LOG"], "trace");
    }

    #[test]
    fn load_project_returns_default_when_castors_dir_missing() {
        let dir = tempdir().unwrap();

        let cfg = load_project(dir.path()).unwrap();

        assert_eq!(cfg, ProjectConfig::default());
    }

    #[test]
    fn load_global_resolves_relative_secret_files_against_config_dir() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        fs::write(
            &path,
            "secrets:\n  - host: api.openai.com\n    header: Authorization\n    value_template: 'Bearer {{value}}'\n    value_from: file:secrets/openai.txt\n",
        )
        .unwrap();

        let cfg = load_global_at(&path).unwrap();

        match &cfg.secrets[0].value_from {
            crate::config::SecretSource::File(secret_path) => {
                assert_eq!(secret_path, &dir.path().join("secrets/openai.txt"));
            }
            other => panic!("expected file secret source, got {other:?}"),
        }
    }

    #[test]
    fn load_project_resolves_relative_secret_files_against_project_config_dir() {
        let dir = tempdir().unwrap();
        let castors_dir = dir.path().join(PROJECT_CONFIG_DIR);
        fs::create_dir(&castors_dir).unwrap();
        let path = castors_dir.join(CONFIG_FILENAME);
        fs::write(
            &path,
            "secrets:\n  - host: api.anthropic.com\n    header: x-api-key\n    value_template: '{{value}}'\n    value_from: file:anthropic.txt\n",
        )
        .unwrap();

        let cfg = load_project(dir.path()).unwrap();

        match &cfg.secrets[0].value_from {
            crate::config::SecretSource::File(secret_path) => {
                assert_eq!(secret_path, &castors_dir.join("anthropic.txt"));
            }
            other => panic!("expected file secret source, got {other:?}"),
        }
    }

    #[test]
    fn load_project_leaves_absolute_secret_files_unchanged() {
        let dir = tempdir().unwrap();
        let castors_dir = dir.path().join(PROJECT_CONFIG_DIR);
        fs::create_dir(&castors_dir).unwrap();
        let path = castors_dir.join(CONFIG_FILENAME);
        fs::write(
            &path,
            "secrets:\n  - host: api.anthropic.com\n    header: x-api-key\n    value_template: '{{value}}'\n    value_from: file:/var/run/secrets/anthropic.txt\n",
        )
        .unwrap();

        let cfg = load_project(dir.path()).unwrap();

        match &cfg.secrets[0].value_from {
            crate::config::SecretSource::File(secret_path) => {
                assert_eq!(
                    secret_path,
                    &PathBuf::from("/var/run/secrets/anthropic.txt")
                );
            }
            other => panic!("expected file secret source, got {other:?}"),
        }
    }
}
