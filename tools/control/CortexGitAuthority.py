#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

DEFAULT_REMOTE = "https://github.com/shifty81/Cortex.git"
MARKER_REL = Path(".cortex") / "last-green-quality-gate.json"

EXCLUDED_DIR_NAMES = {
    ".git",
    ".cortex",
    ".project_control",
    "artifacts",
    "logs",
    "target",
    "updates",
    "__pycache__",
}

EXCLUDED_TOP_LEVEL = {
    "handoffs",
}

EXCLUDED_SUFFIXES = {
    ".zip",
    ".sha256",
    ".pyc",
    ".pyo",
}


class GitError(RuntimeError):
    pass


def project_git_authority(root: Path) -> dict[str, object]:
    path = root / "config" / "cortex" / "github_authority.v1.json"
    if not path.is_file():
        return {}
    try:
        data = json.loads(path.read_text(encoding="utf-8-sig"))
    except Exception as exc:
        raise GitError(f"Invalid GitHub authority file {path}: {exc}") from exc
    if not isinstance(data, dict) or data.get("schema") != "cortex.github_authority.v1":
        raise GitError(f"Unsupported GitHub authority schema in {path}")
    return data


def apply_project_git_defaults(root: Path) -> dict[str, object]:
    authority = project_git_authority(root)
    if not authority or not git_repo(root):
        return authority
    author = authority.get("author") if isinstance(authority.get("author"), dict) else {}
    status = git_identity_status(root)
    if not status.get("configured") and author:
        name = str(author.get("name") or "").strip()
        email = str(author.get("email") or "").strip()
        if name and email:
            clean_name, clean_email = validate_git_identity(name, email)
            git(root, "config", "--local", "user.name", clean_name)
            git(root, "config", "--local", "user.email", clean_email)
            print(f"Git author identity configured from project authority: {clean_name} <{clean_email}>")
    return authority


def run(cmd: list[str], *, cwd: Path | None = None, check: bool = True, timeout: int = 180) -> subprocess.CompletedProcess[str]:
    cp = subprocess.run(
        cmd,
        cwd=str(cwd) if cwd else None,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        timeout=timeout,
        check=False,
    )
    if check and cp.returncode != 0:
        detail = (cp.stderr or cp.stdout).strip()
        raise GitError(detail or f"Command failed ({cp.returncode}): {' '.join(cmd)}")
    return cp


def git(root: Path, *args: str, check: bool = True, timeout: int = 180) -> subprocess.CompletedProcess[str]:
    return run(["git", "-C", str(root), *args], check=check, timeout=timeout)


def git_text(root: Path, *args: str) -> str:
    cp = git(root, *args, check=False)
    return cp.stdout.strip() if cp.returncode == 0 else ""


def git_repo(root: Path) -> bool:
    return git_text(root, "rev-parse", "--is-inside-work-tree").lower() == "true"


def has_commit(root: Path) -> bool:
    return git(root, "rev-parse", "--verify", "HEAD", check=False).returncode == 0


def is_ancestor(root: Path, older: str, newer: str) -> bool:
    return git(root, "merge-base", "--is-ancestor", older, newer, check=False).returncode == 0


def should_skip(root: Path, path: Path) -> bool:
    rel = path.relative_to(root)
    parts = rel.parts
    if not parts:
        return True

    folded_parts = tuple(part.casefold() for part in parts)
    if folded_parts[0] in {x.casefold() for x in EXCLUDED_TOP_LEVEL}:
        return True

    excluded_dirs = {x.casefold() for x in EXCLUDED_DIR_NAMES}
    if any(part in excluded_dirs for part in folded_parts[:-1]):
        return True

    lower_name = path.name.casefold()
    if any(lower_name.endswith(suffix) for suffix in EXCLUDED_SUFFIXES):
        return True

    if lower_name.startswith("cortex_debugbundle_"):
        return True

    return False


def snapshot(root: Path) -> dict[str, object]:
    rows: list[str] = []
    path_count = 0

    for path in sorted(root.rglob("*"), key=lambda p: p.as_posix().casefold()):
        if path.is_symlink() or not path.is_file():
            continue
        if should_skip(root, path):
            continue

        rel = path.relative_to(root).as_posix()
        try:
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            size = path.stat().st_size
        except OSError as exc:
            raise GitError(f"Unable to fingerprint governed source file {rel}: {exc}") from exc
        rows.append(f"{rel}\t{size}\t{digest}")
        path_count += 1

    payload = "\n".join(rows).encode("utf-8")
    return {
        "fingerprint": hashlib.sha256(payload).hexdigest(),
        "pathCount": path_count,
    }


def marker_path(root: Path) -> Path:
    return root / MARKER_REL


def write_json_atomic(path: Path, payload: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".tmp-" + str(os.getpid()))
    tmp.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    os.replace(tmp, path)


def load_marker(root: Path) -> dict[str, object]:
    path = marker_path(root)
    if not path.is_file():
        raise GitError(f"No GREEN quality marker exists yet: {path}")
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception as exc:
        raise GitError(f"GREEN marker is invalid JSON: {exc}") from exc
    if data.get("schema") != "cortex.green_quality_gate.v1":
        raise GitError(f"Unsupported GREEN marker schema: {data.get('schema')}")
    return data


def certify_matches(root: Path, marker: dict[str, object] | None = None) -> tuple[bool, dict[str, object], str]:
    marker = marker or load_marker(root)
    current = snapshot(root)
    expected = str(marker.get("fingerprint") or "")
    actual = str(current["fingerprint"])
    if not expected:
        return False, current, "GREEN marker contains no source fingerprint."
    if expected != actual:
        return False, current, f"Source changed since Full Quality gate: expected={expected} current={actual}"
    return True, current, "Current governed source matches the last GREEN Full Quality gate."


def current_head(root: Path) -> str:
    return git_text(root, "rev-parse", "HEAD") if git_repo(root) and has_commit(root) else ""


def current_branch(root: Path) -> str:
    return git_text(root, "branch", "--show-current") if git_repo(root) else ""


def mark_green(root: Path) -> int:
    snap = snapshot(root)
    marker = {
        "schema": "cortex.green_quality_gate.v1",
        "createdUtc": datetime.now(timezone.utc).isoformat(),
        "project": "Cortex",
        "fingerprint": snap["fingerprint"],
        "pathCount": snap["pathCount"],
        "gitReady": git_repo(root),
        "gitHead": current_head(root) or None,
        "gitBranch": current_branch(root) or None,
    }
    path = marker_path(root)
    path.parent.mkdir(parents=True, exist_ok=True)
    write_json_atomic(path, marker)
    print("GREEN SOURCE MARKER: PASS")
    print(f" Fingerprint : {snap['fingerprint']}")
    print(f" Paths       : {snap['pathCount']}")
    print(f" Marker      : {path}")
    return 0


def ensure_origin(root: Path, remote: str) -> None:
    existing = git_text(root, "remote", "get-url", "origin")
    if existing:
        if existing != remote:
            git(root, "remote", "set-url", "origin", remote)
            print(f"Origin updated: {remote}")
    else:
        git(root, "remote", "add", "origin", remote)
        print(f"Origin added: {remote}")


def safe_backup_name(root: Path) -> str:
    base = "cortex-local-history-backup-" + datetime.now().strftime("%Y%m%d-%H%M%S")
    existing = set(git_text(root, "branch", "--format=%(refname:short)").splitlines())
    name = base
    index = 2
    while name in existing:
        name = f"{base}-{index}"
        index += 1
    return name


def setup_or_repair(root: Path, remote: str) -> int:
    if shutil.which("git") is None:
        raise GitError("Git was not found on PATH.")

    authority = project_git_authority(root)
    configured_remote = str(authority.get("remote") or "").strip() if authority else ""
    if configured_remote:
        remote = configured_remote

    before = snapshot(root)
    green_ok = False
    green_reason = "No GREEN marker"
    try:
        green_ok, _, green_reason = certify_matches(root)
    except Exception as exc:
        green_reason = str(exc)

    print("CORTEX GIT WORKING-FOLDER SETUP / REPAIR")
    print(f" Project     : {root}")
    print(f" Remote      : {remote}")
    print(f" Green source: {'MATCH' if green_ok else 'NOT CERTIFIED'}")

    if not git_repo(root):
        cp = run(["git", "init", "-b", "main", str(root)], check=False, timeout=60)
        if cp.returncode != 0:
            run(["git", "init", str(root)], timeout=60)
            git(root, "branch", "-M", "main")
        print("Git repository initialized.")

    branch = current_branch(root)
    if branch and branch != "main":
        git(root, "branch", "-M", "main")
        print(f"Local branch renamed: {branch} -> main")

    ensure_origin(root, remote)
    apply_project_git_defaults(root)
    git(root, "fetch", "origin", "main", timeout=300)

    if git(root, "rev-parse", "--verify", "origin/main", check=False).returncode != 0:
        raise GitError("origin/main could not be resolved after fetch.")

    remote_head = git_text(root, "rev-parse", "origin/main")
    local_head = current_head(root)

    if not local_head:
        relationship = "adopt-remote"
    elif local_head == remote_head:
        relationship = "already-aligned"
    elif is_ancestor(root, local_head, remote_head):
        relationship = "fast-forward-adopt"
    elif is_ancestor(root, remote_head, local_head):
        relationship = "keep-local-ahead"
    else:
        relationship = "divergent-adopt"

    print(f" Local HEAD  : {local_head or '<unborn>'}")
    print(f" origin/main : {remote_head}")
    print(f" Relationship: {relationship}")

    backup = ""
    if relationship in {"adopt-remote", "fast-forward-adopt", "divergent-adopt"}:
        if relationship == "divergent-adopt":
            if not green_ok:
                raise GitError(
                    "Local and remote histories diverge and current source is not certified by the last GREEN marker. "
                    f"Refusing automatic adoption. Detail: {green_reason}"
                )
            backup = safe_backup_name(root)
            git(root, "branch", backup, local_head)
            print(f"Backup branch created: {backup}")

        # Deliberately preserve every working-tree byte while attaching history/index.
        git(root, "reset", "--mixed", "origin/main", timeout=180)
        print("Local main adopted origin/main history without overwriting working files.")

    git(root, "branch", "--set-upstream-to=origin/main", "main", check=False)

    after = snapshot(root)
    if before != after:
        raise GitError(
            "Governed working-source bytes changed during Git setup/repair. "
            "The operation must be inspected before continuing."
        )

    print("WORKING SOURCE PRESERVATION: PASS")
    print(f" Governed paths: {after['pathCount']}")
    print(f" Fingerprint   : {after['fingerprint']}")
    if backup:
        print(f" Recovery ref  : {backup}")

    print("\nGit status after setup/repair:")
    print(git(root, "status", "--short", "--branch", check=False).stdout.strip() or "## main...origin/main")
    return 0





def git_identity_status(root: Path) -> dict[str, object]:
    local_name = git_text(root, "config", "--local", "--get", "user.name") if git_repo(root) else ""
    local_email = git_text(root, "config", "--local", "--get", "user.email") if git_repo(root) else ""
    global_name = git_text(root, "config", "--global", "--get", "user.name")
    global_email = git_text(root, "config", "--global", "--get", "user.email")
    effective_name = local_name or global_name
    effective_email = local_email or global_email
    return {
        "configured": bool(effective_name and effective_email),
        "effectiveName": effective_name or None,
        "effectiveEmail": effective_email or None,
        "localName": local_name or None,
        "localEmail": local_email or None,
        "globalName": global_name or None,
        "globalEmail": global_email or None,
        "source": "local" if local_name and local_email else ("global" if global_name and global_email else "missing"),
    }


def validate_git_identity(name: str, email: str) -> tuple[str, str]:
    clean_name = str(name or "").strip()
    clean_email = str(email or "").strip()
    if not clean_name:
        raise GitError("Git author name cannot be empty.")
    if "\n" in clean_name or "\r" in clean_name:
        raise GitError("Git author name must be a single line.")
    if not clean_email or "@" not in clean_email or clean_email.startswith("@") or clean_email.endswith("@"):
        raise GitError("Git author email must be a valid non-empty email address.")
    if any(ch in clean_email for ch in "\r\n \t"):
        raise GitError("Git author email must not contain whitespace or line breaks.")
    return clean_name, clean_email


def set_git_identity(root: Path, name: str, email: str, scope: str = "local") -> int:
    if not git_repo(root):
        raise GitError("Cortex is not a Git repository yet. Run Git Setup first.")
    clean_name, clean_email = validate_git_identity(name, email)
    normalized_scope = str(scope or "local").strip().casefold()
    if normalized_scope not in {"local", "global"}:
        raise GitError("Git identity scope must be 'local' or 'global'.")
    flag = "--global" if normalized_scope == "global" else "--local"
    git(root, "config", flag, "user.name", clean_name)
    git(root, "config", flag, "user.email", clean_email)
    status = git_identity_status(root)
    if status.get("effectiveName") != clean_name or status.get("effectiveEmail") != clean_email:
        raise GitError("Git identity verification failed after configuration.")
    print("GIT AUTHOR IDENTITY: PASS")
    print(f" Scope : {normalized_scope}")
    print(f" Name  : {clean_name}")
    print(f" Email : {clean_email}")
    return 0


def show_git_identity(root: Path) -> int:
    status = git_identity_status(root)
    print("GIT AUTHOR IDENTITY")
    print("===================")
    print(f" Configured : {'YES' if status['configured'] else 'NO'}")
    print(f" Source     : {status['source']}")
    print(f" Name       : {status.get('effectiveName') or '<missing>'}")
    print(f" Email      : {status.get('effectiveEmail') or '<missing>'}")
    return 0 if status["configured"] else 2


def require_git_identity(root: Path) -> dict[str, object]:
    apply_project_git_defaults(root)
    status = git_identity_status(root)
    if not status.get("configured"):
        raise GitError(
            "Git author identity is not configured. Open Source Control -> Git Identity, "
            "or run CortexPCC.py git-identity --git-name <name> --git-email <email> "
            "--git-scope local|global before committing certified source."
        )
    return status


def status_summary(root: Path) -> dict[str, object]:
    result: dict[str, object] = {
        "repository": str(root),
        "gitReady": git_repo(root),
        "branch": None,
        "head": None,
        "headShort": None,
        "origin": None,
        "upstream": None,
        "ahead": None,
        "behind": None,
        "clean": False,
        "staged": 0,
        "unstaged": 0,
        "untracked": 0,
        "greenMarker": False,
        "greenMatch": False,
        "greenEligible": False,
        "greenCreatedUtc": None,
        "greenFingerprint": None,
        "greenPaths": None,
        "greenDetail": None,
        "gitIdentityConfigured": False,
        "gitIdentityName": None,
        "gitIdentityEmail": None,
        "gitIdentitySource": "missing",
    }

    if not result["gitReady"]:
        result["greenDetail"] = "Git is not initialized."
        return result

    branch = current_branch(root) or "<detached>"
    head = current_head(root)
    origin = git_text(root, "remote", "get-url", "origin") or "<none>"
    upstream = git_text(root, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}") or "<none>"

    porcelain = git(root, "status", "--porcelain=v1", "-uall", check=False).stdout.splitlines()
    staged = 0
    unstaged = 0
    untracked = 0
    for line in porcelain:
        if line.startswith("??"):
            untracked += 1
            continue
        if len(line) >= 2:
            if line[0] not in (" ", "?"):
                staged += 1
            if line[1] != " ":
                unstaged += 1

    ahead = None
    behind = None
    if upstream != "<none>":
        counts = git_text(root, "rev-list", "--left-right", "--count", f"HEAD...{upstream}")
        parts = counts.split()
        if len(parts) == 2 and all(part.isdigit() for part in parts):
            ahead = int(parts[0])
            behind = int(parts[1])

    result.update({
        "branch": branch,
        "head": head or None,
        "headShort": head[:8] if head else None,
        "origin": origin,
        "upstream": upstream,
        "ahead": ahead,
        "behind": behind,
        "clean": staged == 0 and unstaged == 0 and untracked == 0,
        "staged": staged,
        "unstaged": unstaged,
        "untracked": untracked,
    })

    identity = git_identity_status(root)
    result.update({
        "gitIdentityConfigured": bool(identity.get("configured")),
        "gitIdentityName": identity.get("effectiveName"),
        "gitIdentityEmail": identity.get("effectiveEmail"),
        "gitIdentitySource": identity.get("source"),
    })

    try:
        marker = load_marker(root)
        ok, snap, reason = certify_matches(root, marker)
        result.update({
            "greenMarker": True,
            "greenMatch": ok,
            "greenEligible": ok,
            "greenCreatedUtc": marker.get("createdUtc"),
            "greenFingerprint": snap["fingerprint"],
            "greenPaths": snap["pathCount"],
            "greenDetail": reason,
        })
    except Exception as exc:
        result["greenDetail"] = str(exc)

    return result


def show_summary_json(root: Path) -> int:
    print(json.dumps(status_summary(root), separators=(",", ":")))
    return 0


def refresh_marker_git_identity(root: Path) -> None:
    try:
        marker = load_marker(root)
        ok, snap, _ = certify_matches(root, marker)
        if not ok:
            return
        marker["gitReady"] = git_repo(root)
        marker["gitHead"] = current_head(root) or None
        marker["gitBranch"] = current_branch(root) or None
        marker["fingerprint"] = snap["fingerprint"]
        marker["pathCount"] = snap["pathCount"]
        write_json_atomic(marker_path(root), marker)
    except Exception:
        pass


def show_status(root: Path) -> int:
    print("CORTEX GIT STATUS")
    print("=================")
    print(f" Repository  : {root}")

    if not git_repo(root):
        print(" Git         : NOT INITIALIZED")
        print(" Next action : Git / Source Control -> Initialize / connect / repair")
        return 0

    branch = current_branch(root) or "<detached>"
    head_full = current_head(root)
    head_short = head_full[:12] if head_full else "<unborn>"
    origin = git_text(root, "remote", "get-url", "origin") or "<none>"
    upstream = git_text(root, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}") or "<none>"

    porcelain = git(root, "status", "--porcelain=v1", "-uall", check=False).stdout.splitlines()
    staged = 0
    unstaged = 0
    untracked = 0

    for line in porcelain:
        if line.startswith("??"):
            untracked += 1
            continue
        if len(line) >= 2:
            if line[0] not in (" ", "?"):
                staged += 1
            if line[1] != " ":
                unstaged += 1

    clean = staged == 0 and unstaged == 0 and untracked == 0

    ahead = None
    behind = None
    if upstream != "<none>":
        counts = git_text(root, "rev-list", "--left-right", "--count", f"HEAD...{upstream}")
        parts = counts.split()
        if len(parts) == 2 and all(part.isdigit() for part in parts):
            ahead = int(parts[0])
            behind = int(parts[1])

    print(f" Branch      : {branch}")
    print(f" HEAD        : {head_short}")
    print(f" Origin      : {origin}")
    print(f" Upstream    : {upstream}")
    print(f" Ahead/Behind: {ahead} / {behind}" if ahead is not None else " Ahead/Behind: unavailable")
    print(f" Working tree: {'CLEAN' if clean else 'MODIFIED'}")
    print(f" Staged      : {staged}")
    print(f" Unstaged    : {unstaged}")
    print(f" Untracked   : {untracked}")

    identity = git_identity_status(root)
    print("")
    print("GIT AUTHOR IDENTITY")
    print("-------------------")
    print(f" Configured  : {'YES' if identity['configured'] else 'NO'}")
    print(f" Source      : {identity['source']}")
    print(f" Name        : {identity.get('effectiveName') or '<missing>'}")
    print(f" Email       : {identity.get('effectiveEmail') or '<missing>'}")

    print("")
    print("FULL GREEN SOURCE")
    print("-----------------")
    try:
        marker = load_marker(root)
        ok, snap, reason = certify_matches(root, marker)
        print(f" Marker      : {marker_path(root)}")
        print(f" Created UTC : {marker.get('createdUtc') or '<unknown>'}")
        print(f" Match       : {'YES' if ok else 'NO'}")
        print(f" Paths       : {snap['pathCount']}")
        print(f" Fingerprint : {snap['fingerprint']}")
        print(f" Detail      : {reason}")
        print(f" Commit GREEN: {'ELIGIBLE' if ok else 'BLOCKED'}")
    except Exception as exc:
        print(" Match       : UNAVAILABLE")
        print(f" Detail      : {exc}")
        print(" Commit GREEN: BLOCKED")

    print("")
    print("PORCELAIN STATUS")
    print("----------------")
    if porcelain:
        for line in porcelain:
            print(line)
    else:
        print("<clean>")
    return 0


def review(root: Path) -> int:
    if not git_repo(root):
        raise GitError("Cortex is not a Git repository yet.")
    print("STATUS")
    print("------")
    print(git(root, "status", "--short", "--branch", check=False).stdout.rstrip())
    print("\nDIFF STAT")
    print("---------")
    print(git(root, "diff", "--stat", check=False).stdout.rstrip() or "<no unstaged diff>")
    print("\nSTAGED STAT")
    print("-----------")
    print(git(root, "diff", "--cached", "--stat", check=False).stdout.rstrip() or "<no staged diff>")
    return 0


def reset_operational_paths_from_index(root: Path) -> None:
    operational = [
        ".cortex",
        ".project_control",
        "artifacts",
        "logs",
        "target",
        "updates",
        "handoffs",
    ]
    for relative in operational:
        git(root, "reset", "--", relative, check=False)


def unstage_excluded_paths(root: Path) -> None:
    cp = git(root, "diff", "--cached", "--name-only", "-z", check=False)
    raw = cp.stdout
    for rel in [x for x in raw.split("\0") if x]:
        candidate = root / Path(rel)
        try:
            excluded = should_skip(root, candidate)
        except Exception:
            excluded = True
        if excluded:
            git(root, "reset", "--", rel, check=False)


def stage_governed(root: Path) -> None:
    # Stage once so deletions are represented, then explicitly unstage every path that
    # the governed-source fingerprint excludes. This keeps GREEN staging and GREEN
    # fingerprint semantics identical, including root ZIP/sidecar transport residue.
    git(root, "add", "-A")
    reset_operational_paths_from_index(root)
    unstage_excluded_paths(root)


def require_main_branch(root: Path, action: str) -> None:
    branch = current_branch(root)
    if branch != "main":
        raise GitError(f"{action} requires local branch 'main'; current branch is {branch or '<detached>'}.")


def commit_green(root: Path, message: str) -> int:
    if not git_repo(root):
        raise GitError("Cortex is not a Git repository yet.")
    require_main_branch(root, "GREEN commit")

    marker = load_marker(root)
    ok, snap, reason = certify_matches(root, marker)

    print("GREEN COMMIT PRECHECK")
    print("=====================")
    print(f" Branch      : {current_branch(root) or '<detached>'}")
    print(f" HEAD        : {(current_head(root) or '<unborn>')[:12]}")
    print(f" GREEN match : {'YES' if ok else 'NO'}")
    print(f" Paths       : {snap['pathCount']}")
    print(f" Fingerprint : {snap['fingerprint']}")
    print(f" Detail      : {reason}")

    if not ok:
        raise GitError(reason)

    identity = require_git_identity(root)
    print(f" Git author  : {identity.get('effectiveName')} <{identity.get('effectiveEmail')}> ({identity.get('source')})")

    stage_governed(root)

    staged_stat = git(root, "diff", "--cached", "--stat", check=False).stdout.strip()
    staged_names = git(root, "diff", "--cached", "--name-status", check=False).stdout.strip()

    print("")
    print("STAGED GOVERNED CHANGES")
    print("-----------------------")
    print(staged_stat or "<none>")
    if staged_names:
        print("")
        print(staged_names)

    if git(root, "diff", "--cached", "--quiet", check=False).returncode == 0:
        print("")
        print("Nothing governed is staged; no commit was created.")
        print(git(root, "status", "--short", "--branch", check=False).stdout.strip() or "<clean>")
        return 0

    git(root, "commit", "-m", message, timeout=180)
    commit_line = git(root, "log", "-1", "--oneline", check=False).stdout.strip()

    print("")
    refresh_marker_git_identity(root)

    print("GREEN COMMIT: PASS")
    print(f" Commit      : {commit_line}")
    print("")
    print("STATUS AFTER COMMIT")
    print("-------------------")
    print(git(root, "status", "--short", "--branch", check=False).stdout.strip() or "<clean>")
    return 0


def push_main(root: Path) -> int:
    if not git_repo(root):
        raise GitError("Cortex is not a Git repository yet.")
    require_main_branch(root, "Push")

    print("PUSH PRECHECK")
    print("=============")
    print(git(root, "status", "--short", "--branch", check=False).stdout.strip() or "<clean>")

    git(root, "push", "-u", "origin", "main", timeout=300)

    print("")
    print("PUSH TO origin/main: PASS")
    print(git(root, "status", "--short", "--branch", check=False).stdout.strip() or "<clean>")
    return 0


def commit_push_green(root: Path, message: str) -> int:
    commit_green(root, message)
    return push_main(root)


def fetch_main(root: Path) -> int:
    if not git_repo(root):
        raise GitError("Cortex is not a Git repository yet.")
    git(root, "fetch", "--prune", "origin", "main", timeout=300)
    print("Fetch origin/main: PASS")
    return 0


def compare_main(root: Path) -> int:
    if not git_repo(root):
        raise GitError("Cortex is not a Git repository yet.")
    if git(root, "rev-parse", "--verify", "origin/main", check=False).returncode != 0:
        raise GitError("origin/main is unavailable. Run Fetch first.")
    local = current_head(root)
    remote = git_text(root, "rev-parse", "origin/main")
    print("LOCAL / ORIGIN COMPARISON")
    print("=========================")
    print(f" Local HEAD  : {local or '<unborn>'}")
    print(f" origin/main : {remote or '<missing>'}")
    if not local:
        print(" Relationship: local branch has no commit")
        return 0
    counts = git_text(root, "rev-list", "--left-right", "--count", "HEAD...origin/main").split()
    ahead = int(counts[0]) if len(counts) == 2 and counts[0].isdigit() else 0
    behind = int(counts[1]) if len(counts) == 2 and counts[1].isdigit() else 0
    print(f" Ahead       : {ahead}")
    print(f" Behind      : {behind}")
    print("\nOUTGOING COMMITS")
    print("----------------")
    print(git(root, "log", "--oneline", "origin/main..HEAD", check=False).stdout.strip() or "<none>")
    print("\nINCOMING COMMITS")
    print("----------------")
    print(git(root, "log", "--oneline", "HEAD..origin/main", check=False).stdout.strip() or "<none>")
    return 0


def history(root: Path) -> int:
    if not git_repo(root):
        raise GitError("Cortex is not a Git repository yet.")
    print("RECENT SOURCE HISTORY")
    print("=====================")
    print(git(root, "log", "--graph", "--decorate", "--oneline", "-20", check=False).stdout.strip() or "<no commits>")
    return 0


def verify_sync(root: Path) -> int:
    if not git_repo(root):
        raise GitError("Cortex is not a Git repository yet.")
    summary = status_summary(root)
    print("SOURCE AUTHORITY VERIFICATION")
    print("=============================")
    print(f" Branch      : {summary.get('branch')}")
    print(f" HEAD        : {summary.get('headShort') or '<unborn>'}")
    print(f" Working tree: {'CLEAN' if summary.get('clean') else 'MODIFIED'}")
    print(f" Ahead/behind: {summary.get('ahead')} / {summary.get('behind')}")
    print(f" FULL GREEN  : {'MATCH' if summary.get('greenMatch') else 'NOT MATCHED'}")
    ok = bool(summary.get('clean')) and summary.get('ahead') == 0 and summary.get('behind') == 0 and bool(summary.get('greenMatch'))
    print(f" Authority   : {'GREEN / SYNCED' if ok else 'ATTENTION REQUIRED'}")
    return 0 if ok else 2


def pull_ff_only(root: Path) -> int:
    if not git_repo(root):
        raise GitError("Cortex is not a Git repository yet.")
    porcelain = git(root, "status", "--porcelain=v1", "-uall", check=False).stdout.strip()
    if porcelain:
        raise GitError("Fast-forward pull requires a clean working tree. Commit/stash/review local changes first.")
    before = snapshot(root)
    git(root, "fetch", "--prune", "origin", "main", timeout=300)
    if git(root, "rev-parse", "--verify", "origin/main", check=False).returncode != 0:
        raise GitError("origin/main could not be resolved after fetch.")
    local = current_head(root)
    remote = git_text(root, "rev-parse", "origin/main")
    if local and not (is_ancestor(root, local, remote) or is_ancestor(root, remote, local)):
        raise GitError("Local main and origin/main diverged; refusing automatic pull.")
    git(root, "merge", "--ff-only", "origin/main", timeout=300)
    after = snapshot(root)
    if local == remote and before != after:
        raise GitError("Source changed even though local/origin were already aligned; inspect repository state.")
    print("Fast-forward-only pull: PASS")
    return 0


def manual_commit(root: Path, message: str) -> int:
    if not git_repo(root):
        raise GitError("Cortex is not a Git repository yet.")
    if not message.strip():
        raise GitError("Manual commit message cannot be empty.")
    require_git_identity(root)
    stage_governed(root)
    staged = git(root, "diff", "--cached", "--quiet", check=False)
    if staged.returncode == 0:
        print("Nothing governed is staged; no commit needed.")
        return 0
    git(root, "commit", "-m", message, timeout=180)
    print("Manual governed-source commit created. This path is not GREEN-gate certified.")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Cortex canonical local Git/source-control authority.")
    parser.add_argument("action", choices=[
        "status",
        "summary-json",
        "setup",
        "repair",
        "review",
        "mark-green",
        "commit-green",
        "commit-push-green",
        "identity-status",
        "identity-set",
        "push",
        "fetch",
        "compare",
        "history",
        "verify",
        "pull",
        "manual-commit",
    ])
    parser.add_argument("--root", required=True)
    parser.add_argument("--remote", default=DEFAULT_REMOTE)
    parser.add_argument("--message", default="")
    parser.add_argument("--name", default="")
    parser.add_argument("--email", default="")
    parser.add_argument("--scope", choices=["local", "global"], default="local")
    args = parser.parse_args()

    root = Path(args.root).resolve()
    if not root.is_dir():
        raise GitError(f"Project root does not exist: {root}")

    if args.action == "mark-green":
        return mark_green(root)
    if args.action in {"setup", "repair"}:
        return setup_or_repair(root, args.remote)
    if args.action == "status":
        return show_status(root)
    if args.action == "summary-json":
        return show_summary_json(root)
    if args.action == "review":
        return review(root)
    if args.action == "commit-green":
        return commit_green(root, args.message or "Cortex GREEN checkpoint")
    if args.action == "commit-push-green":
        return commit_push_green(root, args.message or "Cortex GREEN checkpoint")
    if args.action == "identity-status":
        return show_git_identity(root)
    if args.action == "identity-set":
        return set_git_identity(root, args.name, args.email, args.scope)
    if args.action == "push":
        return push_main(root)
    if args.action == "fetch":
        return fetch_main(root)
    if args.action == "compare":
        return compare_main(root)
    if args.action == "history":
        return history(root)
    if args.action == "verify":
        return verify_sync(root)
    if args.action == "pull":
        return pull_ff_only(root)
    if args.action == "manual-commit":
        return manual_commit(root, args.message or "Cortex manual checkpoint")

    raise GitError(f"Unsupported action: {args.action}")


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except GitError as exc:
        print(f"FAIL: {exc}")
        raise SystemExit(1)
    except subprocess.TimeoutExpired as exc:
        print(f"FAIL: command timed out: {exc}")
        raise SystemExit(1)
