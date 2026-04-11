use std::fs;
use std::path::PathBuf;

use anyhow::Context;
use clap::{Args, Subcommand};

use crate::core::state::StateLock;
use crate::engine::{self, Engine};

#[derive(Debug, Args)]
pub struct MitmArgs {
    #[command(subcommand)]
    pub command: MitmCommand,
}

#[derive(Debug, Subcommand)]
pub enum MitmCommand {
    /// Generate/export the mitmproxy public CA certificate without creating a castor.
    Ca(CaArgs),
}

#[derive(Debug, Args)]
pub struct CaArgs {
    /// Where to write the public CA certificate.
    #[arg(short = 'o', long = "out")]
    pub out: Option<PathBuf>,
}

pub fn run(args: MitmArgs) -> anyhow::Result<()> {
    let _state_lock = StateLock::acquire().context("failed to acquire castors state lock")?;
    let engine = engine::current();
    run_with(args, engine.as_ref())
}

pub(crate) fn run_with(args: MitmArgs, engine: &dyn Engine) -> anyhow::Result<()> {
    match args.command {
        MitmCommand::Ca(args) => export_ca(args, engine),
    }
}

fn export_ca(args: CaArgs, engine: &dyn Engine) -> anyhow::Result<()> {
    let out = args.out.map_or_else(default_ca_output_path, Ok)?;
    let cert = engine
        .export_mitm_ca_certificate()
        .context("failed to generate MITM CA certificate")?;
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output directory {}", parent.display()))?;
    }
    fs::write(&out, cert)
        .with_context(|| format!("failed to write MITM CA certificate to {}", out.display()))?;
    println!("wrote MITM CA certificate to {}", out.display());
    Ok(())
}

fn default_ca_output_path() -> anyhow::Result<PathBuf> {
    let infra_dir = crate::core::paths::infra_dir()
        .ok_or_else(|| anyhow::anyhow!("unable to locate user home directory"))?;
    Ok(infra_dir.join("mitmproxy-ca-cert.cer"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::mock::{EngineCall, MockEngine};

    fn ca_args(out: Option<PathBuf>) -> MitmArgs {
        MitmArgs {
            command: MitmCommand::Ca(CaArgs { out }),
        }
    }

    #[test]
    fn ca_command_exports_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("ca.cer");
        let engine = MockEngine::new();

        run_with(ca_args(Some(out.clone())), &engine).unwrap();

        assert_eq!(fs::read_to_string(out).unwrap(), "mock-ca-cert\n");
        assert_eq!(engine.calls(), vec![EngineCall::ExportMitmCaCertificate]);
    }

    #[test]
    fn ca_command_surfaces_engine_errors() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("ca.cer");
        let engine = MockEngine::new();
        engine.fail_export_mitm_ca_certificate.set(true);

        let err = run_with(ca_args(Some(out)), &engine).unwrap_err();

        assert!(err.to_string().contains("failed to generate MITM CA"));
        assert_eq!(engine.calls(), vec![EngineCall::ExportMitmCaCertificate]);
    }
}
