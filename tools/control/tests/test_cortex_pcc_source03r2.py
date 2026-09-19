#!/usr/bin/env python3
"""Isolated native PCC source recovery integration tests: no real project mutations."""
import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import CortexPCC as pcc
import CortexSourceRollup as rollup

class SourceRecoveryIntegration(unittest.TestCase):
    def fixture(self):
        temporary = tempfile.TemporaryDirectory()
        root = Path(temporary.name)
        (root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
        (root / "project.control.json").write_text("{}\n", encoding="utf-8")
        (root / "sample.rs").write_text("fn main() {}\n", encoding="utf-8")
        return temporary, root

    def invoke(self, root):
        log = type("Recorder", (), {"events": [], "emit": lambda self, *a, **k: self.events.append((a, k))})()
        obj = pcc.CortexPCC.__new__(pcc.CortexPCC)
        obj.ctx = type("Context", (), {"root": root})()
        obj.log = log
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            rc = obj.create_source_rollup()
        return rc, out.getvalue(), err.getvalue(), log

    def test_cli_choice(self):
        self.assertEqual(pcc.build_parser().parse_args(["source-rollup"]).command, "source-rollup")

    def test_cli_dispatch(self):
        with patch.object(pcc.CortexPCC, "__init__", return_value=None), patch.object(pcc.CortexPCC, "create_source_rollup", return_value=17) as run:
            rc = pcc.main(["source-rollup", "--root", str(Path(__file__).resolve().parents[3])])
        self.assertEqual(rc, 17)
        run.assert_called_once_with(open_after=False)

    def test_archive_and_sidecar_verified(self):
        tmp, root = self.fixture()
        try:
            rc, out, err, events = self.invoke(root)
            self.assertEqual(rc, 0, err)
            self.assertIn("SOURCE_ROLLUP_VERIFIED=", out)
            path = Path(next(line.split("=", 1)[1] for line in out.splitlines() if line.startswith("SOURCE_ROLLUP_CREATED=")))
            self.assertEqual(path.parent, root / "artifacts" / "source-rollups")
            self.assertEqual(rollup.verify(path)["sha256"], rollup.sha_file(path))
            self.assertEqual(path.with_name(path.name + ".sha256").read_text(encoding="ascii"), f"{rollup.sha_file(path)}  {path.name}\n")
            self.assertTrue(any(args[0] == "PASS" for args, _ in events.events))
            self.assertEqual((root / "sample.rs").read_text(encoding="utf-8"), "fn main() {}\n")
        finally:
            tmp.cleanup()

    def test_sidecar_corruption_fails_closed(self):
        tmp, root = self.fixture()
        try:
            actual = rollup.create
            def corrupt(location):
                archive = actual(location)
                archive.with_name(archive.name + ".sha256").write_text("invalid", encoding="ascii")
                return archive
            with patch.object(pcc.source_rollup, "create", side_effect=corrupt):
                rc, out, err, events = self.invoke(root)
            self.assertEqual(rc, 1)
            self.assertNotIn("SOURCE_ROLLUP_CREATED=", out)
            self.assertIn("SOURCE_ROLLUP_FAILED=", err)
            self.assertEqual(list((root / "artifacts" / "source-rollups").glob("*.zip")), [])
            self.assertTrue(any(args[0] == "FAIL" for args, _ in events.events))
        finally:
            tmp.cleanup()

    def test_export_failure_not_success(self):
        tmp, root = self.fixture()
        try:
            with patch.object(pcc.source_rollup, "create", side_effect=rollup.RollupError("fixture blocked")):
                rc, out, err, _ = self.invoke(root)
            self.assertEqual(rc, 1)
            self.assertNotIn("SOURCE_ROLLUP_CREATED=", out)
            self.assertIn("fixture blocked", err)
        finally:
            tmp.cleanup()

    def test_preserves_actual_menu_and_routes(self):
        text = Path(pcc.__file__).read_text(encoding="utf-8")
        for token in ('print(" 70 Create + verify source recovery archive")', 'elif choice == "70": self.create_source_rollup(open_after=True)', 'print("17 Create + verify source recovery archive")', 'elif choice == "17": self.create_source_rollup(open_after=True)', 'elif choice == "43": self.git.action("setup")', '"source-rollup"'):
            self.assertIn(token, text)

if __name__ == "__main__":
    unittest.main()
