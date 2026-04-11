use std::collections::HashMap;

use anyhow::Context;

use crate::cli::display::format_path;
use crate::core::domain::CastorName;
use crate::core::registry::Registry;
use crate::engine::{self, CastorStatus, Engine};

pub fn run() -> anyhow::Result<()> {
    let engine = engine::current();
    let registry = Registry::load().context("failed to load registry")?;
    run_with(engine.as_ref(), &registry)
}

pub(crate) fn run_with(engine: &dyn Engine, registry: &Registry) -> anyhow::Result<()> {
    if registry.is_empty() {
        println!("no castors registered");
        return Ok(());
    }

    // Best-effort backend join: if the engine is unavailable, fall through
    // with empty status rather than failing `list` entirely.
    let statuses: HashMap<CastorName, CastorStatus> = engine
        .list_managed()
        .map(|cs| cs.into_iter().map(|c| (c.name, c.status)).collect())
        .unwrap_or_default();

    for entry in registry.list() {
        let status = statuses.get(&entry.name).map_or("unknown", status_label);
        println!(
            "{name}\t{image}\t{dir}\t{status}\t{created}",
            name = entry.name,
            image = entry.image,
            dir = format_path(entry.mount_dir.as_path()),
            created = entry.created_at.to_rfc3339(),
        );
    }
    Ok(())
}

fn status_label(status: &CastorStatus) -> &'static str {
    match status {
        CastorStatus::Running => "running",
        CastorStatus::Exited { .. } => "exited",
        CastorStatus::Missing => "missing",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::test_helpers::{fresh_registry, sample_entry};
    use crate::engine::mock::{EngineCall, MockEngine};

    #[test]
    fn empty_registry_skips_engine_query() {
        let (_tmp, registry) = fresh_registry();
        let engine = MockEngine::new();

        run_with(&engine, &registry).unwrap();

        assert!(engine.calls().is_empty());
    }

    #[test]
    fn non_empty_registry_queries_engine_once() {
        let (_tmp, mut registry) = fresh_registry();
        registry
            .insert(sample_entry("alpha", "img", "/tmp/x"))
            .unwrap();
        let engine = MockEngine::new();

        run_with(&engine, &registry).unwrap();

        assert_eq!(engine.calls(), vec![EngineCall::ListManaged]);
    }

    #[test]
    fn falls_back_to_unknown_status_when_engine_fails() {
        let (_tmp, mut registry) = fresh_registry();
        registry
            .insert(sample_entry("alpha", "img", "/tmp/x"))
            .unwrap();
        let engine = MockEngine::new();
        engine.fail_list_managed.set(true);

        // The CLI must not error out just because the backend is unhappy;
        // the registry view is still useful on its own.
        run_with(&engine, &registry).unwrap();
    }
}
