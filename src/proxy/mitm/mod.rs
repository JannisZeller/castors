//! mitmproxy policy rendering and service metadata.

use std::collections::BTreeMap;
use std::fs;

use serde_json::json;
use thiserror::Error;

use crate::config::{Host, ProxyMode, ResolvedConfig, SecretSource};
use crate::proxy::ProxyService;

pub const CONTAINER_NAME: &str = "castors-infra-mitm";
pub const PORT: u16 = 8080;
pub const COMPOSE_PROFILE: &str = "mitm";

pub struct MitmProxyService;

impl ProxyService for MitmProxyService {
    fn mode(&self) -> ProxyMode {
        ProxyMode::Mitm
    }

    fn container_name(&self) -> &'static str {
        CONTAINER_NAME
    }

    fn port(&self) -> u16 {
        PORT
    }

    fn compose_profile(&self) -> Option<&'static str> {
        Some(COMPOSE_PROFILE)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerPolicy {
    pub name: String,
    pub ips: Vec<String>,
    pub config: ResolvedConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPolicy {
    pub policy_json: String,
    pub secrets_json: String,
}

#[derive(Debug, Error)]
pub enum MitmPolicyError {
    #[error("secret injection for host '{0}' requires that host to also appear in allowed_hosts")]
    SecretHostNotAllowed(Host),
    #[error("secret header name must not be empty")]
    EmptyHeaderName,
    #[error("secret header name '{0}' must not contain whitespace or ':'")]
    InvalidHeaderName(String),
    #[error(
        "secret header '{header}' for host '{host}' is missing the `{{value}}` placeholder in value_template"
    )]
    MissingValuePlaceholder { host: Host, header: String },
    #[error("failed to read secret from env var '{name}'")]
    ReadEnv {
        name: String,
        #[source]
        source: std::env::VarError,
    },
    #[error("failed to read secret file at {path}")]
    ReadFile {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to serialize MITM secret bundle")]
    SerializeSecrets(#[source] serde_json::Error),
    #[error("failed to serialize MITM policy")]
    SerializePolicy(#[source] serde_json::Error),
}

pub fn render_policy(
    global: &ResolvedConfig,
    containers: &[ContainerPolicy],
) -> Result<RenderedPolicy, MitmPolicyError> {
    let mut secrets = BTreeMap::new();
    let mut secret_idx = 0usize;
    let global_headers = render_header_rules(global, &mut secrets, &mut secret_idx)?;

    let mut container_policies = serde_json::Map::new();
    for container in containers {
        let headers = render_header_rules(&container.config, &mut secrets, &mut secret_idx)?;
        container_policies.insert(
            container.name.clone(),
            json!({
                "ips": container.ips,
                "allow_domains": hosts_json(&container.config.allowed_hosts),
                "inject_headers": headers,
            }),
        );
    }

    let policy = json!({
        "default": "deny",
        "containers": container_policies,
        "global": {
            "allow_domains": hosts_json(&global.allowed_hosts),
            "inject_headers": global_headers,
        },
    });
    let policy_json =
        serde_json::to_string_pretty(&policy).map_err(MitmPolicyError::SerializePolicy)?;

    let secrets_json =
        serde_json::to_string_pretty(&json!(secrets)).map_err(MitmPolicyError::SerializeSecrets)?;

    Ok(RenderedPolicy {
        policy_json,
        secrets_json,
    })
}

fn hosts_json(hosts: &[Host]) -> Vec<&str> {
    hosts.iter().map(Host::as_str).collect()
}

type HeaderRules = BTreeMap<String, BTreeMap<String, String>>;

fn render_header_rules(
    config: &ResolvedConfig,
    secrets: &mut BTreeMap<String, String>,
    secret_idx: &mut usize,
) -> Result<HeaderRules, MitmPolicyError> {
    let mut out: HeaderRules = BTreeMap::new();
    for secret in &config.secrets {
        if !config.allowed_hosts.iter().any(|host| host == &secret.host) {
            return Err(MitmPolicyError::SecretHostNotAllowed(secret.host.clone()));
        }
        validate_header_name(&secret.header)?;
        if !secret.value_template.contains("{{value}}") {
            return Err(MitmPolicyError::MissingValuePlaceholder {
                host: secret.host.clone(),
                header: secret.header.clone(),
            });
        }
        let raw = resolve_secret_source(&secret.value_from)?;
        let final_value = secret.value_template.replace("{{value}}", &raw);
        let secret_name = format!("CASTORS_SECRET_{secret_idx}");
        *secret_idx += 1;
        secrets.insert(secret_name.clone(), final_value);
        out.entry(secret.host.as_str().to_owned())
            .or_default()
            .insert(secret.header.clone(), format!("${{{secret_name}}}"));
    }
    Ok(out)
}

fn validate_header_name(header: &str) -> Result<(), MitmPolicyError> {
    if header.is_empty() {
        return Err(MitmPolicyError::EmptyHeaderName);
    }
    if header.contains(char::is_whitespace) || header.contains(':') {
        return Err(MitmPolicyError::InvalidHeaderName(header.to_owned()));
    }
    Ok(())
}

fn resolve_secret_source(source: &SecretSource) -> Result<String, MitmPolicyError> {
    match source {
        SecretSource::Env(name) => std::env::var(name).map_err(|source| MitmPolicyError::ReadEnv {
            name: name.clone(),
            source,
        }),
        SecretSource::File(path) => {
            let raw = fs::read_to_string(path).map_err(|source| MitmPolicyError::ReadFile {
                path: path.display().to_string(),
                source,
            })?;
            Ok(raw.trim_end_matches(['\r', '\n']).to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Host, SecretInjection, SecretSource};

    fn host(s: &str) -> Host {
        s.parse().unwrap()
    }

    fn secret(host_name: &str, header: &str, source: SecretSource) -> SecretInjection {
        SecretInjection {
            host: host(host_name),
            header: header.to_owned(),
            value_template: "Bearer {{value}}".to_owned(),
            value_from: source,
        }
    }

    #[test]
    fn render_policy_includes_per_container_allowlists() {
        let global = ResolvedConfig {
            allowed_hosts: vec![host("example.com")],
            ..Default::default()
        };
        let container = ContainerPolicy {
            name: "castor-alpha".to_owned(),
            ips: vec!["172.20.0.4".to_owned()],
            config: ResolvedConfig {
                allowed_hosts: vec![host("api.openai.com"), host("*.anthropic.com")],
                proxy: ProxyMode::Mitm,
                ..Default::default()
            },
        };

        let rendered = render_policy(&global, &[container]).unwrap();
        let policy: serde_json::Value = serde_json::from_str(&rendered.policy_json).unwrap();

        assert_eq!(policy["default"], "deny");
        assert_eq!(policy["containers"]["castor-alpha"]["ips"][0], "172.20.0.4");
        assert_eq!(
            policy["containers"]["castor-alpha"]["allow_domains"],
            json!(["api.openai.com", "*.anthropic.com"])
        );
        assert_eq!(policy["global"]["allow_domains"], json!(["example.com"]));
    }

    #[test]
    fn render_policy_keeps_secret_values_out_of_policy_json() {
        let dir = tempfile::tempdir().unwrap();
        let token_path = dir.path().join("token");
        fs::write(&token_path, "top-secret\n").unwrap();
        let global = ResolvedConfig::default();
        let container = ContainerPolicy {
            name: "castor-alpha".to_owned(),
            ips: vec!["172.20.0.4".to_owned()],
            config: ResolvedConfig {
                allowed_hosts: vec![host("api.openai.com")],
                secrets: vec![secret(
                    "api.openai.com",
                    "Authorization",
                    SecretSource::File(token_path),
                )],
                proxy: ProxyMode::Mitm,
                ..Default::default()
            },
        };

        let rendered = render_policy(&global, &[container]).unwrap();
        let policy: serde_json::Value = serde_json::from_str(&rendered.policy_json).unwrap();

        assert_eq!(
            policy["containers"]["castor-alpha"]["inject_headers"]["api.openai.com"]["Authorization"],
            "${CASTORS_SECRET_0}"
        );
        assert!(!rendered.policy_json.contains("top-secret"));
        assert!(rendered.secrets_json.contains("top-secret"));
    }

    #[test]
    fn render_policy_rejects_header_for_non_allowed_host() {
        let global = ResolvedConfig::default();
        let container = ContainerPolicy {
            name: "castor-alpha".to_owned(),
            ips: vec!["172.20.0.4".to_owned()],
            config: ResolvedConfig {
                allowed_hosts: vec![host("example.com")],
                secrets: vec![secret(
                    "api.openai.com",
                    "Authorization",
                    SecretSource::Env("OPENAI_API_KEY".to_owned()),
                )],
                proxy: ProxyMode::Mitm,
                ..Default::default()
            },
        };

        let err = render_policy(&global, &[container]).unwrap_err();

        assert!(matches!(err, MitmPolicyError::SecretHostNotAllowed(_)));
    }
}
