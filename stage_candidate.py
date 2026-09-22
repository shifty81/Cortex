#!/usr/bin/env python3
"""Stage a Cortex-only N1 candidate from one exact published PCC preimage.

NEVER overwrites the Cortex checkout. Never runs a build, invokes a project
command, or touches Git state. Generates a reviewable source tree and diff in a
separate explicitly selected directory. This is NOT a governed .patch transport.
"""
from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import os
import shutil
import sys
from pathlib import Path

EXPECTED_BLOB = "9c9f611cf70bbaaffc62b9375c0a05bebe11cc06"
PCC_REL = Path("tools/control/CortexPCC.py")
ADDITIONS = (
    Path("tools/control/CortexDesktopReadiness.py"),
    Path("tools/control/tests/test_cortex_desktop_readiness.py"),
)
OLD_LAUNCH = '''        # Process survival is a preliminary startup observation, not UI-ready.
        runtime_logs = self.ctx.root / "artifacts" / "logs" / "runtime"
        runtime_logs.mkdir(parents=True, exist_ok=True)
        runtime_log = runtime_logs / f"cortex-desktop-{local_stamp()}-{uuid.uuid4().hex[:8]}.log"
        try:
            with runtime_log.open("wb") as output:
                proc = subprocess.Popen([str(gui), str(self.ctx.root)], cwd=str(self.ctx.root),
                                        stdout=output, stderr=subprocess.STDOUT,
                                        stdin=subprocess.DEVNULL)
            time.sleep(1.0)
            returncode = proc.poll()
            if returncode is not None:
                self.log.emit("FAIL", f"Cortex Desktop exited during startup (exit={returncode}); runtime log: {runtime_log}; UI not certified")
                return 2
        except OSError as exc:
            self.log.emit("FAIL", f"Cortex Desktop process could not be started: {exc}; runtime log: {runtime_log}")
            return 2
        self.log.emit("INFO", f"Cortex Desktop process running (PID={proc.pid}); UI handshake not certified; log: {runtime_log}")
        return 0
'''
NEW_LAUNCH = '''        # Native window observation is stronger than one-second process survival.
        # It still does NOT certify a provider, chat, agent, or full GUI handshake.
        from CortexDesktopReadiness import wait_for_visible_window
        runtime_logs = self.ctx.root / "artifacts" / "logs" / "runtime"
        runtime_logs.mkdir(parents=True, exist_ok=True)
        runtime_log = runtime_logs / f"cortex-desktop-{local_stamp()}-{uuid.uuid4().hex[:8]}.log"
        try:
            with runtime_log.open("wb") as output:
                proc = subprocess.Popen([str(gui), str(self.ctx.root)], cwd=str(self.ctx.root),
                                        stdout=output, stderr=subprocess.STDOUT,
                                        stdin=subprocess.DEVNULL)
            observed = wait_for_visible_window(proc.pid, proc.poll)
            if observed.status != "WINDOW_VISIBLE":
                self.log.emit("FAIL", f"Cortex Desktop startup {observed.status} (PID={proc.pid}, "
                              f"exit={observed.exit_code}): {observed.detail}; "
                              f"runtime log: {runtime_log}; UI/chat/provider NOT certified")
                return 2
        except OSError as exc:
            self.log.emit("FAIL", f"Cortex Desktop process could not be started: {exc}; runtime log: {runtime_log}")
            return 2
        self.log.emit("INFO", f"Cortex Desktop visible Win32 window (PID={proc.pid}, "
                      f"HWND={observed.window_handle}); chat/provider/agent NOT certified; "
                      f"runtime log: {runtime_log}")
        return 0
'''
OLD_TEST = '        root / "tools/control/tests/test_cortex_upcc_a05_contract.py",\n'
NEW_TEST = OLD_TEST + '        root / "tools/control/tests/test_cortex_desktop_readiness.py",\n'


def git_blob(data: bytes) -> str:
    return hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()


def confined(output: Path, root: Path) -> bool:
    try:
        return os.path.commonpath([os.path.normcase(str(output)), os.path.normcase(str(root))]) == os.path.normcase(str(root))
    except ValueError:  # Different Windows volumes.
        return False


def stage(root: Path, output: Path, *, expected_blob: str = EXPECTED_BLOB) -> dict:
    root = root.expanduser().resolve(strict=True)
    output = output.expanduser().resolve(strict=False)
    if not root.is_dir() or not (root / "apps/cortex_desktop/Cargo.toml").is_file():
        raise ValueError("source is not a Cortex repository root")
    if confined(output, root) or output == root:
        raise ValueError("stage directory must be outside the actual Cortex project")
    if output.exists():
        raise ValueError("stage directory already exists; refusing overwrite")
    manifest_path = root / "project.control.json"
    contract = json.loads(manifest_path.read_text(encoding="utf-8-sig"))
    if contract.get("project", {}).get("id") != "cortex":
        raise ValueError("project manifest does not identify Cortex")
    target = root / PCC_REL
    if target.is_symlink() or not target.is_file():
        raise ValueError("Cortex PCC source missing or linked")
    original = target.read_bytes()
    actual_blob = git_blob(original)
    if actual_blob != expected_blob:
        raise ValueError(f"PCC preimage mismatch: {actual_blob}; candidate targets {expected_blob}. "
                         "No files changed; rebase against the actual checkout.")
    old_text = original.decode("utf-8")
    if old_text.count(OLD_LAUNCH) != 1 or old_text.count(OLD_TEST) != 1:
        raise ValueError("known insertion anchors are absent or ambiguous")
    for addition in ADDITIONS:
        if (root / addition).exists() or (root / addition).is_symlink():
            raise ValueError(f"new module already exists: {addition}; rebase required")
    new_text = old_text.replace(OLD_LAUNCH, NEW_LAUNCH, 1).replace(OLD_TEST, NEW_TEST, 1)
    patch = "".join(difflib.unified_diff(old_text.splitlines(keepends=True),
                                         new_text.splitlines(keepends=True),
                                         fromfile="a/" + PCC_REL.as_posix(),
                                         tofile="b/" + PCC_REL.as_posix(), n=3))
    source = Path(__file__).resolve().parent
    output.mkdir(parents=True, exist_ok=False)
    try:
        staged_pcc = output / PCC_REL
        staged_pcc.parent.mkdir(parents=True)
        staged_pcc.write_bytes(new_text.encode("utf-8"))
        for addition in ADDITIONS:
            destination = output / addition
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source / addition, destination)
        review = output / "review"
        review.mkdir()
        (review / "CortexPCC.diff").write_text(patch, encoding="utf-8")
        report = {
            "status": "STAGED_FOR_REVIEW_ONLY", "product": "Cortex",
            "publishedCommit": "5e5e113d6168acac721bb722d169be04b4ec1c3f",
            "sourcePccGitBlob": actual_blob,
            "sourcePccSha256": hashlib.sha256(original).hexdigest(),
            "stagedPccSha256": hashlib.sha256(new_text.encode("utf-8")).hexdigest(),
            "files": [PCC_REL.as_posix(), *(path.as_posix() for path in ADDITIONS)],
            "sourceCheckoutModified": False,
            "validated": ["PCC exact Git-blob preimage", "Cortex project identity",
                          "unique source anchors", "no overwrites in source tree"],
            "NOT_certified": ["Windows native GUI", "chat provider", "ForgeGUI host", "product Full Gate"],
        }
        (review / "SOURCE_PROVENANCE.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        return report
    except BaseException:
        # Only remove the newly created, dedicated staging directory.
        shutil.rmtree(output)
        raise


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Prepare review-only Cortex N1 startup candidate")
    parser.add_argument("--root", required=True, type=Path, help="actual Cortex checkout")
    parser.add_argument("--stage", required=True, type=Path, help="new directory OUTSIDE that checkout")
    args = parser.parse_args(argv)
    try:
        print(json.dumps(stage(args.root, args.stage), indent=2))
        return 0
    except (OSError, ValueError, UnicodeError, json.JSONDecodeError) as error:
        print(f"BLOCKED: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
