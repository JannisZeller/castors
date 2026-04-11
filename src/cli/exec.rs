use anyhow::{Context, anyhow};
use clap::Args;

use crate::core::domain::CastorName;
use crate::core::registry::Registry;
use crate::engine::{self, Engine};

#[derive(Debug, Args)]
pub struct ExecArgs {
    /// Name of the castor to enter.
    pub castor_name: CastorName,

    /// Shell binary inside the container (e.g. `/bin/bash`). If omitted, uses
    /// the first available of `/bin/zsh`, `/bin/bash`, `/bin/sh`.
    #[arg(long, value_name = "PATH")]
    pub shell: Option<String>,
}

pub fn run(args: ExecArgs) -> anyhow::Result<()> {
    let engine = engine::current();
    let registry = Registry::load().context("failed to load registry")?;
    run_with(args, engine.as_ref(), &registry)
}

pub(crate) fn run_with(
    args: ExecArgs,
    engine: &dyn Engine,
    registry: &Registry,
) -> anyhow::Result<()> {
    if registry.get(&args.castor_name).is_none() {
        return Err(anyhow!("castor '{}' not found", args.castor_name));
    }

    let shell = match args.shell.as_deref() {
        None => None,
        Some(raw) => {
            let t = raw.trim();
            if t.is_empty() {
                return Err(anyhow!("--shell must not be empty"));
            }
            Some(t)
        }
    };

    let status = engine
        .exec_shell(&args.castor_name, shell)
        .with_context(|| format!("failed to exec into castor '{}'", args.castor_name))?;

    // Inner shell exit code is informational; we do not propagate it as a
    // process-level failure of `castors exec` itself.
    if !status.success() {
        if let Some(code) = status.code() {
            eprintln!("(shell exited with code {code})");
        } else {
            eprintln!("(shell terminated by signal)");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::test_helpers::{cn, fresh_registry, sample_entry};
    use crate::engine::mock::{EngineCall, MockEngine};

    fn args(name: &str) -> ExecArgs {
        ExecArgs {
            castor_name: cn(name),
            shell: None,
        }
    }

    #[test]
    fn errors_when_castor_not_in_registry() {
        let (_tmp, registry) = fresh_registry();
        let engine = MockEngine::new();

        let err = run_with(args("ghost"), &engine, &registry).unwrap_err();

        assert!(err.to_string().contains("not found"));
        assert!(engine.calls().is_empty());
    }

    #[test]
    fn calls_exec_shell_when_castor_present() {
        let (_tmp, mut registry) = fresh_registry();
        registry
            .insert(sample_entry("alpha", "img", "/tmp/x"))
            .unwrap();
        let engine = MockEngine::new();

        run_with(args("alpha"), &engine, &registry).unwrap();

        assert_eq!(
            engine.calls(),
            vec![EngineCall::ExecShell(cn("alpha"), None)]
        );
    }

    #[test]
    fn passes_explicit_shell_to_engine() {
        let (_tmp, mut registry) = fresh_registry();
        registry
            .insert(sample_entry("alpha", "img", "/tmp/x"))
            .unwrap();
        let engine = MockEngine::new();

        let mut a = args("alpha");
        a.shell = Some("/bin/ksh".into());
        run_with(a, &engine, &registry).unwrap();

        assert_eq!(
            engine.calls(),
            vec![EngineCall::ExecShell(cn("alpha"), Some("/bin/ksh".into()))]
        );
    }

    #[test]
    fn rejects_empty_shell_flag() {
        let (_tmp, mut registry) = fresh_registry();
        registry
            .insert(sample_entry("alpha", "img", "/tmp/x"))
            .unwrap();
        let engine = MockEngine::new();

        let mut a = args("alpha");
        a.shell = Some("  ".into());
        let err = run_with(a, &engine, &registry).unwrap_err();

        assert!(err.to_string().contains("--shell"));
        assert!(engine.calls().is_empty());
    }

    #[test]
    fn returns_ok_even_when_inner_shell_exits_non_zero() {
        let (_tmp, mut registry) = fresh_registry();
        registry
            .insert(sample_entry("alpha", "img", "/tmp/x"))
            .unwrap();
        let engine = MockEngine::new();
        engine.exec_should_fail.set(true);

        run_with(args("alpha"), &engine, &registry).unwrap();
    }
}
