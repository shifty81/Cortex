#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import re
from pathlib import Path
from typing import Any

DISCOVERY_VERSION = "PCC-DISCOVERY-0.3"
RISK_ALIASES = {
    "read": "read_only",
    "write": "local_mutation",
    "confirm": "local_mutation",
}


def _safe_json(path: Path) -> dict[str, Any] | None:
    try:
        data = json.loads(path.read_text(encoding="utf-8-sig"))
    except Exception:
        return None
    return data if isinstance(data, dict) else None


def _risk(item: dict[str, Any]) -> str:
    raw = str(item.get("risk") or "").strip()
    if raw:
        return RISK_ALIASES.get(raw, raw)
    if bool(item.get("requiresConfirmation")) or bool(item.get("mutates")):
        return "local_mutation"
    return "read_only"


def _canonicalize_contract(root: Path, data: dict[str, Any]) -> dict[str, Any]:
    """Normalize currently-known project.control.json shapes without rewriting the project.

    Older project PCCs used root-level id/name/type/executable/arguments fields while newer
    Cortex contracts use a nested project object and program/args.  The universal PCC accepts
    both and exposes one stable in-memory model.
    """
    project_obj = data.get("project") if isinstance(data.get("project"), dict) else {}
    project_id = str(
        project_obj.get("id")
        or data.get("id")
        or data.get("projectId")
        or root.name
    ).strip()
    name = str(project_obj.get("name") or data.get("name") or root.name).strip()
    kind = str(
        project_obj.get("kind")
        or data.get("kind")
        or data.get("type")
        or _detect_kind(root)
    ).strip()

    commands: list[dict[str, Any]] = []
    for item in data.get("commands", []) or []:
        if not isinstance(item, dict):
            continue
        key = str(item.get("key") or "").strip()
        if not key:
            continue
        program = str(item.get("program") or item.get("executable") or "").strip()
        args_raw = item.get("args") if item.get("args") is not None else item.get("arguments")
        args = [str(x) for x in (args_raw or [])]
        commands.append({
            "key": key,
            "label": str(item.get("label") or key),
            "risk": _risk(item),
            "program": program,
            "args": args,
            "category": str(item.get("category") or "").strip(),
            "mutates": bool(item.get("mutates")),
            "requiresConfirmation": bool(item.get("requiresConfirmation")),
        })

    gate_keys: list[str] = []
    for item in data.get("quality_gates", []) or []:
        if isinstance(item, dict) and item.get("key"):
            gate_keys.append(str(item["key"]))
    if not gate_keys:
        gate_keys = [
            cmd["key"] for cmd in commands
            if cmd["category"].casefold() == "gate" or cmd["key"].casefold().startswith("gate.")
        ]

    root_control = data.get("root_control_center") if isinstance(data.get("root_control_center"), dict) else {}
    root_launcher = str(
        root_control.get("launcher")
        or data.get("rootLauncher")
        or data.get("root_launcher")
        or ""
    ).strip()
    state_dir = str(data.get("stateDirectory") or data.get("state_directory") or "").strip()

    normalized = dict(data)
    normalized["project"] = {"id": project_id, "name": name, "kind": kind}
    normalized["commands"] = commands
    normalized["quality_gates"] = [{"key": key} for key in gate_keys]
    normalized["root_control_center"] = {
        **root_control,
        "launcher": root_launcher,
    }
    normalized["stateDirectory"] = state_dir
    normalized["_pccDiscovery"] = {
        "version": DISCOVERY_VERSION,
        "source": "project.control.json",
        "contractShape": "nested" if project_obj else "root-level",
    }
    return normalized


def _detect_kind(root: Path) -> str:
    if (root / "Cargo.toml").is_file():
        return "rust-workspace"
    if (root / "CMakeLists.txt").is_file() or (root / "engine" / "CMakeLists.txt").is_file():
        return "native-cpp"
    if (root / "gradlew").is_file() or (root / "gradlew.bat").is_file() or (root / "build.gradle").is_file():
        return "java-gradle"
    if (root / "pyproject.toml").is_file() or (root / "requirements.txt").is_file():
        return "python"
    if (root / "package.json").is_file():
        return "node"
    return "project"


_ACTION_TO_KEY = {
    "status": "project.status",
    "project-status": "project.status",
    "health": "project.health",
    "environment-status": "project.environment",
    "environment": "project.environment",
    "control-center-self-test": "project.self-test",
    "self-test": "project.self-test",
    "full-gate": "gate.full",
    "clean-full-gate": "gate.clean-full",
    "fast-gate": "gate.fast",
    "quick-gate": "gate.fast",
    "build": "build.native",
    "build-render": "build.native",
    "build-headless": "build.headless",
    "test": "test.native",
    "run-game": "run.game",
    "run-client": "run.game",
    "run-editor": "run.editor",
    "run-shipyard": "run.shipyard",
    "run-smoke": "run.smoke",
    "patch-status": "patch.status",
    "preview-inbox": "patch.preview",
    "apply-inbox": "patch.apply",
    "undo-last-patch": "recovery.undo-last",
    "root-audit": "audit.root",
    "pass-continuity": "audit.continuity",
    "source-authority": "source.authority",
    "debug-bundle": "diagnostics.bundle",
    "source-rollup": "package.source-rollup",
    "asset-rollup": "package.asset-rollup",
    "capture-baseline": "package.baseline",
    "incremental-handoff": "package.incremental",
    "artifact-index": "artifacts.index",
    "dependency-status": "dependencies.status",
    "git-status": "git.status",
    "git-history": "git.history",
    "git-commit-green": "git.commit-green",
    "git-push": "git.push",
    "git-pull": "git.pull",
    "git-repair": "git.repair-working-copy",
    "repo-authority-audit": "repo.audit",
    "repo-authority-prepare": "repo.prepare",
    "repo-authority-publish": "repo.publish",
    "open-latest-debug": "diagnostics.open-latest",
}


def _action_key(action: str) -> str:
    return _ACTION_TO_KEY.get(action.casefold(), f"legacy.{action.strip().lower().replace('_', '-')}")


def _action_category(key: str) -> str:
    return key.split(".", 1)[0] if "." in key else "project"



_NATIVE_MANIFEST_CANDIDATES = (
    Path("project/forgepy.project.json"),
    Path("forgepy.project.json"),
    Path("project/pcc.project.json"),
)
_NATIVE_ENTRYPOINT_CANDIDATES = (
    Path("PCC.cmd"),
    Path("PROJECT_CONTROL_CENTER.cmd"),
    Path("ProjectControlCenter.cmd"),
    Path("ProjectControlCenter.py"),
    Path("ForgePY.py"),
)


def _flatten_strings(value: Any) -> list[str]:
    out: list[str] = []
    if isinstance(value, str):
        out.append(value)
    elif isinstance(value, dict):
        for key, item in value.items():
            out.append(str(key))
            out.extend(_flatten_strings(item))
    elif isinstance(value, list):
        for item in value:
            out.extend(_flatten_strings(item))
    return out


def _native_provider_text(root: Path, manifest: Path | None, entrypoint: Path | None) -> str:
    chunks: list[str] = []
    if manifest is not None:
        data = _safe_json(manifest)
        if data is not None:
            chunks.extend(_flatten_strings(data))
    for candidate in (entrypoint, root / "ProjectControlCenter.py", root / "ForgePY.py"):
        if candidate is None or not candidate.is_file():
            continue
        try:
            chunks.append(candidate.read_text(encoding="utf-8-sig", errors="replace")[:120000])
        except OSError:
            pass
    return "\n".join(chunks).casefold()


def _native_command_program(root: Path, entrypoint: Path) -> tuple[str, list[str]]:
    rel = entrypoint.relative_to(root).as_posix()
    if entrypoint.suffix.casefold() == ".py":
        return "python", [rel]
    return rel, []


def _discover_native_project_pcc(root: Path) -> dict[str, Any] | None:
    """Discover an existing project-owned PCC before synthesizing build-system commands.

    Native project authority is intentionally bridged through the universal auto-adapter: the
    project keeps ownership of its own PCC semantics while Cortex supplies process hosting,
    shared dependency/cache environment, logging and GUI routing.  Discovery is read-only.
    """
    manifest: Path | None = None
    for rel in _NATIVE_MANIFEST_CANDIDATES:
        candidate = root / rel
        if candidate.is_file():
            manifest = candidate
            break

    entrypoint: Path | None = None
    for rel in _NATIVE_ENTRYPOINT_CANDIDATES:
        candidate = root / rel
        if candidate.is_file():
            entrypoint = candidate
            break

    # A manifest is authority evidence, but Cortex still needs an executable project-owned
    # control entrypoint to route operations without inventing semantics.
    if entrypoint is None:
        return None

    manifest_data = _safe_json(manifest) if manifest is not None else None
    provider_text = _native_provider_text(root, manifest, entrypoint)
    program, prefix = _native_command_program(root, entrypoint)

    def native(key: str, label: str, args: list[str], *, category: str, mutates: bool = False, confirm: bool = False) -> dict[str, Any]:
        return _command(key, label, program, [*prefix, *args], category=category, mutates=mutates, confirm=confirm)

    # These are the stable project-PCC verbs used by Forge/PCC projects.  They are routed to
    # the project's own entrypoint; Cortex does not replace them with cargo/cmake/npm gates.
    commands = [
        native("project.status", "Project PCC status", ["status"], category="project"),
        native("gate.full", "Full project quality gate", ["full"], category="gate", mutates=True),
        native("build.native", "Build through project PCC", ["build"], category="build", mutates=True),
        native("test.native", "Test through project PCC", ["test"], category="test"),
    ]

    # Add conventional operations only when there is evidence for them, keeping discovery
    # deterministic and avoiding unsupported commands on older project control centers.
    if any(token in provider_text for token in ("quick", "fast-gate", "fast gate", "gate.fast")):
        verb = "quick" if "quick" in provider_text else "fast"
        commands.append(native("gate.fast", "Fast project gate", [verb], category="gate"))
    if "audit" in provider_text:
        commands.append(native("audit.project", "Project audit", ["audit"], category="audit"))
    if "update" in provider_text or "patch" in provider_text:
        commands.append(native("patch.apply", "Apply governed project updates", ["updates", "apply"], category="updates", mutates=True, confirm=True))
    if any(token in provider_text for token in ("debug-bundle", "debug bundle", "diagnostics")):
        debug_args = ["debug-bundle"] if "debug-bundle" in provider_text else ["diagnostics", "bundle"]
        commands.append(native("diagnostics.bundle", "Create project debug bundle", debug_args, category="diagnostics"))
    if "run" in provider_text:
        run_args = ["run", "dx12"] if "dx12" in provider_text else ["run"]
        commands.append(native("run.runtime", "Run project through native PCC", run_args, category="run", mutates=True))

    project_name = root.name
    project_id = root.name.lower().replace(" ", "-")
    project_kind = _detect_kind(root)
    if isinstance(manifest_data, dict):
        project_obj = manifest_data.get("project") if isinstance(manifest_data.get("project"), dict) else {}
        project_name = str(project_obj.get("name") or manifest_data.get("name") or project_name).strip()
        project_id = str(project_obj.get("id") or manifest_data.get("id") or project_id).strip()
        project_kind = str(project_obj.get("kind") or manifest_data.get("kind") or project_kind).strip()

    manifest_rel = manifest.relative_to(root).as_posix() if manifest is not None else ""
    entry_rel = entrypoint.relative_to(root).as_posix()
    return {
        "project": {"id": project_id, "name": project_name, "kind": project_kind},
        "commands": commands,
        "quality_gates": [{"key": item["key"]} for item in commands if str(item.get("key") or "").startswith("gate.")],
        "root_control_center": {
            "launcher": entry_rel if entrypoint.suffix.casefold() in {".cmd", ".bat"} else "",
            "native_entrypoint": entry_rel,
            "manifest": manifest_rel,
        },
        "stateDirectory": "",
        "_pccDiscovery": {
            "version": DISCOVERY_VERSION,
            "source": "native-project-pcc",
            "provider": entry_rel,
            "manifest": manifest_rel,
            "entrypoint": entry_rel,
            "authority": "native-project-pcc",
            "fallback": _detect_kind(root),
            "explicitProvider": True,
            "inferredCommands": 0,
        },
    }

def _powershell_candidates(root: Path) -> list[Path]:
    candidates: list[Path] = []
    explicit = [
        root / "tools" / "control" / "ProjectControlCenter.ps1",
        root / "tools" / "control" / "ControlCenter.ps1",
    ]
    candidates.extend(path for path in explicit if path.is_file())
    control_dir = root / "tools" / "control"
    if control_dir.is_dir():
        candidates.extend(sorted(control_dir.glob("*ControlCenter.ps1")))
    candidates.extend(sorted(root.glob("*Tools.ps1")))
    candidates.extend(sorted(root.glob("*ControlCenter.ps1")))
    seen: set[str] = set()
    unique: list[Path] = []
    for path in candidates:
        key = os.path.normcase(str(path.resolve()))
        if key not in seen:
            seen.add(key)
            unique.append(path)
    return unique


def _extract_actions(ps1: Path) -> list[str]:
    try:
        text = ps1.read_text(encoding="utf-8-sig", errors="replace")[:60000]
    except OSError:
        return []
    # Prefer the ValidateSet directly attached to the $Action parameter.
    pattern = re.compile(
        r"\[ValidateSet\((?P<body>.*?)\)\]\s*\[string\]\s*\$Action\b",
        re.IGNORECASE | re.DOTALL,
    )
    match = pattern.search(text)
    if not match:
        # Some scripts put attributes/whitespace between ValidateSet and the type.
        match = re.search(
            r"\[ValidateSet\((?P<body>.*?)\)\].{0,240}?\$Action\b",
            text,
            re.IGNORECASE | re.DOTALL,
        )
    if not match:
        return []
    values = re.findall(r"['\"]([^'\"]+)['\"]", match.group("body"))
    out: list[str] = []
    seen: set[str] = set()
    for value in values:
        value = value.strip()
        if value and value.casefold() not in seen:
            seen.add(value.casefold())
            out.append(value)
    return out


def _discover_from_powershell(root: Path) -> tuple[list[dict[str, Any]], str, str]:
    for script in _powershell_candidates(root):
        actions = _extract_actions(script)
        if not actions:
            continue
        rel = script.relative_to(root).as_posix()
        commands: list[dict[str, Any]] = []
        for action in actions:
            if action.casefold() == "menu":
                continue
            key = _action_key(action)
            mutates = any(token in key for token in ("build", "apply", "commit", "push", "repair", "package", "capture", "publish"))
            args = ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", rel, "-Action", action]
            commands.append({
                "key": key,
                "label": action.replace("-", " ").title(),
                "risk": "local_mutation" if mutates else "read_only",
                "program": "powershell",
                "args": args,
                "category": _action_category(key),
                "mutates": mutates,
                "requiresConfirmation": any(token in key for token in ("apply", "commit", "push", "repair", "publish")),
            })
        launcher = ""
        for candidate in (root / "PROJECT_CONTROL_CENTER.cmd", root / (script.stem + ".cmd")):
            if candidate.is_file():
                launcher = candidate.relative_to(root).as_posix()
                break
        return commands, rel, launcher
    return [], "", ""


def _discover_name(root: Path, ps_script: str = "") -> str:
    stem = Path(ps_script).stem if ps_script else ""
    for suffix in ("ControlCenter", "ProjectControlCenter", "Tools"):
        if stem.casefold().endswith(suffix.casefold()) and len(stem) > len(suffix):
            return stem[: -len(suffix)].replace("_", " ").replace("-", " ").strip()
    return root.name



def _command(key: str, label: str, program: str, args: list[str], *, category: str | None = None, mutates: bool = False, confirm: bool = False, risk: str | None = None) -> dict[str, Any]:
    return {
        "key": key,
        "label": label,
        "risk": RISK_ALIASES.get(risk, risk) if risk else ("local_mutation" if mutates else "read_only"),
        "program": program,
        "args": [str(x) for x in args],
        "category": category or _action_category(key),
        "mutates": bool(mutates),
        "requiresConfirmation": bool(confirm),
    }


def _merge_commands(primary: list[dict[str, Any]], fallback: list[dict[str, Any]]) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    seen: set[str] = set()
    for item in [*primary, *fallback]:
        key = str(item.get("key") or "").strip()
        if not key or key.casefold() in seen:
            continue
        seen.add(key.casefold())
        out.append(item)
    return out


def _infer_generic_commands(root: Path) -> list[dict[str, Any]]:
    """Create a safe local adapter when a project has no explicit PCC contract.

    This never writes into the project.  It derives only commands that can be justified by
    standard build markers already present in the repository.  Explicit project contracts
    and root utilities always outrank these inferred fallbacks.
    """
    commands: list[dict[str, Any]] = []
    commands.append(_command("project.status", "Project / Git status", "__pcc_internal__", ["status"], category="project"))
    commands.append(_command("project.health", "Detected project health", "__pcc_internal__", ["health"], category="project"))
    if (root / ".git").exists():
        commands.extend([
            _command("git.status", "Git status", "git", ["-C", "{root}", "status", "--short", "--branch"], category="git"),
            _command("git.history", "Git history", "git", ["-C", "{root}", "log", "--oneline", "--decorate", "--graph", "-20"], category="git"),
            _command("git.push", "Push current branch", "git", ["-C", "{root}", "push"], category="git", mutates=True, confirm=True),
            _command("git.pull", "Pull fast-forward only", "git", ["-C", "{root}", "pull", "--ff-only"], category="git", mutates=True, confirm=True),
        ])

    if (root / "Cargo.toml").is_file():
        commands.extend([
            _command("gate.full", "Full Rust quality gate", "__pcc_internal__", ["rust-full-gate"], category="gate", mutates=True),
            _command("gate.fast", "Fast Rust development gate", "cargo", ["check", "--workspace", "--all-targets"], category="gate"),
            _command("build.native", "Build Rust workspace", "cargo", ["build", "--workspace"], category="build", mutates=True),
            _command("test.native", "Test Rust workspace", "cargo", ["test", "--workspace", "--all-targets"], category="test"),
        ])

    gradle = root / "gradlew.bat" if (root / "gradlew.bat").is_file() else (root / "gradlew" if (root / "gradlew").is_file() else None)
    if gradle is not None:
        program = str(gradle.name)
        commands.extend([
            _command("gate.full", "Full Gradle quality gate", program, ["check"], category="gate", mutates=True),
            _command("gate.fast", "Fast Gradle check", program, ["test"], category="gate"),
            _command("build.native", "Gradle build", program, ["build"], category="build", mutates=True),
            _command("test.native", "Gradle tests", program, ["test"], category="test"),
        ])

    if (root / "package.json").is_file():
        try:
            package = _safe_json(root / "package.json") or {}
            scripts = package.get("scripts") if isinstance(package.get("scripts"), dict) else {}
        except Exception:
            scripts = {}
        runner = "npm"
        if "test" in scripts:
            commands.append(_command("test.native", "NPM tests", runner, ["test", "--"], category="test"))
        if "build" in scripts:
            commands.append(_command("build.native", "NPM build", runner, ["run", "build"], category="build", mutates=True))
        if "lint" in scripts:
            commands.append(_command("gate.fast", "NPM lint", runner, ["run", "lint"], category="gate"))
        if "build" in scripts and "test" in scripts:
            commands.append(_command("gate.full", "Full Node quality gate", "__pcc_internal__", ["node-full-gate"], category="gate", mutates=True))
        for run_name in ("dev", "start"):
            if run_name in scripts:
                commands.append(_command("run.runtime", f"NPM {run_name}", runner, ["run", run_name], category="run"))
                break

    if (root / "pyproject.toml").is_file() or (root / "requirements.txt").is_file():
        commands.extend([
            _command("gate.fast", "Python compile check", "python", ["-m", "compileall", "-q", "."], category="gate"),
            _command("test.native", "Python unittest discovery", "python", ["-m", "unittest", "discover"], category="test"),
        ])

    cmake_root = root if (root / "CMakeLists.txt").is_file() else (root / "engine" if (root / "engine" / "CMakeLists.txt").is_file() else None)
    if cmake_root is not None:
        # Build-only fallbacks are intentionally conservative: configure details vary heavily.
        for build_dir_name in ("build", "Build", "build-debug", "Builds/windows-x64-debug", "engine/build"):
            build_dir = root / build_dir_name
            if build_dir.is_dir():
                commands.append(_command("build.native", "Build existing CMake tree", "cmake", ["--build", str(build_dir)], category="build", mutates=True))
                break

    return _merge_commands([], commands)


def _infer_project_name(root: Path, fallback: str) -> str:
    # Gradle settings are a particularly useful source for JSON-less Java projects.
    for settings in (root / "settings.gradle", root / "settings.gradle.kts"):
        if settings.is_file():
            try:
                text = settings.read_text(encoding="utf-8-sig", errors="replace")[:20000]
                match = re.search(r"rootProject\.name\s*=\s*['\"]([^'\"]+)['\"]", text)
                if match:
                    return match.group(1).strip()
            except OSError:
                pass
    return fallback

def discover_project_contract_data(root: Path) -> dict[str, Any]:
    root = root.expanduser().resolve()

    # 1) Explicit typed/untyped project contract is always authoritative.
    contract_path = root / "project.control.json"
    if contract_path.is_file():
        data = _safe_json(contract_path)
        if data is not None:
            normalized = _canonicalize_contract(root, data)
            commands = list(normalized.get("commands") or [])
            if commands:
                if not normalized.get("quality_gates"):
                    normalized["quality_gates"] = [
                        {"key": c["key"]} for c in commands if str(c.get("key") or "").startswith("gate.")
                    ]
                discovery = normalized.setdefault("_pccDiscovery", {})
                discovery.update({
                    "inferredCommands": 0,
                    "authority": "project.control.json",
                    "fallback": _detect_kind(root),
                    "provider": str((normalized.get("root_control_center") or {}).get("machine_provider") or "project.control.json"),
                })
                return normalized

    # 2/3) Existing Forge/PCC project authority outranks every build-system fallback.
    native = _discover_native_project_pcc(root)
    if native is not None:
        return native

    # 4) Existing project-local PowerShell control center outranks cargo/cmake/npm inference.
    commands, ps_script, launcher = _discover_from_powershell(root)
    if commands:
        kind = _detect_kind(root)
        name = _infer_project_name(root, _discover_name(root, ps_script))
        return {
            "project": {"id": root.name.lower().replace(" ", "-"), "name": name, "kind": kind},
            "commands": commands,
            "quality_gates": [{"key": c["key"]} for c in commands if c["key"].startswith("gate.")],
            "root_control_center": {"launcher": launcher, "discoveredPowerShell": ps_script},
            "stateDirectory": "",
            "_pccDiscovery": {
                "version": DISCOVERY_VERSION,
                "source": "project-powershell-pcc",
                "provider": ps_script,
                "entrypoint": ps_script,
                "authority": "project-powershell-pcc",
                "fallback": kind,
                "explicitProvider": True,
                "inferredCommands": 0,
            },
        }

    # 5) Only projects with no existing control authority receive a build-system adapter.
    inferred = _infer_generic_commands(root)
    kind = _detect_kind(root)
    name = _infer_project_name(root, root.name)
    markers = {
        "cargo": (root / "Cargo.toml").is_file(),
        "cmake": (root / "CMakeLists.txt").is_file() or (root / "engine" / "CMakeLists.txt").is_file(),
        "gradle": (root / "gradlew").is_file() or (root / "gradlew.bat").is_file(),
        "python": (root / "pyproject.toml").is_file(),
        "node": (root / "package.json").is_file(),
        "git": (root / ".git").exists(),
    }
    return {
        "project": {"id": root.name.lower().replace(" ", "-"), "name": name, "kind": kind},
        "commands": inferred,
        "quality_gates": [{"key": c["key"]} for c in inferred if c["key"].startswith("gate.")],
        "root_control_center": {"launcher": "", "discoveredPowerShell": ""},
        "stateDirectory": "",
        "_pccDiscovery": {
            "version": DISCOVERY_VERSION,
            "source": "filesystem-scan",
            "markers": markers,
            "provider": "generated-local-adapter",
            "authority": "generated-build-system-adapter",
            "fallback": kind,
            "explicitProvider": False,
            "inferredCommands": len(inferred),
        },
    }


def discovery_summary(root: Path) -> dict[str, Any]:
    data = discover_project_contract_data(root)
    discovery = data.get("_pccDiscovery") or {}
    commands = data.get("commands") or []
    project = data.get("project") or {}
    return {
        "name": project.get("name") or root.name,
        "kind": project.get("kind") or "project",
        "source": discovery.get("source") or "unknown",
        "provider": discovery.get("provider") or (data.get("root_control_center") or {}).get("launcher") or "",
        "commands": len(commands),
        "commandKeys": [str(x.get("key")) for x in commands if isinstance(x, dict) and x.get("key")],
        "markers": discovery.get("markers") or {},
        "authority": discovery.get("authority") or discovery.get("source") or "unknown",
        "manifest": discovery.get("manifest") or "",
        "entrypoint": discovery.get("entrypoint") or discovery.get("provider") or "",
        "fallback": discovery.get("fallback") or _detect_kind(root),
    }
