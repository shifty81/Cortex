#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import threading
import time
from datetime import datetime
from pathlib import Path
from typing import Any, Sequence

from PCCSurfaceCommon import (
    BackendClient,
    ProjectContract,
    SurfaceError,
    compact_path,
    latest_debug_bundle,
    open_path,
    resolve_root,
    reveal_file,
    validate_surface,
)

CONSOLE_VERSION = "PCC-CONSOLE-0.1"

CSI = "\x1b["
RESET = CSI + "0m"
CYAN = CSI + "96m"
GREEN = CSI + "92m"
YELLOW = CSI + "93m"
RED = CSI + "91m"
WHITE = CSI + "97m"
GRAY = CSI + "90m"
BOLD = CSI + "1m"


def enable_ansi() -> bool:
    if not sys.stdout.isatty():
        return False
    try:
        if os.name == "nt":
            os.system("")
        return True
    except Exception:
        return False


ANSI = enable_ansi()


def c(text: str, color: str) -> str:
    return f"{color}{text}{RESET}" if ANSI else text


def clear() -> None:
    os.system("cls" if os.name == "nt" else "clear")


def pause() -> None:
    try:
        input(c("Press Enter to return...", GRAY))
    except EOFError:
        pass


class OperatorConsole:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.contract = ProjectContract.load(root)
        self.backend = BackendClient(root)
        self.status: dict[str, Any] = {}
        self.last_action = "None"
        self.last_state = "READY"
        self.last_elapsed = 0.0
        self.last_output = ""

    def refresh(self) -> None:
        try:
            self.status = self.backend.status()
        except Exception as exc:
            self.status = {"error": str(exc)}

    def _kv(self, key: str, value: str, color: str = WHITE) -> None:
        print(f"{c(key.ljust(11), GRAY)}: {c(value, color)}")

    def banner(self) -> None:
        self.refresh()
        clear()
        width = 78
        print(c("=" * width, CYAN))
        print(c(f" {self.contract.name.upper()} PROJECT CONTROL CENTER", CYAN + BOLD if ANSI else CYAN))
        print(c("=" * width, CYAN))

        if self.status.get("error"):
            self._kv("Repository", compact_path(self.root, 62), WHITE)
            self._kv("Status", "Unavailable", RED)
            self._kv("Reason", str(self.status["error"])[-62:], RED)
            print(c("-" * width, CYAN))
            return

        git = self.status.get("git") or {}
        patches = self.status.get("patches") or {}
        hygiene = self.status.get("hygiene") or {}
        binaries = self.status.get("binaries") or {}
        tools = self.status.get("tools") or {}

        changed = int(git.get("staged", 0) or 0) + int(git.get("unstaged", 0) or 0) + int(git.get("untracked", 0) or 0)
        if not git.get("gitReady"):
            git_text, git_color = "Not ready", RED
        elif git.get("clean"):
            git_text, git_color = "Clean", GREEN
        else:
            git_text, git_color = (f"Modified ({changed} path(s))" if changed else "Modified"), YELLOW

        if git.get("greenMatch"):
            green_text, green_color = "GREEN / MATCH", GREEN
        elif git.get("greenMarker"):
            green_text, green_color = "STALE", YELLOW
        else:
            green_text, green_color = "NONE", GRAY

        invalid = int(patches.get("invalid", 0) or 0)
        pending = int(patches.get("pending", 0) or 0)
        if invalid:
            updates, updates_color = f"{invalid} invalid", RED
        elif pending:
            updates, updates_color = f"{pending} pending", YELLOW
        else:
            updates, updates_color = "0 pending", GREEN

        ahead, behind = git.get("ahead"), git.get("behind")
        sync = "Unknown" if ahead is None or behind is None else ("MATCH" if ahead == 0 and behind == 0 else f"{ahead} ahead / {behind} behind")
        sync_color = GREEN if sync == "MATCH" else YELLOW

        self._kv("Repository", compact_path(self.root, 64), WHITE)
        self._kv("Git", git_text, git_color)
        self._kv("Branch", f"{git.get('branch') or '<none>'} @ {git.get('headShort') or '<unborn>'}", WHITE)
        self._kv("Sync", sync, sync_color)
        self._kv("Gate", green_text, green_color)
        self._kv("Updates", updates, updates_color)
        self._kv("Hygiene", "Clean" if hygiene.get("clean", True) else "Needs attention", GREEN if hygiene.get("clean", True) else YELLOW)
        self._kv("Toolchain", "Ready" if tools.get("cargo") and tools.get("rustc") else "Incomplete", GREEN if tools.get("cargo") and tools.get("rustc") else RED)
        self._kv("Cortex", "PCC / CLI-first", GREEN)
        self._kv("Last", f"{self.last_action} [{self.last_state}] in {self.last_elapsed:.1f}s", GREEN if self.last_state == "PASS" else (RED if self.last_state == "FAIL" else GRAY))
        print(c("-" * width, CYAN))

    def footer(self) -> None:
        if self.status.get("error"):
            print(c("[Status:Unavailable]", RED))
            return
        git = self.status.get("git") or {}
        patches = self.status.get("patches") or {}
        hygiene = self.status.get("hygiene") or {}
        bits = [
            ("Git", "Clean" if git.get("clean") else "Modified", GREEN if git.get("clean") else YELLOW),
            ("GREEN", "MATCH" if git.get("greenMatch") else "STALE", GREEN if git.get("greenMatch") else YELLOW),
            ("Updates", str(patches.get("pending", 0)), RED if patches.get("invalid") else (YELLOW if patches.get("pending") else GREEN)),
            ("Hygiene", "Ready" if hygiene.get("clean", True) else "WARN", GREEN if hygiene.get("clean", True) else YELLOW),
            ("Last", self.last_state, GREEN if self.last_state == "PASS" else (RED if self.last_state == "FAIL" else GRAY)),
        ]
        print(" ".join(c(f"[{k}:{v}]", col) for k, v, col in bits))

    def _run(self, command: str, extra: Sequence[str] = (), *, label: str | None = None, show_output: bool = False) -> int:
        label = label or command
        print()
        print(c(f"START {label}", CYAN))
        start = time.monotonic()
        proc = self.backend.popen(command, extra)
        output: list[str] = []

        def reader() -> None:
            assert proc.stdout is not None
            for line in proc.stdout:
                output.append(line)

        t = threading.Thread(target=reader, daemon=True)
        t.start()
        frames = "|/-\\"
        i = 0
        while proc.poll() is None:
            elapsed = time.monotonic() - start
            sys.stdout.write(c(f"\r  {frames[i % len(frames)]} running... {elapsed:6.1f}s", GRAY))
            sys.stdout.flush()
            i += 1
            time.sleep(0.12)
        t.join(timeout=2)
        elapsed = time.monotonic() - start
        sys.stdout.write("\r" + " " * 48 + "\r")
        rc = proc.returncode or 0
        self.last_action = label
        self.last_elapsed = elapsed
        self.last_state = "PASS" if rc == 0 else "FAIL"
        self.last_output = "".join(output)
        if rc == 0:
            print(c(f"PASS {label}", GREEN) + c(f"  ({elapsed:.1f}s)", GRAY))
        else:
            print(c(f"FAIL {label}", RED) + c(f"  (exit {rc}, {elapsed:.1f}s)", GRAY))

        text = self.last_output.strip()
        if show_output and text:
            print(c("-" * 78, GRAY))
            print(text)
        elif rc != 0 and text:
            # Keep the operator surface compact: failure excerpt only; complete evidence stays in artifacts/logs.
            lines = text.splitlines()
            interesting = [line for line in lines if any(token in line.upper() for token in ("FAIL", "ERROR", "INVALID", "MISSING", "PANIC", "FATAL"))]
            excerpt = (interesting[-12:] if interesting else lines[-16:])
            print(c("Failure excerpt (full output remains in PCC logs/evidence):", YELLOW))
            for line in excerpt:
                print("  " + line[:180])
        return rc

    def commit(self, *, push: bool) -> None:
        default = f"{self.contract.name} GREEN checkpoint - {datetime.now().strftime('%Y-%m-%d %H:%M')}"
        msg = input(f"Commit message [{default}]: ").strip() or default
        command = "commit-push-green" if push else "commit-green"
        self._run(command, ["--message", msg], label="Commit + push GREEN" if push else "Commit GREEN", show_output=True)
        pause()

    def main_menu(self) -> int:
        while True:
            self.banner()
            print(c(" 1. FULL QUALITY GATE / CERTIFY GREEN", GREEN))
            print(c(" 2. COMMIT + PUSH CURRENT GREEN", GREEN))
            print()
            print(" 3. Build & run")
            print(" 4. Updates")
            print(" 5. Project operations")
            print(" 6. Diagnostics & recovery")
            print(" 7. Artifacts & logs")
            print(" 8. Advanced / registered commands")
            print(" 9. Source control")
            print(c(" 0. Exit", GRAY))
            print(c("-" * 78, CYAN))
            self.footer()
            choice = input("Select an option: ").strip()
            if choice == "0":
                return 0
            if choice == "1":
                self._run("full", label="Full quality gate")
                pause()
            elif choice == "2":
                self.commit(push=True)
            elif choice == "3":
                self.build_menu()
            elif choice == "4":
                self.update_menu()
            elif choice == "5":
                self.project_menu()
            elif choice == "6":
                self.diagnostics_menu()
            elif choice == "7":
                self.artifacts_menu()
            elif choice == "8":
                self.advanced_menu()
            elif choice == "9":
                self.git_menu()

    def _submenu_header(self, title: str) -> None:
        self.banner()
        print(c(f" {title.upper()}", CYAN + BOLD if ANSI else CYAN))
        print(c("-" * 78, CYAN))

    def build_menu(self) -> None:
        while True:
            self._submenu_header("Build & Run")
            print(" 1. Build debug workspace")
            print(" 2. Build release workspace")
            print(" 3. Quick gate")
            print(" 4. Fast gate")
            print(" 0. Back")
            choice = input("Select: ").strip()
            if choice == "0": return
            mapping = {"1": ("build", "Build debug"), "2": ("build-release", "Build release"), "3": ("quick", "Quick gate"), "4": ("fast", "Fast gate")}
            if choice in mapping:
                cmd, label = mapping[choice]
                self._run(cmd, label=label)
                pause()

    def update_menu(self) -> None:
        while True:
            self._submenu_header("Updates")
            print(" 1. Inspect validated patch queue")
            print(" 2. Apply validated patch queue")
            print(" 3. Open applied history")
            print(" 4. Open failed history")
            print(" 5. Open patch receipts")
            print(" 0. Back")
            choice = input("Select: ").strip()
            patch_root = self.root / "artifacts" / "patches"
            if choice == "0": return
            if choice == "1": self._run("patch-status", label="Patch status", show_output=True); pause()
            elif choice == "2":
                answer = input("Apply currently validated patch queue? [y/N] ").strip().lower()
                if answer in {"y", "yes"}: self._run("patch-apply", ["--yes"], label="Apply updates", show_output=True); pause()
            elif choice == "3": open_path(patch_root / "applied")
            elif choice == "4": open_path(patch_root / "failed")
            elif choice == "5": open_path(patch_root / "receipts")

    def project_menu(self) -> None:
        while True:
            self._submenu_header("Project Operations")
            print(" 1. Project status")
            print(" 2. PCC doctor")
            print(" 3. Root hygiene status")
            print(" 4. Repair root hygiene")
            print(" 5. Open project folder")
            print(" 0. Back")
            choice = input("Select: ").strip()
            if choice == "0": return
            if choice == "1": self._run("status", label="Project status", show_output=True); pause()
            elif choice == "2": self._run("doctor", label="PCC doctor", show_output=True); pause()
            elif choice == "3": self._run("root-hygiene", label="Root hygiene", show_output=True); pause()
            elif choice == "4": self._run("root-hygiene-fix", label="Repair root hygiene", show_output=True); pause()
            elif choice == "5": open_path(self.root)

    def diagnostics_menu(self) -> None:
        while True:
            self._submenu_header("Diagnostics & Recovery")
            print(" 1. PCC self-tests")
            print(" 2. Create debug bundle")
            print(" 3. Verify latest debug bundle")
            print(" 4. Open latest debug bundle")
            print(" 5. Artifact retention dry-run")
            print(" 6. Apply artifact retention policy")
            print(" 0. Back")
            choice = input("Select: ").strip()
            if choice == "0": return
            if choice == "1": self._run("self-test", label="PCC self-tests"); pause()
            elif choice == "2": self._run("debug-bundle", label="Create debug bundle"); pause()
            elif choice == "3": self._run("verify-latest-debug", label="Verify latest debug", show_output=True); pause()
            elif choice == "4":
                path = latest_debug_bundle(self.root)
                reveal_file(path) if path else open_path(self.root / "artifacts" / "debug")
            elif choice == "5": self._run("artifact-prune", label="Artifact retention dry-run", show_output=True); pause()
            elif choice == "6":
                if input("Delete artifacts beyond retention limits? [y/N] ").strip().lower() in {"y", "yes"}:
                    self._run("artifact-prune-apply", label="Apply artifact retention", show_output=True); pause()

    def artifacts_menu(self) -> None:
        while True:
            self._submenu_header("Artifacts & Logs")
            print(" 1. Open artifacts")
            print(" 2. Open session logs")
            print(" 3. Open debug artifacts")
            print(" 4. Open latest debug bundle")
            print(" 5. View last command output")
            print(" 0. Back")
            choice = input("Select: ").strip()
            if choice == "0": return
            if choice == "1": open_path(self.root / "artifacts")
            elif choice == "2": open_path(self.root / "artifacts" / "logs" / "sessions")
            elif choice == "3": open_path(self.root / "artifacts" / "debug")
            elif choice == "4":
                path = latest_debug_bundle(self.root)
                reveal_file(path) if path else open_path(self.root / "artifacts" / "debug")
            elif choice == "5":
                clear(); print(self.last_output or "No command output captured in this operator session."); pause()

    def advanced_menu(self) -> None:
        while True:
            self._submenu_header("Registered Commands")
            print(f" project.control.json declares {len(self.contract.commands)} project command(s).")
            print(c(" These are shown as contract evidence; execution remains behind PCC Core.", GRAY))
            print()
            for index, item in enumerate(self.contract.commands, start=1):
                print(f" {index:2}. {item.key:<22} {item.label} {c('[' + item.risk + ']', GRAY)}")
            print(" 0. Back")
            choice = input("Select: ").strip()
            if choice == "0": return

    def git_menu(self) -> None:
        while True:
            self._submenu_header("Source Control")
            print(" 1. Status / GREEN eligibility")
            print(" 2. Review working changes")
            print(" 3. Recent history")
            print(" 4. Verify local / origin / GREEN")
            print(" 5. Fetch origin/main")
            print(" 6. Compare local vs origin/main")
            print(" 7. Pull origin/main (FF only)")
            print(" 8. Commit current certified GREEN")
            print(" 9. Commit + push current certified GREEN")
            print(" 0. Back")
            choice = input("Select: ").strip()
            if choice == "0": return
            mapping = {"1": "git-status", "2": "git-review", "3": "git-history", "4": "git-verify", "5": "git-fetch", "6": "git-compare", "7": "git-pull"}
            if choice in mapping:
                self._run(mapping[choice], label=mapping[choice], show_output=True); pause()
            elif choice == "8": self.commit(push=False)
            elif choice == "9": self.commit(push=True)


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="Havenwild-style Cortex PCC console surface")
    p.add_argument("--root")
    p.add_argument("--self-test", action="store_true")
    return p


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    root = resolve_root(args.root)
    if args.self_test:
        for note in validate_surface(root):
            print(f"PASS {note}")
        print(f"PASS console-version={CONSOLE_VERSION}")
        return 0
    return OperatorConsole(root).main_menu()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SurfaceError as exc:
        print(c(f"FAIL: {exc}", RED), file=sys.stderr)
        raise SystemExit(1)
