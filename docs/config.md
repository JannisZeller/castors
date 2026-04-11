# Configuration

`castors` reads configuration from up to two YAML files. Both are optional — a fresh install with neither file works, as long as you pass `-i IMAGE` on the command line.

## File locations


| File    | Path on every platform       |
| ------- | ---------------------------- |
| Global  | `~/.castors/config.yaml`     |
| Project | `<DIR>/.castors/config.yaml` |


The same path applies on Linux, macOS, and Windows (`C:\Users\<name>\.castors\...`). This deliberately diverges from XDG and Apple's `~/Library/Application Support/`; the rationale lives in [`src/core/paths.rs`](../src/core/paths.rs).

Generated state lives under `~/.castors/.state/`. The registry is `~/.castors/.state/registry.json`, and state-changing commands wait on `~/.castors/.state/state.lock` before reading or writing generated state. `castors list` and `castors exec` do not acquire the lock because they only read registry state and query Docker; during a concurrent state-changing command they may observe a brief stale registry snapshot or surface a normal backend error.

## Schema

Both files share three sections (`network`, `env`, `secrets`) with identical shape and identical merge rules. Each file additionally carries one **layer-specific block**:

- Global has `defaults:` — values that apply to every castor on this host.
- Project has `castor:` — identity for this specific workdir's castor.

Putting `castor:` in the global file (or `defaults:` in a project file) is a clean parse error, not a silent no-op — both schemas use `deny_unknown_fields`.

### Global: `~/.castors/config.yaml`

```yaml
defaults:
  image: my-default-castor:latest    # fallback image when nothing more specific picks one

network:
  proxy: squid                        # optional: squid (default) or mitm
  allowed_hosts:                      # hosts the shared proxy may allow (enforcement depends on proxy mode; see docs/networking.md)
    - api.openai.com
    - "*.openai.com"
    - registry.npmjs.org

env:                                  # plain env vars, visible to the agent
  RUST_LOG: info

secrets:                              # header injection rules (enforced in MITM mode; Squid mode embeds resolved values in squid.conf)
  - host: api.openai.com
    header: Authorization
    value_template: "Bearer {{value}}"
    value_from: env:OPENAI_API_KEY       # or file:secrets/openai-token
```

### Project: `<DIR>/.castors/config.yaml`

```yaml
castor:
  name: my-agent                      # optional; auto-generated as <dir>-<n> if omitted
  image: this-project:tag             # optional; beats defaults.image

network:
  proxy: mitm                         # opt this castor into HTTPS interception
  allowed_hosts:
    - api.anthropic.com

env:
  GIT_AUTHOR_NAME: Agent

secrets:
  - host: api.anthropic.com
    header: x-api-key
    value_template: "{{value}}"
    value_from: env:ANTHROPIC_API_KEY    # or file:secrets/anthropic-token
```

## Field reference

### `defaults` (global only)


| Field   | Type   | Description                                             |
| ------- | ------ | ------------------------------------------------------- |
| `image` | string | Image tag used when nothing more specific provides one. |


### `castor` (project only)


| Field   | Type   | Description                                                             |
| ------- | ------ | ----------------------------------------------------------------------- |
| `name`  | string | Explicit castor name. ASCII alphanumerics, `-`, `_`. Beats auto-naming. |
| `image` | string | Image tag for this workdir. Beats `defaults.image`.                     |


### `network`


| Field           | Type            | Description                                                                                                      |
| --------------- | --------------- | ---------------------------------------------------------------------------------------------------------------- |
| `proxy`         | string          | `squid` (default) or `mitm`. Project config overrides global config for the castor being created.                |
| `allowed_hosts` | list of strings | Hostnames, `host:port` entries, or `*.example.com` wildcards. URLs, paths, schemes, and whitespace are rejected. |


### `env`

Plain `KEY: VALUE` map. Passed straight through as `docker run -e KEY=VALUE`. **Visible to the agent inside the container.** Do not put secrets here.

There are some reserved environment variables that are overridden by the internally used values. These are `HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY`.

### `secrets`

List of header-injection rules. With `network.proxy: mitm`, these rules are rendered into the mitmproxy policy for each MITM castor. Secret values are resolved by `castors` into a Docker secret mounted only into the MITM container. They are not passed to agent containers.

With `network.proxy: squid` (the default), resolved secret values are embedded in the generated `~/.castors/.state/infra/squid.conf` on the host (visible to anyone with host or Docker admin access). `castors add` prints a warning when this list is non-empty in Squid mode so you do not mistake "configured" for "safe" or assume HTTPS interception.


| Field            | Type   | Description                                                                                |
| ---------------- | ------ | ------------------------------------------------------------------------------------------ |
| `host`           | string | The destination host the rule applies to.                                                  |
| `header`         | string | HTTP header name to set on outbound requests. Case-insensitive when matching for override. |
| `value_template` | string | Header value, with `{{value}}` substituted from the source.                                |
| `value_from`     | string | Source of the secret material: `env:NAME`, `file:/abs/path`, or `file:relative/path`.      |


For `file:` sources, absolute paths are used as written. Relative paths are resolved against the directory containing the config file that declared them:

- In `~/.castors/config.yaml`, `file:secrets/openai-token` resolves to `~/.castors/secrets/openai-token`.
- In `<DIR>/.castors/config.yaml`, `file:secrets/anthropic-token` resolves to `<DIR>/.castors/secrets/anthropic-token`.

## Resolution chain

When `castors add` runs, the value of each field is picked in this order (first non-empty wins):


| Concern       | Order                                                                     |
| ------------- | ------------------------------------------------------------------------- |
| **Mount dir** | CLI positional → current directory                                        |
| **Image**     | CLI `-i` → `castor.image` (project) → `defaults.image` (global) → error   |
| **Name**      | CLI `-n` → `castor.name` (project) → auto-generated `<sanitized-dir>-<n>` |


Auto-naming sanitizes the mount dir's basename: lowercases it, replaces non-`[a-z0-9_]` characters with `-`, collapses runs, and trims edges. So `/work/My Repo!` becomes base `my-repo` → first free of `my-repo-1`, `my-repo-2`, ... If the basename is unusable (e.g. mount dir is `/`), the literal `castor` is used.

Explicit names (CLI or project config) **never** auto-increment: a collision is an error telling you to pick another name or `castors rm` the old one first.

## Merge rules

When both files exist, only the shared sections are merged. The layer-specific blocks (`defaults`, `castor`) feed into resolution, not into merge.


| Section                 | Rule                                                                                      |
| ----------------------- | ----------------------------------------------------------------------------------------- |
| `network.allowed_hosts` | Union, deduplicated, sorted.                                                              |
| `network.proxy`         | Project value wins; otherwise global value; otherwise `squid`.                            |
| `env`                   | Per-key override; project beats global. Non-overlapping keys from both layers survive.    |
| `secrets`               | Per-`(host, header)` override; project beats global. Header matching is case-insensitive. |


## Update semantics

Some changes propagate live to running castors; some require recreating the container. The split is dictated by what Docker can mutate at runtime:


| Field                         | When changes take effect                                                          |
| ----------------------------- | --------------------------------------------------------------------------------- |
| `network.allowed_hosts`       | MITM: live for MITM castors after `castors infra refresh` or when the registry changes (`castors add` / `rm` / `prune` refreshes `policy.json`).<br><br>Squid: `squid.conf` is regenerated by `castors infra refresh`, when infra is ensured, and when the registry changes, including per-workdir rules for registered Squid castors. If Squid is already running, Castors runs `squid -k reconfigure`; otherwise the next Squid start reads the generated file. |
| `secrets`                     | Same as `allowed_hosts` for MITM vs Squid (see above).                            |
| `network.proxy`               | At `castors add` / `castors restart <name>` time only. Existing containers keep their proxy env vars until recreated. |
| `env`                         | At `castors add` / `castors restart <name>` time only. Existing containers keep old env vars until recreated. |
| `castor.image`                | At `castors add` / `castors restart <name>` time. Restart uses the current configured image if present, otherwise the existing registry image. |
| `castor.name`                 | At `castors add` time only. Restart keeps the registry name you pass.             |


## Security note

Anything in `env` is fundamentally readable by the agent — `/proc/1/environ`, child process inheritance, etc. There is no Docker flag that hides env vars from a process running in the container. Treat `env` as documentation, not as a vault.

True secrets belong in `secrets`. MITM mode keeps values out of castor containers (Docker secret + proxy substitution). Squid mode still places resolved values in `squid.conf` on the host. See [docs/networking.md](networking.md) for the full reasoning.

MITM mode additionally requires the castor image to trust the mitmproxy root CA. Run `castors mitm ca` to generate/export the public CA certificate without creating a castor, then bake it into the image or otherwise configure the trust store before relying on HTTPS interception.

The operator's `<DIR>/.castors/` directory is **not** visible inside the container: `castors add` mounts an empty read-only `tmpfs` over `/workspace/.castors`, so the agent cannot read the project config or rewrite policy mid-session. See [docs/networking.md](networking.md#operator-visibility-of-project-config).
