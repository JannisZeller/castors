//! On-disk config schema.
//!
//! Two distinct documents live on disk: a [`GlobalConfig`] at
//! `~/.config/castors/config.yaml` and an optional [`ProjectConfig`] at
//! `<mount-dir>/.castors/config.yaml`. They share three sections that get
//! merged the same way (`network`, `env`, `secrets`) and each carry one
//! layer-specific block:
//!
//! - **Global**: `defaults` — values that apply to every castor on this host
//!   unless overridden. Today: a default image.
//! - **Project**: `castor` — identity for *this* workdir's castor. Today:
//!   name and image overrides.
//!
//! Both structs use `#[serde(deny_unknown_fields)]` so that putting `castor:`
//! in the global file (or `defaults:` in the project file) is a clean parse
//! error rather than a silent no-op.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::core::domain::{CastorName, ImageTag};

// ---------------------------------------------------------------------------
// Top-level documents
// ---------------------------------------------------------------------------

/// Global config: applies to every castor on the host. Lives at
/// `~/.config/castors/config.yaml` (or the platform equivalent).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalConfig {
    /// Cross-cutting defaults. Currently just the fallback image.
    #[serde(default)]
    pub defaults: GlobalDefaults,

    #[serde(default)]
    pub network: NetworkConfig,

    /// Plain environment variables passed via `docker run -e KEY=VAL`.
    /// **Visible to the agent.** Do not put secrets here.
    #[serde(default)]
    pub env: BTreeMap<String, String>,

    /// Header-injection rules for the outbound proxy. Enforced in MITM mode
    /// via proxy policy; in Squid mode values are embedded in `squid.conf`.
    #[serde(default)]
    pub secrets: Vec<SecretInjection>,
}

impl GlobalConfig {
    /// We do the normalization outside of the actual loading process
    /// to not mess around with yaml_serde's deserialization.
    pub(crate) fn normalize_relative_secret_files(&mut self, config_path: &Path) {
        normalize_relative_secret_files(&mut self.secrets, config_path);
    }
}

/// Project config: scoped to one workdir. Lives at
/// `<mount-dir>/.castors/config.yaml`. Optional — its absence just means
/// "use the global config as-is, with auto-naming".
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    /// Per-castor identity (name and image) for this workdir.
    #[serde(default)]
    pub castor: ProjectCastor,

    #[serde(default)]
    pub network: NetworkConfig,

    #[serde(default)]
    pub env: BTreeMap<String, String>,

    #[serde(default)]
    pub secrets: Vec<SecretInjection>,
}

impl ProjectConfig {
    /// We do the normalization outside of the actual loading process
    /// to not mess around with yaml_serde's deserialization.
    pub(crate) fn normalize_relative_secret_files(&mut self, config_path: &Path) {
        normalize_relative_secret_files(&mut self.secrets, config_path);
    }
}

/// Normalizes relative secret file paths against the config file's directory in place.
fn normalize_relative_secret_files(secrets: &mut [SecretInjection], config_path: &Path) {
    let base_dir = config_path.parent().unwrap_or_else(|| Path::new(""));
    for secret in secrets {
        let value_from: &mut SecretSource = &mut secret.value_from;
        if let SecretSource::File(path) = value_from {
            if path.is_relative() {
                *path = base_dir.join(&*path);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Layer-specific sub-blocks
// ---------------------------------------------------------------------------

/// Defaults that apply to every castor on this host unless something more
/// specific overrides them.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalDefaults {
    /// Image to use when neither the CLI nor the project config picks one.
    #[serde(default)]
    pub image: Option<ImageTag>,
}

/// Per-castor identity carried by the project config.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectCastor {
    /// Explicit name. If absent, the CLI auto-generates `<dir>-<n>`.
    #[serde(default)]
    pub name: Option<CastorName>,

    /// Image override. Beats `defaults.image` from the global config.
    #[serde(default)]
    pub image: Option<ImageTag>,
}

// ---------------------------------------------------------------------------
// Shared sections
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    /// Hosts the proxy will allow outbound traffic to. Enforcement depends on
    /// the active proxy mode and how the infra stack applies merged config.
    #[serde(default)]
    pub allowed_hosts: Vec<Host>,

    /// Shared proxy implementation used by this config layer. Project config
    /// overrides global config; absence falls back to Squid.
    #[serde(default)]
    pub proxy: Option<ProxyMode>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    #[default]
    Squid,
    Mitm,
}

/// A validated allowlist host entry.
///
/// Accepts bare hostnames (`api.openai.com`), `host:port` pairs
/// (`registry.npmjs.org:443`), and wildcard domains in `*.example.com`
/// form. Rejects URLs, paths, schemes, other wildcard forms,
/// and anything containing whitespace. Validation is intentionally
/// conservative: nicer to refuse a borderline value loudly here than to
/// pass it to Squid and decode the resulting error message.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Host(String);

impl Host {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Host {
    type Err = HostParseError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let s = raw.trim();
        if s.is_empty() {
            return Err(HostParseError::Empty);
        }
        if s.contains("://") {
            return Err(HostParseError::HasScheme(s.to_owned()));
        }
        if s.contains('/') {
            return Err(HostParseError::HasPath(s.to_owned()));
        }
        if s.contains(char::is_whitespace) {
            return Err(HostParseError::HasWhitespace(s.to_owned()));
        }
        if s.contains('*') && !is_valid_wildcard_host(s) {
            return Err(HostParseError::InvalidWildcard(s.to_owned()));
        }
        if s.starts_with('.') || s.ends_with('.') {
            return Err(HostParseError::HasLeadingOrTrailingDot(s.to_owned()));
        }
        Ok(Self(s.to_owned()))
    }
}

fn is_valid_wildcard_host(host: &str) -> bool {
    let Some(rest) = host.strip_prefix("*.") else {
        return false;
    };
    !rest.is_empty()
        && !rest.contains('*')
        && !rest.starts_with('.')
        && !rest.ends_with('.')
        && !rest.contains("..")
}

impl std::fmt::Display for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Host {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Host {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HostParseError {
    #[error("host must not be empty")]
    Empty,
    #[error("host '{0}' must not include a URL scheme; drop the `https://` prefix")]
    HasScheme(String),
    #[error("host '{0}' must not contain a path component")]
    HasPath(String),
    #[error("host '{0}' must not contain whitespace")]
    HasWhitespace(String),
    #[error(
        "host '{0}' uses an unsupported wildcard; only `*.example.com` style domains are allowed"
    )]
    InvalidWildcard(String),
    #[error("host '{0}' must not start or end with a dot")]
    HasLeadingOrTrailingDot(String),
}

/// A single header-injection rule applied by the (future) outbound proxy.
///
/// At request time the proxy sees an outbound request to `host`, looks up
/// the matching rule, fetches the secret material via [`Self::value_from`],
/// and substitutes `{{value}}` into [`Self::value_template`] to produce the
/// final header value.
///
/// Example YAML:
/// ```yaml
/// secrets:
///   - host: api.openai.com
///     header: Authorization
///     value_template: "Bearer {{value}}"
///     value_from: env:OPENAI_API_KEY
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretInjection {
    pub host: Host,
    pub header: String,
    pub value_template: String,
    pub value_from: SecretSource,
}

/// How the proxy obtains a secret's material at request time.
///
/// On disk this is a compact `scheme:value` string (e.g. `env:OPENAI_API_KEY`
/// or `file:/run/secrets/openai`). The string form keeps the YAML human-
/// friendly while the parsed value is strongly typed inside Rust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretSource {
    /// Read from the operator's host environment variable.
    Env(String),
    /// Read from a file on the operator's host.
    File(PathBuf),
}

impl FromStr for SecretSource {
    type Err = SecretSourceParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (scheme, rest) = s
            .split_once(':')
            .ok_or_else(|| SecretSourceParseError::MissingScheme(s.to_owned()))?;
        match scheme {
            "env" if !rest.is_empty() => Ok(Self::Env(rest.to_owned())),
            "file" if !rest.is_empty() => Ok(Self::File(PathBuf::from(rest))),
            "env" | "file" => Err(SecretSourceParseError::EmptyValue(scheme.to_owned())),
            other => Err(SecretSourceParseError::UnknownScheme(other.to_owned())),
        }
    }
}

impl std::fmt::Display for SecretSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Env(var) => write!(f, "env:{var}"),
            Self::File(path) => write!(f, "file:{}", path.display()),
        }
    }
}

impl Serialize for SecretSource {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SecretSource {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SecretSourceParseError {
    #[error("secret source '{0}' is missing a scheme; expected `env:NAME` or `file:PATH`")]
    MissingScheme(String),
    #[error("unknown secret source scheme '{0}'; expected `env` or `file`")]
    UnknownScheme(String),
    #[error("secret source scheme '{0}' requires a non-empty value")]
    EmptyValue(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(s: &str) -> Host {
        s.parse().unwrap()
    }

    #[test]
    fn host_accepts_bare_hostname() {
        assert_eq!(host("api.openai.com").as_str(), "api.openai.com");
    }

    #[test]
    fn host_accepts_host_port() {
        assert_eq!(
            host("registry.npmjs.org:443").as_str(),
            "registry.npmjs.org:443"
        );
    }

    #[test]
    fn host_rejects_scheme() {
        assert!(matches!(
            "https://github.com".parse::<Host>(),
            Err(HostParseError::HasScheme(_))
        ));
    }

    #[test]
    fn host_rejects_path() {
        assert!(matches!(
            "github.com/foo".parse::<Host>(),
            Err(HostParseError::HasPath(_))
        ));
    }

    #[test]
    fn host_rejects_wildcard_and_dots() {
        assert_eq!(host("*.github.com").as_str(), "*.github.com");
        assert!(matches!(
            "*github.com".parse::<Host>(),
            Err(HostParseError::InvalidWildcard(_))
        ));
        assert!(matches!(
            ".github.com".parse::<Host>(),
            Err(HostParseError::HasLeadingOrTrailingDot(_))
        ));
    }

    #[test]
    fn proxy_mode_parses_from_network_config() {
        let yaml = "network:\n  proxy: mitm\n";
        let cfg: GlobalConfig = yaml_serde::from_str(yaml).unwrap();
        assert_eq!(cfg.network.proxy, Some(ProxyMode::Mitm));
    }

    #[test]
    fn host_rejects_empty_and_whitespace() {
        assert_eq!("".parse::<Host>(), Err(HostParseError::Empty));
        assert!(matches!(
            "github .com".parse::<Host>(),
            Err(HostParseError::HasWhitespace(_))
        ));
    }

    #[test]
    fn secret_source_parses_env_form() {
        let src: SecretSource = "env:OPENAI_API_KEY".parse().unwrap();
        assert_eq!(src, SecretSource::Env("OPENAI_API_KEY".to_owned()));
    }

    #[test]
    fn secret_source_parses_file_form() {
        let src: SecretSource = "file:/run/secrets/openai".parse().unwrap();
        assert_eq!(
            src,
            SecretSource::File(PathBuf::from("/run/secrets/openai"))
        );
    }

    #[test]
    fn secret_source_rejects_missing_scheme() {
        assert!(matches!(
            "OPENAI_API_KEY".parse::<SecretSource>(),
            Err(SecretSourceParseError::MissingScheme(_))
        ));
    }

    #[test]
    fn secret_source_rejects_unknown_scheme() {
        assert!(matches!(
            "vault:foo".parse::<SecretSource>(),
            Err(SecretSourceParseError::UnknownScheme(_))
        ));
    }

    #[test]
    fn secret_source_rejects_empty_value() {
        assert!(matches!(
            "env:".parse::<SecretSource>(),
            Err(SecretSourceParseError::EmptyValue(_))
        ));
    }

    #[test]
    fn global_config_yaml_roundtrip() {
        let yaml = r#"
defaults:
  image: my-default:latest
network:
  allowed_hosts:
    - api.openai.com
env:
  RUST_LOG: debug
secrets:
  - host: api.openai.com
    header: Authorization
    value_template: "Bearer {{value}}"
    value_from: env:OPENAI_API_KEY
"#;
        let cfg: GlobalConfig = yaml_serde::from_str(yaml).unwrap();
        assert_eq!(
            cfg.defaults.image.as_ref().map(ImageTag::as_str),
            Some("my-default:latest")
        );
        assert_eq!(cfg.network.allowed_hosts[0].as_str(), "api.openai.com");
        assert_eq!(cfg.env["RUST_LOG"], "debug");
        assert_eq!(cfg.secrets.len(), 1);
    }

    #[test]
    fn project_config_yaml_roundtrip() {
        let yaml = r#"
castor:
  name: my-agent
  image: this-project:tag
network:
  allowed_hosts:
    - api.anthropic.com
env:
  GIT_AUTHOR_NAME: Agent
"#;
        let cfg: ProjectConfig = yaml_serde::from_str(yaml).unwrap();
        assert_eq!(
            cfg.castor.name.as_ref().map(CastorName::as_str),
            Some("my-agent")
        );
        assert_eq!(
            cfg.castor.image.as_ref().map(ImageTag::as_str),
            Some("this-project:tag")
        );
        assert_eq!(cfg.network.allowed_hosts[0].as_str(), "api.anthropic.com");
    }

    #[test]
    fn global_config_rejects_project_only_block() {
        // `castor:` belongs in project, not global. With deny_unknown_fields
        // this surfaces as a parse error instead of being silently ignored.
        let yaml = "castor:\n  name: foo\n";
        let err = yaml_serde::from_str::<GlobalConfig>(yaml).unwrap_err();
        assert!(err.to_string().contains("castor"));
    }

    #[test]
    fn project_config_rejects_global_only_block() {
        let yaml = "defaults:\n  image: foo:bar\n";
        let err = yaml_serde::from_str::<ProjectConfig>(yaml).unwrap_err();
        assert!(err.to_string().contains("defaults"));
    }

    #[test]
    fn global_config_defaults_when_sections_omitted() {
        let cfg: GlobalConfig = yaml_serde::from_str("{}").unwrap();
        assert!(cfg.defaults.image.is_none());
        assert!(cfg.network.allowed_hosts.is_empty());
        assert!(cfg.env.is_empty());
        assert!(cfg.secrets.is_empty());
    }

    #[test]
    fn project_config_defaults_when_sections_omitted() {
        let cfg: ProjectConfig = yaml_serde::from_str("{}").unwrap();
        assert!(cfg.castor.name.is_none());
        assert!(cfg.castor.image.is_none());
        assert!(cfg.network.allowed_hosts.is_empty());
    }

    #[test]
    fn global_config_rejects_unknown_top_level_keys() {
        let err = yaml_serde::from_str::<GlobalConfig>("foo: bar\n").unwrap_err();
        assert!(err.to_string().contains("foo"));
    }

    #[test]
    fn global_config_surfaces_host_error() {
        let yaml = "network:\n  allowed_hosts:\n    - https://github.com\n";
        let err = yaml_serde::from_str::<GlobalConfig>(yaml).unwrap_err();
        assert!(err.to_string().contains("URL scheme"));
    }
}
