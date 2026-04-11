use anyhow::Context;
use chrono::Utc;
use clap::Args;

use crate::config::{self, GlobalConfig, ProjectConfig, ProxyMode, ResolvedConfig};
use crate::core::domain::{CastorName, ImageTag, MountDir};
use crate::core::registry::{CastorEntry, Registry};
use crate::core::state::StateLock;
use crate::engine::{self, Engine};

#[derive(Debug, Args)]
pub struct AddArgs {
    /// Directory to mount into the castor. Defaults to the current dir.
    #[arg(default_value = ".")]
    pub dir: MountDir,

    /// Container image tag. Overrides `castor.image` (project) and
    /// `defaults.image` (global).
    #[arg(short = 'i', long = "image")]
    pub image: Option<ImageTag>,

    /// Castor name. Overrides `castor.name` (project) and disables the
    /// `<dir>-<n>` auto-naming fallback.
    #[arg(short = 'n', long = "name")]
    pub name: Option<CastorName>,
}

pub fn run(args: AddArgs) -> anyhow::Result<()> {
    let mount_dir = args
        .dir
        .clone()
        .to_absolute()
        .context("failed to resolve mount directory to an absolute path")?;
    mount_dir
        .validate_safe_source()
        .map_err(anyhow::Error::msg)
        .context("refusing unsafe mount directory")?;

    let _state_lock = StateLock::acquire().context("failed to acquire castors state lock")?;
    let global = config::load::load_global().context("failed to load global config")?;
    let project =
        config::load::load_project(mount_dir.as_path()).context("failed to load project config")?;

    let engine = engine::current();
    let mut registry = Registry::load().context("failed to load registry")?;

    run_with(
        args,
        mount_dir,
        &global,
        &project,
        engine.as_ref(),
        &mut registry,
    )
}

/// Testable seam: callers inject the engine, configs, and a (possibly
/// temp-backed) registry. Production [`run`] wires up [`engine::current`],
/// the XDG-default registry, and the on-disk config files.
///
/// `mount_dir` is passed in already absolutized so tests don't have to depend
/// on the process's cwd.
pub(crate) fn run_with(
    args: AddArgs,
    mount_dir: MountDir,
    global: &GlobalConfig,
    project: &ProjectConfig,
    engine: &dyn Engine,
    registry: &mut Registry,
) -> anyhow::Result<()> {
    let (name, image) = config::resolve_identity(
        args.image,
        args.name,
        project,
        global,
        mount_dir.as_path(),
        |n| registry.get(n).is_some(),
    )?;

    let resolved = config::merge::merge(global, project);

    let entry = CastorEntry {
        name,
        image,
        mount_dir,
        created_at: Utc::now(),
    };

    engine
        .ensure_infra(&resolved)
        .context("failed to bring up shared infrastructure")?;
    engine
        .create_and_start(&entry, &resolved)
        .with_context(|| format!("failed to start container for castor '{}'", entry.name))?;

    warn_about_secret_injection(&resolved);

    registry
        .insert(entry.clone())
        .with_context(|| format!("failed to add castor '{}'", entry.name))?;
    registry.save().context("failed to persist registry")?;
    engine
        .refresh_proxy_policy(registry)
        .context("failed to refresh proxy policy")?;

    println!("added castor '{}'", entry.name);
    Ok(())
}

pub(crate) fn warn_about_secret_injection(resolved: &ResolvedConfig) {
    if !resolved.secrets.is_empty() && resolved.proxy == ProxyMode::Squid {
        eprintln!(
            "warning: {} secret injection rule(s): squid-mode can only inject secrets into headers for HTTP requests. HTTPS requires mitm-mode with trusted CA in the image. See docs/networking.md.",
            resolved.secrets.len(),
        );
    }

    if !resolved.secrets.is_empty() && resolved.proxy == ProxyMode::Mitm {
        eprintln!(
            "warning: {} secret injection rule(s): mitm-mode can inject secrets into headers for both HTTP and HTTPS requests. However, to make HTTPS work at all, the castor image must trust the mitmproxy root CA. See docs/networking.md.",
            resolved.secrets.len(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::test_helpers::{cn, fresh_registry, sample_entry};
    use crate::config::schema::{GlobalDefaults, ProjectCastor};
    use crate::engine::mock::{EngineCall, MockEngine};

    /// Builds an `AddArgs` with the bare-minimum CLI form (only a dir).
    fn args(dir: &str) -> AddArgs {
        AddArgs {
            dir: dir.parse().unwrap(),
            image: None,
            name: None,
        }
    }

    fn args_with(dir: &str, image: Option<&str>, name: Option<&str>) -> AddArgs {
        AddArgs {
            dir: dir.parse().unwrap(),
            image: image.map(|s| s.parse().unwrap()),
            name: name.map(|s| s.parse().unwrap()),
        }
    }

    fn mount(dir: &str) -> MountDir {
        // Tests pass absolute paths to avoid cwd dependence.
        dir.parse::<MountDir>().unwrap().to_absolute().unwrap()
    }

    fn global_with_default_image(image: &str) -> GlobalConfig {
        GlobalConfig {
            defaults: GlobalDefaults {
                image: Some(image.parse().unwrap()),
            },
            ..Default::default()
        }
    }

    fn project_with(name: Option<&str>, image: Option<&str>) -> ProjectConfig {
        ProjectConfig {
            castor: ProjectCastor {
                name: name.map(|s| s.parse().unwrap()),
                image: image.map(|s| s.parse().unwrap()),
            },
            ..Default::default()
        }
    }

    // ----- happy path -------------------------------------------------------

    #[test]
    fn registers_castor_after_starting_container() {
        let (_tmp, mut registry) = fresh_registry();
        let engine = MockEngine::new();

        run_with(
            args_with("/tmp/x", Some("img:1"), Some("alpha")),
            mount("/tmp/x"),
            &GlobalConfig::default(),
            &ProjectConfig::default(),
            &engine,
            &mut registry,
        )
        .unwrap();

        assert_eq!(registry.len(), 1);
        let stored = registry.get(&cn("alpha")).unwrap();
        assert_eq!(stored.image.as_str(), "img:1");

        let calls = engine.calls();
        assert_eq!(calls.len(), 3);
        assert_eq!(
            calls[0],
            EngineCall::EnsureInfra(crate::config::ProxyMode::Squid)
        );
        assert!(matches!(
            calls[1],
            EngineCall::CreateAndStart(ref n, _) if *n == cn("alpha")
        ));
        assert_eq!(calls[2], EngineCall::RefreshProxyPolicy);
    }

    #[test]
    fn ensure_infra_runs_before_create_and_start() {
        let (_tmp, mut registry) = fresh_registry();
        let engine = MockEngine::new();

        run_with(
            args_with("/tmp/x", Some("img:1"), Some("alpha")),
            mount("/tmp/x"),
            &GlobalConfig::default(),
            &ProjectConfig::default(),
            &engine,
            &mut registry,
        )
        .unwrap();

        let calls = engine.calls();
        let infra_idx = calls
            .iter()
            .position(|c| *c == EngineCall::EnsureInfra(crate::config::ProxyMode::Squid));
        let start_idx = calls
            .iter()
            .position(|c| matches!(c, EngineCall::CreateAndStart(_, _)));
        assert!(infra_idx < start_idx);
    }

    // ----- identity resolution -----------------------------------------------

    #[test]
    fn auto_names_from_dir_when_nothing_else_provides_one() {
        let (_tmp, mut registry) = fresh_registry();
        let engine = MockEngine::new();

        run_with(
            args_with("/work/myrepo", Some("img:1"), None),
            mount("/work/myrepo"),
            &GlobalConfig::default(),
            &ProjectConfig::default(),
            &engine,
            &mut registry,
        )
        .unwrap();

        assert!(registry.get(&cn("myrepo-1")).is_some());
    }

    #[test]
    fn auto_naming_skips_existing_suffixes() {
        let (_tmp, mut registry) = fresh_registry();
        registry
            .insert(sample_entry("myrepo-1", "img:0", "/tmp/x"))
            .unwrap();
        let engine = MockEngine::new();

        run_with(
            args_with("/work/myrepo", Some("img:1"), None),
            mount("/work/myrepo"),
            &GlobalConfig::default(),
            &ProjectConfig::default(),
            &engine,
            &mut registry,
        )
        .unwrap();

        assert!(registry.get(&cn("myrepo-2")).is_some());
    }

    #[test]
    fn project_image_used_when_cli_omits_image() {
        let (_tmp, mut registry) = fresh_registry();
        let engine = MockEngine::new();

        run_with(
            args_with("/tmp/x", None, Some("alpha")),
            mount("/tmp/x"),
            &GlobalConfig::default(),
            &project_with(None, Some("from-project:tag")),
            &engine,
            &mut registry,
        )
        .unwrap();

        let stored = registry.get(&cn("alpha")).unwrap();
        assert_eq!(stored.image.as_str(), "from-project:tag");
    }

    #[test]
    fn global_default_image_used_as_last_resort() {
        let (_tmp, mut registry) = fresh_registry();
        let engine = MockEngine::new();

        run_with(
            args_with("/tmp/x", None, Some("alpha")),
            mount("/tmp/x"),
            &global_with_default_image("global:tag"),
            &ProjectConfig::default(),
            &engine,
            &mut registry,
        )
        .unwrap();

        let stored = registry.get(&cn("alpha")).unwrap();
        assert_eq!(stored.image.as_str(), "global:tag");
    }

    #[test]
    fn errors_when_no_image_anywhere() {
        let (_tmp, mut registry) = fresh_registry();
        let engine = MockEngine::new();

        let err = run_with(
            args("/tmp/x"),
            mount("/tmp/x"),
            &GlobalConfig::default(),
            &ProjectConfig::default(),
            &engine,
            &mut registry,
        )
        .unwrap_err();

        assert!(err.to_string().contains("no image specified"));
        assert!(engine.calls().is_empty());
        assert_eq!(registry.len(), 0);
    }

    // ----- config threading --------------------------------------------------

    #[test]
    fn merged_env_reaches_engine() {
        let (_tmp, mut registry) = fresh_registry();
        let engine = MockEngine::new();

        let mut global = GlobalConfig::default();
        global.env.insert("RUST_LOG".to_owned(), "info".to_owned());
        global.env.insert("KEEP".to_owned(), "yes".to_owned());

        let mut project = ProjectConfig::default();
        project
            .env
            .insert("RUST_LOG".to_owned(), "debug".to_owned());

        run_with(
            args_with("/tmp/x", Some("img:1"), Some("alpha")),
            mount("/tmp/x"),
            &global,
            &project,
            &engine,
            &mut registry,
        )
        .unwrap();

        let cfg = engine
            .calls()
            .into_iter()
            .find_map(|c| match c {
                EngineCall::CreateAndStart(_, cfg) => Some(cfg),
                _ => None,
            })
            .unwrap();

        assert_eq!(cfg.env["RUST_LOG"], "debug");
        assert_eq!(cfg.env["KEEP"], "yes");
    }

    // ----- collisions and failure modes --------------------------------------

    #[test]
    fn rejects_duplicate_explicit_name_without_touching_engine() {
        let (_tmp, mut registry) = fresh_registry();
        registry
            .insert(sample_entry("alpha", "img:0", "/tmp/x"))
            .unwrap();
        let engine = MockEngine::new();

        let err = run_with(
            args_with("/tmp/y", Some("img:1"), Some("alpha")),
            mount("/tmp/y"),
            &GlobalConfig::default(),
            &ProjectConfig::default(),
            &engine,
            &mut registry,
        )
        .unwrap_err();

        assert!(err.to_string().contains("already taken"));
        assert!(engine.calls().is_empty());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn does_not_register_when_create_and_start_fails() {
        let (_tmp, mut registry) = fresh_registry();
        let engine = MockEngine::new();
        engine.fail_create_and_start.set(true);

        let err = run_with(
            args_with("/tmp/x", Some("img:1"), Some("alpha")),
            mount("/tmp/x"),
            &GlobalConfig::default(),
            &ProjectConfig::default(),
            &engine,
            &mut registry,
        )
        .unwrap_err();

        assert!(err.to_string().contains("failed to start container"));
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn skips_create_and_start_when_ensure_infra_fails() {
        let (_tmp, mut registry) = fresh_registry();
        let engine = MockEngine::new();
        engine.fail_ensure_infra.set(true);

        let _ = run_with(
            args_with("/tmp/x", Some("img:1"), Some("alpha")),
            mount("/tmp/x"),
            &GlobalConfig::default(),
            &ProjectConfig::default(),
            &engine,
            &mut registry,
        )
        .unwrap_err();

        assert_eq!(
            engine.calls(),
            vec![EngineCall::EnsureInfra(crate::config::ProxyMode::Squid)]
        );
        assert_eq!(registry.len(), 0);
    }
}
