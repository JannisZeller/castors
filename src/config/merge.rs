//! Merging the global and project config layers into a [`ResolvedConfig`].
//!
//! Only the *shared* sections (`network`, `env`, `secrets`) take part in
//! merge — the layer-specific blocks (`defaults` on global, `castor` on
//! project) feed into identity resolution instead, which lives in
//! [`super::identity`].
//!
//! Merge rules (project always wins on conflicts):
//!
//! - `network.allowed_hosts`: union, deduplicated, sorted for deterministic
//!   output.
//! - `env`: per-key override. Project entries replace global entries with
//!   the same key; non-overlapping keys from both layers survive.
//! - `secrets`: per `(host, header)` override (case-insensitive on header).
//!   Multiple injections per host are fine, but a project rule with the
//!   same `(host, header)` as a global rule replaces it. Output is sorted
//!   by `(host, header)` for stability.

use std::collections::BTreeMap;

use super::ResolvedConfig;
use super::schema::{GlobalConfig, ProjectConfig, SecretInjection};

/// Combine the shared sections of the global and project documents into the
/// effective per-castor configuration that the engine will see.
#[must_use]
pub fn merge(global: &GlobalConfig, project: &ProjectConfig) -> ResolvedConfig {
    ResolvedConfig {
        allowed_hosts: merge_allowed_hosts(
            global.network.allowed_hosts.clone(),
            project.network.allowed_hosts.clone(),
        ),
        env: merge_env(global.env.clone(), project.env.clone()),
        proxy: project
            .network
            .proxy
            .unwrap_or(global.network.proxy.unwrap_or_default()),
        secrets: merge_secrets(global.secrets.clone(), project.secrets.clone()),
    }
}

fn merge_allowed_hosts<H: Ord>(global: Vec<H>, project: Vec<H>) -> Vec<H> {
    let mut all: Vec<H> = global.into_iter().chain(project).collect();
    all.sort();
    all.dedup();
    all
}

fn merge_env(
    global: BTreeMap<String, String>,
    project: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut out = global;
    out.extend(project);
    out
}

fn merge_secrets(
    global: Vec<SecretInjection>,
    project: Vec<SecretInjection>,
) -> Vec<SecretInjection> {
    let mut by_key: BTreeMap<(String, String), SecretInjection> = BTreeMap::new();
    for s in global.into_iter().chain(project) {
        // Header names are case-insensitive in HTTP, so we normalize the
        // override key. The stored injection keeps whatever casing the
        // user wrote (project wins on collision, so its casing survives).
        let key = (
            s.host.as_str().to_ascii_lowercase(),
            s.header.to_ascii_lowercase(),
        );
        by_key.insert(key, s);
    }
    by_key.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{Host, NetworkConfig, SecretSource};

    fn host(s: &str) -> Host {
        s.parse().unwrap()
    }

    fn injection(host_str: &str, header: &str, var: &str) -> SecretInjection {
        SecretInjection {
            host: host(host_str),
            header: header.to_owned(),
            value_template: "Bearer {{value}}".to_owned(),
            value_from: SecretSource::Env(var.to_owned()),
        }
    }

    #[test]
    fn allowed_hosts_are_unioned_and_deduped() {
        let global = GlobalConfig {
            network: NetworkConfig {
                allowed_hosts: vec![host("api.openai.com"), host("github.com")],
                ..Default::default()
            },
            ..Default::default()
        };
        let project = ProjectConfig {
            network: NetworkConfig {
                allowed_hosts: vec![host("github.com"), host("registry.npmjs.org")],
                ..Default::default()
            },
            ..Default::default()
        };

        let resolved = merge(&global, &project);

        let names: Vec<_> = resolved
            .allowed_hosts
            .iter()
            .map(|h| h.as_str().to_owned())
            .collect();
        assert_eq!(
            names,
            vec!["api.openai.com", "github.com", "registry.npmjs.org"]
        );
    }

    #[test]
    fn env_project_overrides_global_per_key() {
        let mut global_env = BTreeMap::new();
        global_env.insert("RUST_LOG".to_owned(), "info".to_owned());
        global_env.insert("KEEP_ME".to_owned(), "yes".to_owned());

        let mut project_env = BTreeMap::new();
        project_env.insert("RUST_LOG".to_owned(), "debug".to_owned());
        project_env.insert("ADDED".to_owned(), "1".to_owned());

        let resolved = merge(
            &GlobalConfig {
                env: global_env,
                ..Default::default()
            },
            &ProjectConfig {
                env: project_env,
                ..Default::default()
            },
        );

        assert_eq!(resolved.env["RUST_LOG"], "debug");
        assert_eq!(resolved.env["KEEP_ME"], "yes");
        assert_eq!(resolved.env["ADDED"], "1");
    }

    #[test]
    fn secrets_override_per_host_header_pair() {
        let global = GlobalConfig {
            secrets: vec![
                injection("api.openai.com", "Authorization", "GLOBAL_KEY"),
                injection("api.openai.com", "X-Org-Id", "GLOBAL_ORG"),
            ],
            ..Default::default()
        };
        let project = ProjectConfig {
            secrets: vec![
                injection("api.openai.com", "Authorization", "PROJECT_KEY"),
                injection("github.com", "Authorization", "GH_TOKEN"),
            ],
            ..Default::default()
        };

        let resolved = merge(&global, &project);

        assert_eq!(resolved.secrets.len(), 3);
        let openai_auth = resolved
            .secrets
            .iter()
            .find(|s| s.host.as_str() == "api.openai.com" && s.header == "Authorization")
            .unwrap();
        assert_eq!(
            openai_auth.value_from,
            SecretSource::Env("PROJECT_KEY".to_owned())
        );
    }

    #[test]
    fn secret_override_is_case_insensitive_on_header_name() {
        let global = GlobalConfig {
            secrets: vec![injection("api.openai.com", "authorization", "GLOBAL")],
            ..Default::default()
        };
        let project = ProjectConfig {
            secrets: vec![injection("api.openai.com", "Authorization", "PROJECT")],
            ..Default::default()
        };

        let resolved = merge(&global, &project);

        assert_eq!(resolved.secrets.len(), 1);
        assert_eq!(
            resolved.secrets[0].value_from,
            SecretSource::Env("PROJECT".to_owned())
        );
    }

    #[test]
    fn merge_of_two_empties_is_empty() {
        let resolved = merge(&GlobalConfig::default(), &ProjectConfig::default());

        assert!(resolved.allowed_hosts.is_empty());
        assert!(resolved.env.is_empty());
        assert_eq!(resolved.proxy, crate::config::ProxyMode::Squid);
        assert!(resolved.secrets.is_empty());
    }

    #[test]
    fn project_proxy_mode_overrides_global() {
        let mut global = GlobalConfig::default();
        global.network.proxy = Some(crate::config::ProxyMode::Squid);
        let mut project = ProjectConfig::default();
        project.network.proxy = Some(crate::config::ProxyMode::Mitm);

        let resolved = merge(&global, &project);

        assert_eq!(resolved.proxy, crate::config::ProxyMode::Mitm);
    }
}
