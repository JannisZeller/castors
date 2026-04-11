# castors

CLI to run coding agents in isolated **Docker** containers: register a castor, start a long-lived container for a mounted workdir, and `exec` into it. See [AGENTS.md](AGENTS.md) for "product" intent.

## Prerequisites

- Docker (with `docker compose` available as a subcommand)

## Build

```sh
cargo build --release
```

The binary is `target/release/castors` (or `target/debug/castors` without `--release`).

## Commands


| Command             | Purpose                                                                            |
| ------------------- | ---------------------------------------------------------------------------------- |
| `castors add [DIR]` | Create and start a castor. `DIR` defaults to `.`. Optional `-i IMAGE` / `-n NAME`. |
| `castors exec NAME` | Attach a shell in the castor container (starts it if stopped).                     |
| `castors infra refresh` | Re-read config and apply proxy policy for registered castors.                 |
| `castors list`      | List registered castors.                                                           |
| `castors mitm ca`   | Generate/export the mitmproxy public CA certificate without creating a castor.     |
| `castors restart NAME` | Recreate a castor from its current config.                                      |
| `castors rm NAME`   | Remove the container and registry entry.                                           |
| `castors prune`     | Remove all castors and clear the registry.                                         |


Global and project YAML config are optional; you can still use `castors add -i my-image` with no config files.

## Files on disk

Castors keeps operator config and generated state under `~/.castors/` on every platform (see `src/core/paths.rs`).


| Path                                             | Role                                                                                                                  |
| ------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------- |
| `~/.castors/config.yaml`                         | Global defaults and shared policy (`network`, `env`, `secrets`, …).                                                   |
| `~/.castors/.state/registry.json`                | Registry of castor names, images, mount dirs, timestamps.                                                             |
| `~/.castors/.state/state.lock`                   | Advisory lock used by state-changing commands.                                                                        |
| `~/.castors/.state/infra/compose.yaml`           | Materialized shared infra **Compose** project (proxy stack). Overwritten when infra is ensured.                       |
| `~/.castors/.state/infra/squid.conf`             | Materialized **Squid** config for the shared proxy. Overwritten when infra is ensured.                                |
| `~/.castors/.state/infra/mitm/config/policy.json` | Materialized **MITM** policy for the shared proxy.                                                                    |
| `<WORKDIR>/.castors/config.yaml`                 | Project-scoped overrides (optional). Not visible inside the castor container (tmpfs shadow — see [docs/networking.md](docs/networking.md)). |

State-changing commands (`add`, `restart`, `rm`, `prune`, `infra refresh`, and `mitm ca`) wait on `~/.castors/.state/state.lock` before reading or writing generated state. `list` and `exec` intentionally do not acquire that lock because they do not mutate Castors state or generated infra files. During a concurrent state-changing command, `list` may briefly show a stale registry snapshot, and `exec` may pass registry validation but then fail if the target container was removed before Docker handles the exec.


Schema details: [docs/config.md](docs/config.md).

Secret file sources can be absolute (`file:/path/to/token`) or relative
(`file:secrets/token`). Relative paths are resolved against the directory
containing the `config.yaml` that declared them, e.g.
`<WORKDIR>/.castors/secrets/token` for project config.

## Limitations

This repository is still mid-roadmap. In particular:

- **Secret header injection** is modeled in config, but values resolved for Squid may appear **in plain text** inside the generated `~/.castors/.state/infra/squid.conf` and thus in the infra container. That is intentional for an early vertical slice: castor containers should not be able to read that file, but anyone with host access or Docker admin access can. Stronger handling (no secrets on disk, encryption, etc.) is **out of scope for now**.
- **Proxy reloads**: MITM reloads policy from disk on request. Squid policy is rendered with per-workdir rules for registered Squid castors and Castors asks a running Squid container to reconfigure after policy refreshes (see [docs/config.md](docs/config.md) update semantics).
- Castors attach to the internal `castors-shared` network and inject proxy env vars by default; **hard** anti-bypass (beyond internal routing) remains roadmap work — see [docs/stages.md](docs/stages.md) and [docs/docker-backend.md](docs/docker-backend.md).

For squishy details on proxy, secrets, and per-castor rules, see [docs/networking.md](docs/networking.md).
