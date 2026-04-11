use anyhow::Context;
use chrono::Utc;
use clap::Args;

use crate::config::{self, GlobalConfig, ProjectConfig};
use crate::core::domain::CastorName;
use crate::core::registry::{CastorEntry, Registry};
use crate::core::state::StateLock;
use crate::engine::{self, Engine};

#[derive(Debug, Args)]
pub struct RestartArgs {
    /// Name of the castor to recreate.
    pub castor_name: CastorName,
}

pub fn run(args: RestartArgs) -> anyhow::Result<()> {
    let _state_lock = StateLock::acquire().context("failed to acquire castors state lock")?;
    let engine = engine::current();
    let mut registry = Registry::load().context("failed to load registry")?;
    let existing = registry
        .get(&args.castor_name)
        .cloned()
        .with_context(|| format!("castor '{}' not found", args.castor_name))?;
    existing
        .mount_dir
        .validate_safe_source()
        .map_err(anyhow::Error::msg)
        .context("refusing unsafe mount directory")?;

    let global = config::load::load_global().context("failed to load global config")?;
    let project = config::load::load_project(existing.mount_dir.as_path())
        .context("failed to load project config")?;

    run_with(args, &global, &project, engine.as_ref(), &mut registry)
}

pub(crate) fn run_with(
    args: RestartArgs,
    global: &GlobalConfig,
    project: &ProjectConfig,
    engine: &dyn Engine,
    registry: &mut Registry,
) -> anyhow::Result<()> {
    let existing = registry
        .get(&args.castor_name)
        .cloned()
        .with_context(|| format!("castor '{}' not found", args.castor_name))?;
    existing
        .mount_dir
        .validate_safe_source()
        .map_err(anyhow::Error::msg)
        .context("refusing unsafe mount directory")?;

    let resolved = config::merge::merge(global, project);
    let image = project
        .castor
        .image
        .clone()
        .or_else(|| global.defaults.image.clone())
        .unwrap_or_else(|| existing.image.clone());
    let replacement = CastorEntry {
        name: existing.name.clone(),
        image,
        mount_dir: existing.mount_dir,
        created_at: Utc::now(),
    };

    engine
        .ensure_infra(&resolved)
        .context("failed to bring up shared infrastructure")?;
    engine
        .stop_and_remove(&replacement.name)
        .with_context(|| format!("failed to stop container for castor '{}'", replacement.name))?;
    engine
        .create_and_start(&replacement, &resolved)
        .with_context(|| {
            format!(
                "failed to start container for castor '{}'",
                replacement.name
            )
        })?;

    registry
        .remove(&replacement.name)
        .with_context(|| format!("failed to update castor '{}'", replacement.name))?;
    registry
        .insert(replacement.clone())
        .with_context(|| format!("failed to update castor '{}'", replacement.name))?;
    registry.save().context("failed to persist registry")?;
    engine
        .refresh_proxy_policy(registry)
        .context("failed to refresh proxy policy")?;

    super::add::warn_about_secret_injection(&resolved);
    println!("restarted castor '{}'", replacement.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::test_helpers::{cn, fresh_registry, sample_entry};
    use crate::config::schema::{NetworkConfig, ProjectCastor};
    use crate::engine::mock::{EngineCall, MockEngine};

    fn args(name: &str) -> RestartArgs {
        RestartArgs {
            castor_name: cn(name),
        }
    }

    #[test]
    fn recreates_registered_castor_and_refreshes_policy() {
        let (_tmp, mut registry) = fresh_registry();
        registry
            .insert(sample_entry("alpha", "old:tag", "/tmp/x"))
            .unwrap();
        let engine = MockEngine::new();
        let global = GlobalConfig {
            network: NetworkConfig {
                proxy: Some(crate::config::ProxyMode::Mitm),
                ..Default::default()
            },
            ..Default::default()
        };
        let project = ProjectConfig {
            castor: ProjectCastor {
                image: Some("new:tag".parse().unwrap()),
                ..Default::default()
            },
            ..Default::default()
        };

        run_with(args("alpha"), &global, &project, &engine, &mut registry).unwrap();

        assert_eq!(
            registry.get(&cn("alpha")).unwrap().image.as_str(),
            "new:tag"
        );
        assert_eq!(
            engine.calls(),
            vec![
                EngineCall::EnsureInfra(crate::config::ProxyMode::Mitm),
                EngineCall::StopAndRemove(cn("alpha")),
                EngineCall::CreateAndStart(cn("alpha"), config::merge::merge(&global, &project)),
                EngineCall::RefreshProxyPolicy,
            ]
        );
    }

    #[test]
    fn falls_back_to_existing_image_when_config_has_no_image() {
        let (_tmp, mut registry) = fresh_registry();
        registry
            .insert(sample_entry("alpha", "old:tag", "/tmp/x"))
            .unwrap();
        let engine = MockEngine::new();

        run_with(
            args("alpha"),
            &GlobalConfig::default(),
            &ProjectConfig::default(),
            &engine,
            &mut registry,
        )
        .unwrap();

        assert_eq!(
            registry.get(&cn("alpha")).unwrap().image.as_str(),
            "old:tag"
        );
    }

    #[test]
    fn keeps_registry_unchanged_when_create_fails() {
        let (_tmp, mut registry) = fresh_registry();
        registry
            .insert(sample_entry("alpha", "old:tag", "/tmp/x"))
            .unwrap();
        let engine = MockEngine::new();
        engine.fail_create_and_start.set(true);

        let err = run_with(
            args("alpha"),
            &GlobalConfig::default(),
            &ProjectConfig::default(),
            &engine,
            &mut registry,
        )
        .unwrap_err();

        assert!(err.to_string().contains("failed to start container"));
        assert_eq!(
            registry.get(&cn("alpha")).unwrap().image.as_str(),
            "old:tag"
        );
        assert!(
            !engine
                .calls()
                .iter()
                .any(|call| *call == EngineCall::RefreshProxyPolicy)
        );
    }

    #[test]
    fn errors_when_castor_is_unknown() {
        let (_tmp, mut registry) = fresh_registry();
        let engine = MockEngine::new();

        let err = run_with(
            args("ghost"),
            &GlobalConfig::default(),
            &ProjectConfig::default(),
            &engine,
            &mut registry,
        )
        .unwrap_err();

        assert!(err.to_string().contains("castor 'ghost' not found"));
        assert!(engine.calls().is_empty());
    }
}
