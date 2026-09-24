#!/usr/bin/env python3
from __future__ import annotations

import argparse
import ast
import json
import os
import platform
import queue
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import uuid
import zipfile
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Iterable, Sequence

import CortexDesktopReadiness as desktop_readiness
import CortexGitAuthority as source_authority
import CortexPCCMaintenance as maintenance
import CortexSourceRollup as source_rollup
import PCCVaultStorage as vault_storage

PCC_VERSION = "CTX-PCC-12.3"
DEFAULT_REMOTE = "https://github.com/shifty81/Cortex.git"
RESULT_PREFIX = "PCC_RESULT_JSON="


class PCCError(RuntimeError):
    pass


class PCCRestart(RuntimeError):
    pass


def now_utc() -> str:
    return datetime.now(timezone.utc).isoformat()


def local_stamp() -> str:
    return datetime.now().strftime("%Y%m%d-%H%M%S")


def normalize_root(raw: str | os.PathLike[str] | None) -> Path:
    if raw:
        root = Path(raw).expanduser().resolve()
    else:
        root = Path(__file__).resolve().parents[2]
    if not root.is_dir():
        raise PCCError(f"Project root does not exist: {root}")
    return root


def is_windows() -> bool:
    return os.name == "nt"


def which_python() -> list[str]:
    # Current interpreter is always the preferred authority once CortexPCC.py is running.
    if sys.executable:
        return [sys.executable]
    py = shutil.which("python")
    if py:
        return [py]
    launcher = shutil.which("py")
    if launcher:
        return [launcher, "-3"]
    raise PCCError("Python 3 is required by Cortex PCC.")


def open_folder(path: Path) -> None:
    path.mkdir(parents=True, exist_ok=True)
    if is_windows():
        os.startfile(str(path))  # type: ignore[attr-defined]
        return
    opener = shutil.which("xdg-open") or shutil.which("open")
    if opener:
        subprocess.Popen([opener, str(path)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def open_url(url: str) -> None:
    if is_windows():
        os.startfile(url)  # type: ignore[attr-defined]
        return
    opener = shutil.which("xdg-open") or shutil.which("open")
    if opener:
        subprocess.Popen([opener, url], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


@dataclass
class Event:
    level: str
    message: str
    utc: str = field(default_factory=now_utc)
    phase: str | None = None
    command: str | None = None
    data: dict[str, Any] = field(default_factory=dict)


class SessionLog:
    def __init__(self, root: Path, *, quiet: bool = False) -> None:
        self.root = root
        self.quiet = quiet
        self.session_id = f"PCC-{local_stamp()}-{uuid.uuid4().hex[:8]}"
        self.logs_dir = root / "artifacts" / "logs" / "sessions"
        self.logs_dir.mkdir(parents=True, exist_ok=True)
        self.text_path = self.logs_dir / f"cortex-pcc-{self.session_id}.log"
        self.jsonl_path = self.logs_dir / f"cortex-pcc-{self.session_id}.jsonl"
        self.command_output_dir = root / "artifacts" / "logs" / "command-output" / self.session_id
        self._lock = threading.Lock()

    def emit(self, level: str, message: str, *, phase: str | None = None,
             command: str | None = None, data: dict[str, Any] | None = None) -> None:
        event = Event(level=level.upper(), message=message, phase=phase, command=command, data=data or {})
        line = f"[{event.utc}] [{event.level}] {message}"
        with self._lock:
            with self.text_path.open("a", encoding="utf-8") as f:
                f.write(line + "\n")
            with self.jsonl_path.open("a", encoding="utf-8") as f:
                f.write(json.dumps(event.__dict__, sort_keys=True) + "\n")
        if not self.quiet:
            print(f"[{event.level}] {message}")


@dataclass
class CommandResult:
    argv: list[str]
    cwd: str
    returncode: int
    elapsed_seconds: float
    stdout: str
    stderr: str
    timed_out: bool = False
    cancelled: bool = False

    @property
    def ok(self) -> bool:
        return self.returncode == 0 and not self.timed_out and not self.cancelled


class CommandRunner:
    """Streaming subprocess runner that works on Windows without selectors."""

    def __init__(self, log: SessionLog, *, base_env: dict[str, str] | None = None) -> None:
        self.log = log
        self.base_env = dict(base_env or {})
        self._cancel = threading.Event()
        self._active: subprocess.Popen[str] | None = None
        self._lock = threading.Lock()
        self._command_index = 0

    @staticmethod
    def _safe_phase(value: str | None) -> str:
        text = re.sub(r"[^A-Za-z0-9._-]+", "-", (value or "command").strip()).strip("-._")
        return text[:80] or "command"

    def _persist_failure_output(self, result: CommandResult, *, phase: str | None, display: str) -> None:
        """Persist full failed-command output so debug bundles are self-diagnosing."""
        self._command_index += 1
        out_dir = self.log.command_output_dir
        out_dir.mkdir(parents=True, exist_ok=True)
        stem = f"{self._command_index:03d}-{self._safe_phase(phase)}"
        text_path = out_dir / f"{stem}.txt"
        json_path = out_dir / f"{stem}.json"
        body = (
            f"COMMAND: {display}\n"
            f"CWD: {result.cwd}\n"
            f"EXIT: {result.returncode}\n"
            f"TIMED_OUT: {result.timed_out}\n"
            f"CANCELLED: {result.cancelled}\n"
            "\n===== STDOUT =====\n"
            f"{result.stdout}"
            "\n===== STDERR =====\n"
            f"{result.stderr}"
        )
        text_path.write_text(body, encoding="utf-8", errors="replace")
        maintenance.atomic_write_json(
            json_path,
            {
                "schema": "cortex.command_failure.v1",
                "sessionId": self.log.session_id,
                "phase": phase,
                "command": display,
                "cwd": result.cwd,
                "returncode": result.returncode,
                "elapsedSeconds": result.elapsed_seconds,
                "timedOut": result.timed_out,
                "cancelled": result.cancelled,
                "stdoutBytes": len(result.stdout.encode("utf-8", errors="replace")),
                "stderrBytes": len(result.stderr.encode("utf-8", errors="replace")),
                "textArtifact": text_path.name,
            },
        )
        relative = text_path.relative_to(self.log.root).as_posix()
        self.log.emit(
            "INFO",
            f"Captured failed command output: {relative}",
            phase="evidence:command-output",
            data={"path": relative},
        )

    def cancel(self) -> None:
        self._cancel.set()
        with self._lock:
            proc = self._active
        if proc and proc.poll() is None:
            self._terminate_tree(proc)

    @staticmethod
    def _terminate_tree(proc: subprocess.Popen[str]) -> None:
        try:
            if is_windows():
                subprocess.run(
                    ["taskkill", "/PID", str(proc.pid), "/T", "/F"],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    check=False,
                    timeout=10,
                )
            else:
                try:
                    os.killpg(proc.pid, signal.SIGTERM)
                except Exception:
                    proc.terminate()
        except Exception:
            try:
                proc.kill()
            except Exception:
                pass

    def run(self, argv: Sequence[str], *, cwd: Path, timeout: float = 180.0,
            stream: bool = True, phase: str | None = None,
            env: dict[str, str] | None = None) -> CommandResult:
        if not argv:
            raise PCCError("Cannot run an empty command.")
        args = [str(x) for x in argv]
        self._cancel.clear()
        display = subprocess.list2cmdline(args) if is_windows() else " ".join(args)
        self.log.emit("INFO", f"START {display}", phase=phase, command=display)
        start = time.monotonic()
        q: queue.Queue[tuple[str, str | None]] = queue.Queue()
        creationflags = 0
        popen_kwargs: dict[str, Any] = {}
        if is_windows():
            creationflags = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
        else:
            popen_kwargs["start_new_session"] = True
        child_env = os.environ.copy()
        child_env.update(self.base_env)
        if env:
            child_env.update(env)
        proc = subprocess.Popen(
            args,
            cwd=str(cwd),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            stdin=subprocess.DEVNULL,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
            creationflags=creationflags,
            env=child_env,
            **popen_kwargs,
        )
        with self._lock:
            self._active = proc

        stdout_parts: list[str] = []
        stderr_parts: list[str] = []

        def reader(name: str, pipe: Any) -> None:
            try:
                for line in iter(pipe.readline, ""):
                    q.put((name, line))
            finally:
                q.put((name, None))

        threads = [
            threading.Thread(target=reader, args=("stdout", proc.stdout), daemon=True),
            threading.Thread(target=reader, args=("stderr", proc.stderr), daemon=True),
        ]
        for t in threads:
            t.start()

        done_streams: set[str] = set()
        timed_out = False
        cancelled = False
        try:
            while len(done_streams) < 2 or proc.poll() is None:
                if self._cancel.is_set():
                    cancelled = True
                    self._terminate_tree(proc)
                if timeout > 0 and (time.monotonic() - start) > timeout and proc.poll() is None:
                    timed_out = True
                    self._terminate_tree(proc)
                try:
                    name, line = q.get(timeout=0.05)
                except queue.Empty:
                    continue
                if line is None:
                    done_streams.add(name)
                    continue
                if name == "stdout":
                    stdout_parts.append(line)
                    if stream:
                        print(line, end="")
                else:
                    stderr_parts.append(line)
                    if stream:
                        print(line, end="", file=sys.stderr)
            rc = proc.wait(timeout=5)
        finally:
            with self._lock:
                self._active = None
            for t in threads:
                t.join(timeout=0.2)
            for pipe in (proc.stdout, proc.stderr):
                try:
                    if pipe is not None:
                        pipe.close()
                except Exception:
                    pass
        elapsed = time.monotonic() - start
        result = CommandResult(
            argv=args,
            cwd=str(cwd),
            returncode=rc if not timed_out and not cancelled else (124 if timed_out else 130),
            elapsed_seconds=elapsed,
            stdout="".join(stdout_parts),
            stderr="".join(stderr_parts),
            timed_out=timed_out,
            cancelled=cancelled,
        )
        level = "PASS" if result.ok else "FAIL"
        suffix = f" ({elapsed:.2f}s, exit {result.returncode})"
        if timed_out:
            suffix += " TIMEOUT"
        if cancelled:
            suffix += " CANCELLED"
        self.log.emit(level, f"END {display}{suffix}", phase=phase, command=display)
        if not result.ok:
            try:
                self._persist_failure_output(result, phase=phase, display=display)
            except Exception as exc:
                self.log.emit(
                    "WARN",
                    f"Failed to persist command output evidence: {exc}",
                    phase="evidence:command-output",
                )
        return result


@dataclass
class ProjectContext:
    root: Path
    tools: Path
    artifacts: Path
    debug_dir: Path
    patch_receipts: Path
    patch_applied: Path
    patch_failed: Path
    patch_backups: Path
    cargo_toml: Path
    project_control: Path
    remote: str = DEFAULT_REMOTE

    @classmethod
    def create(cls, root: Path, remote: str = DEFAULT_REMOTE) -> "ProjectContext":
        tools = root / "tools" / "control"
        return cls(
            root=root,
            tools=tools,
            artifacts=root / "artifacts",
            debug_dir=root / "artifacts" / "debug",
            patch_receipts=root / "artifacts" / "patches" / "receipts",
            patch_applied=root / "artifacts" / "patches" / "applied",
            patch_failed=root / "artifacts" / "patches" / "failed",
            patch_backups=root / ".project_control" / "patch-backups",
            cargo_toml=root / "Cargo.toml",
            project_control=root / "project.control.json",
            remote=remote,
        )


class JsonAuthorityBridge:
    def __init__(self, ctx: ProjectContext, runner: CommandRunner, log: SessionLog) -> None:
        self.ctx = ctx
        self.runner = runner
        self.log = log

    def _run_python(self, script: Path, args: Sequence[str], *, timeout: float = 180.0,
                    stream: bool = False, phase: str | None = None) -> CommandResult:
        return self.runner.run([*which_python(), str(script), *args], cwd=self.ctx.root,
                               timeout=timeout, stream=stream, phase=phase)

    @staticmethod
    def parse_result_json(text: str) -> dict[str, Any] | None:
        for line in reversed(text.splitlines()):
            if line.startswith(RESULT_PREFIX):
                try:
                    data = json.loads(line[len(RESULT_PREFIX):])
                    return data if isinstance(data, dict) else None
                except json.JSONDecodeError:
                    return None
        return None


class GitAuthority(JsonAuthorityBridge):
    @property
    def script(self) -> Path:
        return self.ctx.tools / "CortexGitAuthority.py"

    def action(self, action: str, *, message: str = "", extra: Sequence[str] = (), stream: bool = True,
               timeout: float = 300.0) -> CommandResult:
        args = [action, "--root", str(self.ctx.root), "--remote", self.ctx.remote]
        if message:
            args += ["--message", message]
        args += list(extra)
        return self._run_python(self.script, args, timeout=timeout, stream=stream, phase=f"git:{action}")

    def summary(self) -> dict[str, Any]:
        result = self.action("summary-json", stream=False, timeout=60)
        if not result.ok:
            return {"gitReady": False, "greenMatch": False, "clean": False, "error": result.stderr or result.stdout}
        try:
            data = json.loads(result.stdout.strip().splitlines()[-1])
            return data if isinstance(data, dict) else {}
        except Exception as exc:
            return {"gitReady": False, "greenMatch": False, "clean": False, "error": str(exc)}


class PatchAuthority(JsonAuthorityBridge):
    @property
    def script(self) -> Path:
        return self.ctx.tools / "CortexPatchAuthority.py"

    def scan(self) -> tuple[int, dict[str, Any]]:
        result = self._run_python(self.script, ["scan", "--root", str(self.ctx.root)],
                                  timeout=120, stream=False, phase="patch:scan")
        payload = self.parse_result_json(result.stdout + "\n" + result.stderr) or {
            "Applied": 0, "Pending": 0, "Invalid": 1, "Ignored": 0,
            "RestartRequired": False, "ValidPatches": [], "InvalidPatches": [],
        }
        return result.returncode, payload

    def apply(self, *, stream: bool = True) -> tuple[int, dict[str, Any]]:
        result = self._run_python(self.script, ["apply", "--root", str(self.ctx.root)],
                                  timeout=600, stream=stream, phase="patch:apply")
        payload = self.parse_result_json(result.stdout + "\n" + result.stderr) or {
            "Applied": 0, "Pending": 0, "Invalid": 1, "Ignored": 0,
            "RestartRequired": False, "ValidPatches": [], "InvalidPatches": [],
        }
        return result.returncode, payload


@dataclass
class Check:
    name: str
    status: str
    detail: str
    elapsed_seconds: float = 0.0

    @property
    def ok(self) -> bool:
        return self.status in {"PASS", "WARN"}


class GateEngine:
    def __init__(self, ctx: ProjectContext, runner: CommandRunner, log: SessionLog,
                 git: GitAuthority, patch: PatchAuthority) -> None:
        self.ctx = ctx
        self.runner = runner
        self.log = log
        self.git = git
        self.patch = patch
        self.failed_stage = ""

    def _check(self, name: str, fn: Callable[[], tuple[str, str]]) -> Check:
        start = time.monotonic()
        try:
            status, detail = fn()
        except Exception as exc:
            status, detail = "FAIL", str(exc)
        elapsed = time.monotonic() - start
        self.log.emit(status, f"{name}: {detail}", phase="quick")
        return Check(name, status, detail, elapsed)

    def _required_files(self) -> tuple[str, str]:
        required = [
            self.ctx.root / "PROJECT_CONTROL_CENTER.cmd",
            self.ctx.cargo_toml,
            self.ctx.project_control,
            self.ctx.tools / "CortexPCC.py",
            self.ctx.tools / "CortexPCCGui.py",
            self.ctx.tools / "CortexPCCConsole.py",
            self.ctx.tools / "PCCSurfaceCommon.py",
            self.ctx.tools / "CortexGitAuthority.py",
            self.ctx.tools / "CortexPatchAuthority.py",
            self.ctx.tools / "CortexPCCMaintenance.py",
            self.ctx.tools / "PCCStoragePaths.py",
            self.ctx.tools / "PCCVaultStorage.py",
        ]
        missing = [str(p.relative_to(self.ctx.root)) for p in required if not p.is_file()]
        return ("FAIL", "missing: " + ", ".join(missing)) if missing else ("PASS", f"{len(required)} required files present")

    def _python_syntax(self) -> tuple[str, str]:
        files = sorted(p for p in self.ctx.tools.rglob("*.py") if "__pycache__" not in p.parts)
        if not files:
            return "FAIL", "no Python PCC files found"
        for path in files:
            ast.parse(path.read_text(encoding="utf-8-sig"), filename=str(path))
        return "PASS", f"AST parsed {len(files)} Python files"

    def _json_contracts(self) -> tuple[str, str]:
        # Project configuration and patch transport metadata are separate authorities.
        # A root PATCH_MANIFEST.json must never make the project-contract gate pass.
        if not self.ctx.project_control.is_file():
            return "FAIL", f"project contract missing: {self.ctx.project_control.name}"
        # The manifest must be semantically valid before Cargo starts. The
        # existing Python controller can still inspect historical rollback
        # descriptors, but they are explicitly WARN, not Rust-compatible.
        from UniversalPCCAudit import _read_manifest, validate_contract
        raw, _, error = _read_manifest(self.ctx.project_control)
        if error or raw is None:
            return "FAIL", error or "project contract missing"
        validated = validate_contract(raw, rust_consumer=False)
        if validated["errors"]:
            return "FAIL", "; ".join(validated["errors"][:8])
        if validated["warnings"]:
            return "WARN", "legacy-provider contract accepted with compatibility warnings; typed Rust readiness NOT certified: " + "; ".join(validated["warnings"][:4])
        return "PASS", "semantic contract valid; commands and required gate stages resolved"

    def _powershell_syntax(self) -> tuple[str, str]:
        ps = shutil.which("pwsh") or shutil.which("powershell") or shutil.which("powershell.exe")
        if not ps:
            return "WARN", "PowerShell unavailable on this host; Windows staged AST gate remains authoritative"
        files = sorted(self.ctx.tools.rglob("*.ps1")) + sorted(self.ctx.tools.rglob("*.psm1"))
        if (self.ctx.root / "scripts").is_dir():
            files += sorted((self.ctx.root / "scripts").rglob("*.ps1"))
            files += sorted((self.ctx.root / "scripts").rglob("*.psm1"))
        if not files:
            return "WARN", "no PowerShell compatibility scripts found"
        # PowerShell's parser returns an errors array without executing the scripts.
        file_list = ",".join("'" + str(p).replace("'", "''") + "'" for p in files)
        expr = (
            "$bad=0; foreach($f in @(" + file_list + ")) {"
            "$t=$null;$e=$null;[System.Management.Automation.Language.Parser]::ParseFile($f,[ref]$t,[ref]$e)|Out-Null;"
            "if($e.Count -gt 0){$bad=1;$e|ForEach-Object{Write-Error (\"${f}: \"+$_.Message)}}}; exit $bad"
        )
        cp = subprocess.run([ps, "-NoLogo", "-NoProfile", "-Command", expr],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                            encoding="utf-8", errors="replace", timeout=60, check=False)
        if cp.returncode != 0:
            return "FAIL", (cp.stderr or cp.stdout).strip()[-2000:]
        return "PASS", f"AST parsed {len(files)} PowerShell compatibility scripts"

    def _patch_authority(self) -> tuple[str, str]:
        code, summary = self.patch.scan()
        if code not in (0, 2):
            return "FAIL", f"patch authority scan exited {code}"
        invalid = int(summary.get("Invalid", 0) or 0)
        pending = int(summary.get("Pending", 0) or 0)
        recovered = int(summary.get("RecoveredSidecars", 0) or 0)
        if invalid:
            details = summary.get("InvalidPatches", []) or []
            first = details[0] if details else {}
            reason = str(first.get("error", "unknown patch validation error"))
            name = str(first.get("name", "recognized patch"))
            return "FAIL", f"{invalid} invalid recognized patch(es); first: {name} - {reason}"
        suffix = f", recovered {recovered} missing sidecar(s)" if recovered else ""
        return "PASS", f"{pending} valid pending patch(es), no invalid patches{suffix}"

    def _git_authority(self) -> tuple[str, str]:
        if not self.git.script.is_file():
            return "FAIL", "Git authority missing"
        result = self.git.action("summary-json", stream=False, timeout=60)
        if not result.ok:
            return "FAIL", f"Git authority exited {result.returncode}: {(result.stderr or result.stdout).strip()[-1000:]}"
        json.loads(result.stdout.strip().splitlines()[-1])
        return "PASS", "Git authority returned machine-readable status"

    def _root_hygiene(self) -> tuple[str, str]:
        summary = maintenance.scan_root_hygiene(self.ctx.root)
        violations = int(summary.get("violationCount", 0) or 0)
        advisories = int(summary.get("advisoryCount", 0) or 0)
        if violations:
            return "WARN", f"{violations} generated root residue item(s); Fast/Full will normalize them into artifacts"
        if advisories:
            return "WARN", f"root clean; {advisories} legacy operational log item(s) can be normalized"
        return "PASS", "generated operational artifacts are contained under artifacts/"

    def _storage_readiness(self) -> tuple[str, str]:
        try:
            vault_storage.ensure_layout(self.ctx.root)
            status = vault_storage.dependency_status(self.ctx.root)
            mirror = vault_storage.mirror_status(self.ctx.root)
        except Exception as exc:
            return "FAIL", f"Vault/shared dependency storage unavailable: {exc}"
        vault_root = Path(str(status.get("vaultRoot") or ""))
        if not vault_root:
            return "FAIL", "Vault root could not be resolved"
        if not os.access(vault_root, os.W_OK):
            return "FAIL", f"Vault root is not writable: {vault_root}"
        mirror_note = "mirror present" if mirror.get("hasSnapshot") else "no mirror yet; Full Gate will create first certified snapshot"
        return "PASS", f"Vault={vault_root}; shared dependency paths ready; {mirror_note}"

    def _cargo_tools(self) -> tuple[str, str]:
        cargo = shutil.which("cargo")
        rustc = shutil.which("rustc")
        if not cargo:
            return "FAIL", "cargo not found on PATH"
        detail = Path(cargo).name
        if rustc:
            detail += f", rustc={Path(rustc).name}"
        return "PASS", detail

    def _windows_linker_toolchain(self) -> tuple[str, str]:
        if os.name != "nt":
            return "PASS", "non-Windows host; MSVC linker check not required"
        linker = shutil.which("link.exe")
        compiler = shutil.which("cl.exe")
        if not linker or not compiler:
            return (
                "FAIL",
                "MSVC amd64 developer environment is not active (link.exe/cl.exe missing). "
                "Run HYDRATE_CORTEX_BUILD_TOOLS.cmd, then restart the PCC.",
            )
        return "PASS", f"linker={Path(linker).name}, compiler={Path(compiler).name}"

    def _cargo_metadata(self) -> tuple[str, str]:
        if not shutil.which("cargo"):
            return "FAIL", "cargo unavailable"
        result = self.runner.run(["cargo", "metadata", "--no-deps", "--format-version", "1", "--quiet"],
                                 cwd=self.ctx.root, timeout=120, stream=False, phase="quick:cargo-metadata")
        if not result.ok:
            return "FAIL", (result.stderr or result.stdout).strip()[-1500:]
        data = json.loads(result.stdout)
        packages = data.get("packages") or []
        return "PASS", f"workspace metadata valid: {len(packages)} package(s)"

    def quick(self) -> tuple[bool, list[Check]]:
        self.failed_stage = "quick"
        checks = [
            self._check("Required PCC/project files", self._required_files),
            self._check("Python PCC syntax", self._python_syntax),
            self._check("JSON contracts", self._json_contracts),
            self._check("Root artifact hygiene", self._root_hygiene),
            self._check("PowerShell compatibility syntax", self._powershell_syntax),
            self._check("Patch authority", self._patch_authority),
            self._check("Git authority", self._git_authority),
            self._check("Vault/shared storage", self._storage_readiness),
            self._check("Rust/Cargo tools", self._cargo_tools),
            self._check("Windows linker toolchain", self._windows_linker_toolchain),
            self._check("Cargo workspace metadata", self._cargo_metadata),
        ]
        failed = next((c for c in checks if c.status == "FAIL"), None)
        ok = failed is None
        if ok:
            self.failed_stage = ""
            self.log.emit("PASS", "QUICK PROJECT GATE GREEN", phase="gate:quick")
        else:
            stage_names = {
                "Required PCC/project files": "pcc-required-files",
                "Python PCC syntax": "pcc-python-syntax",
                "JSON contracts": "project-contract",
                "Root artifact hygiene": "root-hygiene",
                "PowerShell compatibility syntax": "powershell-syntax",
                "Patch authority": "patch-authority",
                "Git authority": "git-authority",
                "Vault/shared storage": "vault-storage",
                "Rust/Cargo tools": "cargo-toolchain",
                "Windows linker toolchain": "windows-linker-toolchain",
                "Cargo workspace metadata": "cargo-metadata",
            }
            self.failed_stage = stage_names.get(failed.name, "quick")
            self.log.emit("FAIL", f"QUICK PROJECT GATE FAILED at {self.failed_stage}: {failed.detail}", phase="gate:quick")
        return ok, checks

    def cargo_step(self, label: str, args: Sequence[str], *, timeout: float = 1800) -> bool:
        self.failed_stage = label
        result = self.runner.run(["cargo", *args], cwd=self.ctx.root, timeout=timeout,
                                 stream=True, phase=f"cargo:{label}")
        if not result.ok:
            if label == "cargo-fmt":
                self.log.emit(
                    "FAIL",
                    "cargo-fmt failed; run `fmt.apply` (or `CortexPCC.py format`) and rerun the gate",
                    phase="gate",
                )
            elif label == "cargo-clippy":
                self.log.emit(
                    "FAIL",
                    "cargo-clippy failed; run `clippy.apply` for machine-fixable lints, review the diff, then rerun the gate",
                    phase="gate",
                )
            else:
                self.log.emit("FAIL", f"{label} failed", phase="gate")
            return False
        return True

    def fast(self) -> bool:
        ok, _ = self.quick()
        if not ok:
            return False
        if not self.cargo_step("cargo-fmt", ["fmt", "--all", "--", "--check"], timeout=300):
            return False
        if not self.cargo_step("cargo-check", ["check", "--workspace", "--all-targets"], timeout=1800):
            return False
        self.failed_stage = ""
        self.log.emit("PASS", "FAST DEVELOPMENT GATE GREEN", phase="gate:fast")
        return True

    def full(self) -> bool:
        ok, _ = self.quick()
        if not ok:
            return False
        # Full Gate must include the Python operational control plane, not
        # only the Rust workspace. All stages run before mark-green.
        self.failed_stage = "python-pcc-regressions"
        if run_universal_python_regressions(self.ctx.root, self.runner) != 0:
            self.log.emit("FAIL", "Python PCC regression tests failed; GREEN not issued", phase="gate:full")
            return False
        steps: list[tuple[str, list[str], float]] = [
            ("cargo-fmt", ["fmt", "--all", "--", "--check"], 300),
            ("cargo-check", ["check", "--workspace", "--all-targets"], 1800),
            ("cargo-test", ["test", "--workspace", "--all-targets"], 3600),
            ("cargo-clippy", ["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"], 3600),
            ("cargo-build", ["build", "--workspace"], 3600),
        ]
        for label, args, timeout in steps:
            if not self.cargo_step(label, args, timeout=timeout):
                return False
        self.failed_stage = "vault-mirror"
        try:
            source_before = source_authority.snapshot(self.ctx.root)
            mirrored = vault_storage.mirror_project(
                self.ctx.root,
                label="full-green-candidate",
                source_fingerprint=str(source_before.get("fingerprint") or ""),
                source_path_count=int(source_before.get("pathCount") or 0),
            )
            source_after = source_authority.snapshot(self.ctx.root)
            if source_after.get("fingerprint") != source_before.get("fingerprint"):
                raise RuntimeError(
                    "Governed source changed while the Vault mirror was being captured; "
                    "snapshot remains un-certified and GREEN was not issued"
                )
            verification = vault_storage.verify_latest_mirror(self.ctx.root, deep_hash=True)
            if verification.get("status") != "PASS":
                raise RuntimeError(
                    "Vault mirror deep verification failed: "
                    f"missing={len(verification.get('missing') or [])} "
                    f"corrupt={len(verification.get('corrupt') or [])}"
                )
            if str(verification.get("snapshotId") or "") != str(mirrored.get("snapshotId") or ""):
                raise RuntimeError("Vault latest snapshot changed before certification")
            stats = mirrored.get("stats") or {}
            self.log.emit(
                "PASS",
                f"Vault mirror captured and deep-verified {stats.get('files', 0)} governed project file(s); "
                f"{stats.get('newObjects', 0)} new object(s), {stats.get('reusedObjects', 0)} reused",
                phase="gate:vault-mirror",
            )
        except Exception as exc:
            self.log.emit("FAIL", f"Vault mirror failed; GREEN not issued: {exc}", phase="gate:vault-mirror")
            return False
        mark = self.git.action("mark-green", stream=True, timeout=120)
        if not mark.ok:
            self.failed_stage = "mark-green"
            self.log.emit("FAIL", "FULL build/test passed but GREEN source marker failed", phase="gate:full")
            return False
        try:
            certification = vault_storage.certify_snapshot(
                self.ctx.root,
                str(mirrored.get("snapshotId") or ""),
                gate="full",
                source_marker="GREEN",
            )
            self.log.emit(
                "PASS",
                f"Certified Vault recovery snapshot {certification.get('snapshotId')}",
                phase="gate:vault-certification",
            )
        except Exception as exc:
            self.failed_stage = "vault-certification"
            self.log.emit("FAIL", f"GREEN source marker exists but Vault certification record failed: {exc}", phase="gate:full")
            return False
        self.failed_stage = ""
        self.log.emit("PASS", "FULL QUALITY GATE GREEN / SOURCE CERTIFIED", phase="gate:full")
        return True


class EvidenceBuilder:
    def __init__(self, ctx: ProjectContext, log: SessionLog, git: GitAuthority, patch: PatchAuthority) -> None:
        self.ctx = ctx
        self.log = log
        self.git = git
        self.patch = patch

    @staticmethod
    def _write_json(path: Path, value: Any) -> None:
        maintenance.atomic_write_json(path, value)

    def create(self, *, reason: str, failed_stage: str = "", exit_code: int = 0,
               open_after: bool = False) -> Path:
        self.ctx.debug_dir.mkdir(parents=True, exist_ok=True)
        stamp = local_stamp()
        safe_reason = re.sub(r"[^A-Za-z0-9._-]+", "_", reason.strip())[:80] or "MANUAL"
        work = Path(tempfile.mkdtemp(prefix="cortex-pcc-evidence-"))
        try:
            meta = {
                "schema": "cortex.pcc_debug_bundle.v3",
                "pccVersion": PCC_VERSION,
                "createdUtc": now_utc(),
                "reason": reason,
                "failedStage": failed_stage or None,
                "exitCode": exit_code,
                "projectRoot": str(self.ctx.root),
                "sessionId": self.log.session_id,
                "platform": platform.platform(),
                "python": sys.version,
                "pythonExecutable": sys.executable,
            }
            self._write_json(work / "bundle.json", meta)
            self._write_json(work / "git-summary.json", self.git.summary())
            _, patch_summary = self.patch.scan()
            self._write_json(work / "patch-summary.json", patch_summary)
            for rel in [
                "project.control.json",
                "Cargo.toml",
                ".cortex/last-green-quality-gate.json",
                "PROJECT_CONTROL_CENTER.cmd",
                "tools/control/CortexPCC.py",
                "tools/control/CortexGitAuthority.py",
                "tools/control/CortexPatchAuthority.py",
            ]:
                src = self.ctx.root / rel
                if src.is_file():
                    dst = work / "source" / rel
                    dst.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(src, dst)
            for src in [self.log.text_path, self.log.jsonl_path]:
                if src.is_file():
                    dst = work / "logs" / src.name
                    dst.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(src, dst)
            if self.log.command_output_dir.is_dir():
                for src in sorted(self.log.command_output_dir.iterdir()):
                    if src.is_file():
                        dst = work / "command-output" / src.name
                        dst.parent.mkdir(parents=True, exist_ok=True)
                        shutil.copy2(src, dst)
            receipts = sorted(self.ctx.patch_receipts.glob("*.json"), key=lambda p: p.stat().st_mtime, reverse=True)[:20] if self.ctx.patch_receipts.is_dir() else []
            for src in receipts:
                dst = work / "patch-receipts" / src.name
                dst.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(src, dst)
            versions: dict[str, str | None] = {}
            for name, argv in {
                "python": [sys.executable, "--version"],
                "git": ["git", "--version"],
                "cargo": ["cargo", "--version"],
                "rustc": ["rustc", "--version"],
            }.items():
                if shutil.which(argv[0]) or argv[0] == sys.executable:
                    try:
                        cp = subprocess.run(argv, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace", timeout=20, check=False)
                        versions[name] = cp.stdout.strip() or None
                    except Exception as exc:
                        versions[name] = f"ERROR: {exc}"
                else:
                    versions[name] = None
            self._write_json(work / "tool-versions.json", versions)
            if self.log.logs_dir.is_dir():
                recent_logs = sorted(self.log.logs_dir.glob("cortex-pcc-*.log"), key=lambda p: p.stat().st_mtime, reverse=True)[:10]
                for src in recent_logs:
                    dst = work / "recent-session-logs" / src.name
                    dst.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(src, dst)
            hygiene = maintenance.scan_root_hygiene(self.ctx.root)
            self._write_json(work / "root-hygiene.json", hygiene)
            try:
                self._write_json(work / "vault-storage.json", {
                    "dependencies": vault_storage.dependency_status(self.ctx.root),
                    "mirror": vault_storage.mirror_status(self.ctx.root),
                    "health": vault_storage.storage_health(self.ctx.root),
                    "retention": vault_storage.snapshot_retention_plan(self.ctx.root),
                    "gc": vault_storage.cas_gc_plan(self.ctx.root),
                })
            except Exception as exc:
                self._write_json(work / "vault-storage.json", {"status": "ERROR", "error": str(exc)})
            manifest = maintenance.debug_manifest_for_tree(work)
            self._write_json(work / "MANIFEST.json", manifest)
            out = self.ctx.debug_dir / f"Cortex_DebugBundle_{stamp}_{safe_reason}.zip"
            if out.exists():
                out = self.ctx.debug_dir / f"Cortex_DebugBundle_{stamp}_{safe_reason}_{uuid.uuid4().hex[:6]}.zip"
            with zipfile.ZipFile(out, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as zf:
                for path in sorted(work.rglob("*")):
                    if path.is_file():
                        zf.write(path, path.relative_to(work).as_posix())
            verification = maintenance.verify_debug_bundle(out)
            sidecar = maintenance.write_debug_sidecar(out)
            maintenance.write_latest_debug_pointer(
                self.ctx.debug_dir, out, reason=reason, exit_code=exit_code,
                failed_stage=failed_stage, verification=verification,
            )
            self.log.emit(
                "PASS",
                f"Debug bundle verified: {out} (sha256={verification['sha256'][:12]}..., sidecar={sidecar.name})",
                phase="evidence",
            )
            if open_after:
                open_folder(self.ctx.debug_dir)
            return out
        finally:
            shutil.rmtree(work, ignore_errors=True)


class CortexPCC:
    def __init__(self, root: Path, *, remote: str = DEFAULT_REMOTE, quiet: bool = False) -> None:
        self.ctx = ProjectContext.create(root, remote)
        self.log = SessionLog(root, quiet=quiet)
        vault_storage.ensure_layout(root)
        shared_env = vault_storage.dependency_environment(root)
        if os.environ.get("CORTEX_SHARED_DEPENDENCIES", "1").strip().casefold() in {"0", "false", "off", "no"}:
            shared_env = {}
        self.runner = CommandRunner(self.log, base_env=shared_env)
        self.git = GitAuthority(self.ctx, self.runner, self.log)
        self.patch = PatchAuthority(self.ctx, self.runner, self.log)
        self.gates = GateEngine(self.ctx, self.runner, self.log, self.git, self.patch)
        self.evidence = EvidenceBuilder(self.ctx, self.log, self.git, self.patch)
        self._install_signal_handlers()

    def _install_signal_handlers(self) -> None:
        def handle(_sig: int, _frame: Any) -> None:
            self.log.emit("WARN", "Cancellation requested; terminating active child process.")
            self.runner.cancel()
        try:
            signal.signal(signal.SIGINT, handle)
        except Exception:
            pass

    def patch_status(self, *, verbose: bool = True) -> tuple[int, dict[str, Any]]:
        code, summary = self.patch.scan()
        if verbose:
            print(f"Pending valid : {summary.get('Pending', 0)}")
            print(f"Invalid       : {summary.get('Invalid', 0)}")
            print(f"Recovered SHA : {summary.get('RecoveredSidecars', 0)}")
            print(f"Ignored ZIPs  : {summary.get('Ignored', 0)}")
            for item in summary.get("ValidPatches", []) or []:
                print(f"  VALID   {item.get('patchId')}  {Path(str(item.get('path'))).name}")
            for item in summary.get("InvalidPatches", []) or []:
                print(f"  INVALID {item.get('name')}: {item.get('error')}")
        return code, summary

    def patch_apply(self, *, confirm: bool = True) -> int:
        code, summary = self.patch_status(verbose=True)
        if int(summary.get("Invalid", 0) or 0) > 0:
            self.log.emit("FAIL", "Patch queue is blocked by invalid recognized patch(es).")
            return 2
        pending = int(summary.get("Pending", 0) or 0)
        if pending == 0:
            self.log.emit("PASS", "No pending validated patch queue.")
            return 0
        if confirm and sys.stdin.isatty():
            answer = input(f"Apply {pending} validated patch(es) now? [y/N] ").strip().lower()
            if answer not in {"y", "yes"}:
                self.log.emit("INFO", "Patch apply cancelled by operator.")
                return 0
        rc, result = self.patch.apply(stream=True)
        if rc != 0 or int(result.get("Invalid", 0) or 0) > 0:
            self.log.emit("FAIL", "Patch intake failed.")
            return rc or 1
        if result.get("RestartRequired"):
            self.log.emit("WARN", "PCC authority changed. Relaunching Project Control Center.")
            self.restart()
            raise PCCRestart("PCC authority changed and a replacement controller was launched.")
        return 0

    def restart(self) -> None:
        # Operator surfaces are now authoritative clients of this core.  A PCC self-update
        # must relaunch through the root bootstrap so the default GUI/console policy is
        # preserved instead of dropping the operator into a raw CortexPCC.py console.
        launcher = self.ctx.root / "PROJECT_CONTROL_CENTER.cmd"
        if is_windows() and launcher.is_file():
            subprocess.Popen(
                ["cmd.exe", "/c", str(launcher)],
                cwd=str(self.ctx.root),
                creationflags=getattr(subprocess, "CREATE_NEW_CONSOLE", 0),
            )
            return
        gui = self.ctx.tools / "CortexPCCGui.py"
        if gui.is_file():
            subprocess.Popen([*which_python(), str(gui), "--root", str(self.ctx.root)], cwd=str(self.ctx.root))
            return
        argv = [*which_python(), str(self.ctx.tools / "CortexPCC.py"), "--root", str(self.ctx.root)]
        subprocess.Popen(argv, cwd=str(self.ctx.root))

    def status(self, *, as_json: bool = False) -> int:
        git = self.git.summary()
        _, patch = self.patch.scan()
        target = self.cargo_target_dir()
        cli = self.binary_path("cortex", target)
        gui = self.binary_path("cortex_desktop", target)
        hygiene = maintenance.scan_root_hygiene(self.ctx.root)
        status = {
            "schema": "cortex.pcc_status.v2",
            "pccVersion": PCC_VERSION,
            "projectRoot": str(self.ctx.root),
            "git": git,
            "patches": {
                "pending": patch.get("Pending", 0),
                "invalid": patch.get("Invalid", 0),
                "ignored": patch.get("Ignored", 0),
            },
            "hygiene": hygiene,
            "artifacts": {
                "root": str(self.ctx.artifacts),
                "debug": str(self.ctx.debug_dir),
                "logs": str(self.log.logs_dir),
                "latestDebugText": str(self.ctx.debug_dir / "LATEST_DEBUG_BUNDLE.txt"),
                "latestDebugJson": str(self.ctx.debug_dir / "LATEST_DEBUG_BUNDLE.json"),
            },
            "tools": {
                "python": sys.executable,
                "cargo": shutil.which("cargo"),
                "rustc": shutil.which("rustc"),
                "git": shutil.which("git"),
                "powershell": shutil.which("pwsh") or shutil.which("powershell") or shutil.which("powershell.exe"),
            },
            "cargoTarget": str(target) if target else None,
            "storage": {
                "dependencies": vault_storage.dependency_status(self.ctx.root),
                "mirror": vault_storage.mirror_status(self.ctx.root),
            },
            "binaries": {"cli": str(cli) if cli and cli.is_file() else None, "gui": str(gui) if gui and gui.is_file() else None},
            "session": {"id": self.log.session_id, "log": str(self.log.text_path), "jsonl": str(self.log.jsonl_path)},
        }
        if as_json:
            print(json.dumps(status, separators=(",", ":"), sort_keys=True))
            return 0
        self.print_banner(status)
        return 0

    def cargo_target_dir(self) -> Path | None:
        if not shutil.which("cargo") or not self.ctx.cargo_toml.is_file():
            return self.ctx.root / "target"
        result = self.runner.run(["cargo", "metadata", "--no-deps", "--format-version", "1", "--quiet"],
                                 cwd=self.ctx.root, timeout=60, stream=False, phase="status:cargo-metadata")
        if result.ok:
            try:
                value = json.loads(result.stdout).get("target_directory")
                if value:
                    return Path(value)
            except Exception:
                pass
        return self.ctx.root / "target"

    @staticmethod
    def binary_path(name: str, target: Path | None) -> Path | None:
        if not target:
            return None
        exe = name + (".exe" if is_windows() else "")
        for profile in ("debug", "release"):
            p = target / profile / exe
            if p.is_file():
                return p
        return target / "debug" / exe

    def print_banner(self, status: dict[str, Any] | None = None) -> None:
        if status is None:
            # Avoid recursively printing JSON; build a lightweight status snapshot.
            git = self.git.summary()
            _, patch = self.patch.scan()
            status = {"git": git, "patches": {"pending": patch.get("Pending", 0), "invalid": patch.get("Invalid", 0)}, "hygiene": maintenance.scan_root_hygiene(self.ctx.root)}
        git = status.get("git", {})
        patches = status.get("patches", {})
        hygiene = status.get("hygiene", {})
        print("=" * 72)
        print(" CORTEX PROJECT CONTROL CENTER")
        print("=" * 72)
        print(f" Controller : {PCC_VERSION}")
        print(f" Repository : {self.ctx.root}")
        print(f" Git        : {'Ready' if git.get('gitReady') else 'Not ready'} / {'Clean' if git.get('clean') else 'Modified'}")
        print(f" Branch     : {git.get('branch') or '<none>'} @ {git.get('headShort') or '<unborn>'}")
        ahead, behind = git.get("ahead"), git.get("behind")
        sync = "unknown" if ahead is None or behind is None else ("synced" if ahead == 0 and behind == 0 else f"{ahead} ahead / {behind} behind")
        print(f" Sync       : {sync}")
        print(f" FULL GREEN : {'MATCH' if git.get('greenMatch') else 'STALE / NONE'}")
        print(f" Updates    : {patches.get('pending', 0)} pending / {patches.get('invalid', 0)} invalid")
        print(f" Hygiene    : {'Clean' if hygiene.get('clean', True) else str(hygiene.get('violationCount', 0)) + ' root residue'}")
        print(f" Active log : {self.log.text_path}")
        print("-" * 72)

    def root_hygiene(self, *, repair: bool = False, as_json: bool = False) -> int:
        if repair:
            result = maintenance.repair_root_hygiene(self.ctx.root)
            moved = len(result.get("moved", []))
            self.log.emit("PASS", f"Root hygiene repair moved {moved} generated operational item(s) into artifacts/.")
            payload = result
        else:
            payload = maintenance.scan_root_hygiene(self.ctx.root)
        if as_json:
            print(json.dumps(payload, separators=(",", ":"), sort_keys=True))
        else:
            print(f"Root violations : {payload.get('violationCount', payload.get('after', {}).get('violationCount', 0))}")
            if "advisoryCount" in payload:
                print(f"Legacy advisories: {payload.get('advisoryCount', 0)}")
            for item in payload.get("items", []) or []:
                print(f"  {item.get('severity','?').upper():9} {item.get('path')} [{item.get('kind')}]")
            if payload.get("moved"):
                for item in payload["moved"]:
                    print(f"  MOVED {item['from']} -> {item['to']}")
        remaining = maintenance.scan_root_hygiene(self.ctx.root)
        return 0 if remaining.get("clean") else 2

    def artifact_status(self, *, as_json: bool = False) -> int:
        payload = maintenance.doctor(self.ctx.root)
        if as_json:
            print(json.dumps(payload, separators=(",", ":"), sort_keys=True))
        else:
            print(json.dumps(payload, indent=2, sort_keys=True))
        return 0 if payload.get("healthy") else 2

    def create_source_rollup(self, *, open_after: bool = False) -> int:
        """Create and verify a recoverable source snapshot without mutating source/Git."""
        archive: Path | None = None
        try:
            archive = source_rollup.create(self.ctx.root)
            verification = source_rollup.verify(archive)
            actual_sha = source_rollup.sha_file(archive)
            sidecar = archive.with_name(archive.name + ".sha256")
            expected_sidecar = f"{actual_sha}  {archive.name}\n"
            if not sidecar.is_file() or sidecar.read_text(encoding="ascii") != expected_sidecar:
                raise source_rollup.RollupError("source rollup checksum sidecar mismatch")
            if verification.get("sha256") != actual_sha:
                raise source_rollup.RollupError("source rollup verification digest mismatch")
            print("SOURCE_ROLLUP_CREATED=" + str(archive))
            print("SOURCE_ROLLUP_SHA256=" + actual_sha)
            print("SOURCE_ROLLUP_VERIFIED=" + json.dumps(verification, sort_keys=True))
            self.log.emit("PASS", f"Source rollup created and independently verified: {archive}", phase="recovery:source-rollup")
            if open_after:
                open_folder(archive.parent)
            return 0
        except (source_rollup.RollupError, OSError, ValueError, zipfile.BadZipFile) as exc:
            # The exporter already verifies before publication. If the second check fails,
            # quarantine only this newly created archive so it cannot be shared as verified.
            if archive is not None:
                try:
                    archive.unlink(missing_ok=True)
                    archive.with_name(archive.name + ".sha256").unlink(missing_ok=True)
                except OSError:
                    pass
            self.log.emit("FAIL", f"Source rollup creation/verification failed: {exc}", phase="recovery:source-rollup")
            print("SOURCE_ROLLUP_FAILED=" + str(exc), file=sys.stderr)
            return 1

    def verify_latest_debug(self) -> int:
        latest = self.ctx.debug_dir / "LATEST_DEBUG_BUNDLE.json"
        if not latest.is_file():
            self.log.emit("FAIL", f"Latest debug authority missing: {latest}")
            return 1
        try:
            data = json.loads(latest.read_text(encoding="utf-8"))
            path = Path(str(data.get("path", "")))
            result = maintenance.verify_debug_bundle(path)
            expected = str(data.get("sha256", "")).lower()
            if expected and expected != result["sha256"]:
                raise maintenance.MaintenanceError("LATEST_DEBUG_BUNDLE.json SHA-256 does not match bundle")
            self.log.emit("PASS", f"Latest debug bundle verified: {path.name}")
            return 0
        except Exception as exc:
            self.log.emit("FAIL", f"Latest debug verification failed: {exc}")
            return 1

    def prune_artifacts(self, *, apply: bool = False, keep_debug: int = 30, keep_log_files: int = 200) -> int:
        result = maintenance.prune_artifacts(
            self.ctx.root, keep_debug=keep_debug, keep_log_files=keep_log_files, apply=apply
        )
        print(f"Candidates : {result['deleteCount']}")
        print(f"Reclaim    : {result['reclaimBytes']} bytes")
        print(f"Applied    : {result['applied']}")
        if not apply and result["deleteCount"]:
            print("Dry-run only. Use artifact-prune-apply for explicit deletion.")
        return 0

    def gate(self, kind: str, *, evidence: bool = True) -> int:
        kind_l = kind.lower()
        try:
            with maintenance.OperationLock(self.ctx.root, f"gate:{kind_l}"):
                if kind_l in {"fast", "full"}:
                    hygiene = maintenance.scan_root_hygiene(self.ctx.root)
                    if hygiene.get("violationCount") or hygiene.get("advisoryCount"):
                        repaired = maintenance.repair_root_hygiene(self.ctx.root)
                        self.log.emit(
                            "PASS",
                            f"Operational hygiene normalized {len(repaired.get('moved', []))} item(s) before {kind_l} gate.",
                            phase="maintenance",
                        )
                if kind_l == "quick":
                    ok, _ = self.gates.quick()
                elif kind_l == "fast":
                    ok = self.gates.fast()
                elif kind_l == "full":
                    ok = self.gates.full()
                else:
                    raise PCCError(f"Unknown gate: {kind}")
        except maintenance.MaintenanceError as exc:
            self.gates.failed_stage = "pcc-operation-lock"
            self.log.emit("FAIL", str(exc), phase="gate")
            ok = False
        if evidence:
            self.evidence.create(
                reason=f"{kind_l.upper()}_{'GREEN' if ok else 'FAIL'}",
                failed_stage=self.gates.failed_stage,
                exit_code=0 if ok else 1,
                open_after=not ok,
            )
        return 0 if ok else 1

    def storage_status(self, *, as_json: bool = False) -> int:
        payload = {
            "schema": "cortex.pcc_storage_status.v1",
            "dependencies": vault_storage.dependency_status(self.ctx.root),
            "mirror": vault_storage.mirror_status(self.ctx.root),
        }
        if as_json:
            print(json.dumps(payload, separators=(",", ":"), sort_keys=True))
            return 0
        deps = payload["dependencies"]
        mirror = payload["mirror"]
        print("VAULT / SHARED STORAGE")
        print(f" Vault root     : {deps.get('vaultRoot')}")
        print(f" Project key    : {deps.get('projectKey')}")
        print(f" Shared enabled : {deps.get('enabled')}")
        print(f" Cargo home     : {(deps.get('paths') or {}).get('cargo_home')}")
        print(f" Cargo target   : {(deps.get('paths') or {}).get('cargo_target_dir')}")
        print(f" sccache        : {deps.get('sccache') or 'not installed (optional)'}")
        if mirror.get("hasSnapshot"):
            latest = mirror.get("latest") or {}
            stats = latest.get("stats") or {}
            print(f" Latest mirror  : {latest.get('snapshotId')}")
            print(f" Mirrored files : {stats.get('files', 0)}")
            print(f" New objects    : {stats.get('newObjects', 0)}")
        else:
            print(" Latest mirror  : none yet")
        return 0

    def storage_reclaim_plan(self, *, all_registered: bool = False) -> int:
        roots: list[Path] = [self.ctx.root.resolve()]
        if all_registered:
            try:
                from PCCSurfaceCommon import ProjectRegistry
                for entry in ProjectRegistry().entries():
                    candidate = Path(entry.root).expanduser().resolve()
                    if candidate.is_dir() and os.path.normcase(str(candidate)) not in {os.path.normcase(str(x)) for x in roots}:
                        roots.append(candidate)
            except Exception as exc:
                self.log.emit("WARN", f"Registered-project inventory unavailable for reclaim plan: {exc}", phase="storage:reclaim-plan")
        plans = [vault_storage.reclaim_plan(root) for root in roots]
        total = sum(int(plan.get("reclaimBytes") or 0) for plan in plans)
        print("SHARED STORAGE RECLAIM PLAN (DRY-RUN ONLY)")
        print(f" Projects    : {len(plans)}")
        print(f" Candidates  : {sum(int(plan.get('candidateCount') or 0) for plan in plans)}")
        print(f" Reclaimable : {total} bytes")
        for plan in plans:
            for item in plan.get("candidates") or []:
                print(f" - {item.get('path')} :: {item.get('bytes')} bytes :: {item.get('reason')}")
        print("No files were moved or deleted. Cleanup remains a separate governed action after Windows certification.")
        return 0

    def storage_health(self, *, as_json: bool = False) -> int:
        try:
            payload = vault_storage.storage_health(self.ctx.root)
        except Exception as exc:
            self.log.emit("FAIL", f"Storage health inspection failed: {exc}", phase="storage:health")
            return 1
        if as_json:
            print(json.dumps(payload, indent=2, sort_keys=True))
            return 0 if payload.get("status") in {"PASS", "WARN"} else 1
        print("VAULT STORAGE HEALTH")
        print(f" Status       : {payload.get('status')}")
        print(f" Vault        : {payload.get('vaultRoot')}")
        print(f" Projects     : {payload.get('projects', 0)}")
        print(f" Snapshots    : {payload.get('snapshots', 0)}")
        print(f" CAS objects  : {(payload.get('cas') or {}).get('files', 0)}")
        print(f" CAS bytes    : {(payload.get('cas') or {}).get('bytes', 0)}")
        print(f" GC eligible  : {payload.get('gcGarbageObjects', 0)} object(s) / {payload.get('gcGarbageBytes', 0)} bytes")
        print(f" Local reclaim: {payload.get('currentProjectReclaimBytes', 0)} bytes")
        for warning in payload.get("warnings") or []:
            print(f" WARN         : {warning}")
        return 0

    def vault_retention(self, *, apply: bool = False) -> int:
        try:
            payload = vault_storage.apply_snapshot_retention(self.ctx.root) if apply else vault_storage.snapshot_retention_plan(self.ctx.root)
        except Exception as exc:
            self.log.emit("FAIL", f"Vault snapshot retention failed: {exc}", phase="vault:retention")
            return 1
        print(json.dumps(payload, indent=2, sort_keys=True))
        level = "PASS" if apply else "INFO"
        self.log.emit(level, f"Vault snapshot retention {'applied' if apply else 'planned'}: prune={payload.get('pruneCount', 0)}", phase="vault:retention")
        return 0

    def vault_gc(self, *, action: str = "plan") -> int:
        try:
            if action == "plan":
                payload = vault_storage.cas_gc_plan(self.ctx.root)
            elif action == "stage":
                payload = vault_storage.stage_cas_gc(self.ctx.root)
            elif action == "restore":
                payload = vault_storage.restore_cas_gc(self.ctx.root)
            elif action == "purge":
                payload = vault_storage.purge_cas_gc(self.ctx.root)
            else:
                raise ValueError(action)
        except Exception as exc:
            self.log.emit("FAIL", f"Vault CAS GC {action} failed: {exc}", phase=f"vault:gc:{action}")
            return 1
        print(json.dumps(payload, indent=2, sort_keys=True))
        self.log.emit("PASS" if action != "plan" else "INFO", f"Vault CAS GC {action} complete", phase=f"vault:gc:{action}")
        return 0

    def storage_prepare(self) -> int:
        try:
            paths = vault_storage.ensure_layout(self.ctx.root)
        except Exception as exc:
            self.log.emit("FAIL", f"Shared dependency/Vault storage prepare failed: {exc}", phase="storage:prepare")
            return 1
        self.log.emit("PASS", f"Shared dependency/Vault storage ready: {paths.get('vault_root')}", phase="storage:prepare")
        for key, value in sorted(paths.items()):
            print(f" {key:20} {value}")
        print("Legacy dependency caches are NOT deleted automatically. Existing caches may be retired only after explicit migration/verification.")
        return 0

    def vault_mirror(self, *, label: str = "working") -> int:
        try:
            payload = vault_storage.mirror_project(self.ctx.root, label=label)
        except Exception as exc:
            self.log.emit("FAIL", f"Project Vault mirror failed: {exc}", phase="vault:mirror")
            return 1
        stats = payload.get("stats") or {}
        self.log.emit(
            "PASS",
            f"Project mirrored to Vault: {stats.get('files', 0)} files; "
            f"{stats.get('newObjects', 0)} new object(s), {stats.get('reusedObjects', 0)} reused",
            phase="vault:mirror",
        )
        print(f"Snapshot: {payload.get('snapshotId')}")
        print(f"Vault    : {payload.get('vaultRoot')}")
        return 0

    def vault_mirror_all_registered(self) -> int:
        try:
            from PCCSurfaceCommon import ProjectRegistry
            entries = ProjectRegistry().entries()
        except Exception as exc:
            self.log.emit("FAIL", f"Registered-project inventory unavailable: {exc}", phase="vault:mirror-all")
            return 1
        roots: list[Path] = []
        seen: set[str] = set()
        for entry in entries:
            root = Path(entry.root).expanduser().resolve()
            key = os.path.normcase(str(root))
            if root.is_dir() and key not in seen:
                seen.add(key)
                roots.append(root)
        current_key = os.path.normcase(str(self.ctx.root.resolve()))
        if current_key not in seen:
            roots.insert(0, self.ctx.root.resolve())
        failures: list[str] = []
        for index, root in enumerate(roots, start=1):
            self.log.emit("INFO", f"Mirroring registered project {index}/{len(roots)}: {root}", phase="vault:mirror-all")
            try:
                payload = vault_storage.mirror_project(root, label="registered-project-sweep")
                stats = payload.get("stats") or {}
                self.log.emit("PASS", f"Mirrored {root.name}: {stats.get('files', 0)} file(s)", phase="vault:mirror-all")
            except Exception as exc:
                failures.append(f"{root}: {exc}")
                self.log.emit("FAIL", f"Mirror failed for {root}: {exc}", phase="vault:mirror-all")
        print(f"Registered projects: {len(roots)}")
        print(f"Mirror failures    : {len(failures)}")
        for failure in failures:
            print(f" - {failure}")
        return 1 if failures else 0

    def vault_verify(self, *, deep: bool = False) -> int:
        result = vault_storage.verify_latest_mirror(self.ctx.root, deep_hash=deep)
        print(json.dumps(result, indent=2, sort_keys=True))
        if result.get("status") == "PASS":
            self.log.emit("PASS", f"Latest Vault mirror verified ({result.get('checkedObjects', 0)} objects)", phase="vault:verify")
            return 0
        self.log.emit("FAIL", f"Vault mirror verification failed: missing={len(result.get('missing') or [])}, corrupt={len(result.get('corrupt') or [])}", phase="vault:verify")
        return 1

    def format_source(self) -> int:
        """Apply canonical rustfmt formatting as an explicit, user-invoked source mutation."""
        if not shutil.which("cargo"):
            self.log.emit("FAIL", "Cannot format source: cargo not found on PATH", phase="cargo:format-apply")
            return 1
        apply_result = self.runner.run(
            ["cargo", "fmt", "--all"],
            cwd=self.ctx.root,
            timeout=300,
            stream=True,
            phase="cargo:format-apply",
        )
        if not apply_result.ok:
            self.log.emit("FAIL", "Canonical Rust formatting apply failed", phase="cargo:format-apply")
            return 1
        check_result = self.runner.run(
            ["cargo", "fmt", "--all", "--", "--check"],
            cwd=self.ctx.root,
            timeout=300,
            stream=True,
            phase="cargo:format-verify",
        )
        if not check_result.ok:
            self.log.emit("FAIL", "Rust formatting verification still fails after apply", phase="cargo:format-verify")
            return 1
        self.log.emit("PASS", "Canonical Rust formatting applied and verified", phase="cargo:format-apply")
        return 0

    def build(self, *, release: bool = False, package: str | None = None) -> int:
        args = ["build"]
        if package:
            args += ["-p", package]
        else:
            args += ["--workspace"]
        if release:
            args.append("--release")
        try:
            with maintenance.OperationLock(self.ctx.root, "build"):
                result = self.runner.run(["cargo", *args], cwd=self.ctx.root, timeout=3600, stream=True, phase="build")
        except maintenance.MaintenanceError as exc:
            self.log.emit("FAIL", str(exc), phase="build")
            return 1
        if not result.ok:
            self.evidence.create(reason="BUILD_FAIL", failed_stage="cargo-build", exit_code=result.returncode, open_after=True)
            return 1
        return 0

    def launch_gui(self) -> int:
        # Fail before any build or process launch when the typed Rust consumer
        # cannot deserialize the manifest. Never rewrite rollback metadata.
        from UniversalPCCAudit import inspect_project
        inspection = inspect_project(self.ctx.root, rust_consumer=True)
        if inspection["status"] == "INVALID":
            problems = "; ".join(inspection.get("errors", [])[:6])
            self.log.emit("FAIL", f"Cortex Desktop launch blocked by typed project contract: {problems}. "
                          "Inspect: python tools/control/CortexPCC.py contract-migrate --root . ; "
                          "explicitly repair only if approved: contract-migrate --root . --yes")
            return 2
        target = self.cargo_target_dir()
        gui = self.binary_path("cortex_desktop", target)
        if not gui or not gui.is_file():
            self.log.emit("INFO", "Cortex Desktop is not built; building cortex_desktop.")
            if self.build(package="cortex_desktop") != 0:
                return 1
            target = self.cargo_target_dir()
            gui = self.binary_path("cortex_desktop", target)
        if not gui or not gui.is_file():
            self.log.emit("FAIL", "Cortex Desktop executable not found after build.")
            return 1
        # Process survival is a preliminary startup observation, not UI-ready.
        runtime_logs = self.ctx.root / "artifacts" / "logs" / "runtime"
        runtime_logs.mkdir(parents=True, exist_ok=True)
        runtime_log = runtime_logs / f"cortex-desktop-{local_stamp()}-{uuid.uuid4().hex[:8]}.log"
        try:
            with runtime_log.open("wb") as output:
                proc = subprocess.Popen([str(gui), str(self.ctx.root)], cwd=str(self.ctx.root),
                                        stdout=output, stderr=subprocess.STDOUT,
                                        stdin=subprocess.DEVNULL)
            observation = desktop_readiness.wait_for_visible_window(
                proc.pid,
                proc.poll,
                timeout=15.0,
            )
            if observation.status != "WINDOW_VISIBLE":
                self.log.emit(
                    "FAIL",
                    f"Cortex Desktop startup not certified ({observation.status}): {observation.detail}; "
                    f"runtime log: {runtime_log}",
                )
                return 2
        except OSError as exc:
            self.log.emit("FAIL", f"Cortex Desktop process could not be started: {exc}; runtime log: {runtime_log}")
            return 2
        self.log.emit(
            "PASS",
            f"Cortex Desktop visible native window observed for PID {proc.pid}; "
            f"provider/chat readiness remains separate; runtime log: {runtime_log}",
        )
        return 0

    def startup(self) -> None:
        self.print_banner()
        code, patch = self.patch_status(verbose=False)
        recovered = int(patch.get("RecoveredSidecars", 0) or 0)
        if recovered:
            for item in patch.get("RecoveredSidecarDetails", []) or []:
                self.log.emit("WARN", f"Recovered missing patch SHA-256 sidecar after full internal ZIP validation: {item.get('name')}")
        if int(patch.get("Invalid", 0) or 0):
            self.log.emit("FAIL", f"Startup found {patch.get('Invalid')} invalid recognized patch(es); source unchanged.")
            for item in patch.get("InvalidPatches", []) or []:
                self.log.emit("FAIL", f"Invalid patch: {item.get('name')} - {item.get('error')}")
        elif int(patch.get("Pending", 0) or 0):
            self.log.emit("WARN", f"Startup found {patch.get('Pending')} validated pending patch(es); apply explicitly from Source / Project Control.")
        else:
            self.log.emit("PASS", "Startup patch scan: no pending source updates.")
        hygiene = maintenance.scan_root_hygiene(self.ctx.root)
        if hygiene.get("violationCount") or hygiene.get("advisoryCount"):
            self.log.emit(
                "WARN",
                f"Startup hygiene scan found {hygiene.get('violationCount', 0)} root residue / {hygiene.get('advisoryCount', 0)} legacy log item(s); source unchanged. Fast/Full or Diagnostics repair will normalize them.",
            )
        else:
            self.log.emit("PASS", "Startup hygiene scan: operational artifacts contained under artifacts/.")
        rc = self.gate("quick", evidence=False)
        self.evidence.create(reason="STARTUP_GREEN" if rc == 0 else "STARTUP_FAIL",
                             failed_stage=self.gates.failed_stage, exit_code=rc, open_after=rc != 0)

    def source_menu(self) -> None:
        while True:
            self.print_banner()
            print("SOURCE / PROJECT CONTROL")
            print("  1 Status / GREEN eligibility")
            print("  2 Review working changes")
            print("  3 Recent source history")
            print("  4 Open Cortex GitHub")
            print(" 10 Scan / inspect pending patches")
            print(" 11 Apply validated pending patch queue")
            print(" 12 Open applied patch history")
            print(" 13 Open failed patch history")
            print(" 14 Open patch receipts")
            print(" 15 Root artifact hygiene scan / repair")
            print(" 16 Vault / shared dependency storage status")
            print(" 17 Prepare shared dependency paths")
            print(" 18 Mirror current project into Vault")
            print(" 27 Space reclaim plan - current project, dry-run")
            print(" 28 Space reclaim plan - all registered projects, dry-run")
            print(" 29 Vault storage health / accounting")
            print(" 34 Snapshot retention plan - dry-run")
            print(" 35 Vault CAS garbage-collection plan - dry-run")
            print(" 19 Apply + verify canonical Rust formatting")
            print(" 26 Mirror all registered projects into Vault")
            print(" 20 Quick gate")
            print(" 21 Fast gate")
            print(" 22 FULL QUALITY GATE / CERTIFY GREEN")
            print(" 23 Create debug / certification bundle")
            print(" 24 Verify latest Vault project mirror")
            print(" 25 Deep-hash verify latest Vault project mirror")
            print(" 30 Commit current source only if FULL GREEN matches")
            print(" 31 Commit + push only if FULL GREEN matches")
            print(" 32 Push committed main to origin")
            print(" 33 Verify local / origin / GREEN authority")
            print(" 40 Fetch origin/main")
            print(" 41 Compare local vs origin/main")
            print(" 42 Pull origin/main - clean + fast-forward only")
            print(" 43 Initialize / connect / repair against origin/main")
            print(" 60 Manual governed-source commit - not GREEN protected")
            print(" 70 Create + verify source recovery archive")
            print("  0 Back")
            choice = input("Select: ").strip()
            if choice == "0":
                return
            if choice == "1": self.git.action("status")
            elif choice == "2": self.git.action("review")
            elif choice == "3": self.git.action("history")
            elif choice == "4": open_url(self.ctx.remote)
            elif choice == "10": self.patch_status(verbose=True)
            elif choice == "11": self.patch_apply(confirm=True)
            elif choice == "12": open_folder(self.ctx.patch_applied)
            elif choice == "13": open_folder(self.ctx.patch_failed)
            elif choice == "14": open_folder(self.ctx.patch_receipts)
            elif choice == "15": self.root_hygiene(repair=True)
            elif choice == "16": self.storage_status()
            elif choice == "17": self.storage_prepare()
            elif choice == "18": self.vault_mirror(label="manual-working")
            elif choice == "19": self.format_source()
            elif choice == "20": self.gate("quick")
            elif choice == "21": self.gate("fast")
            elif choice == "22": self.gate("full")
            elif choice == "23": self.evidence.create(reason="MANUAL_CERTIFICATION", open_after=True)
            elif choice == "24": self.vault_verify(deep=False)
            elif choice == "25": self.vault_verify(deep=True)
            elif choice == "26": self.vault_mirror_all_registered()
            elif choice == "27": self.storage_reclaim_plan(all_registered=False)
            elif choice == "28": self.storage_reclaim_plan(all_registered=True)
            elif choice == "29": self.storage_health()
            elif choice == "34": self.vault_retention(apply=False)
            elif choice == "35": self.vault_gc(action="plan")
            elif choice in {"30", "31", "60"}:
                default = f"Cortex GREEN checkpoint - {datetime.now().strftime('%Y-%m-%d %H:%M')}"
                msg = input(f"Commit message [{default}]: ").strip() or default
                action = {"30": "commit-green", "31": "commit-push-green", "60": "manual-commit"}[choice]
                self.git.action(action, message=msg)
            elif choice == "32": self.git.action("push")
            elif choice == "33": self.git.action("verify")
            elif choice == "40": self.git.action("fetch")
            elif choice == "41": self.git.action("compare")
            elif choice == "42": self.git.action("pull")
            elif choice == "43": self.git.action("setup")
            elif choice == "70": self.create_source_rollup(open_after=True)
            else:
                print("Unknown option.")
                continue
            input("Press Enter to continue...")

    def build_menu(self) -> None:
        while True:
            self.print_banner()
            print("BUILD / RUN")
            print(" 1 Build Cortex workspace - debug")
            print(" 2 Build Cortex Desktop package")
            print(" 3 Build Cortex workspace - release")
            print(" 4 Launch Cortex GUI / Project Control")
            print(" 5 Native status")
            print(" 0 Back")
            choice = input("Select: ").strip()
            if choice == "0": return
            if choice == "1": self.build()
            elif choice == "2": self.build(package="cortex_desktop")
            elif choice == "3": self.build(release=True)
            elif choice == "4": self.launch_gui()
            elif choice == "5": self.status()
            else: continue
            input("Press Enter to continue...")

    def diagnostics_menu(self) -> None:
        while True:
            self.print_banner()
            print("DIAGNOSTICS / RECOVERY / EVIDENCE")
            print(" 1 Native project status / health")
            print(" 2 Run Python PCC self-tests")
            print(" 3 Create debug bundle + open artifacts")
            print(" 4 Open debug artifacts")
            print(" 5 Open logs")
            print(" 6 Open all artifacts")
            print(" 7 Open project folder")
            print(" 8 Open patch backups")
            print(" 9 Open failed patches")
            print("10 Open patch receipts")
            print("11 Root artifact hygiene scan")
            print("12 Repair generated root residue")
            print("13 Verify latest debug bundle")
            print("14 Artifact retention dry-run")
            print("15 Apply artifact retention policy")
            print("16 PCC doctor / artifact status")
            print("17 Create + verify source recovery archive")
            print(" 0 Back")
            choice = input("Select: ").strip()
            if choice == "0": return
            if choice == "1": self.status()
            elif choice == "2": run_self_tests(self.ctx.root, self.runner)
            elif choice == "3": self.evidence.create(reason="MANUAL", open_after=True)
            elif choice == "4": open_folder(self.ctx.debug_dir)
            elif choice == "5": open_folder(self.log.logs_dir)
            elif choice == "6": open_folder(self.ctx.artifacts)
            elif choice == "7": open_folder(self.ctx.root)
            elif choice == "8": open_folder(self.ctx.patch_backups)
            elif choice == "9": open_folder(self.ctx.patch_failed)
            elif choice == "10": open_folder(self.ctx.patch_receipts)
            elif choice == "11": self.root_hygiene(repair=False)
            elif choice == "12": self.root_hygiene(repair=True)
            elif choice == "13": self.verify_latest_debug()
            elif choice == "14": self.prune_artifacts(apply=False)
            elif choice == "15":
                answer = input("Delete artifacts beyond retention limits? [y/N] ").strip().lower()
                if answer in {"y", "yes"}: self.prune_artifacts(apply=True)
            elif choice == "16": self.artifact_status()
            elif choice == "17": self.create_source_rollup(open_after=True)
            else: continue
            input("Press Enter to continue...")

    def interactive(self) -> int:
        self.startup()
        while True:
            self.print_banner()
            print("ROOT CONTROL")
            print(" 1 FULL QUALITY GATE / CERTIFY GREEN")
            print(" 2 COMMIT CURRENT CERTIFIED GREEN")
            print(" 3 Launch Cortex GUI / Project Control")
            print(" 4 SOURCE / PROJECT CONTROL")
            print(" 5 BUILD / RUN")
            print(" 6 DIAGNOSTICS / RECOVERY")
            print(" 0 Exit")
            choice = input("Select: ").strip()
            if choice == "0": return 0
            if choice == "1": self.gate("full"); input("Press Enter to continue...")
            elif choice == "2":
                default = f"Cortex GREEN checkpoint - {datetime.now().strftime('%Y-%m-%d %H:%M')}"
                msg = input(f"Commit message [{default}]: ").strip() or default
                self.git.action("commit-green", message=msg); input("Press Enter to continue...")
            elif choice == "3": self.launch_gui()
            elif choice == "4": self.source_menu()
            elif choice == "5": self.build_menu()
            elif choice == "6": self.diagnostics_menu()


def run_universal_python_regressions(root: Path, runner: CommandRunner | None = None) -> int:
    """Require both reusable PCC fixtures and Cortex integration tests.

    This is a Full Gate stage: missing files must FAIL rather than silently skip.
    """
    tests = [
        root / "tools/control/tests/test_cortex_pcc.py",
        root / "tools/control/tests/test_universal_pcc_audit.py",
        root / "tools/control/tests/test_cortex_upcc_a01_integration.py",
        root / "tools/control/tests/test_universal_pcc_plan.py",
        root / "tools/control/tests/test_cortex_upcc_a02_integration.py",
        root / "tools/control/tests/test_cortex_upcc_a03_restart.py",
        root / "tools/control/tests/test_cortex_upcc_a04_parity.py",
        root / "tools/control/tests/test_cortex_upcc_a05_contract.py",
        root / "tools/control/tests/test_pcc_vault_storage.py",
        root / "tools/control/tests/test_cortex_bootstrap_portable.py",
    ]
    missing = [str(p.relative_to(root)) for p in tests if not p.is_file()]
    if missing:
        print("[FAIL] Missing mandatory Python PCC tests: " + ", ".join(missing))
        return 2
    command = [*which_python(), "-B", "-m", "unittest", "-v", *map(str, tests)]
    if runner:
        result = runner.run(command, cwd=root, timeout=900, stream=True, phase="gate:python-pcc-tests")
        if not result.ok:
            return result.returncode if result.returncode != 0 else 2
    else:
        result = subprocess.run(command, cwd=str(root), check=False)
        if result.returncode != 0:
            return result.returncode
    provider_tests = root / "tools/pcc/tests"
    if provider_tests.is_dir():
        if not list(provider_tests.glob("test_*.py")):
            print("[FAIL] Universal Python PCC provider test suite is empty: " + str(provider_tests))
            return 2
        command = [*which_python(), "-B", "-m", "unittest", "discover", "-s", str(provider_tests), "-p", "test_*.py", "-v"]
        environment = os.environ.copy()
        provider_src = str(root / "tools/pcc/src")
        environment["PYTHONPATH"] = provider_src + (os.pathsep + environment["PYTHONPATH"] if environment.get("PYTHONPATH") else "")
        if runner:
            result = runner.run(command, cwd=root, timeout=900, stream=True, phase="gate:universal-provider-tests", env=environment)
            return result.returncode if not result.timed_out and not result.cancelled else 2
        return subprocess.run(command, cwd=str(root), check=False, env=environment).returncode
    print("[FAIL] Missing Universal Python PCC provider test directory: " + str(provider_tests))
    return 2


def run_self_tests(root: Path, runner: CommandRunner | None = None) -> int:
    test_file = root / "tools" / "control" / "tests" / "test_cortex_pcc.py"
    if not test_file.is_file():
        print(f"Self-test file missing: {test_file}")
        return 1
    argv = [*which_python(), "-m", "unittest", "-v", str(test_file)]
    if runner:
        result = runner.run(argv, cwd=root, timeout=600, stream=True, phase="pcc:self-test")
        return result.returncode
    return subprocess.run(argv, cwd=str(root), check=False).returncode


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Cortex Python Project Control Center")
    parser.add_argument("command", nargs="?", default="interactive", choices=[
        "interactive", "status", "status-json", "quick", "fast", "full",
        "patch-status", "patch-apply", "debug-bundle", "git-status", "git-review",
        "git-history", "git-verify", "git-fetch", "git-compare", "git-pull", "git-setup",
        "git-identity", "git-identity-status", "commit-green", "commit-push-green", "push", "build", "build-release", "format",
        "storage-status", "storage-status-json", "storage-health", "storage-health-json", "storage-prepare", "storage-reclaim-plan", "storage-reclaim-plan-all", "vault-mirror", "vault-mirror-all", "vault-verify", "vault-verify-deep",
        "vault-retention-plan", "vault-retention-apply", "vault-gc-plan", "vault-gc-stage", "vault-gc-restore", "vault-gc-purge",
        "launch-gui", "self-test", "doctor", "doctor-json",
        "root-hygiene", "root-hygiene-fix", "artifact-status",
        "artifact-prune", "artifact-prune-apply", "verify-latest-debug", "source-rollup", "universal-audit", "universal-plan", "forgepy-parity", "contract-migrate",
    ])
    parser.add_argument("--root")
    parser.add_argument("--scan-root", action="append", default=[], help="Additional roots for read-only PCC inventory")
    parser.add_argument("--max-depth", type=int, default=5, help="Bounded inventory depth")
    parser.add_argument("--gate-key", default="full", help="Requested gate for universal-plan; plan only, no execution")
    parser.add_argument("--project-root", help="Other repository to inspect using universal-plan (read-only)")
    parser.add_argument("--rust-consumer", action="store_true", help="Enforce typed Rust compatibility in universal-plan")
    parser.add_argument("--forgepy-root", help="Optional exact ForgePY checkout for read-only feature inventory")
    parser.add_argument("--expected-sha256", help="Optional additional contract migration SHA-256 preimage")
    parser.add_argument("--strict", action="store_true", help="Fail forgepy-parity on missing source anchors or known contract blockers")
    parser.add_argument("--remote", default=DEFAULT_REMOTE)
    parser.add_argument("--message", default="")
    parser.add_argument("--git-name", default="")
    parser.add_argument("--git-email", default="")
    parser.add_argument("--git-scope", choices=["local", "global"], default="local")
    parser.add_argument("--yes", action="store_true", help="Do not prompt for explicit patch apply confirmation.")
    parser.add_argument("--no-evidence", action="store_true", help="Skip debug bundle creation for gate command.")
    parser.add_argument("--reason", default="HEADLESS_MANUAL")
    parser.add_argument("--failed-stage", default="")
    parser.add_argument("--exit-code", type=int, default=0)
    parser.add_argument("--open-folder", action="store_true")
    parser.add_argument("--quiet", action="store_true")
    return parser


def _dispatch_main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    root = normalize_root(args.root)
    cmd = args.command
    if cmd == "contract-migrate":
        # Inspection must not bootstrap the app or invoke any project command.
        # Mutation is explicitly opted into with --yes, backup and shared lock.
        from CortexContractMigration import main as migration_main
        migration_args = ["--root", str(root)]
        if args.expected_sha256:
            migration_args.extend(["--expected-sha256", args.expected_sha256])
        if args.yes:
            migration_args.append("--yes")
        return migration_main(migration_args)
    if cmd == "forgepy-parity":
        # No controller construction, imported donor code, background scan, or mutation.
        from ForgePYParity import main as parity_main
        parity_args = ["--root", str(root)]
        if args.forgepy_root:
            parity_args.extend(["--forgepy-root", args.forgepy_root])
        if args.strict:
            parity_args.append("--strict")
        return parity_main(parity_args)
    if cmd == "universal-plan":
        # Independent read-only cross-project plan; no CortexPCC construction.
        from UniversalPCCPlan import main as plan_main
        target = normalize_root(args.project_root) if args.project_root else root
        plan_args = ["--root", str(target), "--gate", args.gate_key]
        if args.rust_consumer:
            plan_args.append("--rust-consumer")
        return plan_main(plan_args)
    if cmd == "universal-audit":
        # Must be handled before CortexPCC construction: an audit must not create
        # session logs, perform startup hygiene or run any project command.
        from UniversalPCCAudit import main as audit_main
        audit_args = ["inventory", "--root", str(root), "--max-depth", str(args.max_depth)]
        for scan_root in args.scan_root:
            audit_args.extend(["--scan-root", scan_root])
        return audit_main(audit_args)
    pcc = CortexPCC(root, remote=args.remote, quiet=(args.quiet or cmd == "status-json"))
    if cmd == "interactive": return pcc.interactive()
    if cmd == "status": return pcc.status()
    if cmd == "status-json": return pcc.status(as_json=True)
    if cmd == "storage-status": return pcc.storage_status(as_json=False)
    if cmd == "storage-status-json": return pcc.storage_status(as_json=True)
    if cmd == "storage-health": return pcc.storage_health(as_json=False)
    if cmd == "storage-health-json": return pcc.storage_health(as_json=True)
    if cmd == "storage-prepare": return pcc.storage_prepare()
    if cmd == "storage-reclaim-plan": return pcc.storage_reclaim_plan(all_registered=False)
    if cmd == "storage-reclaim-plan-all": return pcc.storage_reclaim_plan(all_registered=True)
    if cmd == "vault-mirror": return pcc.vault_mirror(label="manual-cli")
    if cmd == "vault-mirror-all": return pcc.vault_mirror_all_registered()
    if cmd == "vault-verify": return pcc.vault_verify(deep=False)
    if cmd == "vault-verify-deep": return pcc.vault_verify(deep=True)
    if cmd == "vault-retention-plan": return pcc.vault_retention(apply=False)
    if cmd == "vault-retention-apply":
        if not args.yes:
            print("Refusing metadata retention mutation without --yes")
            return 2
        return pcc.vault_retention(apply=True)
    if cmd == "vault-gc-plan": return pcc.vault_gc(action="plan")
    if cmd in {"vault-gc-stage", "vault-gc-restore", "vault-gc-purge"}:
        if not args.yes:
            print(f"Refusing {cmd} mutation without --yes")
            return 2
        return pcc.vault_gc(action={"vault-gc-stage":"stage", "vault-gc-restore":"restore", "vault-gc-purge":"purge"}[cmd])
    if cmd in {"quick", "fast", "full"}: return pcc.gate(cmd, evidence=not args.no_evidence)
    if cmd == "patch-status": return 0 if pcc.patch_status()[1].get("Invalid", 0) == 0 else 2
    if cmd == "patch-apply": return pcc.patch_apply(confirm=not args.yes)
    if cmd == "debug-bundle":
        pcc.evidence.create(reason=args.reason, failed_stage=args.failed_stage, exit_code=args.exit_code, open_after=args.open_folder)
        return 0
    if cmd == "git-identity-status":
        return pcc.git.action("identity-status", stream=True).returncode
    if cmd == "git-identity":
        return pcc.git.action(
            "identity-set",
            extra=["--name", args.git_name, "--email", args.git_email, "--scope", args.git_scope],
            stream=True,
        ).returncode
    git_map = {
        "git-status": "status", "git-review": "review", "git-history": "history",
        "git-verify": "verify", "git-fetch": "fetch", "git-compare": "compare",
        "git-pull": "pull", "git-setup": "setup", "push": "push",
    }
    if cmd in git_map: return pcc.git.action(git_map[cmd], stream=True).returncode
    if cmd in {"commit-green", "commit-push-green"}:
        msg = args.message or "Cortex GREEN checkpoint"
        return pcc.git.action(cmd, message=msg, stream=True).returncode
    if cmd == "build": return pcc.build()
    if cmd == "build-release": return pcc.build(release=True)
    if cmd == "format": return pcc.format_source()
    if cmd == "launch-gui": return pcc.launch_gui()
    if cmd == "root-hygiene": return pcc.root_hygiene(repair=False)
    if cmd == "root-hygiene-fix": return pcc.root_hygiene(repair=True)
    if cmd in {"doctor", "artifact-status"}: return pcc.artifact_status(as_json=False)
    if cmd == "doctor-json": return pcc.artifact_status(as_json=True)
    if cmd == "artifact-prune": return pcc.prune_artifacts(apply=False)
    if cmd == "artifact-prune-apply": return pcc.prune_artifacts(apply=True)
    if cmd == "verify-latest-debug": return pcc.verify_latest_debug()
    if cmd == "source-rollup": return pcc.create_source_rollup(open_after=args.open_folder)
    if cmd == "self-test": return run_self_tests(root, pcc.runner)
    raise PCCError(f"Unsupported command: {cmd}")


def main(argv: Sequence[str] | None = None) -> int:
    """Public Python API: normalize an intentional control-plane restart to exit 0.

    ProjectControlCenter.py imports this function rather than running CortexPCC.py
    as __main__.  A successful self-update raised PCCRestart through that wrapper,
    producing a Python traceback and an incorrect GUI FAIL despite an applied patch.
    Keep the internal exception so nested interactive menus stop using stale code.
    Do not catch patch errors or unsuccessful restart launches: they must still fail.
    """
    try:
        return _dispatch_main(argv)
    except PCCRestart:
        return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except PCCRestart:
        raise SystemExit(0)
    except KeyboardInterrupt:
        print("Cortex PCC cancelled.", file=sys.stderr)
        raise SystemExit(130)
    except PCCError as exc:
        print(f"FAIL: {exc}", file=sys.stderr)
        raise SystemExit(1)
