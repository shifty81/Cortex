#!/usr/bin/env python3
from __future__ import annotations
import hashlib, os, re, shutil, subprocess
from pathlib import Path
from typing import Any, Mapping
from PCCVolumeAuthority import resolve_volume_context
BROKER_VERSION="PCC-PORTABLE-PROJECT-ENV-0.5"

def _project_id(root:Path)->str:
    manifest=root/"project.control.json"
    if manifest.is_file():
        try:
            import json
            d=json.loads(manifest.read_text(encoding="utf-8-sig"))
            raw=str((d.get("project") or {}).get("id") or d.get("projectId") or "").strip()
            if raw: return re.sub(r"[^A-Za-z0-9_.-]+","-",raw)[:96]
        except Exception: pass
    slug=re.sub(r"[^A-Za-z0-9_.-]+","-",root.name).strip("-") or "project"
    return slug+"-"+hashlib.sha256(str(root).casefold().encode()).hexdigest()[:8]

def _prepend(env:dict[str,str],*paths:Path):
    vals=[str(p) for p in paths if p.exists()]
    env["PATH"]=os.pathsep.join(vals+[x for x in env.get("PATH","").split(os.pathsep) if x])

def portable_project_environment(root:Path, env:Mapping[str,str]|None=None)->tuple[dict[str,str],dict[str,Any]]:
    root=root.resolve(); out=dict(os.environ if env is None else env); ctx=resolve_volume_context(root)
    pid=_project_id(root); shared=ctx.shared_root
    cargo=shared/"dependencies"/"rust"/"cargo-home"
    sccache=shared/"dependencies"/"rust"/"sccache"
    target=shared/"build"/"rust"/"targets"/pid
    npm=shared/"dependencies"/"node"/"npm-cache"
    pip=shared/"dependencies"/"python"/"pip-cache"
    gradle=shared/"dependencies"/"java"/"gradle-home"
    nuget=shared/"dependencies"/"dotnet"/"nuget-packages"
    vcpkgd=shared/"dependencies"/"cpp"/"vcpkg-downloads"
    vcpkgb=shared/"dependencies"/"cpp"/"vcpkg-binary-cache"
    for p in (cargo,sccache,target,npm,pip,gradle,nuget,vcpkgd,vcpkgb): p.mkdir(parents=True,exist_ok=True)
    out.update({
      "CORTEX_VOLUME_ROOT":str(ctx.mount_root),"CORTEX_VOLUME_ID":ctx.volume_id,
      "CORTEX_SHARED_ROOT":str(shared),"PCC_SHARED_ROOT":str(shared),
      "CORTEX_ARTIFACTS_ROOT":str(ctx.artifacts_root),"CORTEX_PROJECTS_ROOT":str(ctx.projects_root),
      "CORTEX_PROJECT_ID":pid,"CARGO_HOME":str(cargo),"CARGO_TARGET_DIR":str(target),
      "npm_config_cache":str(npm),"PIP_CACHE_DIR":str(pip),"GRADLE_USER_HOME":str(gradle),
      "NUGET_PACKAGES":str(nuget),"VCPKG_DOWNLOADS":str(vcpkgd),
      "VCPKG_DEFAULT_BINARY_CACHE":str(vcpkgb),"RUSTUP_AUTO_INSTALL":"0","PYTHONUNBUFFERED":"1"
    })
    # Portable toolchains, if present. Machine-installed tools remain fallback.
    tc=shared/"toolchains"
    for family in ("rust","python","node","cmake","ninja","git"):
        base=tc/family
        if base.is_dir():
            versions=sorted([p for p in base.iterdir() if p.is_dir()], reverse=True)
            if versions: _prepend(out,versions[0],versions[0]/"bin",versions[0]/"Scripts")
    if shutil.which("sccache",path=out.get("PATH","")): out["RUSTC_WRAPPER"]="sccache"
    report={"schema":"pcc.portable_project_environment.v1","brokerVersion":BROKER_VERSION,
      "projectId":pid,"projectRoot":str(root),"volumeId":ctx.volume_id,"mountRoot":str(ctx.mount_root),
      "sharedRoot":str(shared),"cargoHome":str(cargo),"cargoTargetDir":str(target),
      "artifactRoot":str(ctx.artifacts_root/"projects"/pid)}
    return out,report
