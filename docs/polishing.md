# Polishing notes

Loose end-of-stage UX/quality items that are intentionally deferred until the
core functionality (config, infra, proxy) is in place. Promote individual
items into stages of their own once they get prioritized.

## CLI

- `**castors status <name>**` — surface `engine.inspect_status()` per castor:
running / exited(code) / missing. Lets users debug "why didn't my exec
work" without dropping to `docker ps`.
- **Table-formatted `list`** — current output is tab-separated for easy
scripting but unreadable for humans. Add column alignment (consider
`comfy-table` or hand-rolled padding). Maybe a `--format=json|table|tsv`
flag once we have a real reason to script against it.
- `**castors add --dry-run**` — print the resolved docker invocation that
would be issued without running it. Useful for debugging the engine
layer.

## Errors and diagnostics

- **Friendlier "docker not installed" message** — currently surfaces the
raw `EngineError::BackendUnavailable("docker binary not found in PATH")`.
Suggest install instructions or link to docs.
- **Detect "docker daemon not running"** vs binary missing — different
remediation, currently both surface as backend errors.
- **Stale registry entry hint** — when `exec` finds a registry entry but
the engine reports `Missing`, suggest `castors rm <name>` explicitly
rather than just propagating the error.
- **Color the "[infra] todo:" notices** so they're visibly distinct from
real CLI output until infra is implemented for real.

## Output

- **Quiet flag** — `castors -q add ...` for use in scripts; suppress the
"added castor 'foo'" noise.
- **Verbose flag** — `castors -v ...` to surface the docker invocations
being made, useful when troubleshooting.

## Internal

- **Replace `String`-typed `EngineError::Backend`** with a structured type
once we have a real second backend or more sophisticated error
reporting needs. Today the daemon's stderr message is captured as a
string, which is enough for users to act on.
