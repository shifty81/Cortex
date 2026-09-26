#!/usr/bin/env python3
from __future__ import annotations
import json, os, uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
VOLUME_SCHEMA="cortex.volume.v1"; MARKER_NAME=".cortex-volume.json"; LAYOUT_SCHEMA="cortex.volume_layout.v2"
class VolumeAuthorityError(RuntimeError): pass
@dataclass(frozen=True)
class VolumeContext:
    volume_id:str; mount_root:Path; cortex_root:Path; state_root:Path; artifacts_root:Path
    backups_root:Path; catalogs_root:Path; git_root:Path; intake_root:Path; models_root:Path
    objects_root:Path; projects_root:Path; recovery_root:Path; shared_root:Path; source_root:Path
    tools_root:Path; vault_root:Path; archive_root:Path; exports_root:Path
    @property
    def registry_root(self): return self.state_root/"registry"
    @property
    def conversations_root(self): return self.state_root/"conversations"
    @property
    def logs_root(self): return self.state_root/"logs"
    @property
    def checkpoints_root(self): return self.state_root/"checkpoints"
    @property
    def receipts_root(self): return self.state_root/"receipts"
    def as_dict(self): return {k:str(v) if isinstance(v,Path) else v for k,v in self.__dict__.items()}
def _repo_root(): return Path(__file__).resolve().parents[2]
def _candidate_roots(start:Path):
    out=[]
    for x in (start,*start.parents):
        if x not in out: out.append(x)
    return out
def discover_volume_root(start:Path|str|None=None):
    override=os.environ.get("CORTEX_VOLUME_ROOT")
    if override:
        p=Path(override).expanduser().resolve()
        return p if (p/MARKER_NAME).is_file() else None
    base=Path(start).expanduser().resolve() if start else _repo_root()
    for x in _candidate_roots(base):
        if (x/MARKER_NAME).is_file(): return x
    return None
def read_marker(root:Path):
    p=root/MARKER_NAME
    if not p.is_file(): return None
    d=json.loads(p.read_text(encoding="utf-8-sig"))
    if d.get("schema")!=VOLUME_SCHEMA or not str(d.get("volumeId") or "").strip():
        raise VolumeAuthorityError(f"Invalid Cortex volume marker: {p}")
    return d
def _layout_from_volume(root:Path)->dict[str,Any]:
    # Bootstrap rule: the Cortex checkout is a sibling authority named Cortex.
    # Once loaded, the manifest remains authoritative for every other path.
    candidates=[
      root/"Cortex"/"config"/"cortex"/"volume_layout.v2.json",
      _repo_root()/"config"/"cortex"/"volume_layout.v2.json",
    ]
    for p in candidates:
        if p.is_file():
            d=json.loads(p.read_text(encoding="utf-8-sig"))
            if d.get("schema")!=LAYOUT_SCHEMA: raise VolumeAuthorityError(f"Invalid volume layout: {p}")
            return d
    raise VolumeAuthorityError("Missing volume layout; checked: "+", ".join(str(p) for p in candidates))
def initialize_volume(volume_root:Path|str,*,cortex_root:Path|str|None=None):
    root=Path(volume_root).expanduser().resolve(); root.mkdir(parents=True,exist_ok=True)
    if read_marker(root) is None:
        payload={"schema":VOLUME_SCHEMA,"volumeId":str(uuid.uuid4()),"role":"portable-development-environment",
                 "createdUtc":datetime.now(timezone.utc).isoformat()}
        tmp=root/(MARKER_NAME+".tmp"); tmp.write_text(json.dumps(payload,indent=2)+"\n"); os.replace(tmp,root/MARKER_NAME)
    return resolve_volume_context(cortex_root or root/"Cortex",create=True)
def resolve_volume_context(cortex_root:Path|str|None=None,*,create=False):
    start=Path(cortex_root).expanduser().resolve() if cortex_root else _repo_root()
    root=discover_volume_root(start)
    if root is None: raise VolumeAuthorityError(f"Unable to discover marked Cortex volume from {start}")
    marker=read_marker(root)
    if marker is None: raise VolumeAuthorityError(f"Cortex volume marker missing: {root/MARKER_NAME}")
    a=_layout_from_volume(root)["authorities"]
    def P(key): return (root/a[key]).resolve()
    ctx=VolumeContext(str(marker["volumeId"]),root,P("cortex"),P("state"),P("artifacts"),P("backups"),
      P("catalogs"),P("git"),P("intake"),P("models"),P("objects"),P("projects"),P("recovery"),
      P("shared"),P("source"),P("tools"),P("vault"),P("archive"),P("exports"))
    if create:
        for v in ctx.__dict__.values():
            if isinstance(v,Path) and v!=root:v.mkdir(parents=True,exist_ok=True)
        for p in (ctx.registry_root,ctx.conversations_root,ctx.logs_root,ctx.checkpoints_root,ctx.receipts_root):p.mkdir(parents=True,exist_ok=True)
    return ctx
def volume_relative(path:Path|str,ctx:VolumeContext):
    try:return Path(path).expanduser().resolve().relative_to(ctx.mount_root).as_posix()
    except ValueError as e:raise VolumeAuthorityError(f"Path is outside Cortex volume: {path}") from e
def resolve_volume_path(relative_path:str,ctx:VolumeContext):
    raw=Path(relative_path)
    if raw.is_absolute() or ".." in raw.parts:raise VolumeAuthorityError(f"Unsafe volume-relative path: {relative_path}")
    return (ctx.mount_root/raw).resolve()
