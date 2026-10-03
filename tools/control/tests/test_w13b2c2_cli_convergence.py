from __future__ import annotations

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[3]


class CliConvergenceP1Tests(unittest.TestCase):
    def text(self, relative: str) -> str:
        return (ROOT / relative).read_text(encoding="utf-8")

    def test_runtime_injects_portable_authority_without_forcing_project_target_dir(self) -> None:
        source = self.text("tools/control/CortexAgentRuntime.py")
        self.assertIn("_PORTABLE_AUTHORITY_KEYS", source)
        self.assertIn("env=worker_env", source)
        helper = source[source.index("_PORTABLE_AUTHORITY_KEYS"):source.index("@dataclass(frozen=True)")]
        self.assertIn('"CORTEX_HOME"', helper)
        self.assertIn('"CORTEX_VAULT_ROOT"', helper)
        self.assertNotIn('"CARGO_TARGET_DIR"', helper)

    def test_natural_pcc_chat_routes_before_provider_preflight(self) -> None:
        source = self.text("tools/control/CortexPythonBridge.py")
        main = source[source.index("def main() -> int:"):]
        local_route = main.index('if runtime is not None and args.mode == "chat":')
        provider = main.index("_ensure_provider_ready(")
        self.assertLess(local_route, provider)
        self.assertIn("provider=lazy-until-required", main)

    def test_pcc_chat_exposes_live_stage_boundary(self) -> None:
        source = self.text("crates/cortex_cli/src/lib.rs")
        start = source.index("fn pcc_chat_command(")
        block = source[start:source.index("\nfn desktop_command(", start)]
        for stage in (
            "pcc_chat.start",
            "controller.open.start",
            "controller.open.done",
            "request.route.start",
            "request.route.done",
            "pcc_chat.done",
        ):
            self.assertIn(stage, block)

    def test_embedded_new_project_uses_fresh_authoritative_conversation(self) -> None:
        source = self.text("crates/cortex_desktop_core/src/lib.rs")
        self.assertIn(
            "intent == DeveloperIntent::NewProject && !self.owns_runtime_lifetime",
            source,
        )
        self.assertIn("route.new_project.conversation.start", source)
        self.assertIn("new_project.proposal.append.done", source)
        self.assertIn("new_project.pending.persist.done", source)

    def test_embedded_controller_skips_desktop_only_heavy_initialization(self) -> None:
        source = self.text("crates/cortex_desktop_core/src/lib.rs")
        self.assertIn("Embedded PCC/CLI turns", source)
        self.assertIn("Embedded PCC/CLI turns keep Vault memory lazy", source)
        self.assertIn("full Desktop overview is presentation state", source)


if __name__ == "__main__":
    unittest.main()
