//! Test-only recording mock for [`Engine`].
//!
//! Records every method call into a `Vec<EngineCall>` for assertion, and
//! exposes a handful of `Cell` / `RefCell` knobs so individual tests can
//! make selected methods fail or return canned values.
//!
//! The mock is only compiled under `#[cfg(test)]`; downstream crates do not
//! see it. If we ever want to share it externally, gate it behind a
//! `test-support` feature instead.
//!
//! `ExitStatus` has no portable constructor in stable Rust, so we obtain
//! real `ExitStatus` values by spawning `true` and `false` once at
//! construction time. This keeps the mock platform-agnostic at the cost of
//! two cheap process spawns per test.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::process::{Command, ExitStatus};

use crate::config::{ProxyMode, ResolvedConfig};
use crate::core::domain::CastorName;
use crate::core::registry::{CastorEntry, Registry};
use crate::engine::{CastorStatus, Engine, EngineError, ManagedContainer};

/// One recorded interaction with the mock. Tests assert on a `Vec` of these
/// to verify both *which* methods ran and *in what order*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineCall {
    EnsureInfra(ProxyMode),
    RefreshProxyPolicy,
    ExportMitmCaCertificate,
    /// Carries the resolved config so tests can verify env / secrets / hosts
    /// were threaded through correctly.
    CreateAndStart(CastorName, ResolvedConfig),
    /// Second argument is `None` when the CLI uses automatic shell selection.
    ExecShell(CastorName, Option<String>),
    StopAndRemove(CastorName),
    InspectStatus(CastorName),
    ListManaged,
    TeardownInfraIfIdle,
}

/// Recording mock implementation of [`Engine`]. Defaults to "everything
/// succeeds with empty results"; flip the public `Cell`s to inject failures
/// or seed return values.
pub struct MockEngine {
    calls: RefCell<Vec<EngineCall>>,

    /// Statuses returned by `inspect_status`, keyed by castor name.
    /// Unknown names return [`CastorStatus::Missing`].
    pub statuses: RefCell<HashMap<CastorName, CastorStatus>>,

    /// Containers returned by `list_managed`.
    pub managed: RefCell<Vec<ManagedContainer>>,

    pub fail_ensure_infra: Cell<bool>,
    pub fail_refresh_proxy_policy: Cell<bool>,
    pub fail_export_mitm_ca_certificate: Cell<bool>,
    pub fail_create_and_start: Cell<bool>,
    pub fail_stop_and_remove: Cell<bool>,
    pub fail_list_managed: Cell<bool>,

    /// If `true`, `exec_shell` returns a non-zero `ExitStatus` (exit code 1).
    pub exec_should_fail: Cell<bool>,

    success_status: ExitStatus,
    failure_status: ExitStatus,
}

impl MockEngine {
    #[must_use]
    pub fn new() -> Self {
        let success_status = Command::new("true")
            .status()
            .expect("spawn `true` for ExitStatus fixture");
        let failure_status = Command::new("false")
            .status()
            .expect("spawn `false` for ExitStatus fixture");
        Self {
            calls: RefCell::new(Vec::new()),
            statuses: RefCell::new(HashMap::new()),
            managed: RefCell::new(Vec::new()),
            fail_ensure_infra: Cell::new(false),
            fail_refresh_proxy_policy: Cell::new(false),
            fail_export_mitm_ca_certificate: Cell::new(false),
            fail_create_and_start: Cell::new(false),
            fail_stop_and_remove: Cell::new(false),
            fail_list_managed: Cell::new(false),
            exec_should_fail: Cell::new(false),
            success_status,
            failure_status,
        }
    }

    /// Snapshot of the recorded calls in invocation order.
    #[must_use]
    pub fn calls(&self) -> Vec<EngineCall> {
        self.calls.borrow().clone()
    }

    fn record(&self, call: EngineCall) {
        self.calls.borrow_mut().push(call);
    }
}

impl Default for MockEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for MockEngine {
    fn create_and_start(
        &self,
        entry: &CastorEntry,
        config: &ResolvedConfig,
    ) -> Result<(), EngineError> {
        self.record(EngineCall::CreateAndStart(
            entry.name.clone(),
            config.clone(),
        ));
        if self.fail_create_and_start.get() {
            return Err(EngineError::Backend(
                "mock: create_and_start configured to fail".into(),
            ));
        }
        Ok(())
    }

    fn exec_shell(
        &self,
        name: &CastorName,
        shell: Option<&str>,
    ) -> Result<ExitStatus, EngineError> {
        self.record(EngineCall::ExecShell(
            name.clone(),
            shell.map(str::to_owned),
        ));
        if self.exec_should_fail.get() {
            Ok(self.failure_status)
        } else {
            Ok(self.success_status)
        }
    }

    fn stop_and_remove(&self, name: &CastorName) -> Result<(), EngineError> {
        self.record(EngineCall::StopAndRemove(name.clone()));
        if self.fail_stop_and_remove.get() {
            return Err(EngineError::Backend(
                "mock: stop_and_remove configured to fail".into(),
            ));
        }
        Ok(())
    }

    fn inspect_status(&self, name: &CastorName) -> Result<CastorStatus, EngineError> {
        self.record(EngineCall::InspectStatus(name.clone()));
        Ok(self
            .statuses
            .borrow()
            .get(name)
            .cloned()
            .unwrap_or(CastorStatus::Missing))
    }

    fn list_managed(&self) -> Result<Vec<ManagedContainer>, EngineError> {
        self.record(EngineCall::ListManaged);
        if self.fail_list_managed.get() {
            return Err(EngineError::Backend(
                "mock: list_managed configured to fail".into(),
            ));
        }
        Ok(self.managed.borrow().clone())
    }

    fn ensure_infra(&self, config: &ResolvedConfig) -> Result<(), EngineError> {
        self.record(EngineCall::EnsureInfra(config.proxy));
        if self.fail_ensure_infra.get() {
            return Err(EngineError::Backend(
                "mock: ensure_infra configured to fail".into(),
            ));
        }
        Ok(())
    }

    fn refresh_proxy_policy(&self, _registry: &Registry) -> Result<(), EngineError> {
        self.record(EngineCall::RefreshProxyPolicy);
        if self.fail_refresh_proxy_policy.get() {
            return Err(EngineError::Backend(
                "mock: refresh_proxy_policy configured to fail".into(),
            ));
        }
        Ok(())
    }

    fn export_mitm_ca_certificate(&self) -> Result<Vec<u8>, EngineError> {
        self.record(EngineCall::ExportMitmCaCertificate);
        if self.fail_export_mitm_ca_certificate.get() {
            return Err(EngineError::Backend(
                "mock: export_mitm_ca_certificate configured to fail".into(),
            ));
        }
        Ok(b"mock-ca-cert\n".to_vec())
    }

    fn teardown_infra_if_idle(&self) -> Result<(), EngineError> {
        self.record(EngineCall::TeardownInfraIfIdle);
        Ok(())
    }
}
