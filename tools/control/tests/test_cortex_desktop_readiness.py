"""PCC desktop-startup truth: tests never launch a real GUI or provider."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from CortexDesktopReadiness import wait_for_visible_window


class Clock:
    def __init__(self):
        self.t = 0.0

    def now(self):
        return self.t

    def sleep(self, delta):
        self.t += delta


class DesktopReadinessTests(unittest.TestCase):
    def observe(self, poll=lambda: None, finder=lambda pid: None, **overrides):
        clock = Clock()
        arguments = dict(platform="win32", timeout=1.0, interval=.25,
                         clock=clock.now, sleep=clock.sleep,
                         find_window=finder)
        arguments.update(overrides)
        return wait_for_visible_window(2049, poll, **arguments)

    def test_visible_window_is_not_chat_ready(self):
        result = self.observe(finder=lambda pid: 900 if pid == 2049 else None)
        self.assertEqual(result.status, "WINDOW_VISIBLE")
        self.assertEqual(result.window_handle, 900)
        self.assertIn("NOT certified", result.detail)

    def test_early_exit_zero_is_still_failure(self):
        result = self.observe(poll=lambda: 0)
        self.assertEqual(result.status, "EARLY_EXIT")
        self.assertEqual(result.exit_code, 0)

    def test_early_exit_nonzero(self):
        self.assertEqual(self.observe(poll=lambda: 101).exit_code, 101)

    def test_no_visible_window_is_not_launch_success(self):
        result = self.observe()
        self.assertEqual(result.status, "NO_VISIBLE_WINDOW")
        self.assertIn("may still be running", result.detail)

    def test_poll_error_fails_closed(self):
        def broken():
            raise OSError("access denied")
        self.assertEqual(self.observe(poll=broken).status, "PROBE_ERROR")

    def test_window_probe_error_fails_closed(self):
        def broken(pid):
            raise OSError("window station unavailable")
        self.assertEqual(self.observe(finder=broken).status, "PROBE_ERROR")

    def test_nonwindows_is_not_certified(self):
        self.assertEqual(self.observe(platform="linux").status, "UNSUPPORTED_HOST")

    def test_rejects_unbounded_or_invalid_values(self):
        for args in ({"timeout": -1}, {"interval": 0}):
            with self.subTest(args=args), self.assertRaises(ValueError):
                self.observe(**args)

    def test_delayed_visible_window(self):
        count = [0]
        def delayed(pid):
            count[0] += 1
            return 777 if count[0] == 3 else None
        self.assertEqual(self.observe(finder=delayed).window_handle, 777)

    def test_no_global_process_or_shell_execution(self):
        import inspect
        import CortexDesktopReadiness as module
        source = inspect.getsource(module)
        self.assertNotIn("subprocess.Popen", source)
        self.assertNotIn("os.system", source)
        self.assertNotIn("TerminateProcess", source)


if __name__ == "__main__":
    unittest.main()
