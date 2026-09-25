from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

CONTROL = Path(__file__).resolve().parents[1]
if str(CONTROL) not in sys.path:
    sys.path.insert(0, str(CONTROL))

import CortexGitAuthority as git_authority


class PortableGitCheckoutTrustTests(unittest.TestCase):
    def test_probe_classifies_dubious_ownership_as_untrusted_checkout(self) -> None:
        cp = subprocess.CompletedProcess(
            args=["git"], returncode=128, stdout="",
            stderr="fatal: detected dubious ownership in repository at 'G:/Cortex'\nTo add an exception, call: git config --global --add safe.directory G:/Cortex",
        )
        with tempfile.TemporaryDirectory() as td, patch.object(git_authority, "git", return_value=cp):
            root = Path(td)
            (root / ".git").mkdir()
            probe = git_authority.git_repo_probe(root)
        self.assertFalse(probe["ready"])
        self.assertEqual(probe["blocker"], "UNTRUSTED_CHECKOUT")

    def test_require_git_repo_preserves_actual_checkout_trust_failure(self) -> None:
        with patch.object(git_authority, "git_repo_probe", return_value={
            "ready": False,
            "blocker": "UNTRUSTED_CHECKOUT",
            "detail": "fatal: detected dubious ownership in repository at 'G:/Cortex'",
        }):
            with self.assertRaisesRegex(git_authority.GitError, "Trust Current Checkout"):
                git_authority.require_git_repo(Path("G:/Cortex"))

    def test_trust_checkout_adds_only_exact_path_then_requires_successful_probe(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td).resolve()
            git_dir = root / ".git"
            git_dir.mkdir()
            (git_dir / "config").write_text(
                '[remote "origin"]\n\turl = https://github.com/shifty81/Cortex.git\n',
                encoding="utf-8",
            )
            calls: list[list[str]] = []

            def fake_run(cmd, **kwargs):
                calls.append([str(x) for x in cmd])
                if "--file" in cmd:
                    return subprocess.CompletedProcess(cmd, 0, "https://github.com/shifty81/Cortex.git\n", "")
                if "--get-all" in cmd:
                    return subprocess.CompletedProcess(cmd, 1, "", "")
                return subprocess.CompletedProcess(cmd, 0, "", "")

            with patch.object(git_authority, "git_repo_probe", side_effect=[
                {"ready": False, "blocker": "UNTRUSTED_CHECKOUT", "detail": "dubious ownership"},
                {"ready": True, "blocker": "", "detail": ""},
            ]), patch.object(git_authority, "run", side_effect=fake_run):
                rc = git_authority.trust_checkout(root, "https://github.com/shifty81/Cortex.git")

            self.assertEqual(rc, 0)
            safe_calls = [c for c in calls if "safe.directory" in c]
            self.assertTrue(any("--add" in c and str(root) in c for c in safe_calls))
            self.assertFalse(any("*" in c for c in safe_calls))

    def test_surface_exposes_explicit_trust_command_and_gate_checks_git_ready(self) -> None:
        pcc = (CONTROL / "CortexPCC.py").read_text(encoding="utf-8")
        gui = (CONTROL / "CortexPCCGui.py").read_text(encoding="utf-8")
        surface = (CONTROL / "PCCSurfaceCommon.py").read_text(encoding="utf-8")
        self.assertIn('"git-trust"', pcc)
        self.assertIn('"git-trust": "trust"', pcc)
        self.assertIn('if not bool(payload.get("gitReady")):', pcc)
        self.assertIn('"Trust Current Checkout"', gui)
        self.assertIn('"git-trust"', gui)
        self.assertIn('"git-trust": "Trust Current Checkout"', surface)


if __name__ == "__main__":
    unittest.main()
