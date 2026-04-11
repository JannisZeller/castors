use anyhow::Context;

use crate::core::registry::Registry;
use crate::core::state::StateLock;
use crate::engine::{self, Engine};

pub fn run() -> anyhow::Result<()> {
    let _state_lock = StateLock::acquire().context("failed to acquire castors state lock")?;
    let engine = engine::current();
    let mut registry = Registry::load().context("failed to load registry")?;
    run_with(engine.as_ref(), &mut registry)
}

pub(crate) fn run_with(engine: &dyn Engine, registry: &mut Registry) -> anyhow::Result<()> {
    let names: Vec<_> = registry.list().map(|e| e.name.clone()).collect();
    let total = names.len();

    for name in &names {
        engine
            .stop_and_remove(name)
            .with_context(|| format!("failed to stop container for castor '{name}'"))?;
    }

    registry.clear();
    registry.save().context("failed to persist registry")?;
    if total > 0 {
        engine
            .refresh_proxy_policy(registry)
            .context("failed to refresh proxy policy")?;
    }

    engine
        .teardown_infra_if_idle()
        .context("failed to tear down shared infrastructure")?;

    println!("pruned {total} castor(s)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::test_helpers::{cn, fresh_registry, sample_entry};
    use crate::engine::mock::{EngineCall, MockEngine};

    #[test]
    fn empty_registry_only_calls_teardown() {
        let (_tmp, mut registry) = fresh_registry();
        let engine = MockEngine::new();

        run_with(&engine, &mut registry).unwrap();

        assert_eq!(engine.calls(), vec![EngineCall::TeardownInfraIfIdle]);
    }

    #[test]
    fn stops_every_castor_then_clears_registry() {
        let (_tmp, mut registry) = fresh_registry();
        registry
            .insert(sample_entry("alpha", "img", "/tmp/x"))
            .unwrap();
        registry
            .insert(sample_entry("beta", "img", "/tmp/y"))
            .unwrap();
        let engine = MockEngine::new();

        run_with(&engine, &mut registry).unwrap();

        assert_eq!(registry.len(), 0);
        // Registry::list iterates name-sorted, so call order is alphabetic.
        assert_eq!(
            engine.calls(),
            vec![
                EngineCall::StopAndRemove(cn("alpha")),
                EngineCall::StopAndRemove(cn("beta")),
                EngineCall::RefreshProxyPolicy,
                EngineCall::TeardownInfraIfIdle,
            ],
        );
    }

    #[test]
    fn aborts_on_first_stop_failure_without_clearing_registry() {
        let (_tmp, mut registry) = fresh_registry();
        registry
            .insert(sample_entry("alpha", "img", "/tmp/x"))
            .unwrap();
        registry
            .insert(sample_entry("beta", "img", "/tmp/y"))
            .unwrap();
        let engine = MockEngine::new();
        engine.fail_stop_and_remove.set(true);

        let err = run_with(&engine, &mut registry).unwrap_err();

        assert!(err.to_string().contains("failed to stop container"));
        assert_eq!(registry.len(), 2);
        // Aborts after the first failed stop; never reaches teardown.
        assert_eq!(engine.calls(), vec![EngineCall::StopAndRemove(cn("alpha"))],);
    }
}
