#!/usr/bin/env python3
"""Universal project-control capability resolver.

Discovery describes candidates.  This module decides whether a requested Cortex
capability is actually supported by the provider.  It deliberately refuses to
invent conventional legacy PCC verbs such as ``full`` merely because a launcher
exists.
"""
from __future__ import annotations

import re
from pathlib import Path
from typing import Any

RESOLVER_VERSION = "PCC-PROJECT-CONTROL-RESOLVER-0.1"

ALIASES: dict[str, tuple[str, ...]] = {
    "full": ("gate.full", "build.full", "quality.full"),
    "quick": ("gate.fast", "build.fast", "gate.quick", "build.quick"),
    "fast": ("gate.fast", "build.fast", "gate.quick", "build.quick"),
    "build": ("build.native", "build.debug", "build", "build.render", "build.headless"),
    "build-release": ("build.release", "build.native", "build"),
    "launch-gui": ("run.game", "run.client", "run.editor", "run.runtime"),
    "patch-status": ("patch.status", "patch.preview"),
    "patch-apply": ("patch.apply",),
    "git-status": ("git.status", "project.status"),
    "git-review": ("git.review", "git.status"),
    "git-history": ("git.history",),
    "git-verify": ("repo.audit", "git.status"),
    "push": ("git.push",),
    "git-pull": ("git.pull",),
    "self-test": ("project.self-test", "control.self-test", "self-test"),
    "doctor": ("project.health", "project.status"),
    "root-hygiene": ("audit.root", "project.audit"),
    "root-hygiene-fix": ("maintenance.hygiene", "maintenance.repair"),
    "debug-bundle": ("diagnostics.bundle",),
    "verify-latest-debug": ("diagnostics.verify-latest",),
    "commit-green": ("git.commit-green",),
}

_SYNTHETIC_GATE_KEYS = {"gate.full", "build.full", "quality.full", "gate.fast", "gate.quick", "build.fast", "build.quick"}
_PCC_NAMES = {"project_control_center.cmd", "projectcontrolcenter.cmd", "pcc.cmd"}


def _commands(data: dict[str, Any]) -> list[dict[str, Any]]:
    return [x for x in data.get("commands", []) if isinstance(x, dict) and str(x.get("key") or "").strip()]


def _candidate(data: dict[str, Any], requested: str) -> dict[str, Any] | None:
    index = {str(item.get("key")).casefold(): item for item in _commands(data)}
    direct = index.get(requested.casefold())
    if direct is not None:
        return direct
    for alias in ALIASES.get(requested.casefold(), ()):
        found = index.get(alias.casefold())
        if found is not None:
            return found
    return None


def _read(path: Path, limit: int = 200_000) -> str:
    try:
        return path.read_text(encoding="utf-8-sig", errors="replace")[:limit]
    except OSError:
        return ""


def _provider_operation(command: dict[str, Any]) -> str:
    args = [str(x).strip() for x in (command.get("args") or [])]
    if not args:
        return ""
    # Native PCC discovery currently emits the conventional operation as the
    # first argument.  For PowerShell contracts, prefer -Action/-Operation.
    for flag in ("-Action", "-Operation", "--action", "--operation"):
        for i, value in enumerate(args[:-1]):
            if value.casefold() == flag.casefold():
                return args[i + 1]
    return args[-1] if len(args) == 1 else args[0]


def _powershell_targets(root: Path, launcher: Path) -> list[Path]:
    text = _read(launcher)
    out: list[Path] = []
    for match in re.finditer(r'(?i)(?:"([^"\r\n]+\.ps1)"|([^\s"\r\n]+\.ps1))', text):
        raw = (match.group(1) or match.group(2) or "").replace("%~dp0", "").lstrip("\\/")
        raw = raw.replace("\\", "/")
        candidate = (launcher.parent / raw).resolve()
        if candidate.is_file() and candidate not in out:
            out.append(candidate)
    if not out:
        out.extend(sorted(root.glob("tools/**/*.ps1")))
    return out


def _declared_ps_operations(text: str) -> set[str]:
    operations: set[str] = set()
    # ValidateSet attached to Action or Operation.
    for match in re.finditer(r"(?is)\[ValidateSet\((.*?)\)\].{0,260}?\$(?:Action|Operation)\b", text):
        operations.update(v.casefold() for v in re.findall(r"['\"]([^'\"]+)['\"]", match.group(1)))
    # Conservative switch labels.  This intentionally does not treat arbitrary
    # mentions of the word 'full' as proof of an executable provider operation.
    for match in re.finditer(r"(?im)^\s*['\"]([^'\"]+)['\"]\s*\{", text):
        operations.add(match.group(1).strip().casefold())
    return operations


def _legacy_operation_supported(root: Path, command: dict[str, Any], operation: str) -> tuple[bool, str]:
    program = str(command.get("program") or "").strip()
    if not program or not operation:
        return False, "provider operation could not be identified"
    path = Path(program)
    if not path.is_absolute():
        path = root / path
    suffix = path.suffix.casefold()
    if suffix == ".ps1" and path.is_file():
        ops = _declared_ps_operations(_read(path))
        return (operation.casefold() in ops, f"PowerShell declared operations: {', '.join(sorted(ops)) or '<none>'}")
    if suffix in {".cmd", ".bat"} and path.is_file():
        for ps1 in _powershell_targets(root, path):
            ops = _declared_ps_operations(_read(ps1))
            if operation.casefold() in ops:
                return True, f"provider operation declared by {ps1.relative_to(root).as_posix()}"
        return False, f"legacy launcher did not prove operation '{operation}'"
    # Explicit project.control commands that target non-PCC executables are
    # declarations in their own right and remain usable.
    return True, "explicit non-PCC command"


def resolve_project_operation(root: Path, data: dict[str, Any], requested: str) -> dict[str, Any]:
    command = _candidate(data, requested)
    if command is None:
        return {
            "status": "BLOCKED",
            "errorCode": "CORTEX-PC-1001",
            "reason": f"Universal operation '{requested}' is not available for this project.",
            "requested": requested,
            "resolverVersion": RESOLVER_VERSION,
        }

    key = str(command.get("key") or "")
    discovery = data.get("_pccDiscovery") if isinstance(data.get("_pccDiscovery"), dict) else {}
    source = str(discovery.get("source") or "")
    authority = str(discovery.get("authority") or "")
    program = str(command.get("program") or "")
    basename = Path(program).name.casefold()
    legacyish = source == "native-project-pcc" or authority == "native-project-pcc" or basename in _PCC_NAMES

    if legacyish and key.casefold() in _SYNTHETIC_GATE_KEYS:
        operation = _provider_operation(command)
        supported, evidence = _legacy_operation_supported(root, command, operation)
        if not supported:
            return {
                "status": "BLOCKED",
                "errorCode": "CORTEX-PC-1004",
                "reason": f"Advertised capability '{key}' is not proven usable by its legacy provider.",
                "requested": requested,
                "capability": key,
                "providerOperation": operation,
                "evidence": evidence,
                "sourceImplicated": False,
                "controlPlaneImplicated": True,
                "resolverVersion": RESOLVER_VERSION,
            }

    return {
        "status": "READY",
        "requested": requested,
        "capability": key,
        "command": command,
        "provenance": {
            "source": source or "project-contract",
            "authority": authority or "project-contract",
            "provider": program,
        },
        "resolverVersion": RESOLVER_VERSION,
    }
