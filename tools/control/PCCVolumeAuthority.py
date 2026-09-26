#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

VOLUME_SCHEMA = "cortex.volume.v1"
MARKER_NAME = ".cortex-volume.json"


class VolumeAuthorityError(RuntimeError):
    pass


@dataclass(frozen=True)
class VolumeContext:
    volume_id: str
    mount_root: Path
    cortex_root: Path
    projects_root: Path
    vault_root: Path
    models_root: Path
    shared_root: Path
    intake_root: Path
    state_root: Path

    @property
    def registry_root(self) -> Path:
        return self.state_root / "registry"

    @property
    def conversations_root(self) -> Path:
        return self.state_root / "conversations"

    @property
    def logs_root(self) -> Path:
        return self.state_root / "logs"

    @property
    def checkpoints_root(self) -> Path:
        return self.state_root / "checkpoints"

    @property
    def git_root(self) -> Path:
        return self.state_root / "git"

    def as_dict(self) -> dict[str, str]:
        return {
            "volumeId": self.volume_id,
            "mountRoot": str(self.mount_root),
            "cortexRoot": str(self.cortex_root),
            "projectsRoot": str(self.projects_root),
            "vaultRoot": str(self.vault_root),
            "modelsRoot": str(self.models_root),
            "sharedRoot": str(self.shared_root),
            "intakeRoot": str(self.intake_root),
            "stateRoot": str(self.state_root),
        }


def _candidate_roots(start: Path) -> list[Path]:
    start = start.expanduser().resolve()
    out: list[Path] = []
    for candidate in (start, *start.parents):
        if candidate not in out:
            out.append(candidate)
    anchor = Path(start.anchor) if start.anchor else None
    if anchor and anchor not in out:
        out.append(anchor)
    return out


def discover_volume_root(start: Path | str | None = None) -> Path | None:
    override = os.environ.get("CORTEX_VOLUME_ROOT")
    if override:
        root = Path(override).expanduser().resolve()
        return root if root.is_dir() else None
    base = Path(start).expanduser().resolve() if start else Path(__file__).resolve().parents[2]
    for candidate in _candidate_roots(base):
        if (candidate / MARKER_NAME).is_file():
            return candidate
    # Compatibility bootstrap: a Cortex checkout directly below a portable mount.
    if base.name.casefold() == "cortex" and base.parent.is_dir():
        return base.parent
    for parent in base.parents:
        if parent.name.casefold() == "cortex" and parent.parent.is_dir():
            return parent.parent
    # Source-rollup/test compatibility: recognize the Cortex checkout by its control surface.
    if (base / "tools" / "control" / "CortexPCC.py").is_file() and base.parent.is_dir():
        return base.parent
    return None


def marker_path(volume_root: Path) -> Path:
    return volume_root / MARKER_NAME


def read_marker(volume_root: Path) -> dict[str, Any] | None:
    path = marker_path(volume_root)
    if not path.is_file():
        return None
    data = json.loads(path.read_text(encoding="utf-8-sig"))
    if data.get("schema") != VOLUME_SCHEMA or not str(data.get("volumeId") or "").strip():
        raise VolumeAuthorityError(f"Invalid Cortex volume marker: {path}")
    return data


def initialize_volume(volume_root: Path | str, *, cortex_root: Path | str | None = None) -> VolumeContext:
    root = Path(volume_root).expanduser().resolve()
    root.mkdir(parents=True, exist_ok=True)
    existing = read_marker(root)
    if existing is None:
        payload = {
            "schema": VOLUME_SCHEMA,
            "volumeId": str(uuid.uuid4()),
            "role": "portable-development-environment",
            "createdUtc": datetime.now(timezone.utc).isoformat(),
        }
        path = marker_path(root)
        temp = path.with_suffix(path.suffix + ".tmp")
        temp.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
        os.replace(temp, path)
    return resolve_volume_context(cortex_root or (root / "Cortex"), create=True)


def resolve_volume_context(cortex_root: Path | str | None = None, *, create: bool = False) -> VolumeContext:
    cortex = Path(cortex_root).expanduser().resolve() if cortex_root else Path(__file__).resolve().parents[2]
    volume_root = discover_volume_root(cortex)
    if volume_root is None:
        raise VolumeAuthorityError(f"Unable to discover Cortex volume from {cortex}")
    marker = read_marker(volume_root)
    if marker is None:
        if not create:
            # Compatibility identity until explicit volume initialization. Never persist drive letter as identity.
            volume_id = "bootstrap-uninitialized"
        else:
            return initialize_volume(volume_root, cortex_root=cortex)
    else:
        volume_id = str(marker["volumeId"])
    if cortex.name.casefold() != "cortex" and (volume_root / "Cortex").is_dir():
        cortex = (volume_root / "Cortex").resolve()
    ctx = VolumeContext(
        volume_id=volume_id,
        mount_root=volume_root,
        cortex_root=cortex,
        projects_root=volume_root / "Projects",
        vault_root=volume_root / "Vault",
        models_root=volume_root / "Models",
        shared_root=volume_root / "shared",
        intake_root=volume_root / "Intake",
        state_root=volume_root / ".cortex",
    )
    if create:
        for path in (ctx.projects_root, ctx.vault_root, ctx.models_root, ctx.shared_root, ctx.intake_root,
                     ctx.state_root, ctx.registry_root, ctx.conversations_root, ctx.logs_root,
                     ctx.checkpoints_root, ctx.git_root):
            path.mkdir(parents=True, exist_ok=True)
    return ctx


def volume_relative(path: Path | str, ctx: VolumeContext) -> str:
    target = Path(path).expanduser().resolve()
    try:
        return target.relative_to(ctx.mount_root).as_posix()
    except ValueError as exc:
        raise VolumeAuthorityError(f"Path is outside Cortex volume: {target}") from exc


def resolve_volume_path(relative_path: str, ctx: VolumeContext) -> Path:
    raw = Path(relative_path)
    if raw.is_absolute() or ".." in raw.parts:
        raise VolumeAuthorityError(f"Unsafe volume-relative path: {relative_path}")
    return (ctx.mount_root / raw).resolve()
