from __future__ import annotations

import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]


class W13B2C2RollupCorrectionsTests(unittest.TestCase):
    def text(self, rel: str) -> str:
        return (ROOT / rel).read_text(encoding="utf-8")

    def test_toolchain_broker_keeps_windows_environment_case_insensitive(self) -> None:
        source = self.text("tools/control/PCCSharedEnvironment.py")
        self.assertIn('BROKER_VERSION = "PCC-TOOLCHAIN-BROKER-0.5"', source)
        self.assertIn("def _merge_environment_overlay(", source)
        self.assertIn('canonical = "PATH" if folded == "path" else key', source)
        self.assertNotIn("matches[0] if matches else key", source)
        self.assertIn('"npm_config_cache": str(paths["npm_cache"])', self.text("tools/control/PCCVaultStorage.py"))
        self.assertIn("_merge_environment_overlay(result, captured)", source)

    def test_portable_project_environment_owns_rustup_and_build_paths(self) -> None:
        source = self.text("tools/control/PCCPortableProjectEnvironment.py")
        self.assertIn('BROKER_VERSION="PCC-PORTABLE-PROJECT-ENV-0.6"', source)
        self.assertIn('rustup=shared/"toolchains"/"rust"/"rustup-home"', source)
        self.assertIn('"RUSTUP_HOME":str(rustup)', source)
        self.assertIn('"RUSTUP_AUTO_INSTALL":"0"', source)
        self.assertIn('_prepend(out,cargo/"bin")', source)
        self.assertIn('"CARGO_TARGET_DIR":str(target)', source)

    def test_r4_bounded_new_project_classifier_is_present(self) -> None:
        source = self.text("crates/cortex_desktop_core/src/lib.rs")
        self.assertIn("let qualified_project = words.iter().enumerate()", source)
        self.assertIn("words[index.saturating_sub(5)..index]", source)
        self.assertIn('"build this project with a new feature"', source)
        self.assertNotIn("explicit_new_project_qualifier", source)

    def test_r6_clippy_only_runtime_helpers_are_test_gated(self) -> None:
        source = self.text("crates/cortex_desktop_core/src/lib.rs")
        for name in (
            "m11u2_prompt_requests_runtime",
            "m11u2_prompt_requests_window",
            "m11u2_prompt_requests_title_change",
        ):
            self.assertIn(f"#[cfg(test)]\n    fn {name}", source)

    def test_r6_debug_bundle_contains_bounded_command_diagnostics(self) -> None:
        source = self.text("tools/control/CortexPCC.py")
        self.assertIn('work / "command-output" / src.name', source)
        self.assertIn("self.log.command_output_dir.glob", source)
        self.assertIn("copied_bytes + size > 4 * 1024 * 1024", source)
        self.assertIn("safe_phase = re.sub", source)

    def test_r4_portable_rust_formatter_is_present(self) -> None:
        source = self.text("tools/control/CortexCanonicalRustFormat.py")
        self.assertIn("resolve_toolchain_environment", source)
        self.assertIn('[cargo, "fmt", "--all"]', source)
        self.assertIn('env["RUSTUP_AUTO_INSTALL"] = "0"', source)

    def test_new_project_classifier_and_route_fast_lane_are_single_and_explicit(self) -> None:
        source = self.text("crates/cortex_desktop_core/src/lib.rs")
        marker = "explicit new standalone project language must outrank generic create/build/code cues"
        self.assertEqual(source.count(marker), 1)
        self.assertIn('"standalone"', source)
        self.assertIn("let qualified_project = words.iter().enumerate()", source)
        self.assertIn("DeveloperIntent::NewProject", source)
        self.assertIn("route.new_project.conversation.start", source)
        self.assertIn("new_project.pending.persist.done", source)
        self.assertIn("Runtime proof required:", source)


if __name__ == "__main__":
    unittest.main()
