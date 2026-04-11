# Castors Stages

This document breaks the higher-level roadmap in [docs/plan.md](/Users/jzeller/castors/docs/plan.md) into smaller execution stages.

It assumes the current architecture decisions:

- Rust CLI
- Docker-compatible runtime first
- `docker` for castor lifecycle
- `docker compose` for shared infrastructure
- `Squid` as the first proxy implementation
- narrow, capability-based backend abstraction

## Stage 0: Lock The Initial Decisions

### Goal

Freeze the implementation assumptions that would otherwise cause churn during scaffolding.

### Tasks

- Confirm the first supported runtime is Docker.
- Confirm the first proxy stack is `Squid`.
- Decide the first machine-managed state format for the castor registry.
- Decide whether shared infra starts automatically or through an explicit command.
- Confirm the initial networking policy target: best-effort proxying or stricter enforcement.

### Outputs

- A short recorded decision set in `docs/plan.md` or `AGENTS.md`.
- No remaining blockers for scaffolding the CLI.

### Exit Criteria

- The initial backend and proxy model are stable enough that code can be organized around them.

## Stage 1: Bootstrap The Rust Workspace

### Goal

Create the initial Rust project layout for a CLI-first codebase.

### Tasks

- Initialize the crate and repository structure.
- Add core dependencies for CLI parsing, config serialization, error handling, and logging.
- Add a minimal command entrypoint.
- Define a rough module layout for CLI, config, registry, engine, and infra code.
- Add basic developer commands for build, test, and formatting.

### Outputs

- A compiling Rust CLI skeleton.
- A predictable source tree for later stages.

### Exit Criteria

- `cargo build` works.
- The CLI boots and prints help.

## Stage 2: Model The Domain

### Goal

Define the core application types before wiring them to Docker.

### Tasks

- Define configuration types for global config and project overrides.
- Define normalized runtime config types after merge resolution.
- Define registry types for persisted castor metadata.
- Define engine-facing request and response types.
- Define capability types for backend feature reporting.

### Outputs

- Stable Rust types for config, state, and engine operations.
- A shared vocabulary for the rest of the codebase.

### Exit Criteria

- Core types compile and are usable without Docker-specific code.

## Stage 3: Build The CLI Surface

### Goal

Implement the user-facing command structure without full backend behavior yet.

### Tasks

- Add command parsing for `add`, `exec`, `rm`, and `prune`.
- Define command options and argument validation.
- Add structured error messages for invalid input.
- Wire commands into application service entrypoints.
- Keep Docker calls stubbed or mocked behind interfaces.

### Outputs

- A usable command surface that mirrors the design in `AGENTS.md`.
- A thin CLI layer separated from backend logic.

### Exit Criteria

- Commands parse cleanly and route into internal application handlers.

## Stage 4: Implement Config Resolution

### Goal

Make configuration deterministic before lifecycle work depends on it.

### Tasks

- Load global YAML config from `~/.config/castors/`.
- Load project-local config from `.castors/`.
- Implement merge rules and conflict handling.
- Resolve bind mounts, image defaults, and networking-related settings into normalized config.
- Add tests for merge behavior and path normalization.

### Outputs

- A `ConfigResolver` that produces normalized runtime input.
- Confidence that config behavior is predictable.

### Exit Criteria

- Config loading and merge rules are tested and stable.

## Stage 5: Implement The Castor Registry

### Goal

Persist enough state to make named castor lifecycle reliable across sessions.

### Tasks

- Choose and implement the initial machine-managed state format.
- Persist castor metadata such as name, image, mount directory, engine backend, and created resource identifiers.
- Add lookup, update, remove, and list operations.
- Handle stale or missing runtime resources gracefully.
- Add tests for registry persistence and recovery behavior.

### Outputs

- A working `CastorRegistry`.
- Stable identifiers for lifecycle commands.

### Exit Criteria

- Registry state survives process restarts and supports all core command flows.

## Stage 6: Define The Runtime And Infra Boundaries

### Goal

Put the right abstractions in place before implementing Docker integration.

### Tasks

- Define the `ContainerEngine` interface.
- Define the `SharedInfraOrchestrator` interface.
- Define the `ProxyStack` interface and the first `SquidProxyStack` contract.
- Separate per-castor lifecycle operations from shared-infra operations.
- Model capability reporting for features that may differ by backend.

### Outputs

- Clean engine and infra boundaries that fit the current architecture.
- A clear seam for future backends.

### Exit Criteria

- The interfaces are small, concrete, and sufficient for the Docker path.

## Stage 7: Implement `DockerEngine`

### Goal

Make the core castor lifecycle work against Docker.

### Tasks

- Implement image validation and container creation.
- Implement start and inspect behavior.
- Implement interactive `exec` behavior for entering a castor shell.
- Implement removal and prune behavior.
- Attach bind mounts according to normalized config.
- Translate Docker errors into application-level errors.

### Outputs

- A working `DockerEngine`.
- End-to-end behavior for core castor lifecycle commands.

### Exit Criteria

- `add`, `exec`, `rm`, and `prune` work against Docker, including attachment to the internal `castors-shared` bridge (`docker run --network ...`) used by the shared proxy stack.

## Stage 8: Implement Shared Infra With Compose

### Goal

Stand up the singleton support services needed for isolation.

### Tasks

- Define the Compose-managed shared services layout.
- Implement `SharedInfraOrchestrator` for Docker Compose.
- Add lifecycle commands or internal flows for bringing infra up and down.
- Add health checks and readiness detection for shared services.
- Persist enough metadata to reconnect castors with shared infra correctly.

### Outputs

- Shared infrastructure can be started, inspected, and reused.
- The project has a place to host proxy/logging services that run once.

### Exit Criteria

- Shared infra can be brought up reliably and recognized across repeated CLI runs.

## Stage 9: Add `SquidProxyStack`

### Goal

Introduce the first real proxy-based networking layer.

### Tasks

- Containerize or configure the initial `Squid` service.
- Connect `Squid` to the shared infrastructure network layout.
- Define how allowlists are rendered into `Squid` configuration.
- Define where proxy logs are written and how they are surfaced.
- Add health and misconfiguration diagnostics.

### Outputs

- A functioning `SquidProxyStack`.
- A concrete implementation of the proxy abstraction.

### Exit Criteria

- `Squid` runs as shared infra and is configurable from castors-managed settings.

### Implementation status (vs this repo today)

- Squid ships in the materialized Compose project; `squid.conf` is rendered under `~/.castors/.state/infra/`. Optional **MITM** (`mitmproxy`) is a Compose **profile** toggled when `network.proxy: mitm` ensures infra.
- State-changing commands use an advisory lock at `~/.castors/.state/state.lock`; read-only `list` and `exec` skip the lock and may briefly observe stale state during concurrent changes.
- Squid policy refresh runs `squid -k reconfigure` after rewrites when the Squid container is already running, so rendered per-castor rules become effective without an infra restart.

## Stage 10: Route Castors Through The Proxy

### Goal

Make castors consume the proxy stack in a deterministic way.

### Tasks

- Connect castor containers to the correct Docker network.
- Inject `HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY` settings.
- Ensure proxy configuration is derived from normalized config.
- Validate that castors can reach allowed destinations through the proxy path.
- Decide and implement the first anti-bypass behavior supported by Docker.

### Outputs

- Castors use the shared proxy path by default.
- Networking behavior becomes part of the core product, not just an optional add-on.

### Exit Criteria

- Castors attach to the shared internal network, receive proxy URLs via `HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY`, and honor `network.proxy: squid | mitm` when those services are up.

## Stage 11: Harden The MVP

### Goal

Make the first release reliable enough for real use.


### Tasks

- Improve CLI output and error recovery paths.
- Add targeted integration tests around Docker, Compose, and proxy orchestration.
- Add observability hooks and better diagnostics for startup failures.
- Handle stale registry entries and orphaned runtime resources.
- Tighten docs for installation, local prerequisites, and expected runtime behavior.
- Pick up the **Stage 10 residual** (see below): stronger guarantees than internal network + explicit proxy env alone, and evidence that workloads actually use the proxy path.

### Networking hardening backlog (anti-bypass and validation)

Stage 10 establishes routing via `castors-shared` and proxy environment variables. Stage 11 closes the gap for operators who need confidence and stricter envelopes. Work through these roughly in order (cheap validation first; heavy enforcement last):

1. **Operational proof / regression tests** — In CI or smoke scripts, assert that direct connections from a castor to arbitrary public endpoints fail where policy says they should, while allowed traffic via the proxy succeeds. Optionally correlate Squid / mitmproxy logs with expected destinations. Catches regressions in compose, networks, and config materialization; does not by itself block a determined bypass.

2. **Controlled DNS** — Point castors at a known resolver, log or policy-align queries, and document how allowlists relate to the names clients actually resolve. Reduces “surprise” egress via odd resolvers; blocking DoH or hard-coded IPs is disproportionately hard without lower-layer filtering.

3. **Shrink lateral movement on the shared bridge** — If castors must not talk to each other, use per-castor networks, small subnets, or host firewall rules so the stable peer set is the proxy (and any explicitly allowed infra), not every other castor on `castors-shared`.

4. **Live Squid policy verification** — Keep regression coverage around `squid.conf` regeneration plus `squid -k reconfigure` so a long-running Squid process cannot silently serve stale ACLs. Keeps “configured” and “enforced” aligned now that Squid policy rendering can include per-castor rules.

5. **Stronger enforcement (explicit opt-in complexity)** — For threat models that assume malicious in-container code: transparent proxy (iptables/nftables REDIRECT/TPROXY into Squid/MITM), or `--network none` plus a vetted forwarding path. These are the classic next step when “trust the image + internal net + env” is not enough.

6. **Proxy authentication (optional)** — Require credentials for use of the shared proxy so ad-hoc processes cannot use the proxy without material the operator controls. Complements network isolation; does not replace it.

### Outputs

- A stable Docker-first MVP.
- Better operator experience when things fail.
- A documented path from Stage 10’s defaults toward measured, testable hardening.

### Exit Criteria

- The core workflow works reliably on a clean machine with documented prerequisites.
- At least the **first two** items in “Networking hardening backlog” are either implemented or explicitly deferred with rationale (so Stage 10’s “still open” line has an owner).

## Stage 12: Evaluate Portability

### Goal

Only after the Docker-first flow is stable, test how portable the abstractions actually are.

### Tasks

- Review where Docker assumptions leaked into interfaces.
- Identify the minimum work needed for a Podman backend.
- Revisit capability reporting based on real Docker experience.
- Decide whether a second backend is worth the complexity.

### Outputs

- A grounded portability assessment instead of a theoretical one.
- A concrete follow-up plan for Podman or another backend if still desired.

### Exit Criteria

- The project has enough production-like experience to judge whether a second backend is justified.

## Recommended Execution Order

1. Stage 0
2. Stage 1
3. Stage 2
4. Stage 3
5. Stage 4
6. Stage 5
7. Stage 6
8. Stage 7
9. Stage 8
10. Stage 9
11. Stage 10
12. Stage 11
13. Stage 12

## Suggested MVP Cut

If you want the smallest useful vertical slice before full networking enforcement, stop after:

- Stage 7 for a Docker-only castor lifecycle MVP
- Stage 10 for a containerized, proxy-routed MVP aligned with the project safety goal
