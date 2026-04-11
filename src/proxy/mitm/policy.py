"""Castors mitmproxy policy addon.

The addon intentionally keeps all policy in a single JSON file and reloads on
mtime changes so Castors can update allowlists and injected headers without
restarting the proxy container.
"""

from __future__ import annotations

import json
import os
import re
from pathlib import Path
from typing import Any

from mitmproxy import ctx, http

POLICY_PATH = Path(os.environ.get("CASTORS_POLICY_PATH", "/config/policy.json"))
SECRETS_PATH = Path(
    os.environ.get("CASTORS_SECRETS_PATH", "/run/secrets/castors_policy_secrets")
)
PLACEHOLDER_RE = re.compile(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}")


class Policy:
    def __init__(self) -> None:
        self._policy_mtime: float | None = None
        self._secrets_mtime: float | None = None
        self._policy: dict[str, Any] = {
            "default": "deny",
            "containers": {},
            "global": {"allow_domains": [], "inject_headers": {}},
        }
        self._secrets: dict[str, str] = {}

    def request(self, flow: http.HTTPFlow) -> None:
        self._reload_if_needed()

        client_ip = flow.client_conn.peername[0] if flow.client_conn.peername else ""
        domain = (flow.request.pretty_host or flow.request.host or "").lower()
        port = flow.request.port
        identity, container_policy = self._container_for_ip(client_ip)
        effective = self._effective_policy(container_policy)

        if not self._domain_allowed(domain, effective.get("allow_domains", []), port):
            ctx.log.info(f"container={identity} domain={domain} decision=blocked headers=[]")
            flow.response = http.Response.make(403, b"Domain not allowed\n")
            return

        injected = self._inject_headers(flow, domain, port, effective.get("inject_headers", {}))
        ctx.log.info(
            f"container={identity} domain={domain} decision=allowed headers={injected}"
        )

    def _reload_if_needed(self) -> None:
        policy_mtime = self._mtime(POLICY_PATH)
        secrets_mtime = self._mtime(SECRETS_PATH)
        if policy_mtime == self._policy_mtime and secrets_mtime == self._secrets_mtime:
            return

        self._policy = self._load_policy()
        self._secrets = self._load_secrets()
        self._policy_mtime = policy_mtime
        self._secrets_mtime = secrets_mtime

    def _load_policy(self) -> dict[str, Any]:
        if not POLICY_PATH.exists():
            ctx.log.warn(f"policy file {POLICY_PATH} does not exist; denying all")
            return {"default": "deny", "containers": {}, "global": {}}

        with POLICY_PATH.open("r", encoding="utf-8") as f:
            parsed = json.load(f)

        if not isinstance(parsed, dict):
            return {"default": "deny", "containers": {}, "global": {}}
        return parsed

    def _load_secrets(self) -> dict[str, str]:
        if not SECRETS_PATH.exists():
            return {}
        with SECRETS_PATH.open("r", encoding="utf-8") as f:
            parsed = json.load(f)
        if not isinstance(parsed, dict):
            return {}
        return {str(k): str(v) for k, v in parsed.items()}

    def _container_for_ip(self, client_ip: str) -> tuple[str, dict[str, Any] | None]:
        containers = self._policy.get("containers", {})
        if not isinstance(containers, dict):
            return client_ip or "unknown", None

        for name, policy in containers.items():
            if not isinstance(policy, dict):
                continue
            ips = policy.get("ips", [])
            if name == client_ip or client_ip in ips:
                return str(name), policy
        return client_ip or "unknown", None

    def _effective_policy(self, container_policy: dict[str, Any] | None) -> dict[str, Any]:
        global_policy = self._policy.get("global", {})
        if not isinstance(global_policy, dict):
            global_policy = {}

        if not container_policy:
            return global_policy

        allow_domains = container_policy.get("allow_domains") or global_policy.get(
            "allow_domains", []
        )
        inject_headers = self._merged_headers(
            global_policy.get("inject_headers", {}),
            container_policy.get("inject_headers", {}),
        )
        return {"allow_domains": allow_domains, "inject_headers": inject_headers}

    def _merged_headers(
        self, global_headers: Any, container_headers: Any
    ) -> dict[str, dict[str, str]]:
        merged = self._normalize_headers(global_headers)
        for domain, headers in self._normalize_headers(container_headers).items():
            merged.setdefault(domain, {}).update(headers)
        return merged

    def _normalize_headers(self, raw: Any) -> dict[str, dict[str, str]]:
        if not isinstance(raw, dict):
            return {}
        normalized: dict[str, dict[str, str]] = {}
        for domain, headers in raw.items():
            if not isinstance(headers, dict):
                continue
            normalized[str(domain).lower()] = {
                str(header): str(value) for header, value in headers.items()
            }
        return normalized

    def _domain_allowed(self, domain: str, allow_domains: Any, port: int | None) -> bool:
        if not isinstance(allow_domains, list):
            return False
        for allowed in allow_domains:
            pattern = str(allowed).lower()
            candidate = f"{domain}:{port}" if port is not None and ":" in pattern else domain
            if pattern.startswith("*."):
                suffix = pattern[2:]
                if candidate.endswith(f".{suffix}") and candidate != suffix:
                    return True
            elif candidate == pattern:
                return True
        return False

    def _inject_headers(
        self, flow: http.HTTPFlow, domain: str, port: int | None, inject_headers: Any
    ) -> list[str]:
        headers_by_domain = self._normalize_headers(inject_headers)
        injected: list[str] = []

        for pattern, headers in headers_by_domain.items():
            if not self._domain_allowed(domain, [pattern], port):
                continue
            for header, template in headers.items():
                flow.request.headers.pop(header, None)
                flow.request.headers[header] = self._substitute(template)
                injected.append(header)
        return injected

    def _substitute(self, value: str) -> str:
        def replace(match: re.Match[str]) -> str:
            return self._secrets.get(match.group(1), "")

        return PLACEHOLDER_RE.sub(replace, value)

    def _mtime(self, path: Path) -> float | None:
        try:
            return path.stat().st_mtime
        except FileNotFoundError:
            return None


addons = [Policy()]
