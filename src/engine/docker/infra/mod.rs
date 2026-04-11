//! Shared infrastructure stack lifecycle (Docker backend).
//!
//! Owns the one-per-host Compose project named [`cmd::PROJECT_NAME`]. The
//! compose file is embedded in the binary via `include_str!` and materialized
//! on disk at `~/.castors/.state/infra/compose.yaml` the first time
//! [`ensure_running`] runs.
//!
//! Policy: the binary is the source of truth. The materialized file is
//! overwritten on every [`ensure_running`] call, so operators who need to
//! experiment should edit the template in the castors source tree (or,
//! eventually, wait for a `castors infra edit` escape hatch — see
//! `docs/polishing.md`).
//!
//! This module is deliberately unaware that castors exist: it only knows how
//! to bring its own stack up and down. The "only tear down when no castors
//! remain" policy lives one layer up in
//! [`crate::engine::docker::DockerEngine::teardown_infra_if_idle`].

mod cmd;

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use crate::engine::docker::process::{run_capture, stderr_str, stdout_str};
use crate::engine::types::EngineError;
use crate::proxy::{self, mitm, squid};

/// Generated infra filenames and subdirectories under
/// [`crate::core::paths::infra_dir`].
const COMPOSE_FILENAME: &str = "compose.yaml";
const SQUID_FILENAME: &str = "squid.conf";
const MITM_CONFIG_DIR: &str = "mitm/config";
const MITM_SCRIPTS_DIR: &str = "mitm/scripts";
const MITM_POLICY_FILENAME: &str = "policy.json";
const MITM_SECRETS_FILENAME: &str = "castors-policy-secrets.json";
const MITM_POLICY_SCRIPT_FILENAME: &str = "policy.py";
const PROXY_READY_TIMEOUT: Duration = Duration::from_secs(20);
const PROXY_READY_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// The compose file template, embedded at compile time. Kept next to the
/// module that owns it so the source-of-truth artifact lives with the code
/// that materializes it.
const COMPOSE_TEMPLATE: &str = include_str!("compose.yaml");

/// Brings the shared infra stack up. Idempotent: if the project is already
/// running, `docker compose up -d` is effectively a no-op.
pub fn ensure_running(mode: crate::config::ProxyMode) -> Result<(), EngineError> {
    let compose_file = materialize_infra_files()?;
    let service = proxy::service(mode);
    let args = cmd::up_args(&compose_file.to_string_lossy(), service.compose_profile());
    let output = run_capture(&args)?;
    if !output.status.success() {
        return Err(EngineError::Backend(format!(
            "docker compose up failed: {}",
            stderr_str(&output).trim()
        )));
    }

    verify_shared_network_is_internal()?;
    wait_until_proxy_ready(squid::CONTAINER_NAME)?;
    if let Some(profile) = service.compose_profile() {
        if profile == mitm::COMPOSE_PROFILE {
            wait_until_proxy_ready(mitm::CONTAINER_NAME)?;
        }
    }
    Ok(())
}

pub fn refresh_proxy_policy(registry: &crate::core::registry::Registry) -> Result<(), EngineError> {
    let global = crate::config::load::load_global()
        .map_err(|e| EngineError::Backend(format!("failed to load global proxy config: {e}")))?;
    let global_config =
        crate::config::merge::merge(&global, &crate::config::ProjectConfig::default());
    let containers = load_container_configs(registry, &global)?;

    materialize_squid_policy(&global_config, &containers)?;
    reconfigure_squid_if_running()?;
    materialize_mitm_policy(&global_config, &containers)
}

pub fn export_mitm_ca_certificate() -> Result<Vec<u8>, EngineError> {
    let compose_file = materialize_compose_file()?;
    materialize_mitm_script()?;
    materialize_empty_mitm_policy_if_missing()?;
    let args = cmd::export_mitm_ca_certificate_args(&compose_file.to_string_lossy());
    let output = run_capture(&args)?;
    if !output.status.success() {
        return Err(EngineError::Backend(format!(
            "failed to generate MITM CA certificate: {}",
            stderr_str(&output).trim()
        )));
    }
    if output.stdout.is_empty() {
        return Err(EngineError::Backend(
            "failed to generate MITM CA certificate: command produced no certificate".into(),
        ));
    }
    Ok(output.stdout)
}

/// Tears the shared infra stack down unconditionally. Safe to call when the
/// stack is not running: if the compose file has never been materialized,
/// this returns `Ok(())` immediately.
///
/// Callers who want to tear down *only when no castors remain* should go
/// through [`crate::engine::Engine::teardown_infra_if_idle`] instead.
pub fn teardown() -> Result<(), EngineError> {
    let path = compose_file_path()?;
    if !path.exists() {
        return Ok(());
    }
    let args = cmd::down_args(&path.to_string_lossy());
    let output = run_capture(&args)?;
    if output.status.success() {
        return Ok(());
    }
    Err(EngineError::Backend(format!(
        "docker compose down failed: {}",
        stderr_str(&output).trim()
    )))
}

#[derive(Debug, PartialEq, Eq)]
enum ProxyStatus {
    Ready,
    Waiting(String),
}

fn infra_dir() -> Result<PathBuf, EngineError> {
    crate::core::paths::infra_dir()
        .ok_or_else(|| EngineError::Backend("unable to locate user home directory".into()))
}

fn compose_file_path() -> Result<PathBuf, EngineError> {
    Ok(infra_dir()?.join(COMPOSE_FILENAME))
}

fn squid_file_path() -> Result<PathBuf, EngineError> {
    Ok(infra_dir()?.join(SQUID_FILENAME))
}

fn mitm_config_dir() -> Result<PathBuf, EngineError> {
    Ok(infra_dir()?.join(MITM_CONFIG_DIR))
}

fn mitm_scripts_dir() -> Result<PathBuf, EngineError> {
    Ok(infra_dir()?.join(MITM_SCRIPTS_DIR))
}

fn mitm_policy_path() -> Result<PathBuf, EngineError> {
    Ok(mitm_config_dir()?.join(MITM_POLICY_FILENAME))
}

fn mitm_secrets_path() -> Result<PathBuf, EngineError> {
    Ok(mitm_config_dir()?.join(MITM_SECRETS_FILENAME))
}

fn mitm_policy_script_path() -> Result<PathBuf, EngineError> {
    Ok(mitm_scripts_dir()?.join(MITM_POLICY_SCRIPT_FILENAME))
}

fn materialize_compose_file() -> Result<PathBuf, EngineError> {
    let path = compose_file_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            EngineError::Backend(format!(
                "failed to create infra dir {}: {e}",
                parent.display()
            ))
        })?;
    }
    fs::write(&path, COMPOSE_TEMPLATE).map_err(|e| {
        EngineError::Backend(format!(
            "failed to write compose file {}: {e}",
            path.display()
        ))
    })?;
    Ok(path)
}

fn materialize_infra_files() -> Result<PathBuf, EngineError> {
    let compose_file = materialize_compose_file()?;
    materialize_squid_file()?;
    materialize_mitm_script()?;
    materialize_empty_mitm_policy_if_missing()?;
    Ok(compose_file)
}

fn materialize_squid_file() -> Result<PathBuf, EngineError> {
    let global = crate::config::load::load_global()
        .map_err(|e| EngineError::Backend(format!("failed to load global proxy config: {e}")))?;
    let config = crate::config::merge::merge(&global, &crate::config::ProjectConfig::default());
    let rendered = squid::render_config(&config)
        .map_err(|e| EngineError::Backend(format!("failed to render squid.conf: {e}")))?;
    write_squid_file(rendered)
}

fn write_squid_file(rendered: String) -> Result<PathBuf, EngineError> {
    let path = squid_file_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            EngineError::Backend(format!(
                "failed to create infra dir {}: {e}",
                parent.display()
            ))
        })?;
    }
    fs::write(&path, rendered).map_err(|e| {
        EngineError::Backend(format!(
            "failed to write squid config {}: {e}",
            path.display()
        ))
    })?;
    Ok(path)
}

#[derive(Debug)]
struct ResolvedContainerPolicy {
    name: String,
    ips: Vec<String>,
    config: crate::config::ResolvedConfig,
}

fn materialize_mitm_script() -> Result<PathBuf, EngineError> {
    const POLICY_SCRIPT: &str = include_str!("../../../proxy/mitm/policy.py");
    let path = mitm_policy_script_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            EngineError::Backend(format!(
                "failed to create MITM scripts dir {}: {e}",
                parent.display()
            ))
        })?;
    }
    fs::write(&path, POLICY_SCRIPT).map_err(|e| {
        EngineError::Backend(format!(
            "failed to write MITM policy script {}: {e}",
            path.display()
        ))
    })?;
    Ok(path)
}

fn materialize_empty_mitm_policy_if_missing() -> Result<(), EngineError> {
    let policy_path = mitm_policy_path()?;
    let secrets_path = mitm_secrets_path()?;
    if let Some(parent) = policy_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            EngineError::Backend(format!(
                "failed to create MITM config dir {}: {e}",
                parent.display()
            ))
        })?;
    }
    if !policy_path.exists() {
        let rendered = mitm::render_policy(&crate::config::ResolvedConfig::default(), &[])
            .map_err(|e| EngineError::Backend(format!("failed to render MITM policy: {e}")))?;
        fs::write(&policy_path, rendered.policy_json).map_err(|e| {
            EngineError::Backend(format!(
                "failed to write MITM policy {}: {e}",
                policy_path.display()
            ))
        })?;
        fs::write(&secrets_path, rendered.secrets_json).map_err(|e| {
            EngineError::Backend(format!(
                "failed to write MITM secret bundle {}: {e}",
                secrets_path.display()
            ))
        })?;
    } else if !secrets_path.exists() {
        fs::write(&secrets_path, "{}").map_err(|e| {
            EngineError::Backend(format!(
                "failed to write MITM secret bundle {}: {e}",
                secrets_path.display()
            ))
        })?;
    }
    Ok(())
}

fn load_container_configs(
    registry: &crate::core::registry::Registry,
    global: &crate::config::GlobalConfig,
) -> Result<Vec<ResolvedContainerPolicy>, EngineError> {
    let mut containers = Vec::new();

    for entry in registry.list() {
        let project =
            crate::config::load::load_project(entry.mount_dir.as_path()).map_err(|e| {
                EngineError::Backend(format!(
                    "failed to load project config for castor '{}': {e}",
                    entry.name
                ))
            })?;
        let config = crate::config::merge::merge(global, &project);
        let ip = inspect_castor_ip(&entry.name)?;
        containers.push(ResolvedContainerPolicy {
            name: format!("castor-{}", entry.name.as_str()),
            ips: ip.into_iter().collect(),
            config,
        });
    }

    Ok(containers)
}

fn materialize_squid_policy(
    global_config: &crate::config::ResolvedConfig,
    containers: &[ResolvedContainerPolicy],
) -> Result<(), EngineError> {
    let containers = containers
        .iter()
        .map(|container| squid::ContainerPolicy {
            name: container.name.clone(),
            ips: container.ips.clone(),
            config: container.config.clone(),
        })
        .collect::<Vec<_>>();
    let rendered = squid::render_policy(global_config, &containers)
        .map_err(|e| EngineError::Backend(format!("failed to render squid.conf: {e}")))?;
    write_squid_file(rendered)?;
    Ok(())
}

fn reconfigure_squid_if_running() -> Result<(), EngineError> {
    let status = run_capture(&cmd::inspect_container_health_args(squid::CONTAINER_NAME))?;
    if !status.status.success() {
        let stderr = stderr_str(&status);
        if is_missing_or_stopped_container(&stderr) {
            return Ok(());
        }
        return Err(EngineError::Backend(format!(
            "failed to inspect Squid container before reconfigure: {}",
            stderr.trim()
        )));
    }

    if !container_is_running(&stdout_str(&status)) {
        return Ok(());
    }

    let output = run_capture(&cmd::squid_reconfigure_args())?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = stderr_str(&output);
    if is_missing_or_stopped_container(&stderr) {
        return Ok(());
    }

    Err(EngineError::Backend(format!(
        "failed to reconfigure Squid after policy refresh: {}",
        stderr.trim()
    )))
}

fn container_is_running(stdout: &str) -> bool {
    stdout
        .trim()
        .split_once('|')
        .is_some_and(|(state, _)| state == "running")
}

fn is_missing_or_stopped_container(stderr: &str) -> bool {
    let stderr = stderr.to_ascii_lowercase();
    stderr.contains("no such object")
        || stderr.contains("no such container")
        || stderr.contains("is not running")
}

fn materialize_mitm_policy(
    global_config: &crate::config::ResolvedConfig,
    containers: &[ResolvedContainerPolicy],
) -> Result<(), EngineError> {
    let containers = containers
        .iter()
        .filter(|container| container.config.proxy == crate::config::ProxyMode::Mitm)
        .map(|container| mitm::ContainerPolicy {
            name: container.name.clone(),
            ips: container.ips.clone(),
            config: container.config.clone(),
        })
        .collect::<Vec<_>>();

    let rendered = mitm::render_policy(global_config, &containers)
        .map_err(|e| EngineError::Backend(format!("failed to render MITM policy: {e}")))?;
    write_mitm_policy_files(rendered)
}

fn inspect_castor_ip(
    name: &crate::core::domain::CastorName,
) -> Result<Option<String>, EngineError> {
    let container_name = super::castors::container_name(name);
    let output = run_capture(&cmd::inspect_container_ip_args(&container_name))?;
    if !output.status.success() {
        let stderr = stderr_str(&output);
        if stderr.contains("No such object") || stderr.contains("No such container") {
            return Ok(None);
        }
        return Err(EngineError::Backend(format!(
            "failed to inspect IP for castor '{}': {}",
            name,
            stderr.trim()
        )));
    }
    let ip = stdout_str(&output).trim().to_owned();
    Ok((!ip.is_empty() && ip != "<no value>").then_some(ip))
}

fn write_mitm_policy_files(rendered: mitm::RenderedPolicy) -> Result<(), EngineError> {
    let policy_path = mitm_policy_path()?;
    let secrets_path = mitm_secrets_path()?;
    if let Some(parent) = policy_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            EngineError::Backend(format!(
                "failed to create MITM config dir {}: {e}",
                parent.display()
            ))
        })?;
    }
    fs::write(&policy_path, rendered.policy_json).map_err(|e| {
        EngineError::Backend(format!(
            "failed to write MITM policy {}: {e}",
            policy_path.display()
        ))
    })?;
    fs::write(&secrets_path, rendered.secrets_json).map_err(|e| {
        EngineError::Backend(format!(
            "failed to write MITM secret bundle {}: {e}",
            secrets_path.display()
        ))
    })?;
    Ok(())
}

fn verify_shared_network_is_internal() -> Result<(), EngineError> {
    let output = run_capture(&cmd::inspect_shared_network_internal_args())?;
    if !output.status.success() {
        return Err(EngineError::Backend(format!(
            "failed to inspect shared network isolation: {}",
            stderr_str(&output).trim()
        )));
    }

    if shared_network_is_internal(&stdout_str(&output)) {
        return Ok(());
    }

    Err(EngineError::Backend(
        "shared network 'castors-shared' is not internal; remove existing castors/infra and recreate it to enforce proxy-only egress".into(),
    ))
}

fn shared_network_is_internal(stdout: &str) -> bool {
    stdout.trim() == "true"
}

fn wait_until_proxy_ready(container_name: &str) -> Result<(), EngineError> {
    let started = Instant::now();
    let mut last_state = "not yet inspected".to_owned();

    while started.elapsed() < PROXY_READY_TIMEOUT {
        let output = run_capture(&cmd::inspect_container_health_args(container_name))?;
        if output.status.success() {
            match parse_proxy_status(&stdout_str(&output)) {
                ProxyStatus::Ready => return Ok(()),
                ProxyStatus::Waiting(state) => last_state = state,
            }
        } else {
            last_state = stderr_str(&output).trim().to_owned();
        }
        thread::sleep(PROXY_READY_POLL_INTERVAL);
    }

    Err(EngineError::Backend(format!(
        "proxy container '{container_name}' did not become ready within {}s (last state: {last_state})",
        PROXY_READY_TIMEOUT.as_secs()
    )))
}

fn parse_proxy_status(stdout: &str) -> ProxyStatus {
    let line = stdout.trim();
    let (state, health) = line.split_once('|').unwrap_or((line, "unknown"));
    match (state, health) {
        ("running", "healthy" | "none") => ProxyStatus::Ready,
        _ => ProxyStatus::Waiting(format!("{state}|{health}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_template_parses_as_non_empty_yaml_document() {
        // Not a full schema check; just guard against accidentally shipping
        // an empty or truncated template on a build-system regression.
        assert!(
            COMPOSE_TEMPLATE.contains("services:"),
            "compose template must declare a services block"
        );
        assert!(
            COMPOSE_TEMPLATE.contains("castors-shared"),
            "compose template must declare the shared network name"
        );
        assert!(
            COMPOSE_TEMPLATE.contains("internal: true"),
            "shared network must be internal so castors cannot bypass the proxy"
        );
        assert!(
            COMPOSE_TEMPLATE.contains("castors-egress"),
            "compose template must give the proxy a separate egress network"
        );
        assert!(
            COMPOSE_TEMPLATE.contains("ubuntu/squid"),
            "compose template must declare a real Squid image"
        );
        assert!(
            COMPOSE_TEMPLATE.contains("./squid.conf:/etc/squid/squid.conf:ro"),
            "compose template must mount the rendered squid.conf"
        );
    }

    #[test]
    fn compose_file_path_lives_under_castors_home_infra_subdir() {
        // We can't pin the absolute path because it depends on the test
        // environment's HOME, but the suffix is stable policy.
        let p = compose_file_path().expect("test env should expose a HOME");
        assert!(p.ends_with(".castors/.state/infra/compose.yaml"));
    }

    #[test]
    fn squid_file_path_lives_under_castors_home_infra_subdir() {
        let p = squid_file_path().expect("test env should expose a HOME");
        assert!(p.ends_with(".castors/.state/infra/squid.conf"));
    }

    #[test]
    fn parse_proxy_status_accepts_running_healthy_and_running_none() {
        assert_eq!(parse_proxy_status("running|healthy"), ProxyStatus::Ready);
        assert_eq!(parse_proxy_status("running|none"), ProxyStatus::Ready);
    }

    #[test]
    fn parse_proxy_status_waits_for_non_ready_states() {
        assert_eq!(
            parse_proxy_status("running|starting"),
            ProxyStatus::Waiting("running|starting".to_owned())
        );
        assert_eq!(
            parse_proxy_status("exited|unhealthy"),
            ProxyStatus::Waiting("exited|unhealthy".to_owned())
        );
    }

    #[test]
    fn shared_network_is_internal_only_accepts_true() {
        assert!(shared_network_is_internal("true\n"));
        assert!(!shared_network_is_internal("false\n"));
        assert!(!shared_network_is_internal(""));
    }

    #[test]
    fn container_is_running_only_accepts_running_state() {
        assert!(container_is_running("running|healthy\n"));
        assert!(container_is_running("running|none\n"));
        assert!(!container_is_running("exited|none\n"));
        assert!(!container_is_running(""));
    }

    #[test]
    fn missing_or_stopped_container_errors_are_non_fatal_for_reconfigure() {
        assert!(is_missing_or_stopped_container(
            "Error: No such container: castors-infra-squid"
        ));
        assert!(is_missing_or_stopped_container(
            "error: no such object: castors-infra-squid"
        ));
        assert!(is_missing_or_stopped_container(
            "container castors-infra-squid is not running"
        ));
        assert!(!is_missing_or_stopped_container("squid: Bungled config"));
    }

    // Note: we deliberately do NOT unit-test materialize_compose_file here.
    // Overriding HOME at test time races with other threads in the parallel
    // test runner (dirs::home_dir reads $HOME on Unix), and the function is
    // trivial wrapper code — `create_dir_all` + `fs::write` — so the cost /
    // value tradeoff is bad. End-to-end coverage comes from manual smoke
    // tests with a real docker daemon.
}
