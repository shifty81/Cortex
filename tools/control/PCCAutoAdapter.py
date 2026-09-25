#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable, Sequence

from PCCProjectDiscovery import discover_project_contract_data
from PCCRepoHygiene import prepare as repo_hygiene_prepare
from PCCSharedEnvironment import apply_shared_toolchain_environment

AUTO_ADAPTER_VERSION = "PCC-AUTO-ADAPTER-0.2"


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


def _run_quiet(argv: Sequence[str], cwd: Path, timeout: float = 15.0) -> subprocess.CompletedProcess[str]:
    # GUI-launched adapters inherit the PCC hidden console host.  Do not use
    # CREATE_NO_WINDOW here: a no-console child can cause its own grandchildren to
    # allocate transient consoles.  Inheriting one hidden console keeps the whole tree dark.
    return subprocess.run(
        list(argv), cwd=str(cwd), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, encoding="utf-8", errors="replace", timeout=timeout,
        check=False, creationflags=0,
    )


def _git(root: Path, *args: str) -> subprocess.CompletedProcess[str]:
    git = shutil.which("git") or "git"
    return _run_quiet([git, "-C", str(root), *args], root)


def _git_status(root: Path) -> dict[str, Any]:
    ready = bool(shutil.which("git")) and (root / ".git").exists()
    result: dict[str, Any] = {
        "gitReady": ready,
        "clean": False,
        "staged": 0,
        "unstaged": 0,
        "untracked": 0,
        "branch": "",
        "headShort": "",
        "ahead": None,
        "behind": None,
        "greenMarker": False,
        "greenMatch": False,
    }
    if not ready:
        return result
    cp = _git(root, "status", "--porcelain=v1", "--untracked-files=all")
    lines = [line for line in cp.stdout.splitlines() if line]
    staged = unstaged = untracked = 0
    for line in lines:
        code = line[:2] if len(line) >= 2 else "  "
        if code == "??":
            untracked += 1
            continue
        if code[0] not in {" ", "?"}:
            staged += 1
        if code[1] not in {" ", "?"}:
            unstaged += 1
    result.update(staged=staged, unstaged=unstaged, untracked=untracked, clean=(len(lines) == 0))
    branch = _git(root, "branch", "--show-current")
    result["branch"] = branch.stdout.strip()
    head = _git(root, "rev-parse", "--short=12", "HEAD")
    if head.returncode == 0:
        result["headShort"] = head.stdout.strip()
    counts = _git(root, "rev-list", "--left-right", "--count", "HEAD...@{upstream}")
    if counts.returncode == 0:
        parts = counts.stdout.split()
        if len(parts) == 2 and all(p.isdigit() for p in parts):
            result["ahead"] = int(parts[0]); result["behind"] = int(parts[1])
    return result


def _state_candidates(root: Path, data: dict[str, Any]) -> list[Path]:
    out: list[Path] = []
    declared = str(data.get("stateDirectory") or "").strip()
    if declared:
        out.append(root / declared)
    out.extend([root / ".cortex", root / ".havenwild", root / ".subspace", root / ".project_control"])
    seen: set[str] = set(); unique: list[Path] = []
    for path in out:
        key = os.path.normcase(str(path))
        if key not in seen:
            seen.add(key); unique.append(path)
    return unique


def _green_marker(root: Path, data: dict[str, Any], git: dict[str, Any]) -> tuple[bool, bool, str]:
    for state in _state_candidates(root, data):
        for name in ("last-green-quality-gate.json", "last_green_quality_gate.json", "green.json"):
            path = state / name
            if not path.is_file():
                continue
            try:
                payload = json.loads(path.read_text(encoding="utf-8-sig"))
            except Exception:
                payload = {}
            result = str(payload.get("result") or payload.get("status") or "PASS").strip().upper()
            marker = result in {"PASS", "GREEN", "OK", "SUCCESS"}
            # Universal fallback: a PASS marker + a clean working tree is treated as current.
            # Project-native providers remain authoritative whenever they can report stronger evidence.
            match = marker and bool(git.get("clean"))
            return marker, match, str(path)
    return False, False, ""


def _count_updates(root: Path) -> tuple[int, int]:
    candidates: list[Path] = []
    inbox = root / "updates" / "inbox"
    if inbox.is_dir():
        candidates.extend(inbox.glob("*.zip"))
    for path in root.glob("*.zip"):
        low = path.name.casefold()
        if "patch" in low and not any(x in low for x in ("debug", "bundle", "backup", "handoff", "rollup")):
            candidates.append(path)
    unique = {os.path.normcase(str(path.resolve())) for path in candidates if path.is_file()}
    return len(unique), 0


def _toolchain(root: Path) -> tuple[dict[str, bool], str]:
    tools: dict[str, bool] = {}
    labels: list[str] = []
    if (root / "Cargo.toml").is_file():
        tools["cargo"] = bool(shutil.which("cargo")); tools["rustc"] = bool(shutil.which("rustc")); labels.append("Rust/Cargo")
    if (root / "CMakeLists.txt").is_file() or (root / "engine" / "CMakeLists.txt").is_file():
        tools["cmake"] = bool(shutil.which("cmake")); labels.append("CMake/C++")
    if (root / "gradlew").is_file() or (root / "gradlew.bat").is_file():
        tools["gradle"] = True; labels.append("Gradle")
    if (root / "pyproject.toml").is_file():
        tools["python"] = True; labels.append("Python")
    return tools, ", ".join(labels) if labels else "Project tooling"


def _hygiene(root: Path) -> dict[str, Any]:
    offenders: list[str] = []
    for path in root.iterdir():
        if not path.is_file():
            continue
        low = path.name.casefold()
        if low.startswith("latest_debug_bundle") or ("debugbundle" in low and path.suffix.casefold() in {".zip", ".txt", ".json"}):
            offenders.append(path.name)
    return {"clean": not offenders, "violationCount": len(offenders), "violations": offenders}


def _latest_session_log(root: Path) -> str:
    roots = [root / "artifacts" / "logs" / "sessions", root / "logs" / "sessions"]
    files: list[Path] = []
    for folder in roots:
        if folder.is_dir():
            files.extend(path for path in folder.iterdir() if path.is_file())
    if not files:
        return ""
    try:
        return str(max(files, key=lambda p: p.stat().st_mtime))
    except OSError:
        return ""


def _has_run_command(data: dict[str, Any]) -> bool:
    return any(str(item.get("key") or "").startswith("run.") for item in data.get("commands", []) if isinstance(item, dict))


def status_payload(root: Path) -> dict[str, Any]:
    data = discover_project_contract_data(root)
    git = _git_status(root)
    marker, match, marker_path = _green_marker(root, data, git)
    git["greenMarker"] = marker; git["greenMatch"] = match; git["greenMarkerPath"] = marker_path
    pending, invalid = _count_updates(root)
    tools, toolchain = _toolchain(root)
    discovery = data.get("_pccDiscovery") or {}
    return {
        "schema": "pcc.auto_status.v1",
        "adapter": {"version": AUTO_ADAPTER_VERSION, "source": discovery.get("source"), "provider": discovery.get("provider")},
        "git": git,
        "patches": {"pending": pending, "invalid": invalid},
        "hygiene": _hygiene(root),
        "binaries": {"gui": "Registered runtime command" if _has_run_command(data) else ""},
        "tools": tools,
        "toolchain": toolchain,
        "session": {"log": _latest_session_log(root)},
    }


def _commands(data: dict[str, Any]) -> list[dict[str, Any]]:
    return [x for x in data.get("commands", []) if isinstance(x, dict) and str(x.get("key") or "").strip()]


def _find_command(data: dict[str, Any], key: str) -> dict[str, Any] | None:
    commands = _commands(data)
    index = {str(item.get("key")).casefold(): item for item in commands}
    if key.casefold() in index:
        return index[key.casefold()]
    for alias in ALIASES.get(key, ()): 
        if alias.casefold() in index:
            return index[alias.casefold()]
    return None


def _resolve_program(program: str) -> str:
    value = program.strip()
    low = value.casefold()
    if low in {"pwsh", "pwsh.exe"}:
        return shutil.which("pwsh") or shutil.which("powershell") or "powershell.exe"
    if low in {"powershell", "powershell.exe"}:
        return shutil.which("powershell") or shutil.which("pwsh") or "powershell.exe"
    if low in {"python", "python.exe", "python3"}:
        return sys.executable
    if low in {"cmd", "cmd.exe"}:
        return os.environ.get("COMSPEC") or "cmd.exe"
    return shutil.which(value) or value


def _expand_args(root: Path, args: Iterable[str]) -> list[str]:
    out: list[str] = []
    for raw in args:
        value = str(raw).replace("{root}", str(root)).replace("${ROOT}", str(root))
        out.append(value)
    return out


def _prepare_argv(root: Path, program_raw: str, args: list[str]) -> list[str]:
    raw = program_raw.strip()
    if not raw:
        return []
    if raw == "__pcc_internal__":
        return [raw, *args]
    candidate = root / raw
    if candidate.is_file():
        program = str(candidate)
    else:
        program = _resolve_program(raw)
    if os.name == "nt" and Path(program).suffix.casefold() in {".cmd", ".bat"}:
        comspec = os.environ.get("COMSPEC") or "cmd.exe"
        return [comspec, "/d", "/s", "/c", program, *args]
    return [program, *args]


def _stream_argv(root: Path, argv: list[str], *, label: str = "command", stdin_text: str = "") -> int:
    if not argv:
        print(f"[FAIL] {label} has no executable.", flush=True)
        return 2
    print("[AUTO] " + subprocess.list2cmdline(argv), flush=True)
    proc = subprocess.Popen(
        argv,
        cwd=str(root),
        stdin=subprocess.PIPE if stdin_text else subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
        bufsize=1,
        creationflags=0,
        env=apply_shared_toolchain_environment(os.environ.copy(), root=root),
    )
    if stdin_text and proc.stdin is not None:
        try:
            proc.stdin.write(stdin_text)
            proc.stdin.flush()
            proc.stdin.close()
        except OSError:
            pass
    assert proc.stdout is not None
    for line in proc.stdout:
        print(line, end="", flush=True)
    return proc.wait()


def _internal_operation(root: Path, name: str) -> int:
    name = name.casefold()
    if name in {"status", "health"}:
        payload = status_payload(root)
        print(f"Project : {root.name}")
        print(f"Root    : {root}")
        git = payload.get("git") or {}
        print(f"Git     : {'Clean' if git.get('clean') else 'Modified' if git.get('gitReady') else 'Not initialized'}")
        print(f"Tooling : {payload.get('toolchain') or 'Detected project'}")
        return 0
    if name == "rust-full-gate":
        stages = [
            ["cargo", "fmt", "--all", "--", "--check"],
            ["cargo", "check", "--workspace", "--all-targets"],
            ["cargo", "test", "--workspace", "--all-targets"],
            ["cargo", "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"],
            ["cargo", "build", "--workspace"],
        ]
        for argv in stages:
            print(f"[STEP] {' '.join(argv)}", flush=True)
            rc = _stream_argv(root, argv, label=name)
            if rc != 0:
                print(f"[FAIL] Full Rust quality gate stopped at: {' '.join(argv)}", flush=True)
                return rc
        print("[PASS] Full Rust quality gate GREEN", flush=True)
        return 0
    if name == "node-full-gate":
        for argv in (["npm", "test", "--"], ["npm", "run", "build"]):
            print(f"[STEP] {' '.join(argv)}", flush=True)
            rc = _stream_argv(root, list(argv), label=name)
            if rc != 0:
                return rc
        print("[PASS] Full Node quality gate GREEN", flush=True)
        return 0
    print(f"[FAIL] Unknown PCC internal operation: {name}", flush=True)
    return 2


def _stream_command(root: Path, item: dict[str, Any], *, stdin_text: str = "") -> int:
    args = _expand_args(root, item.get("args") or [])
    program_raw = str(item.get("program") or "")
    print(f"[AUTO] {item.get('label') or item.get('key')} [{item.get('key')}]", flush=True)
    if program_raw == "__pcc_internal__":
        if not args:
            print("[FAIL] Internal adapter command is missing its operation name.", flush=True)
            return 2
        return _internal_operation(root, args[0])
    argv = _prepare_argv(root, program_raw, args)
    return _stream_argv(root, argv, label=str(item.get("key") or "command"), stdin_text=stdin_text)


def _run_command_core(root: Path, command: str, *, message: str = "", assume_yes: bool = False) -> int:
    data = discover_project_contract_data(root)
    if command == "status-json":
        print(json.dumps(status_payload(root), separators=(",", ":")))
        return 0
    if command == "commit-push-green":
        commit = _find_command(data, "commit-green")
        push = _find_command(data, "push")
        if commit is None or push is None:
            print("[FAIL] Project contract does not expose both GREEN commit and push commands.")
            return 2
        rc = _stream_command(root, commit, stdin_text=(message + "\n") if message else "\n")
        if rc != 0:
            return rc
        return _stream_command(root, push)

    item = _find_command(data, command)
    if item is None:
        print(f"[FAIL] Universal operation '{command}' is not available for this project.")
        print("[INFO] Registered keys: " + ", ".join(str(x.get("key")) for x in _commands(data)))
        return 2

    stdin_text = ""
    if command == "commit-green":
        stdin_text = (message + "\n") if message else "\n"
    elif assume_yes:
        stdin_text = "Y\n"
    return _stream_command(root, item, stdin_text=stdin_text)



_AUTO_CLEAN_COMMANDS = {
    "full", "quick", "fast", "build", "build-release", "self-test",
    "commit-green", "commit-push-green", "push", "git-pull", "debug-bundle",
}

def run_command(root: Path, command: str, *, message: str = "", assume_yes: bool = False) -> int:
    # When invoked directly (outside the GUI operation host), preserve the same universal
    # repo-clean invariant.  The GUI host sets PCC_OPERATION_HOST_ACTIVE to avoid duplicate work.
    hosted = os.environ.get("PCC_OPERATION_HOST_ACTIVE") == "1"
    do_clean = command in _AUTO_CLEAN_COMMANDS and not hosted
    if do_clean:
        try:
            result = repo_hygiene_prepare(root, apply=True)
            print(f"[PASS] Pre-operation repository transport hygiene moved {int(result.get('moved', 0) or 0)} artifact(s).")
        except Exception as exc:
            print(f"[FAIL] Pre-operation repository transport hygiene failed: {exc}")
            return 1
    rc = 1
    try:
        rc = _run_command_core(root, command, message=message, assume_yes=assume_yes)
        return rc
    finally:
        if do_clean:
            try:
                result = repo_hygiene_prepare(root, apply=True)
                print(f"[PASS] Post-operation repository transport hygiene moved {int(result.get('moved', 0) or 0)} artifact(s).")
            except Exception as exc:
                print(f"[WARN] Post-operation repository transport hygiene failed: {exc}")


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Universal PCC auto-discovered project adapter")
    parser.add_argument("command")
    parser.add_argument("--root", required=True)
    parser.add_argument("--message", default="")
    parser.add_argument("--yes", action="store_true")
    ns, _unknown = parser.parse_known_args(argv)
    root = Path(ns.root).expanduser().resolve()
    if not root.is_dir():
        print(f"[FAIL] Project root does not exist: {root}")
        return 2
    return run_command(root, str(ns.command), message=str(ns.message), assume_yes=bool(ns.yes))


if __name__ == "__main__":
    raise SystemExit(main())
