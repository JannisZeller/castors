//! Squid configuration rendering for the shared proxy stack.
//!
//! Renders `squid.conf`: global defaults plus source-IP scoped rules for
//! registered castors that use Squid. Docker engine decides where that rendered
//! file lives and how it is mounted; this module only knows Squid syntax.

use std::fmt::Write as _;
use std::fs;

use thiserror::Error;

use crate::config::{Host, ProxyMode, ResolvedConfig, SecretInjection, SecretSource};
use crate::proxy::ProxyService;

pub const CONTAINER_NAME: &str = "castors-infra-squid";
pub const PORT: u16 = 3128;

const SQUID_TEMPLATE: &str = include_str!("squid.conf");
const POLICY_RULES_PLACEHOLDER: &str = "{{CASTORS_POLICY_RULES}}";

pub struct SquidProxyService;

impl ProxyService for SquidProxyService {
    fn mode(&self) -> ProxyMode {
        ProxyMode::Squid
    }

    fn container_name(&self) -> &'static str {
        CONTAINER_NAME
    }

    fn port(&self) -> u16 {
        PORT
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerPolicy {
    pub name: String,
    pub ips: Vec<String>,
    pub config: ResolvedConfig,
}

#[derive(Debug, Error)]
pub enum SquidConfigError {
    #[error("secret injection for host '{0}' requires that host to also appear in allowed_hosts")]
    SecretHostNotAllowed(Host),
    #[error("invalid allowlist host '{host}': port '{port}' is not a valid u16")]
    InvalidPort { host: Host, port: String },
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
}

#[derive(Debug)]
struct HostRule {
    host: Host,
    domain: String,
    port: Option<u16>,
    injections: Vec<HeaderInjection>,
}

#[derive(Debug)]
struct HeaderInjection {
    header: String,
    value: String,
}

pub fn render_config(config: &ResolvedConfig) -> Result<String, SquidConfigError> {
    render_policy(config, &[])
}

pub fn render_policy(
    global: &ResolvedConfig,
    containers: &[ContainerPolicy],
) -> Result<String, SquidConfigError> {
    let mut rules = String::new();

    let managed_ips: Vec<&str> = containers
        .iter()
        .flat_map(|container| container.ips.iter().map(String::as_str))
        .collect();

    for (container_idx, container) in containers.iter().enumerate() {
        if container.config.proxy != ProxyMode::Squid || container.ips.is_empty() {
            continue;
        }

        let source_acl = format!("castor_{container_idx}_src");
        writeln!(rules, "# {}", container.name).unwrap();
        writeln!(rules, "acl {source_acl} src {}", container.ips.join(" ")).unwrap();

        let host_rules = build_host_rules(&container.config)?;
        let comment_prefix = format!("{}: ", container.name);
        render_policy_rules(
            &mut rules,
            &host_rules,
            &format!("castor_{container_idx}"),
            Some(&comment_prefix),
            &[source_acl],
        );
    }

    if !managed_ips.is_empty() {
        writeln!(rules, "# Registered castors with source-specific policy.").unwrap();
        writeln!(
            rules,
            "acl managed_castors_src src {}",
            managed_ips.join(" ")
        )
        .unwrap();
        writeln!(rules).unwrap();
    }

    let global_rules = build_host_rules(global)?;
    let global_guards = if managed_ips.is_empty() {
        Vec::new()
    } else {
        vec!["!managed_castors_src".to_owned()]
    };
    render_policy_rules(&mut rules, &global_rules, "global", None, &global_guards);

    Ok(SQUID_TEMPLATE.replace(POLICY_RULES_PLACEHOLDER, &rules))
}

fn render_policy_rules(
    out: &mut String,
    rules: &[HostRule],
    acl_prefix: &str,
    comment_prefix: Option<&str>,
    base_guard_acls: &[String],
) {
    for (idx, rule) in rules.iter().enumerate() {
        let dst_acl = format!("{acl_prefix}_{idx}_dst");
        writeln!(
            out,
            "# {}{}",
            comment_prefix.unwrap_or_default(),
            rule.host.as_str()
        )
        .unwrap();
        writeln!(out, "acl {dst_acl} dstdomain {}", rule.domain).unwrap();

        let mut guard_acls = base_guard_acls.to_vec();
        guard_acls.push(dst_acl.clone());
        if let Some(port) = rule.port {
            let port_acl = format!("{acl_prefix}_{idx}_port");
            writeln!(out, "acl {port_acl} port {port}").unwrap();
            guard_acls.push(port_acl);
        }

        for injection in &rule.injections {
            writeln!(
                out,
                "request_header_add {} \"{}\" {}",
                injection.header,
                escape_quoted(&injection.value),
                guard_acls.join(" ")
            )
            .unwrap();
        }

        writeln!(out, "http_access allow {}", guard_acls.join(" ")).unwrap();
        writeln!(out).unwrap();
    }
}

fn build_host_rules(config: &ResolvedConfig) -> Result<Vec<HostRule>, SquidConfigError> {
    let mut rules: Vec<HostRule> = config
        .allowed_hosts
        .iter()
        .cloned()
        .map(|host| {
            let (domain, port) = split_host_port(&host)?;
            Ok(HostRule {
                host,
                domain,
                port,
                injections: Vec::new(),
            })
        })
        .collect::<Result<_, SquidConfigError>>()?;

    for secret in &config.secrets {
        let rule = rules
            .iter_mut()
            .find(|rule| rule.host == secret.host)
            .ok_or_else(|| SquidConfigError::SecretHostNotAllowed(secret.host.clone()))?;
        let injection = header_injection(secret)?;
        rule.injections.push(injection);
    }

    rules.sort_by(|a, b| a.host.as_str().cmp(b.host.as_str()));
    Ok(rules)
}

fn split_host_port(host: &Host) -> Result<(String, Option<u16>), SquidConfigError> {
    let raw = host.as_str();
    match raw.rsplit_once(':') {
        Some((domain, port)) if !domain.is_empty() => {
            let port = port
                .parse::<u16>()
                .map_err(|_| SquidConfigError::InvalidPort {
                    host: host.clone(),
                    port: port.to_owned(),
                })?;
            Ok((squid_domain(domain), Some(port)))
        }
        _ => Ok((squid_domain(raw), None)),
    }
}

fn squid_domain(domain: &str) -> String {
    domain
        .strip_prefix("*.")
        .map_or_else(|| domain.to_owned(), |suffix| format!(".{suffix}"))
}

fn header_injection(secret: &SecretInjection) -> Result<HeaderInjection, SquidConfigError> {
    validate_header_name(&secret.header)?;
    if !secret.value_template.contains("{{value}}") {
        return Err(SquidConfigError::MissingValuePlaceholder {
            host: secret.host.clone(),
            header: secret.header.clone(),
        });
    }

    let raw = resolve_secret_source(&secret.value_from)?;
    Ok(HeaderInjection {
        header: secret.header.clone(),
        value: secret.value_template.replace("{{value}}", &raw),
    })
}

fn validate_header_name(header: &str) -> Result<(), SquidConfigError> {
    if header.is_empty() {
        return Err(SquidConfigError::EmptyHeaderName);
    }
    if header.contains(char::is_whitespace) || header.contains(':') {
        return Err(SquidConfigError::InvalidHeaderName(header.to_owned()));
    }
    Ok(())
}

fn resolve_secret_source(source: &SecretSource) -> Result<String, SquidConfigError> {
    match source {
        SecretSource::Env(name) => {
            std::env::var(name).map_err(|source| SquidConfigError::ReadEnv {
                name: name.clone(),
                source,
            })
        }
        SecretSource::File(path) => {
            let raw = fs::read_to_string(path).map_err(|source| SquidConfigError::ReadFile {
                path: path.display().to_string(),
                source,
            })?;
            Ok(trim_secret_trailing_newlines(&raw).to_owned())
        }
    }
}

fn trim_secret_trailing_newlines(value: &str) -> &str {
    value.trim_end_matches(['\r', '\n'])
}

fn escape_quoted(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ResolvedConfig, SecretSource};
    use std::collections::BTreeMap;

    fn host(s: &str) -> Host {
        s.parse().unwrap()
    }

    fn secret(
        host_name: &str,
        header: &str,
        template: &str,
        from: SecretSource,
    ) -> SecretInjection {
        SecretInjection {
            host: host(host_name),
            header: header.to_owned(),
            value_template: template.to_owned(),
            value_from: from,
        }
    }

    #[test]
    fn render_config_emits_base_proxy_settings_and_allow_rules() {
        let cfg = ResolvedConfig {
            allowed_hosts: vec![host("api.openai.com"), host("registry.npmjs.org:443")],
            ..Default::default()
        };

        let rendered = render_config(&cfg).unwrap();

        assert!(rendered.contains("http_port 3128"));
        assert!(rendered.contains("cache deny all"));
        assert!(!rendered.contains(POLICY_RULES_PLACEHOLDER));
        assert!(rendered.contains("acl global_0_dst dstdomain api.openai.com"));
        assert!(rendered.contains("acl global_1_dst dstdomain registry.npmjs.org"));
        assert!(rendered.contains("acl global_1_port port 443"));
        assert!(rendered.contains("http_access deny all"));
    }

    #[test]
    fn render_config_converts_wildcards_to_squid_domain_acl_form() {
        let cfg = ResolvedConfig {
            allowed_hosts: vec![host("*.openai.com")],
            ..Default::default()
        };

        let rendered = render_config(&cfg).unwrap();

        assert!(rendered.contains("acl global_0_dst dstdomain .openai.com"));
    }

    #[test]
    fn render_policy_adds_source_scoped_rules_for_squid_castors() {
        let dir = tempfile::tempdir().unwrap();
        let alpha_token = dir.path().join("alpha-token");
        fs::write(&alpha_token, "alpha-secret\n").unwrap();

        let global = ResolvedConfig {
            allowed_hosts: vec![host("global.example.com")],
            ..Default::default()
        };
        let alpha = ContainerPolicy {
            name: "castor-alpha".to_owned(),
            ips: vec!["172.20.0.5".to_owned()],
            config: ResolvedConfig {
                allowed_hosts: vec![host("api.alpha.com")],
                secrets: vec![secret(
                    "api.alpha.com",
                    "Authorization",
                    "Bearer {{value}}",
                    SecretSource::File(alpha_token),
                )],
                env: BTreeMap::new(),
                proxy: ProxyMode::Squid,
            },
        };
        let beta = ContainerPolicy {
            name: "castor-beta".to_owned(),
            ips: vec!["172.20.0.6".to_owned()],
            config: ResolvedConfig {
                allowed_hosts: vec![host("api.beta.com")],
                env: BTreeMap::new(),
                proxy: ProxyMode::Mitm,
                secrets: Vec::new(),
            },
        };

        let rendered = render_policy(&global, &[alpha, beta]).unwrap();

        assert!(rendered.contains("# castor-alpha"));
        assert!(rendered.contains("acl castor_0_src src 172.20.0.5"));
        assert!(rendered.contains("# castor-alpha: api.alpha.com"));
        assert!(rendered.contains("acl castor_0_0_dst dstdomain api.alpha.com"));
        assert!(rendered.contains(
            "request_header_add Authorization \"Bearer alpha-secret\" castor_0_src castor_0_0_dst"
        ));
        assert!(rendered.contains("http_access allow castor_0_src castor_0_0_dst"));
        assert!(rendered.contains("acl managed_castors_src src 172.20.0.5 172.20.0.6"));
        assert!(rendered.contains("http_access allow !managed_castors_src global_0_dst"));
        assert!(!rendered.contains("api.beta.com"));
    }

    #[test]
    fn template_contains_policy_placeholder() {
        assert!(SQUID_TEMPLATE.contains(POLICY_RULES_PLACEHOLDER));
    }

    #[test]
    fn render_config_resolves_and_injects_secret_headers() {
        let dir = tempfile::tempdir().unwrap();
        let token_path = dir.path().join("openai-token");
        fs::write(&token_path, "super-secret\n").unwrap();

        let cfg = ResolvedConfig {
            allowed_hosts: vec![host("api.openai.com")],
            secrets: vec![secret(
                "api.openai.com",
                "Authorization",
                "Bearer {{value}}",
                SecretSource::File(token_path),
            )],
            env: BTreeMap::new(),
            proxy: ProxyMode::Squid,
        };

        let rendered = render_config(&cfg).unwrap();

        assert!(
            rendered
                .contains("request_header_add Authorization \"Bearer super-secret\" global_0_dst")
        );
    }

    #[test]
    fn render_config_uses_pre_resolved_secret_file_paths() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join(".castors").join("secrets");
        fs::create_dir_all(&nested).unwrap();
        let token_path = nested.join("openai-token");
        fs::write(&token_path, "from-normalized-path\n").unwrap();

        let cfg = ResolvedConfig {
            allowed_hosts: vec![host("api.openai.com")],
            secrets: vec![secret(
                "api.openai.com",
                "Authorization",
                "Bearer {{value}}",
                SecretSource::File(token_path),
            )],
            env: BTreeMap::new(),
            proxy: ProxyMode::Squid,
        };

        let rendered = render_config(&cfg).unwrap();

        assert!(rendered.contains(
            "request_header_add Authorization \"Bearer from-normalized-path\" global_0_dst"
        ));
    }

    #[test]
    fn render_config_rejects_secret_host_not_in_allowlist() {
        let cfg = ResolvedConfig {
            allowed_hosts: vec![host("registry.npmjs.org")],
            secrets: vec![secret(
                "api.openai.com",
                "Authorization",
                "Bearer {{value}}",
                SecretSource::Env("OPENAI_API_KEY".to_owned()),
            )],
            ..Default::default()
        };

        let err = render_config(&cfg).unwrap_err();
        assert!(matches!(err, SquidConfigError::SecretHostNotAllowed(_)));
    }

    #[test]
    fn render_config_rejects_non_numeric_ports() {
        let cfg = ResolvedConfig {
            allowed_hosts: vec![host("api.openai.com:https")],
            ..Default::default()
        };

        let err = render_config(&cfg).unwrap_err();
        assert!(matches!(err, SquidConfigError::InvalidPort { .. }));
    }

    #[test]
    fn render_config_rejects_missing_value_placeholder() {
        let cfg = ResolvedConfig {
            allowed_hosts: vec![host("api.openai.com")],
            secrets: vec![secret(
                "api.openai.com",
                "Authorization",
                "Bearer token",
                SecretSource::Env("OPENAI_API_KEY".to_owned()),
            )],
            ..Default::default()
        };

        let err = render_config(&cfg).unwrap_err();
        assert!(matches!(
            err,
            SquidConfigError::MissingValuePlaceholder { .. }
        ));
    }

    #[test]
    fn render_config_rejects_invalid_header_name() {
        let cfg = ResolvedConfig {
            allowed_hosts: vec![host("api.openai.com")],
            secrets: vec![secret(
                "api.openai.com",
                "X Bad",
                "{{value}}",
                SecretSource::Env("OPENAI_API_KEY".to_owned()),
            )],
            ..Default::default()
        };

        let err = render_config(&cfg).unwrap_err();
        assert!(matches!(err, SquidConfigError::InvalidHeaderName(_)));
    }

    #[test]
    fn resolve_secret_source_trims_trailing_newlines_from_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token.txt");
        fs::write(&path, "abc123\n").unwrap();

        let resolved = resolve_secret_source(&SecretSource::File(path)).unwrap();

        assert_eq!(resolved, "abc123");
    }

    #[test]
    fn escape_quoted_escapes_quotes_and_backslashes() {
        let escaped = escape_quoted("Bearer a\\b\"c");
        assert_eq!(escaped, "Bearer a\\\\b\\\"c");
    }

    #[test]
    fn trim_secret_trailing_newlines_only_removes_line_endings() {
        assert_eq!(trim_secret_trailing_newlines("abc \n\n"), "abc ");
    }
}
