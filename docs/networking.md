# Networking, proxy, and security

Squid remains the default explicit proxy. MITM mode is optional and selected per castor with `network.proxy: mitm`. Both implementations are shared infra on the `castors-shared` internal network; castors use stable service-name proxy URLs in `HTTP_PROXY` / `HTTPS_PROXY`.

This page also covers **why `env` is not secret** and how **`secrets`** behave under Squid vs MITM.

## Secrets, env, and the agent

Anything passed as `docker run -e KEY=VALUE` is visible to every process in the container (`/proc/…/environ`, inheritance, etc.). There is no Docker flag that hides environment variables from the workload. Use **`env:`** only for non-sensitive settings.

Ways to keep credentials out of the agent:

- **Proxy-mediated injection (recommended for HTTP APIs)** — Values live outside the castor. The proxy adds headers (for example `Authorization`) on outbound requests to allowlisted hosts. Suited to token-shaped secrets over HTTP(S) when the stack is set up correctly.
- **Sidecar credential broker** — A daemon on the shared network holds credentials and exposes a constrained API (for example over a socket). Flexible but heavy for v1.
- **Document `env` honestly** — If it is not sensitive, say so; if it is sensitive, do not rely on env inside the castor.

**Current behavior:**

- **MITM mode** — Implements proxy-mediated injection with values in a Docker secret mounted only into `castors-infra-mitm`. The generated `policy.json` uses `${CASTORS_SECRET_N}` placeholders; substitution happens in the proxy at request time. Castors never pass those values into agent containers.
- **Squid mode** — Resolved secret material is embedded in `~/.castors/.state/infra/squid.conf` on the host. Castors still do not put it in the agent’s env, but anyone with host or Docker admin access can read that file. `castors add` warns when `secrets:` is non-empty in Squid mode because Squid header injection and HTTPS semantics are easy to misunderstand (see limitations below).

**TLS with MITM** — mitmproxy keeps a CA in the `castors-mitm-ca` volume. Run `castors mitm ca` to export the public certificate and **trust only that cert** in images that should use HTTPS through the MITM (never bake the private key into agent images; do not commit CA private keys).

Schema and merge rules for `secrets:` and `network` live in [docs/config.md](config.md).

## Can one proxy have per-castor rules?

Yes, cleanly, in principle: Squid’s ACL engine can match on source IP, and each castor has its own IP on `castors-shared`.

```sh
# castor-alpha (172.20.0.5)
acl alpha_src    src 172.20.0.5
acl alpha_hosts  dstdomain api.alpha.com
http_access allow alpha_src alpha_hosts
# castor-beta (172.20.0.6)
acl beta_src    src 172.20.0.6
acl beta_hosts  dstdomain api.beta.com
http_access allow beta_src beta_hosts
# Global rules apply to everyone
acl global_hosts dstdomain api.global.com
http_access allow global_hosts
http_access deny all
```

**Today:** MITM implements per-castor policy in `policy.json` (mounted at `/config/policy.json` in the mitm container); the addon reloads on file changes. Squid renders source-IP scoped per-castor rules into `squid.conf` for registered Squid castors, plus global fallback rules for unregistered/provisioning traffic.

Example generated MITM policy:

```json
{
  "default": "deny",
  "containers": {
    "castor-alpha": {
      "ips": ["172.20.0.5"],
      "allow_domains": ["api.openai.com", "*.openai.com"],
      "inject_headers": {
        "api.openai.com": {
          "Authorization": "${CASTORS_SECRET_0}"
        }
      }
    }
  },
  "global": {
    "allow_domains": ["example.com"],
    "inject_headers": {}
  }
}
```

## Squid reload and live policy

Squid can apply a rewritten `squid.conf` with `squid -k reconfigure`. Castors regenerates `~/.castors/.state/infra/squid.conf` when infra is ensured, when the registry changes (`castors add` / `rm` / `prune`), and when you run `castors infra refresh`; it then asks a running Squid container to reconfigure. If Squid is not running yet, the next start reads the generated file normally.

MITM avoids that for HTTP policy: `castors infra refresh`, `add`, `rm`, and `prune` rewrite `policy.json`, and the addon reloads from disk.

## Operator visibility of project config

`castors add` mounts a **read-only tmpfs** over `/workspace/.castors` inside the castor so the agent cannot read or change the project’s `.castors/config.yaml` after the container is created.

## MITM mode limitations

- Explicit proxy only — no transparent iptables interception.
- HTTPS interception needs apps that honor `HTTP_PROXY` / `HTTPS_PROXY` **and** trust the mitmproxy CA (`castors mitm ca`).
- Castors does not modify arbitrary images; bake trust or ship tooling that installs the cert.
- Non-HTTP protocols and stacks that ignore proxy env vars are out of scope for this layer.
