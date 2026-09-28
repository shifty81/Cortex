from __future__ import annotations

import os
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

CONTROL = Path(__file__).resolve().parents[1]
if str(CONTROL) not in sys.path:
    sys.path.insert(0, str(CONTROL))

import CortexPythonBridge as bridge
import CortexPCCGui as gui


class RuntimeBridgeW10Tests(unittest.TestCase):
    def test_bounded_log_enumeration_never_recurses(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / 'sessions').mkdir()
            (root / 'sessions' / 'recent.log').write_text('recent', encoding='utf-8')
            (root / 'ancient.log').write_text('old', encoding='utf-8')
            (root / 'unrelated').mkdir()
            (root / 'unrelated' / 'hidden.log').write_text('ignored', encoding='utf-8')
            with mock.patch.object(Path, 'rglob', side_effect=AssertionError('unbounded traversal')):
                logs = bridge._bounded_recent_logs(root)
            self.assertIn(root / 'sessions' / 'recent.log', logs)
            self.assertNotIn(root / 'unrelated' / 'hidden.log', logs)

    def test_default_chat_worker_request_does_not_scan_logs(self) -> None:
        with mock.patch.object(bridge, 'load_messages', return_value=[]), \
             mock.patch.object(bridge, '_workspace_context', return_value='context') as context:
            answer = bridge._worker_request_text(Path('cortex'), Path('workspace'), 'cid', 'chat', 'hello')
            self.assertIn('Current user request:\nhello', answer)
            context.assert_called_with(Path('workspace'), include_logs=False)

    def test_chat_context_never_queries_build_doctor_or_git(self) -> None:
        with tempfile.TemporaryDirectory() as td, \
             mock.patch('PCCBuildDoctor.latest_failure', side_effect=AssertionError('chat must not enumerate historic logs')), \
             mock.patch.object(bridge, '_run_bounded', side_effect=AssertionError('chat must not run git')):
            root = Path(td)
            (root / '.git').mkdir()
            value = bridge._workspace_context(root, include_logs=False)
            self.assertIn('Diagnostic evidence is available', value)

    def test_repair_worker_request_retains_bounded_diagnostics(self) -> None:
        with mock.patch.object(bridge, 'load_messages', return_value=[]), \
             mock.patch.object(bridge, '_workspace_context', return_value='context') as context:
            bridge._worker_request_text(Path('cortex'), Path('workspace'), 'cid', 'repair', 'fix source')
            context.assert_called_with(Path('workspace'), include_logs=True)

    def test_ordinary_questions_and_natural_mutations_enter_common_chat_controller(self) -> None:
        self.assertEqual(gui._chat_execution_intent('What can Cortex repair?', 'chat'), ('chat', 'What can Cortex repair?'))
        self.assertEqual(gui._chat_execution_intent('Fix the failing Rust test', 'chat'), ('chat', 'Fix the failing Rust test'))
        self.assertEqual(gui._chat_execution_intent('Create a new project', 'chat'), ('chat', 'Create a new project'))
        self.assertEqual(gui._chat_execution_intent('/repair fix the build', 'chat'), ('repair', 'fix the build'))
        self.assertEqual(gui._chat_execution_intent('/plan inspect this', 'chat'), ('plan', 'inspect this'))

    def test_natural_mutation_launches_common_chat_without_python_preapproval(self) -> None:
        calls = []
        input_box = SimpleNamespace(get=lambda *_: 'Fix the compiler error', delete=lambda *_: calls.append('deleted'))
        fake = SimpleNamespace(chat_input=input_box, chat_mode_var=SimpleNamespace(get=lambda: 'Chat'),
            root_path=Path('project'), _popup=lambda *a, **k: calls.append('popup') or False,
            _append_chat_message=lambda *a: calls.append(('message', a)),
            _start_cortex_cli=lambda *a, **k: calls.append(('launch', a, k)))
        self.assertEqual(gui.CortexPCCGui._submit_chat_input(fake), 'break')
        self.assertNotIn('popup', calls)
        self.assertEqual(calls[-1], ('launch', ('chat', 'Fix the compiler error'), {'surface': 'chat'}))

    def test_approved_repair_runs_explicit_transactional_mode(self) -> None:
        calls = []
        input_box = SimpleNamespace(get=lambda *_: '/repair fix a failing test', delete=lambda *_: calls.append('deleted'))
        fake = SimpleNamespace(chat_input=input_box, chat_mode_var=SimpleNamespace(get=lambda: 'Chat'),
            root_path=Path('project'), _popup=lambda *a, **k: True,
            _append_chat_message=lambda *a: calls.append(('message',a)),
            _start_cortex_cli=lambda *a, **k: calls.append(('launch',a,k)))
        self.assertEqual(gui.CortexPCCGui._submit_chat_input(fake), 'break')
        self.assertEqual(calls[-1], ('launch', ('repair', 'fix a failing test'), {'surface': 'chat'}))

    def test_console_repair_prefix_requires_confirmation(self) -> None:
        calls=[]
        fake=SimpleNamespace(root_path=Path('project'), _popup=lambda *a, **k: False,
            _start_cortex_cli=lambda *a: calls.append(a))
        gui.CortexPCCGui._dispatch_console_input(fake, '/repair fix build')
        self.assertEqual(calls, [])


if __name__ == '__main__':
    unittest.main()
