//! Pure argv builders for `docker compose ...` against the shared infra
//! project.
//!
//! Same pattern as [`super::super::castors::cmd`]: construction lives in
//! pure functions so the tricky flag ordering can be exhaustively unit
//! tested without spawning docker.

/// Compose project name used for the shared infra stack. Fixed so repeated
/// invocations across processes always target the same project regardless
/// of where the binary is invoked from (compose defaults the project to the
/// compose file's parent directory name otherwise).
pub const PROJECT_NAME: &str = "castors-infra";

/// Shared internal network that castor containers attach to directly.
pub const SHARED_NETWORK_NAME: &str = super::super::SHARED_NETWORK_NAME;

pub const MITM_CA_EXPORT_SCRIPT: &str = r#"cert=/home/mitmproxy/.mitmproxy/mitmproxy-ca-cert.cer
mitmdump --listen-host 127.0.0.1 --listen-port 8080 >/tmp/castors-mitm-ca.log 2>&1 &
pid=$!
for _ in 1 2 3 4 5 6 7 8 9 10; do
  [ -s "$cert" ] && break
  sleep 1
done
kill "$pid" >/dev/null 2>&1 || true
wait "$pid" >/dev/null 2>&1 || true
cat "$cert""#;

/// `docker compose -p castors-infra -f <path> [--profile <profile>] up -d`
///
/// Idempotent on the docker side: if everything is already up, compose
/// simply confirms and exits 0.
#[must_use]
pub fn up_args(compose_file: &str, compose_profile: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "compose".into(),
        "-p".into(),
        PROJECT_NAME.into(),
        "-f".into(),
        compose_file.into(),
    ];
    if let Some(profile) = compose_profile {
        args.push("--profile".into());
        args.push(profile.into());
    }
    args.extend(["up".into(), "-d".into()]);
    args
}

/// `docker compose -p castors-infra -f <path> --profile mitm down`
///
/// Stops and removes the infra containers and the shared network. Named
/// volumes (e.g. `castors-proxy-logs`) intentionally persist so proxy logs
/// survive a stack bounce. Operators that want a hard wipe can invoke
/// `docker volume rm` manually.
///
/// Optional infra services must be included here too. In particular, Compose
/// otherwise ignores the profiled MITM service during `down`, leaving
/// `castors-infra-mitm` behind after the last castor is removed.
#[must_use]
pub fn down_args(compose_file: &str) -> Vec<String> {
    vec![
        "compose".into(),
        "-p".into(),
        PROJECT_NAME.into(),
        "-f".into(),
        compose_file.into(),
        "--profile".into(),
        crate::proxy::mitm::COMPOSE_PROFILE.into(),
        "down".into(),
    ]
}

/// Runs mitmproxy in a one-shot compose container and prints the generated
/// public CA certificate to stdout. The named `castors-mitm-ca` volume persists
/// the CA material after the one-shot container exits.
#[must_use]
pub fn export_mitm_ca_certificate_args(compose_file: &str) -> Vec<String> {
    vec![
        "compose".into(),
        "-p".into(),
        PROJECT_NAME.into(),
        "-f".into(),
        compose_file.into(),
        "--profile".into(),
        crate::proxy::mitm::COMPOSE_PROFILE.into(),
        "run".into(),
        "--rm".into(),
        "--no-deps".into(),
        "--entrypoint".into(),
        "sh".into(),
        "mitm".into(),
        "-c".into(),
        MITM_CA_EXPORT_SCRIPT.into(),
    ]
}

#[must_use]
pub fn inspect_container_health_args(container_name: &str) -> Vec<String> {
    vec![
        "inspect".into(),
        "--format".into(),
        "{{.State.Status}}|{{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}".into(),
        container_name.into(),
    ]
}

/// `docker network inspect --format '{{.Internal}}' castors-shared`
#[must_use]
pub fn inspect_shared_network_internal_args() -> Vec<String> {
    vec![
        "network".into(),
        "inspect".into(),
        "--format".into(),
        "{{.Internal}}".into(),
        SHARED_NETWORK_NAME.into(),
    ]
}

/// `docker inspect --format '{{(index .NetworkSettings.Networks "castors-shared").IPAddress}}' <container>`
#[must_use]
pub fn inspect_container_ip_args(container_name: &str) -> Vec<String> {
    vec![
        "inspect".into(),
        "--format".into(),
        format!(
            "{{{{(index .NetworkSettings.Networks \"{}\").IPAddress}}}}",
            SHARED_NETWORK_NAME
        ),
        container_name.into(),
    ]
}

/// `docker exec castors-infra-squid squid -k reconfigure`
#[must_use]
pub fn squid_reconfigure_args() -> Vec<String> {
    vec![
        "exec".into(),
        crate::proxy::squid::CONTAINER_NAME.into(),
        "squid".into(),
        "-k".into(),
        "reconfigure".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_args_uses_fixed_project_and_detached_flag() {
        let args = up_args("/tmp/compose.yaml", None);
        assert_eq!(
            args,
            vec![
                "compose",
                "-p",
                PROJECT_NAME,
                "-f",
                "/tmp/compose.yaml",
                "up",
                "-d",
            ]
        );
    }

    #[test]
    fn down_args_uses_fixed_project_profile_and_no_volume_flag() {
        let args = down_args("/tmp/compose.yaml");
        assert_eq!(
            args,
            vec![
                "compose",
                "-p",
                PROJECT_NAME,
                "-f",
                "/tmp/compose.yaml",
                "--profile",
                "mitm",
                "down",
            ]
        );
        assert!(
            !args.iter().any(|a| a == "--volumes" || a == "-v"),
            "down must not remove named volumes by default"
        );
    }

    #[test]
    fn up_and_down_share_project_name() {
        let up = up_args("a", None);
        let down = down_args("b");
        let up_proj = &up[up.iter().position(|a| a == "-p").unwrap() + 1];
        let down_proj = &down[down.iter().position(|a| a == "-p").unwrap() + 1];
        assert_eq!(up_proj, down_proj);
    }

    #[test]
    fn export_mitm_ca_certificate_uses_mitm_profile_and_one_shot_container() {
        let args = export_mitm_ca_certificate_args("/tmp/compose.yaml");
        assert_eq!(
            args,
            vec![
                "compose",
                "-p",
                PROJECT_NAME,
                "-f",
                "/tmp/compose.yaml",
                "--profile",
                "mitm",
                "run",
                "--rm",
                "--no-deps",
                "--entrypoint",
                "sh",
                "mitm",
                "-c",
                MITM_CA_EXPORT_SCRIPT,
            ]
        );
    }

    #[test]
    fn up_args_can_enable_compose_profile() {
        let args = up_args("/tmp/compose.yaml", Some("mitm"));
        assert_eq!(
            args,
            vec![
                "compose",
                "-p",
                PROJECT_NAME,
                "-f",
                "/tmp/compose.yaml",
                "--profile",
                "mitm",
                "up",
                "-d",
            ]
        );
    }

    #[test]
    fn project_name_is_disjoint_from_castor_container_prefix() {
        // The castor submodule uses `castor-<name>`. The infra project must
        // not collide with that namespace so operators can eyeball
        // `docker ps` output.
        assert!(!PROJECT_NAME.starts_with("castor-"));
        assert_eq!(PROJECT_NAME, "castors-infra");
    }

    #[test]
    fn inspect_container_health_args_targets_given_container() {
        let args = inspect_container_health_args("castors-infra-mitm");
        assert_eq!(
            args,
            vec![
                "inspect",
                "--format",
                "{{.State.Status}}|{{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}",
                "castors-infra-mitm",
            ]
        );
    }

    #[test]
    fn inspect_shared_network_internal_args_targets_shared_network() {
        let args = inspect_shared_network_internal_args();
        assert_eq!(
            args,
            vec![
                "network",
                "inspect",
                "--format",
                "{{.Internal}}",
                SHARED_NETWORK_NAME,
            ]
        );
    }

    #[test]
    fn inspect_container_ip_args_targets_shared_network() {
        let args = inspect_container_ip_args("castor-alpha");
        assert_eq!(
            args,
            vec![
                "inspect",
                "--format",
                "{{(index .NetworkSettings.Networks \"castors-shared\").IPAddress}}",
                "castor-alpha",
            ]
        );
    }

    #[test]
    fn squid_reconfigure_args_targets_squid_container() {
        assert_eq!(
            squid_reconfigure_args(),
            vec!["exec", "castors-infra-squid", "squid", "-k", "reconfigure"]
        );
    }
}
