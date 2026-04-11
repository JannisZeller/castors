//! Pure argv builders for `docker run / exec / start / inspect / ps / rm`.
//!
//! Keeping argument construction in pure functions means the most error-prone
//! part of the backend (getting the right flags in the right order) can be
//! unit-tested without invoking docker.

use crate::config::ResolvedConfig;
use crate::core::domain::CastorName;
use crate::core::registry::CastorEntry;
use crate::engine::docker::labels::{NAME_KEY, ROLE_CASTOR, ROLE_KEY};
use crate::proxy;

use super::super::SHARED_NETWORK_NAME;
use super::container_name;

/// Canonical mount point inside the castor container.
///
/// Images may chdir elsewhere via their entrypoint; this is just where
/// `castors` will bind-mount the host directory and set the initial workdir.
pub const WORKSPACE_PATH: &str = "/workspace";

/// Subdirectory of [`WORKSPACE_PATH`] that is shadowed by a read-only tmpfs
/// at container start, hiding the operator's `.castors/config.yaml` from the
/// agent inside the container. See `docs/networking.md`.
pub const PROJECT_CONFIG_SHADOW: &str = "/workspace/.castors";

/// Hostnames/IPs that should stay local to the castor and not be sent through
/// the shared HTTP proxy.
pub const NO_PROXY_VALUE: &str = "localhost,127.0.0.1,::1";

/// `docker run -d --name ... --label ... --cap-drop ALL
///   --tmpfs /tmp --tmpfs /run --tmpfs /var/tmp
///   --network castors-shared
///   --mount type=tmpfs,destination=/workspace/.castors,readonly
///   -e KEY=VAL ... -e HTTP_PROXY=... -w /workspace IMAGE`
///
/// Flag order:
/// 1. `-d` and labels first — mandatory framing.
/// 2. Container hardening flags: drop ambient capabilities, block privilege
///    escalation, and add explicit tmpfs scratch areas for runtime state.
/// 3. Internal shared network attachment so the proxy is reachable by container
///    name while direct internet egress is blocked by Docker.
/// 4. Bind mount of the workdir.
/// 5. Read-only tmpfs over `/workspace/.castors`. Layered *after* the bind
///    mount so it shadows whatever the host had there.
/// 6. Env vars from the resolved config (sorted by key for deterministic argv),
///    then backend-controlled proxy env vars.
/// 7. Workdir, then image last (positional argument).
#[must_use]
pub fn run_args(entry: &CastorEntry, config: &ResolvedConfig) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "-d".into(),
        "--name".into(),
        container_name(&entry.name),
        "--label".into(),
        format!("{ROLE_KEY}={ROLE_CASTOR}"),
        "--label".into(),
        format!("{NAME_KEY}={}", entry.name),
        // Remove all ambient Linux capabilities from the container. The image
        // can still run normal userland code, but it does not get default
        // extras such as chown, net raw sockets, or module/admin-style powers.
        "--cap-drop".into(),
        "ALL".into(),
        // Prevent processes from gaining additional privileges via setuid
        // binaries or file capabilities.
        "--security-opt".into(),
        "no-new-privileges".into(),
        // Writable scratch areas for shells, package tools, lock files, and
        // runtime state.
        "--tmpfs".into(),
        "/tmp".into(),
        "--tmpfs".into(),
        "/run".into(),
        "--tmpfs".into(),
        "/var/tmp".into(),
        "--network".into(),
        SHARED_NETWORK_NAME.into(),
        "-v".into(),
        format!("{}:{WORKSPACE_PATH}", entry.mount_dir.as_path().display()),
        "--mount".into(),
        format!("type=tmpfs,destination={PROJECT_CONFIG_SHADOW},readonly"),
    ];

    // BTreeMap iteration is already key-sorted, so argv stays deterministic.
    for (key, value) in &config.env {
        args.push("-e".into());
        args.push(format!("{key}={value}"));
    }
    for (key, value) in proxy_env(config) {
        args.push("-e".into());
        args.push(format!("{key}={value}"));
    }

    args.push("-w".into());
    args.push(WORKSPACE_PATH.into());
    args.push(entry.image.as_str().into());
    args
}

fn proxy_env(config: &ResolvedConfig) -> [(&'static str, String); 6] {
    let proxy_url = proxy::service(config.proxy).proxy_url();
    [
        ("HTTP_PROXY", proxy_url.clone()),
        ("http_proxy", proxy_url.clone()),
        ("HTTPS_PROXY", proxy_url.clone()),
        ("https_proxy", proxy_url.clone()),
        ("NO_PROXY", NO_PROXY_VALUE.to_owned()),
        ("no_proxy", NO_PROXY_VALUE.to_owned()),
    ]
}

/// `docker start CONTAINER`
#[must_use]
pub fn start_args(name: &CastorName) -> Vec<String> {
    vec!["start".into(), container_name(name)]
}

/// Script passed to `docker exec … /bin/sh -c …` when no explicit shell path
/// is given: run the first executable among zsh, bash, and sh, else `exec
/// /bin/sh`.
pub(crate) const EXEC_SHELL_AUTO_SCRIPT: &str = concat!(
    "for s in /bin/zsh /bin/bash /bin/sh; do ",
    "[ -x \"$s\" ] && exec \"$s\"; ",
    "done; ",
    "exec /bin/sh"
);

/// `docker exec -it CONTAINER /bin/sh -c '<auto>'`, or `docker exec -it
/// CONTAINER <shell>` when `shell` is `Some`.
#[must_use]
pub fn exec_args(name: &CastorName, shell: Option<&str>) -> Vec<String> {
    let mut args = vec!["exec".into(), "-it".into(), container_name(name)];
    match shell {
        Some(path) => args.push(path.to_owned()),
        None => {
            args.push("/bin/sh".into());
            args.push("-c".into());
            args.push(EXEC_SHELL_AUTO_SCRIPT.into());
        }
    }
    args
}

/// `docker rm -f CONTAINER` — force-removes (and stops) in one call.
#[must_use]
pub fn rm_force_args(name: &CastorName) -> Vec<String> {
    vec!["rm".into(), "-f".into(), container_name(name)]
}

/// `docker inspect --format '{{.State.Status}}|{{.State.ExitCode}}' CONTAINER`
#[must_use]
pub fn inspect_args(name: &CastorName) -> Vec<String> {
    vec![
        "inspect".into(),
        "--format".into(),
        "{{.State.Status}}|{{.State.ExitCode}}".into(),
        container_name(name),
    ]
}

/// `docker ps -a --filter label=castors.role=castor --format '{{.Label "castors.name"}}|{{.State}}'`
#[must_use]
pub fn list_args() -> Vec<String> {
    vec![
        "ps".into(),
        "-a".into(),
        "--filter".into(),
        format!("label={ROLE_KEY}={ROLE_CASTOR}"),
        "--format".into(),
        format!("{{{{.Label \"{NAME_KEY}\"}}}}|{{{{.State}}}}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::domain::{ImageTag, MountDir};
    use chrono::{DateTime, Utc};
    use std::collections::BTreeMap;
    use std::str::FromStr;

    fn entry(name: &str, image: &str, dir: &str) -> CastorEntry {
        CastorEntry {
            name: CastorName::from_str(name).unwrap(),
            image: ImageTag::from_str(image).unwrap(),
            mount_dir: MountDir::from_str(dir).unwrap(),
            created_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
        }
    }

    #[test]
    fn run_args_has_labels_and_mount() {
        let args = run_args(
            &entry("alpha", "img:latest", "/tmp/x"),
            &ResolvedConfig::default(),
        );
        assert_eq!(args[0], "run");
        assert_eq!(args[1], "-d");
        assert!(args.contains(&"--name".to_string()));
        assert!(args.contains(&"castor-alpha".to_string()));
        assert!(args.contains(&"castors.role=castor".to_string()));
        assert!(args.contains(&"castors.name=alpha".to_string()));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--network" && w[1] == SHARED_NETWORK_NAME)
        );
        assert!(args.contains(&"/tmp/x:/workspace".to_string()));
        assert_eq!(args.last().unwrap(), "img:latest");
    }

    #[test]
    fn run_args_shadows_project_config_dir_with_readonly_tmpfs() {
        let args = run_args(
            &entry("alpha", "img:1", "/tmp/x"),
            &ResolvedConfig::default(),
        );
        assert!(args.windows(2).any(|w| {
            w[0] == "--mount" && w[1] == "type=tmpfs,destination=/workspace/.castors,readonly"
        }));
    }

    #[test]
    fn run_args_applies_default_container_hardening() {
        let args = run_args(
            &entry("alpha", "img:1", "/tmp/x"),
            &ResolvedConfig::default(),
        );

        assert!(
            args.windows(2)
                .any(|w| w[0] == "--cap-drop" && w[1] == "ALL")
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--security-opt" && w[1] == "no-new-privileges")
        );
        assert!(args.windows(2).any(|w| w[0] == "--tmpfs" && w[1] == "/tmp"));
        assert!(args.windows(2).any(|w| w[0] == "--tmpfs" && w[1] == "/run"));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--tmpfs" && w[1] == "/var/tmp")
        );
        assert!(
            !args
                .windows(2)
                .any(|w| w[0] == "--tmpfs" && w[1] == "/root")
        );
    }

    #[test]
    fn run_args_emits_env_in_sorted_order() {
        let mut env = BTreeMap::new();
        env.insert("BBB".to_owned(), "2".to_owned());
        env.insert("AAA".to_owned(), "1".to_owned());
        env.insert("CCC".to_owned(), "3".to_owned());
        let cfg = ResolvedConfig {
            env,
            ..Default::default()
        };

        let args = run_args(&entry("alpha", "img:1", "/tmp/x"), &cfg);
        let env_pairs: Vec<&str> = args
            .windows(2)
            .filter(|w| w[0] == "-e")
            .map(|w| w[1].as_str())
            .collect();

        assert_eq!(
            env_pairs,
            vec![
                "AAA=1",
                "BBB=2",
                "CCC=3",
                "HTTP_PROXY=http://castors-infra-squid:3128",
                "http_proxy=http://castors-infra-squid:3128",
                "HTTPS_PROXY=http://castors-infra-squid:3128",
                "https_proxy=http://castors-infra-squid:3128",
                "NO_PROXY=localhost,127.0.0.1,::1",
                "no_proxy=localhost,127.0.0.1,::1"
            ]
        );
    }

    #[test]
    fn run_args_injects_shared_proxy_env_vars() {
        let args = run_args(
            &entry("alpha", "img:1", "/tmp/x"),
            &ResolvedConfig::default(),
        );

        assert!(
            args.windows(2)
                .any(|w| { w[0] == "-e" && w[1] == "HTTP_PROXY=http://castors-infra-squid:3128" })
        );
        assert!(
            args.windows(2)
                .any(|w| { w[0] == "-e" && w[1] == "HTTPS_PROXY=http://castors-infra-squid:3128" })
        );
        assert!(
            args.windows(2)
                .any(|w| { w[0] == "-e" && w[1] == "NO_PROXY=localhost,127.0.0.1,::1" })
        );
    }

    #[test]
    fn run_args_routes_proxy_env_to_mitm_when_configured() {
        let cfg = ResolvedConfig {
            proxy: crate::config::ProxyMode::Mitm,
            ..Default::default()
        };
        let args = run_args(&entry("alpha", "img:1", "/tmp/x"), &cfg);

        assert!(
            args.windows(2)
                .any(|w| { w[0] == "-e" && w[1] == "HTTP_PROXY=http://castors-infra-mitm:8080" })
        );
        assert!(
            args.windows(2)
                .any(|w| { w[0] == "-e" && w[1] == "HTTPS_PROXY=http://castors-infra-mitm:8080" })
        );
    }

    #[test]
    fn backend_proxy_env_is_emitted_after_user_env() {
        let mut env = BTreeMap::new();
        env.insert("HTTP_PROXY".to_owned(), "http://user-proxy:8080".to_owned());
        let cfg = ResolvedConfig {
            env,
            ..Default::default()
        };

        let args = run_args(&entry("alpha", "img:1", "/tmp/x"), &cfg);
        let env_pairs: Vec<&str> = args
            .windows(2)
            .filter(|w| w[0] == "-e")
            .map(|w| w[1].as_str())
            .collect();

        let user_idx = env_pairs
            .iter()
            .position(|p| *p == "HTTP_PROXY=http://user-proxy:8080")
            .unwrap();
        let backend_idx = env_pairs
            .iter()
            .position(|p| *p == "HTTP_PROXY=http://castors-infra-squid:3128")
            .unwrap();
        assert!(user_idx < backend_idx);
    }

    #[test]
    fn run_args_image_remains_last_positional_after_env_injection() {
        let mut env = BTreeMap::new();
        env.insert("FOO".to_owned(), "bar".to_owned());
        let cfg = ResolvedConfig {
            env,
            ..Default::default()
        };

        let args = run_args(&entry("alpha", "img:final", "/tmp/x"), &cfg);

        assert_eq!(args.last().unwrap(), "img:final");
        // -w /workspace must immediately precede the image.
        let last_idx = args.len() - 1;
        assert_eq!(args[last_idx - 2], "-w");
        assert_eq!(args[last_idx - 1], WORKSPACE_PATH);
    }

    #[test]
    fn run_args_with_empty_config_emits_backend_proxy_env() {
        let args = run_args(
            &entry("alpha", "img:1", "/tmp/x"),
            &ResolvedConfig::default(),
        );
        assert_eq!(args.iter().filter(|a| *a == "-e").count(), 6);
    }

    #[test]
    fn exec_args_auto_prefers_inline_shell_probe() {
        let args = exec_args(&CastorName::from_str("alpha").unwrap(), None);
        assert_eq!(
            args,
            vec![
                "exec",
                "-it",
                "castor-alpha",
                "/bin/sh",
                "-c",
                EXEC_SHELL_AUTO_SCRIPT,
            ]
        );
    }

    #[test]
    fn exec_args_explicit_shell_is_single_argv() {
        let args = exec_args(&CastorName::from_str("alpha").unwrap(), Some("/bin/bash"));
        assert_eq!(args, vec!["exec", "-it", "castor-alpha", "/bin/bash"]);
    }

    #[test]
    fn rm_force_args_uses_force_flag() {
        let args = rm_force_args(&CastorName::from_str("alpha").unwrap());
        assert_eq!(args, vec!["rm", "-f", "castor-alpha"]);
    }

    #[test]
    fn inspect_args_requests_status_and_exit_code() {
        let args = inspect_args(&CastorName::from_str("alpha").unwrap());
        assert_eq!(args[0], "inspect");
        assert_eq!(args[1], "--format");
        assert_eq!(args[2], "{{.State.Status}}|{{.State.ExitCode}}");
        assert_eq!(args[3], "castor-alpha");
    }

    #[test]
    fn list_args_filters_by_castor_role() {
        let args = list_args();
        assert!(args.contains(&"label=castors.role=castor".to_string()));
        let format = args.iter().find(|a| a.starts_with("{{.Label")).unwrap();
        assert_eq!(format, "{{.Label \"castors.name\"}}|{{.State}}");
    }
}
