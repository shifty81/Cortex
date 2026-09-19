#!/usr/bin/env python3
"""Read-only, evidence-limited ForgePY/Cortex PCC capability inventory.

This inventory deliberately cannot certify semantic feature parity. It checks
explicitly named source anchors and reports what still needs execution tests.
Never scans a drive, imports an untrusted donor, or executes project code.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

SCHEMA = "cortex.forgepy.parity_inventory.v1"
DONOR_REFERENCE = {
    "repository": "shifty81/Forge",
    "commit": "f00ca0fea2dfa29dc930fcc18ddcedc347a7c6ad",
    "identity": "ForgePY F797 published candidate (not proof of a newer local version)",
}

# A path is a discoverable source anchor, not proof that its advertised feature works.
# Any missing donor path is a missing source anchor, not proof the feature is absent.
CAPABILITIES: tuple[tuple[str, str, str, tuple[str, ...], tuple[str, ...]], ...] = (
    ("01", "Discovery / stable project registry", "P0", ("app/PCCProjectDiscovery.py",), ("tools/control/PCCSurfaceCommon.py",)),
    ("02", "Component-aware build-plan onboarding", "P0", ("app/PCCAutoAdapter.py",), ("tools/control/UniversalPCCPlan.py",)),
    ("03", "Project-owned PCC provider routing", "P0", ("app/PCCSurfaceCommon.py",), ("tools/pcc/src/pcc/control.py", "crates/cortex_pcc/src/lib.rs")),
    ("04", "Registered command / permission validation", "P0", ("app/PCCAutoAdapter.py",), ("tools/control/UniversalPCCAudit.py", "crates/cortex_project/src/lib.rs")),
    ("05", "Unified operation host / streaming", "P0", ("app/PCCOperationHost.py",), ("tools/control/CortexPCC.py",)),
    ("06", "Durable jobs / cancellation / recovery", "P0", ("app/ForgeJobs.py",), ("crates/cortex_jobs/src/lib.rs",)),
    ("07", "Exact-source Full Gate / GREEN receipt", "P0", ("tools/ForgePYGate.py",), ("tools/control/CortexPCC.py",)),
    ("08", "Guarded Git / GitHub commit and push", "P0", ("app/ForgeGitManager.py",), ("tools/control/CortexGitAuthority.py",)),
    ("09", "Local ForgeGit mirror / source history", "P1", ("app/ForgePYInternalGit.py",), ("tools/control/CortexGitAuthority.py",)),
    ("10", "Patch owner discovery / routing", "P0", ("app/ForgePYIntake.py",), ("tools/control/CortexPatchAuthority.py",)),
    ("11", "Patch safety / transactions / receipts", "P0", ("app/ForgePYPatchEngine.py",), ("tools/control/CortexPatchAuthority.py",)),
    ("12", "Patch checkpoint / multi-patch rollback", "P0", ("app/ForgePatchCheckpoint.py",), ("tools/control/CortexPatchAuthority.py",)),
    ("13", "Downloads watcher / queue / review", "P1", ("app/ForgePYIntake.py",), ("tools/control/CortexPCCGui.py",)),
    ("14", "Toolchain doctor / environment readiness", "P1", ("app/ForgeStandalone.py",), ("tools/control/CortexPCC.py",)),
    ("15", "Artifacts / package lineage", "P1", ("app/ForgeArtifactIndex.py",), ("tools/control/CortexPCCMaintenance.py",)),
    ("16", "Debug handoff / evidence collection", "P0", ("app/PCCOperationHost.py",), ("tools/control/CortexPCC.py",)),
    ("17", "Vault catalog / ownership / search", "P1", ("app/PCCVaultCatalog.py",), ("tools/control/PCCVaultCatalog.py",)),
    ("18", "Vault asset dependency resolution", "P1", ("app/ForgeAssetResolver.py",), ("crates/cortex_vault/src/lib.rs",)),
    ("19", "Backup catalog and restore", "P1", ("app/ForgeBackupRuntime.py",), ("crates/cortex_transactions/src/lib.rs",)),
    ("20", "Project GUI / live console / operations", "P1", ("app/ForgeGui.py",), ("tools/control/CortexPCCGui.py",)),
    ("21", "Source authority and governance", "P0", ("app/ForgeSourceAuthority.py",), ("tools/control/CortexGitAuthority.py",)),
    ("22", "Portable self-hosted verification", "P0", ("VerifyForgePY.cmd",), ("PROJECT_CONTROL_CENTER.cmd",)),
)


def _paths(root: Path | None, names: tuple[str, ...]) -> dict[str, bool | None]:
    if root is None:
        return {name: None for name in names}
    return {name: (root / name).is_file() for name in names}


def _json_info(root: Path | None) -> dict[str, Any]:
    if root is None:
        return {"supplied": False}
    manifest = root / "project.control.json"
    value: dict[str, Any] = {"supplied": True, "root": str(root), "manifestPresent": manifest.is_file()}
    if manifest.is_file():
        try:
            raw = json.loads(manifest.read_text(encoding="utf-8-sig"))
            if not isinstance(raw, dict):
                raise ValueError("manifest root must be an object")
            project = raw.get("project", {})
            if isinstance(project, dict):
                value["declaredProjectId"] = project.get("id")
                value["declaredBuild"] = project.get("candidateBuild") or project.get("build")
            else:
                value["declaredProjectId"] = str(project)
            value["declaredSchema"] = raw.get("schema")
        except (OSError, ValueError) as error:
            value["manifestError"] = str(error)
    return value


def inventory(cortex_root: Path, forgepy_root: Path | None = None) -> dict[str, Any]:
    """Metadata-only: no subprocesses, writes, imports, or network calls."""
    cortex_root = cortex_root.expanduser().resolve(strict=True)
    if not cortex_root.is_dir():
        raise NotADirectoryError(str(cortex_root))
    if forgepy_root is not None:
        forgepy_root = forgepy_root.expanduser().resolve(strict=True)
        if not forgepy_root.is_dir():
            raise NotADirectoryError(str(forgepy_root))
    rows: list[dict[str, Any]] = []
    for ident, title, priority, donor_names, target_names in CAPABILITIES:
        donor = _paths(forgepy_root, donor_names)
        target = _paths(cortex_root, target_names)
        target_present = all(target.values())
        donor_present = None if forgepy_root is None else all(donor.values())
        rows.append({
            "id": ident,
            "capability": title,
            "priority": priority,
            "donorSourceAnchors": donor,
            "cortexSourceAnchors": target,
            "donorSourcePresent": donor_present,
            "cortexSourcePresent": target_present,
            "status": (
                "CORTEX_ANCHOR_MISSING" if not target_present else
                "DONOR_NOT_SUPPLIED" if donor_present is None else
                "DONOR_ANCHOR_MISSING" if not donor_present else
                "SOURCE_ANCHORS_PRESENT_BEHAVIOR_UNVERIFIED"
            ),
            "behaviorCertified": False,
        })
    manifest = cortex_root / "project.control.json"
    blockers: list[str] = []
    if manifest.is_file():
        try:
            data = json.loads(manifest.read_text(encoding="utf-8-sig"))
            for command in data.get("commands", []):
                if isinstance(command, dict) and command.get("rollback") in ("git_or_snapshot", "process_stop"):
                    blockers.append(
                        f"typed rollback policy unresolved: {command.get('key', '?')} -> {command['rollback']}"
                    )
        except (OSError, ValueError, AttributeError, TypeError) as error:
            blockers.append(f"Cortex manifest unreadable: {error}")
    else:
        blockers.append("Cortex project.control.json not found")
    return {
        "schema": SCHEMA,
        "scope": "read_only_source_inventory_not_semantic_parity",
        "referenceDonor": DONOR_REFERENCE,
        "cortex": _json_info(cortex_root),
        "forgepy": _json_info(forgepy_root),
        "capabilityCount": len(rows),
        "sourceAnchorMissing": sum(not row["cortexSourcePresent"] for row in rows),
        "unverifiedBehaviorCount": len(rows),
        "blockingContractIssues": blockers,
        "parityCertified": False,
        "capabilities": rows,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Read-only ForgePY/Cortex PCC source parity inventory")
    parser.add_argument("--root", required=True, help="Cortex project root")
    parser.add_argument("--forgepy-root", help="Optional local ForgePY installation to inventory")
    parser.add_argument("--strict", action="store_true", help="Nonzero on missing Cortex anchor or invalid contract")
    args = parser.parse_args(argv)
    try:
        report = inventory(Path(args.root), Path(args.forgepy_root) if args.forgepy_root else None)
    except (OSError, ValueError) as error:
        print(json.dumps({"schema": SCHEMA, "error": str(error), "parityCertified": False}, sort_keys=True))
        return 2
    print(json.dumps(report, indent=2, sort_keys=True))
    return 2 if args.strict and (report["sourceAnchorMissing"] or report["blockingContractIssues"]) else 0


if __name__ == "__main__":
    raise SystemExit(main())
