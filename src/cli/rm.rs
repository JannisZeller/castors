use anyhow::Context;
use clap::Args;

use crate::core::domain::CastorName;
use crate::core::registry::Registry;
use crate::core::state::StateLock;
use crate::engine::{self, Engine};

#[derive(Debug, Args)]
pub struct RmArgs {
    /// Name of the castor to remove.
    pub castor_name: CastorName,
}

pub fn run(args: RmArgs) -> anyhow::Result<()> {
    let _state_lock = StateLock::acquire().context("failed to acquire castors state lock")?;
    let engine = engine::current();
    let mut registry = Registry::load().context("failed to load registry")?;
    run_with(args, engine.as_ref(), &mut registry)
}

pub(crate) fn run_with(
    args: RmArgs,
    engine: &dyn Engine,
    registry: &mut Registry,
) -> anyhow::Result<()> {
    engine
        .stop_and_remove(&args.castor_name)
        .with_context(|| format!("failed to stop container for castor '{}'", args.castor_name))?;

    registry
        .remove(&args.castor_name)
        .with_context(|| format!("failed to remove castor '{}'", args.castor_name))?;
    registry.save().context("failed to persist registry")?;
    engine
        .refresh_proxy_policy(registry)
        .context("failed to refresh proxy policy")?;

    engine
        .teardown_infra_if_idle()
        .context("failed to tear down shared infrastructure")?;

    println!("removed castor '{}'", args.castor_name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::test_helpers::{cn, fresh_registry, sample_entry};
    use crate::engine::mock::{EngineCall, MockEngine};

    fn args(name: &str) -> RmArgs {
        RmArgs {
            castor_name: cn(name),
        }
    }

    #[test]
    fn removes_entry_and_calls_stop_then_teardown() {
        let (_tmp, mut registry) = fresh_registry();
        registry
            .insert(sample_entry("alpha", "img", "/tmp/x"))
            .unwrap();
        let engine = MockEngine::new();

        run_with(args("alpha"), &engine, &mut registry).unwrap();

        assert_eq!(registry.len(), 0);
        assert_eq!(
            engine.calls(),
            vec![
                EngineCall::StopAndRemove(cn("alpha")),
                EngineCall::RefreshProxyPolicy,
                EngineCall::TeardownInfraIfIdle,
            ],
        );
    }

    #[test]
    fn keeps_registry_unchanged_when_stop_fails() {
        let (_tmp, mut registry) = fresh_registry();
        registry
            .insert(sample_entry("alpha", "img", "/tmp/x"))
            .unwrap();
        let engine = MockEngine::new();
        engine.fail_stop_and_remove.set(true);

        let err = run_with(args("alpha"), &engine, &mut registry).unwrap_err();

        assert!(err.to_string().contains("failed to stop container"));
        assert_eq!(registry.len(), 1);
        assert_eq!(engine.calls(), vec![EngineCall::StopAndRemove(cn("alpha"))]);
    }

    #[test]
    fn errors_when_castor_unknown_to_registry() {
        let (_tmp, mut registry) = fresh_registry();
        let engine = MockEngine::new();

        // stop_and_remove is intentionally tolerant of missing containers, so
        // it runs first; the registry-side check is what surfaces the error.
        let err = run_with(args("ghost"), &engine, &mut registry).unwrap_err();

        assert!(err.to_string().contains("failed to remove castor"));
        assert_eq!(engine.calls(), vec![EngineCall::StopAndRemove(cn("ghost"))]);
    }
}
