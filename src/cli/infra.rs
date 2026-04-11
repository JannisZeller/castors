use anyhow::Context;
use clap::{Args, Subcommand};

use crate::core::registry::Registry;
use crate::core::state::StateLock;
use crate::engine::{self, Engine};

#[derive(Debug, Args)]
pub struct InfraArgs {
    #[command(subcommand)]
    pub command: InfraCommand,
}

#[derive(Debug, Subcommand)]
pub enum InfraCommand {
    /// Re-read config and apply proxy policy for registered castors.
    Refresh,
}

pub fn run(args: InfraArgs) -> anyhow::Result<()> {
    let _state_lock = StateLock::acquire().context("failed to acquire castors state lock")?;
    let engine = engine::current();
    let registry = Registry::load().context("failed to load registry")?;
    run_with(args, engine.as_ref(), &registry)
}

pub(crate) fn run_with(
    args: InfraArgs,
    engine: &dyn Engine,
    registry: &Registry,
) -> anyhow::Result<()> {
    match args.command {
        InfraCommand::Refresh => refresh(engine, registry),
    }
}

fn refresh(engine: &dyn Engine, registry: &Registry) -> anyhow::Result<()> {
    engine
        .refresh_proxy_policy(registry)
        .context("failed to refresh infrastructure policy")?;
    println!("refreshed infrastructure policy");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::test_helpers::fresh_registry;
    use crate::engine::mock::{EngineCall, MockEngine};

    fn refresh_args() -> InfraArgs {
        InfraArgs {
            command: InfraCommand::Refresh,
        }
    }

    #[test]
    fn refresh_command_refreshes_proxy_policy() {
        let (_tmp, registry) = fresh_registry();
        let engine = MockEngine::new();

        run_with(refresh_args(), &engine, &registry).unwrap();

        assert_eq!(engine.calls(), vec![EngineCall::RefreshProxyPolicy]);
    }

    #[test]
    fn refresh_command_surfaces_engine_errors() {
        let (_tmp, registry) = fresh_registry();
        let engine = MockEngine::new();
        engine.fail_refresh_proxy_policy.set(true);

        let err = run_with(refresh_args(), &engine, &registry).unwrap_err();

        assert!(
            err.to_string()
                .contains("failed to refresh infrastructure policy")
        );
        assert_eq!(engine.calls(), vec![EngineCall::RefreshProxyPolicy]);
    }
}
