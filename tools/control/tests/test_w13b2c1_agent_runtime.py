from __future__ import annotations

import contextlib
import io
import os
import json
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

CONTROL = Path(__file__).resolve().parents[1]
if str(CONTROL) not in sys.path:
    sys.path.insert(0, str(CONTROL))

import CortexAgentCLI as cli
import CortexAgentRuntime as runtime
import CortexPCCGui as gui
import CortexPythonBridge as bridge


class RuntimeTraceTests(unittest.TestCase):
    def test_streams_stderr_status_and_keeps_json_stdout_pristine(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            code = ('import sys,time,json; print("loading model",file=sys.stderr,flush=True);'
                    'time.sleep(.2); print(json.dumps({"text":"Hello, world!"}),flush=True)')
            shown = []
            result = runtime.run_controller([sys.executable, '-u', '-c', code], cortex_root=root,
                workspace=root, prompt='A private request', heartbeat_seconds=.05,
                timeout_seconds=4.0, reporter=shown.append)
            self.assertEqual(result.returncode, 0)
            self.assertEqual(json.loads(result.stdout)['text'], 'Hello, world!')
            self.assertTrue(any('loading model' in line for line in shown))
            self.assertFalse(any('no completed response yet' in line for line in shown))
            traces = [json.loads(line) for line in result.log_path.read_text(encoding='utf-8').splitlines()]
            self.assertEqual(traces[0]['kind'], 'started')
            self.assertEqual(traces[-1]['kind'], 'finished')
            self.assertFalse(any('A private request' in line or 'Hello, world!' in line for line in result.log_path.read_text().splitlines()))
            self.assertEqual(traces[-1]['exitCode'], 0)

    def test_live_execution_snapshot_is_projected_without_spam(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            local = root / 'localappdata'
            state_lines = [
                'schema=1', 'execution_id=exec-test', 'conversation_id=c', 'workspace_root=x',
                'mode=apply', 'phase=building', 'current=Running project validation',
                'next_expected=inspect compiler diagnostics', 'current_tool=build.project_validate',
                'current_target=Cargo.toml', 'model=Qwen-Test', 'model_role=tool',
                'agent_iteration=2', 'agent_iteration_budget=8', 'missing_dependencies=',
                'grounded_dependencies=', 'transaction_id=t', 'provider_scope=',
            ]
            prefix = "\n".join(state_lines) + "\n"
            code = (
                'import json,os,pathlib,time;'
                'base=pathlib.Path(os.environ["LOCALAPPDATA"])/"Open2D"/"Cortex"/"executions"/"fixture";'
                'base.mkdir(parents=True,exist_ok=True);'
                'now=int(time.time()*1000);'
                f'prefix={prefix!r};'
                'state=prefix+f"provider_last_activity_unix_ms={now}\\ntool_last_activity_unix_ms={now}\\nstarted_unix_ms={now}\\nupdated_unix_ms={now}\\nterminal_error=\\nowner_pid={os.getpid()}\\n";'
                '(base/"active.state").write_text(state,encoding="utf-8");'
                'time.sleep(.2);print(json.dumps({"text":"done"}),flush=True)'
            )
            shown = []
            with mock.patch.dict(os.environ, {'LOCALAPPDATA': str(local)}, clear=False):
                result = runtime.run_controller(
                    [sys.executable, '-u', '-c', code], cortex_root=root, workspace=root,
                    prompt='build it', heartbeat_seconds=.05, timeout_seconds=4.0, reporter=shown.append)
            self.assertEqual(result.returncode, 0)
            live = [line for line in shown if line.startswith('[CortexLive] ')]
            self.assertTrue(live)
            self.assertTrue(any('building' in line and 'build.project_validate' in line for line in live))
            self.assertFalse(any('no completed response yet' in line for line in shown))

    def test_spawn_error_produces_durable_failure_record(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            with mock.patch.object(runtime.subprocess, 'Popen', side_effect=FileNotFoundError('worker absent')):
                result = runtime.run_controller(['cortex.exe', 'pcc-chat'], cortex_root=root,
                    workspace=root, prompt='new project', reporter=lambda *_: None)
            self.assertEqual(result.returncode, 127)
            self.assertIn('"kind": "spawn_failed"', result.log_path.read_text())

    def test_timeout_is_explicit_failure(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            result = runtime.run_controller([sys.executable, '-u', '-c', 'import time;time.sleep(5)'],
                cortex_root=root, workspace=root, prompt='sleep', heartbeat_seconds=.05,
                timeout_seconds=.2, reporter=lambda *_: None)
            self.assertEqual(result.returncode, 124)
            self.assertTrue(result.timed_out)
            self.assertIn('"kind": "timeout"', result.log_path.read_text())

    def test_cli_dispatch_forwards_chat_to_exact_bridge_and_switches_project(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            target = root / 'new-project'
            target.mkdir()
            bridge_script = root / 'bridge.py'
            bridge_script.write_text('import sys;print("[CortexWorkspace] " + sys.argv[sys.argv.index("--workspace")+1]);print("answer")\n', encoding='utf-8')
            lines = []
            rc, workspace = cli.dispatch(bridge_script, cortex_root=root, workspace=root,
                conversation_id='x', prompt='please create', reporter=lines.append)
            self.assertEqual((rc, workspace), (0, root))
            self.assertIn('answer', lines)
            bridge_script.write_text(f'import sys;print("[CortexWorkspace] " + {str(target)!r});print("created")\n', encoding='utf-8')
            rc, workspace = cli.dispatch(bridge_script, cortex_root=root, workspace=root,
                conversation_id='x', prompt='Yes', reporter=lines.append)
            self.assertEqual((rc, workspace), (0, target))

    def test_workspace_bridge_fixture_escapes_windows_path_literals(self) -> None:
        # Windows temp paths contain \U, \t and \n sequences; inserting one
        # directly into generated Python code causes a SyntaxError or changes it.
        windows_path = r'C:\Users\Shifty\AppData\Local\Temp\tmp07w_tkbs\new-project'
        fixture = f'print("[CortexWorkspace] " + {windows_path!r})\n'
        rendered = io.StringIO()
        with contextlib.redirect_stdout(rendered):
            exec(compile(fixture, '<workspace-fixture>', 'exec'), {})
        self.assertEqual(rendered.getvalue().strip(), f'[CortexWorkspace] {windows_path}')

    def test_cli_worker_discovery_prefers_portable_shared_build_to_stale_local_target(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            shared = root / 'shared-target'
            worker = shared / 'debug' / 'cortex.exe'
            worker.parent.mkdir(parents=True)
            worker.write_bytes(b'worker')
            stale = root / 'target' / 'debug' / 'cortex.exe'
            stale.parent.mkdir(parents=True)
            stale.write_bytes(b'stale')
            with mock.patch.dict('os.environ', {'CARGO_TARGET_DIR': '', 'CORTEX_CLI_EXE': ''}), \
                 mock.patch('PCCStoragePaths.dependency_paths', return_value={'cargo_target_dir': shared}), \
                 mock.patch.object(bridge.shutil, 'which', return_value=None):
                self.assertEqual(bridge._resolve_runtime(root), worker.resolve())

    def test_gui_rejects_busy_submission_without_losing_composer(self) -> None:
        calls = []
        composer = SimpleNamespace(get=lambda *_: 'Yes', delete=lambda *_: calls.append('deleted'))
        fake = SimpleNamespace(_busy=True, chat_input=composer, chat_worker_label=SimpleNamespace(
            configure=lambda **kwargs: calls.append(kwargs)), _append_chat_message=lambda *_: calls.append('message'),
            _start_cortex_cli=lambda *_1, **_2: calls.append('launched'))
        self.assertEqual(gui.CortexPCCGui._submit_chat_input(fake), 'break')
        self.assertFalse('deleted' in calls or 'message' in calls or 'launched' in calls)


if __name__ == '__main__':
    unittest.main()
