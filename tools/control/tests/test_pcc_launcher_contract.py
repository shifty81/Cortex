from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[3]
LAUNCHER = ROOT / "PROJECT_CONTROL_CENTER.cmd"

class LauncherContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = LAUNCHER.read_text(encoding="utf-8", errors="replace")

    def test_bare_launch_routes_to_gui(self):
        self.assertIn('if not "%~1"=="" goto :headless', self.text)
        self.assertIn('goto :launch_gui', self.text)

    def test_cli_is_explicit(self):
        self.assertIn('if /I "%~1"=="--cli" goto :interactive_cli', self.text)
        self.assertIn('CLI fallback is NOT automatic', self.text)

    def test_gui_failure_does_not_route_to_cli_or_done(self):
        section = self.text.split(':gui_fail', 1)[1].split(':done', 1)[0]
        self.assertNotIn('goto :interactive_cli', section)
        self.assertNotIn('debug-bundle', section.lower())

    def test_portable_python_is_not_backslash_quote_escaped(self):
        self.assertNotIn('set "PY_CMD=\\"', self.text)
        self.assertIn('set "PY_EXE=%CORTEX_PYTHON_EXE%"', self.text)

    def test_headless_dispatch_requires_explicit_argument(self):
        self.assertIn('call :run_python "%PCC_CORE%" %* --root "%CORTEX_ROOT%"', self.text)

if __name__ == '__main__':
    unittest.main()
