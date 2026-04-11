//! Docker label keys shared by the castor and infra submodules.
//!
//! Labels are the source of truth for "what is currently a managed castor /
//! piece of infra". Using labels means lifecycle state is self-cleaning across
//! crashes and external `docker rm` calls; we do not maintain a separate
//! counter.

/// Marker label that identifies any container managed by `castors`.
pub const ROLE_KEY: &str = "castors.role";

/// Per-castor label that maps a container back to its registry entry.
pub const NAME_KEY: &str = "castors.name";

/// Value of `ROLE_KEY` for castor containers themselves.
pub const ROLE_CASTOR: &str = "castor";

/// Value of `ROLE_KEY` for shared infrastructure containers
/// (proxy, future monitoring, etc.). Used by `engine::docker::infra`
/// once the shared stack is implemented.
#[allow(dead_code)]
pub const ROLE_INFRA: &str = "infra";
