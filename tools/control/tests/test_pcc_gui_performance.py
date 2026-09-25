import unittest
from pathlib import Path


class PccGuiPerformanceRegressionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = (Path(__file__).resolve().parents[1] / "CortexPCCGui.py").read_text(encoding="utf-8")

    def test_project_selection_probe_is_debounced_and_async(self):
        self.assertIn("self._project_probe_after = self.window.after(120, launch_probe)", self.text)
        self.assertIn("threading.Thread(target=work, daemon=True).start()", self.text)
        self.assertIn('self._event_q.put(("project-detail"', self.text)

    def test_vault_tree_initialization_is_lazy(self):
        self.assertIn("self._vault_tree_initialized = False", self.text)
        self.assertIn('elif name == "Vault / Forge" and not self._vault_tree_initialized:', self.text)
        self.assertIn("self.window.after(1, self._vault_refresh_tree)", self.text)


if __name__ == "__main__":
    unittest.main()
