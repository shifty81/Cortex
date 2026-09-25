from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

CONTROL = Path(__file__).resolve().parents[1]
if str(CONTROL) not in sys.path:
    sys.path.insert(0, str(CONTROL))

from CortexPCCGui import CortexPCCGui


class PortableGitIdentityGuiTests(unittest.TestCase):
    def make_gui(self, root: Path, values: dict[tuple[str, str], str]) -> CortexPCCGui:
        gui = object.__new__(CortexPCCGui)
        gui.root_path = root
        gui._git_config_value = lambda scope, key: values.get((scope, key), "")
        return gui

    def write_authority(self, root: Path, *, name="shifty81", email="50773914+shifty81@users.noreply.github.com") -> None:
        path = root / "config" / "cortex" / "github_authority.v1.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({
            "schema": "cortex.github_authority.v1",
            "repository": "shifty81/Cortex",
            "remote": "https://github.com/shifty81/Cortex.git",
            "defaultBranch": "main",
            "author": {"name": name, "email": email, "scope": "local"},
        }), encoding="utf-8")

    def test_project_authority_satisfies_gui_commit_precheck_before_local_materialization(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            self.write_authority(root)
            snap = self.make_gui(root, {})._git_identity_snapshot()
            self.assertTrue(snap["configured"])
            self.assertEqual(snap["source"], "project-authority")
            self.assertTrue(snap["authority_pending_local_apply"])
            self.assertEqual(snap["name"], "shifty81")

    def test_project_authority_outranks_machine_global_identity_for_governed_project(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            self.write_authority(root)
            values = {
                ("global", "user.name"): "Laptop User",
                ("global", "user.email"): "laptop@example.invalid",
            }
            snap = self.make_gui(root, values)._git_identity_snapshot()
            self.assertEqual(snap["source"], "project-authority")
            self.assertEqual(snap["email"], "50773914+shifty81@users.noreply.github.com")

    def test_existing_repository_local_identity_remains_authoritative(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            self.write_authority(root)
            values = {
                ("local", "user.name"): "Explicit Local",
                ("local", "user.email"): "local@example.invalid",
                ("global", "user.name"): "Global",
                ("global", "user.email"): "global@example.invalid",
            }
            snap = self.make_gui(root, values)._git_identity_snapshot()
            self.assertEqual(snap["source"], "local")
            self.assertEqual(snap["name"], "Explicit Local")
            self.assertFalse(snap["authority_pending_local_apply"])

    def test_global_identity_remains_fallback_for_ordinary_repository(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            values = {
                ("global", "user.name"): "Global",
                ("global", "user.email"): "global@example.invalid",
            }
            snap = self.make_gui(root, values)._git_identity_snapshot()
            self.assertTrue(snap["configured"])
            self.assertEqual(snap["source"], "global")


if __name__ == "__main__":
    unittest.main()
