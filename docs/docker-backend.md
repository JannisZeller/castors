## Castors Docker Backend

This note records how the Docker-backed runtime is intended to behave and where the seam lives so future backends (Podman, remote engines, Kubernetes-flavored adapters) can plug in without changing the rest of the CLI.

It builds on the direction in [docs/plan.md](/Users/jzeller/castors/docs/plan.md) and the staged execution in [docs/stages.md](/Users/jzeller/castors/docs/stages.md).

## Lifecycle Model

### `castors add <image> <dir> <name>`

- Insert the entry into the registry.
- Start a long-lived container right away with `docker run -d`.
- The container's lifetime is the image's responsibility:
  - Long-running entrypoints (e.g. an agent process) stay alive and can be exec-ed into across many sessions.
  - One-shot entrypoints exit on their own. The container is "finished" but its filesystem state is preserved.
- If the docker call fails, roll back the registry insert so on-disk state never claims a castor that does not exist.

### `castors exec <name>`

- Look up the entry, then dispatch on docker container state:
  - **Running** → `docker exec -it castor-<name> <shell>`
  - **Exited** → `docker start castor-<name>` then `docker exec -it ...`. This re-runs the entrypoint. For long-running entrypoints that crashed, this is what the user wants. For genuinely one-shot images the container will exit again before exec attaches, and we surface that as a clear error rather than masking it.
  - **Not present in docker** (e.g. someone ran `docker rm` behind our back) → error pointing the user at `castors rm` + `castors add`.

### `castors rm <name>` and `castors prune`

- `docker stop castor-<name>` then `docker rm castor-<name>` for each affected entry.
- Drop the registry entry only after docker confirms the container is gone.
- Be tolerant of containers that no longer exist in docker; the registry remove should still succeed in that case.

### `castors list`

- Joins registry entries with a single batched `docker ps -a --filter label=castors --format ...` query so each entry can show its status (`running`, `exited`, `missing`).

## Naming and Labels

Every container the CLI creates is tagged with at least:

- `--name castor-<name>` for human-readable identification in `docker ps`.
- `--label castors.role=castor` to mark it as a managed castor.
- `--label castors.name=<name>` to map back to the registry entry.

Shared infrastructure containers (proxy, future monitoring) are tagged with `--label castors.role=infra`. Labels are the source of truth for "what is currently a castor"; we deliberately avoid maintaining a separate counter because labels are self-cleaning across crashes.

## Shared Infrastructure

Per the plan:

- A user-defined Docker network (`castors-net`) connects castors and shared services.
- Shared services (Squid first, future monitoring later) are declared in a Compose file and brought up via `docker compose -f castors-infra/docker-compose.yml up -d`.
- The CLI is the orchestrator for lifecycle:
  - Bring infra up before launching the first castor (advisory file lock to avoid races).
  - After each castor exits or is removed, query `docker ps --filter label=castors.role=castor`. When the count of running castors hits zero, bring infra down.
  - Auto-shutdown is opt-in via config; some workflows prefer keeping the proxy warm.

The Squid stack is the first concrete `ProxyStack` implementation but is intentionally separable from the engine so it can be swapped or replaced.

## What Stays Out of the Engine for v1

Explicitly deferred to keep the first slice honest:

- Networking enforcement beyond best-effort proxy routing (hard anti-bypass is Stage 10).
- Image pulls during `add`. We let docker handle "image not found" for now.
- Container reuse across multiple `add` calls.
- Restart policies. Crashed castors stay crashed until the user re-execs.

## Abstraction Boundary

The rest of the CLI must not import `std::process::Command` or talk to the docker binary directly. All container-related work goes through a single trait that lives in `engine::ContainerEngine` (introduced when the second backend or first real test mock requires it; until then, a concrete `engine::docker` module behind a small free-function API serves the same role).

Recommended trait shape, kept narrow on purpose:

- `create_and_start(entry: &CastorEntry) -> Result<()>`
- `exec_shell(name: &CastorName) -> Result<ExitStatus>`
- `stop_and_remove(name: &CastorName) -> Result<()>`
- `inspect_status(name: &CastorName) -> Result<CastorStatus>`
- `list_managed() -> Result<Vec<ManagedContainer>>`
- `capabilities() -> EngineCapabilities`

Notes on the shape:

- Inputs are domain types (`CastorEntry`, `CastorName`), never raw strings, so the trait is hard to misuse.
- Outputs are structured (`CastorStatus`, `ExitStatus`, `ManagedContainer`) instead of raw command output.
- `EngineCapabilities` reports features that may genuinely differ between backends (interactive exec, label filtering, network attach modes, anti-bypass mechanisms). Backends declare what they can and cannot do instead of the CLI assuming Docker semantics everywhere.
- Shared infrastructure orchestration lives in a separate trait (`SharedInfraOrchestrator`) so a backend that does not own the proxy stack does not need to implement it.

This boundary lets Podman, a remote docker context, or a future Kubernetes-flavored adapter plug in by implementing the same trait.

## Things to Be Honest About Up Front

- `docker exec` requires a running container. For one-shot entrypoints, exec is best-effort and can race the container's exit. We do not paper over this with sleep wrappers by default.
- `docker start` re-runs the entrypoint; it is not "resume from snapshot." Filesystem state survives, processes do not.
- A determined process inside a castor can bypass an HTTP-only proxy via raw IP. True isolation requires per-container firewall rules or a `--network none` + transparent proxy setup. That is Stage 10 work.
- Compose project naming matters. We always pass `--project-name castors` so the aux stack does not fork by cwd.
- Stale containers named `castor-<name>` cause `docker run` to fail. We surface the error verbatim in v1 and consider an explicit `--force` flag later.
