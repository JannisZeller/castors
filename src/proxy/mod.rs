//! Proxy stack implementations and config renderers.
//!
//! This module is deliberately backend-independent. Runtime adapters decide
//! how to run and mount a proxy; this module owns proxy-specific policy
//! rendering.

use crate::config::ProxyMode;

pub mod mitm;
pub mod squid;

static SQUID_SERVICE: squid::SquidProxyService = squid::SquidProxyService;
static MITM_SERVICE: mitm::MitmProxyService = mitm::MitmProxyService;

/// Shared contract implemented by each proxy service Castors can run.
///
/// The trait intentionally stays small: it exposes the stable runtime facts
/// needed by Docker orchestration while each implementation owns its own
/// policy/config rendering details.
pub trait ProxyService {
    fn mode(&self) -> ProxyMode;
    fn container_name(&self) -> &'static str;
    fn port(&self) -> u16;

    fn compose_profile(&self) -> Option<&'static str> {
        None
    }

    fn proxy_url(&self) -> String {
        format!("http://{}:{}", self.container_name(), self.port())
    }
}

#[must_use]
pub fn service(mode: ProxyMode) -> &'static dyn ProxyService {
    match mode {
        ProxyMode::Squid => &SQUID_SERVICE,
        ProxyMode::Mitm => &MITM_SERVICE,
    }
}
