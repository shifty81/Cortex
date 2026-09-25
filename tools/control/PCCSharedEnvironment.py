#!/usr/bin/env python3
"""Bounded shared/portable toolchain broker for Cortex/PCC operations.

Detection is intentionally read-only: this module never installs software, never
performs network access, and never mutates project source.  It constructs the
child-process environment required by an operation and reports what was found.
"""
from __future__ import annotations

import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any, Mapping

BROKER_VERSION = "PCC-TOOLCHAIN-BROKER-0.4"
DEFAULT_PROBE_TIMEOUT = 20.0


def _version_key(path: Path) -> tuple[int, ...]:
    numbers = re.findall(r"\d+", path.parent.name)
    return tuple(int(value) for value in numbers) if numbers else (0,)


def _repo_root(control_file: Path | None = None) -> Path:
    marker = (control_file or Path(__file__)).resolve()
    return marker.parents[2]


def _candidate_shared_roots(env: Mapping[str, str], control_file: Path) -> list[Path]:
    roots: list[Path] = []
    for key in ("CORTEX_SHARED_ROOT", "PCC_SHARED_ROOT"):
        raw = str(env.get(key) or "").strip()
        if raw:
            roots.append(Path(raw).expanduser())

    repo_root = _repo_root(control_file)
    anchor = Path(repo_root.anchor) if repo_root.anchor else repo_root
    roots.append(anchor / "shared")

    raw_vault = str(env.get("PCC_VAULT_ROOT") or env.get("CORTEX_VAULT_ROOT") or "").strip()
    if raw_vault:
        vault = Path(raw_vault).expanduser()
        vault_anchor = Path(vault.anchor) if vault.anchor else vault
        roots.append(vault_anchor / "shared")

    unique: list[Path] = []
    seen: set[str] = set()
    for root in roots:
        key = os.path.normcase(os.path.abspath(str(root)))
        if key not in seen:
            seen.add(key)
            unique.append(root)
    return unique


def discover_shared_python(
    env: Mapping[str, str] | None = None,
    *,
    control_file: Path | None = None,
) -> Path | None:
    source = dict(os.environ if env is None else env)
    explicit = str(source.get("CORTEX_PYTHON_EXE") or "").strip()
    if explicit:
        candidate = Path(explicit).expanduser()
        if candidate.is_file():
            return candidate.resolve()

    marker = control_file or Path(__file__)
    candidates: list[Path] = []
    for shared in _candidate_shared_roots(source, marker):
        python_root = shared / "toolchains" / "python"
        if not python_root.is_dir():
            continue
        direct = python_root / ("python.exe" if os.name == "nt" else "python3")
        if direct.is_file():
            candidates.append(direct)
        pattern = "*/python.exe" if os.name == "nt" else "*/bin/python3"
        candidates.extend(path for path in python_root.glob(pattern) if path.is_file())

    if not candidates:
        return None
    return max(candidates, key=_version_key).resolve()


def _prepend_path(env: dict[str, str], entries: list[Path | str]) -> None:
    existing = [item for item in str(env.get("PATH") or "").split(os.pathsep) if item]
    result: list[str] = []
    seen: set[str] = set()
    for raw in [*entries, *existing]:
        value = str(raw).strip()
        if not value:
            continue
        key = os.path.normcase(os.path.abspath(value))
        if key in seen:
            continue
        seen.add(key)
        result.append(value)
    env["PATH"] = os.pathsep.join(result)


def _which(name: str, env: Mapping[str, str]) -> str | None:
    return shutil.which(name, path=str(env.get("PATH") or ""))


def _merge_vault_dependency_environment(root: Path, env: dict[str, str]) -> tuple[dict[str, str], str]:
    """Merge the existing Vault dependency policy without provisioning anything."""
    try:
        from PCCVaultStorage import dependency_environment

        overlay = dependency_environment(root)
    except Exception as exc:
        return env, f"unavailable: {exc}"

    inherited_path = str(env.get("PATH") or "")
    overlay_path = str(overlay.get("PATH") or "")
    env.update({str(k): str(v) for k, v in overlay.items() if v is not None})
    if overlay_path or inherited_path:
        merged = [item for item in overlay_path.split(os.pathsep) if item]
        merged.extend(item for item in inherited_path.split(os.pathsep) if item)
        _prepend_path(env, merged)
    return env, "ready"


def _vswhere_candidates(env: Mapping[str, str]) -> list[Path]:
    out: list[Path] = []
    explicit = str(env.get("CORTEX_VSWHERE_EXE") or "").strip()
    if explicit:
        out.append(Path(explicit))
    for key in ("ProgramFiles(x86)", "ProgramFiles"):
        root = str(env.get(key) or os.environ.get(key) or "").strip()
        if root:
            out.append(Path(root) / "Microsoft Visual Studio" / "Installer" / "vswhere.exe")
    return out


def _discover_vsdevcmd(env: Mapping[str, str], *, timeout: float) -> Path | None:
    explicit = str(env.get("CORTEX_VSDEVCMD") or "").strip()
    if explicit:
        candidate = Path(explicit).expanduser()
        if candidate.is_file():
            return candidate.resolve()

    for vswhere in _vswhere_candidates(env):
        if not vswhere.is_file():
            continue
        try:
            cp = subprocess.run(
                [
                    str(vswhere),
                    "-latest",
                    "-products", "*",
                    "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                    "-property", "installationPath",
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=max(1.0, min(timeout, 8.0)),
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            continue
        install = cp.stdout.strip().splitlines()[0].strip() if cp.returncode == 0 and cp.stdout.strip() else ""
        if install:
            candidate = Path(install) / "Common7" / "Tools" / "VsDevCmd.bat"
            if candidate.is_file():
                return candidate.resolve()

    roots: list[Path] = []
    for key in ("ProgramFiles", "ProgramFiles(x86)"):
        value = str(env.get(key) or os.environ.get(key) or "").strip()
        if value:
            roots.append(Path(value))
    for root in roots:
        for edition in ("BuildTools", "Community", "Professional", "Enterprise"):
            candidate = root / "Microsoft Visual Studio" / "2022" / edition / "Common7" / "Tools" / "VsDevCmd.bat"
            if candidate.is_file():
                return candidate.resolve()
    return None


def _capture_cmd_environment(vsdevcmd: Path, *, timeout: float) -> dict[str, str]:
    """Capture VsDevCmd environment without relying on fragile cmd /S quoting.

    The previous inline command could fail when the Visual Studio path contained
    spaces/quotes.  A short temporary .cmd wrapper is deterministic and is
    deleted immediately after the bounded probe.
    """
    if os.name != "nt":
        return {}
    comspec = os.environ.get("ComSpec") or "cmd.exe"
    wrapper: Path | None = None
    try:
        fd, raw = tempfile.mkstemp(prefix="cortex-vsdevcmd-", suffix=".cmd")
        os.close(fd)
        wrapper = Path(raw)
        wrapper.write_text(
            "@echo off\r\n"
            f'call "{vsdevcmd}" -no_logo -arch=amd64 -host_arch=amd64 >nul\r\n'
            "if errorlevel 1 exit /b %errorlevel%\r\n"
            "set\r\n",
            encoding="utf-8",
        )
        cp = subprocess.run(
            [comspec, "/d", "/c", str(wrapper)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=max(1.0, timeout),
            check=False,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
    finally:
        if wrapper is not None:
            try:
                wrapper.unlink(missing_ok=True)
            except OSError:
                pass
    if cp.returncode != 0:
        detail = (cp.stderr or cp.stdout).strip()[-2000:]
        raise RuntimeError(f"VsDevCmd exited {cp.returncode}: {detail or '<no diagnostic text>'}")
    result: dict[str, str] = {}
    for line in cp.stdout.splitlines():
        if "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        if key:
            result[key] = value
    return result


def _tool_report(env: Mapping[str, str]) -> dict[str, Any]:
    python = str(env.get("CORTEX_PYTHON_EXE") or "").strip() or _which("python.exe" if os.name == "nt" else "python3", env)
    git = str(env.get("CORTEX_GIT_EXE") or "").strip() or _which("git.exe" if os.name == "nt" else "git", env)
    cargo = str(env.get("CORTEX_CARGO_EXE") or "").strip() or _which("cargo.exe" if os.name == "nt" else "cargo", env)
    rustc = str(env.get("CORTEX_RUSTC_EXE") or "").strip() or _which("rustc.exe" if os.name == "nt" else "rustc", env)
    cl = _which("cl.exe", env) if os.name == "nt" else None
    link = _which("link.exe", env) if os.name == "nt" else None
    return {
        "python": python,
        "git": git,
        "cargo": cargo,
        "rustc": rustc,
        "cl": cl,
        "link": link,
    }


def resolve_toolchain_environment(
    root: Path,
    env: Mapping[str, str] | None = None,
    *,
    control_file: Path | None = None,
    include_msvc: bool = True,
    timeout: float = DEFAULT_PROBE_TIMEOUT,
) -> tuple[dict[str, str], dict[str, Any]]:
    """Resolve a bounded child environment and a structured diagnostic report."""
    started = time.monotonic()
    project_root = root.expanduser().resolve()
    result = dict(os.environ if env is None else env)
    result["PYTHONUNBUFFERED"] = "1"

    result, vault_status = _merge_vault_dependency_environment(project_root, result)

    python = discover_shared_python(result, control_file=control_file)
    if python is not None:
        result["CORTEX_PYTHON_EXE"] = str(python)
        shared_root = python.parent.parent.parent
        result.setdefault("CORTEX_SHARED_ROOT", str(shared_root))
        result.setdefault("PCC_SHARED_ROOT", str(shared_root))
        _prepend_path(result, [python.parent, python.parent / "Scripts"])

    msvc: dict[str, Any] = {"required": os.name == "nt", "status": "not_required" if os.name != "nt" else "missing"}
    if os.name == "nt" and include_msvc:
        before = _tool_report(result)
        if before.get("cl") and before.get("link"):
            msvc = {"required": True, "status": "active", "vsDevCmd": str(result.get("CORTEX_VSDEVCMD") or "")}
        else:
            vsdevcmd = _discover_vsdevcmd(result, timeout=timeout)
            if vsdevcmd is None:
                msvc = {"required": True, "status": "missing", "vsDevCmd": ""}
            else:
                try:
                    captured = _capture_cmd_environment(vsdevcmd, timeout=timeout)
                    result.update(captured)
                    result["CORTEX_VSDEVCMD"] = str(vsdevcmd)
                    after = _tool_report(result)
                    status = "activated" if after.get("cl") and after.get("link") else "activation_incomplete"
                    msvc = {"required": True, "status": status, "vsDevCmd": str(vsdevcmd)}
                except subprocess.TimeoutExpired:
                    msvc = {"required": True, "status": "activation_timeout", "vsDevCmd": str(vsdevcmd)}
                except Exception as exc:
                    msvc = {"required": True, "status": "activation_failed", "vsDevCmd": str(vsdevcmd), "error": str(exc)}

    tools = _tool_report(result)
    report = {
        "schema": "pcc.toolchain_resolution.v1",
        "brokerVersion": BROKER_VERSION,
        "projectRoot": str(project_root),
        "elapsedMs": int((time.monotonic() - started) * 1000),
        "vaultDependencyEnvironment": vault_status,
        "tools": tools,
        "msvc": msvc,
    }
    return result, report


def apply_shared_toolchain_environment(
    env: Mapping[str, str] | None = None,
    *,
    control_file: Path | None = None,
    root: Path | None = None,
    include_msvc: bool = True,
    timeout: float = DEFAULT_PROBE_TIMEOUT,
) -> dict[str, str]:
    effective_root = (root or _repo_root(control_file)).expanduser().resolve()
    resolved, _report = resolve_toolchain_environment(
        effective_root,
        env,
        control_file=control_file,
        include_msvc=include_msvc,
        timeout=timeout,
    )
    return resolved
