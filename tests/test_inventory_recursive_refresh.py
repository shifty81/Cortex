"""R8J-B: durable, scoped recursive reconciliation without a whole-volume rebuild."""
from __future__ import annotations
from contextlib import closing
import importlib.util
import json
import os
import threading
from pathlib import Path
import sqlite3
import tempfile
import types
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('r8jb', ROOT/'tools/control/CortexPersistentVolumeInventory.py')
index = importlib.util.module_from_spec(spec)
spec.loader.exec_module(index)


class RecursiveRefreshTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.drive = Path(self.tmp.name)/'drive'
        self.drive.mkdir()
        self.vid = 'fixture-portable-volume'
        (self.drive/'.cortex-volume.json').write_text(json.dumps({'schema':'cortex.volume.v1','volumeId':self.vid}))
        (self.drive/'vault_catalog.db').write_bytes(b'CATALOG UNTOUCHED')
        for relative in ('projects/alpha/src', 'projects/beta/docs', 'Git/repository'):
            (self.drive/relative).mkdir(parents=True)
        (self.drive/'projects/alpha/src/old.rs').write_text('old')
        (self.drive/'projects/alpha/src/stay.rs').write_text('before')
        (self.drive/'projects/beta/docs/file.txt').write_text('beta')
        (self.drive/'Git/repository/keep.txt').write_text('unrelated')
        self.db = self.drive/'.cortex/inventory'/self.vid/'inventory.sqlite3'
        self.assertEqual(index.run_scan(self.drive,self.db,volume_id=self.vid)['state'], 'complete')

    def paths(self, term=''):
        return {r['path']:r for r in index.query_entries(self.db,volume_id=self.vid,query=term)['rows']}

    def test_recursive_existing_children_reconcile_and_leave_other_tree_alone(self):
        (self.drive/'projects/alpha/src/old.rs').unlink()
        (self.drive/'projects/alpha/src/stay.rs').write_text('after with changes')
        new = self.drive/'projects/alpha/new/nested'
        new.mkdir(parents=True)
        (new/'new.dat').write_text('new item')
        result = index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative='projects/alpha')
        self.assertEqual(result['state'],'complete')
        self.assertEqual(result['pending'],0)
        self.assertGreaterEqual(result['refreshed'],4)
        self.assertNotIn('projects/alpha/src/old.rs',self.paths('old.rs'))
        self.assertEqual(self.paths('stay.rs')['projects/alpha/src/stay.rs']['size_bytes'],18)
        self.assertIn('projects/alpha/new/nested/new.dat',self.paths('new.dat'))
        self.assertIn('Git/repository/keep.txt',self.paths('keep.txt'))
        status=index.index_status(self.db,self.drive,volume_id=self.vid)
        self.assertEqual(status['state'],'complete')
        self.assertEqual(status['pending_directories'],0)
        self.assertEqual((self.drive/'vault_catalog.db').read_bytes(),b'CATALOG UNTOUCHED')
        with closing(sqlite3.connect(self.db)) as conn:
            self.assertEqual(conn.execute('PRAGMA user_version').fetchone()[0],1)

    def test_volume_root_recursive_refresh_visits_all_descendants(self):
        """The root's presentation label '.' is not its canonical queue path ''."""
        (self.drive/'projects/alpha/src/root_scope_new.txt').write_text('new')
        result=index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative='')
        self.assertEqual(result['state'],'complete')
        self.assertEqual(result['pending'],0)
        self.assertGreater(result['refreshed'],1)
        self.assertIn('projects/alpha/src/root_scope_new.txt',self.paths('root_scope_new.txt'))
        with closing(sqlite3.connect(self.db)) as conn:
            actual=conn.execute('SELECT COUNT(*) FROM entries').fetchone()[0]
        self.assertEqual(index.index_status(self.db,self.drive,volume_id=self.vid)['entries'],actual)

    def test_successful_root_retry_clears_previous_scandir_access_gap(self):
        original=index.os.scandir
        def blocked(path):
            if Path(path)==self.drive: raise PermissionError('transient root denial')
            return original(path)
        with mock.patch.object(index.os,'scandir',side_effect=blocked):
            first=index.refresh_directory(self.drive,self.db,volume_id=self.vid,relative='')
        self.assertTrue(first['refresh']['unreadable'])
        self.assertEqual(index.query_access_gaps(self.db,volume_id=self.vid,query='refresh_scandir')['total'],1)
        second=index.refresh_directory(self.drive,self.db,volume_id=self.vid,relative='')
        self.assertNotIn('unreadable',second['refresh'])
        self.assertEqual(index.query_access_gaps(self.db,volume_id=self.vid,query='refresh_scandir')['total'],0)

    def test_pause_preserves_committed_work_and_durable_queue_resumes(self):
        (self.drive/'projects/alpha/src/old.rs').unlink()
        (self.drive/'projects/alpha/newdir').mkdir()
        (self.drive/'projects/alpha/newdir/child.txt').write_text('hi')
        stop=[False]
        def progress(_): stop[0]=True
        partial=index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative='projects/alpha',
                                   progress=progress,cancelled=lambda:stop[0])
        self.assertEqual(partial['state'],'paused')
        self.assertGreater(partial['pending'],0)
        with self.assertRaisesRegex(RuntimeError, 'Finish or resume'):
            index.run_scan(self.drive,self.db,volume_id=self.vid)
        with self.assertRaisesRegex(RuntimeError,'unfinished recursive refresh'):
            index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative='Git')
        resumed=index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative=None)
        self.assertEqual(resumed['state'],'complete')
        self.assertEqual(resumed['pending'],0)
        self.assertIn('projects/alpha/newdir/child.txt',self.paths('child.txt'))
        self.assertEqual(index.index_status(self.db,self.drive,volume_id=self.vid)['pending_directories'],0)

    def test_another_worker_cannot_claim_a_live_recursive_job(self):
        first_progress=threading.Event()
        release=threading.Event()
        outcome=[]
        def progress(_):
            first_progress.set()
            if not release.wait(5): raise RuntimeError('test worker release timed out')
        def worker():
            try:
                outcome.append(index.refresh_tree(self.drive,self.db,volume_id=self.vid,
                                                 relative='projects/alpha',progress=progress))
            except BaseException as exc:
                outcome.append(exc)
        primary=threading.Thread(target=worker,daemon=True)
        primary.start()
        try:
            self.assertTrue(first_progress.wait(5),'first recursive worker did not start')
            with self.assertRaisesRegex(RuntimeError,'already active'):
                index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative=None)
        finally:
            release.set()
            primary.join(5)
        self.assertFalse(primary.is_alive())
        self.assertEqual(len(outcome),1)
        self.assertIsInstance(outcome[0],dict,outcome)
        self.assertEqual(outcome[0]['state'],'complete')

    def test_cancel_before_first_folder_retains_old_records(self):
        (self.drive/'projects/alpha/src/old.rs').unlink()
        result=index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative='projects/alpha',
                                  cancelled=lambda:True)
        self.assertEqual(result['state'],'paused')
        self.assertIn('projects/alpha/src/old.rs',self.paths('old.rs'))
        self.assertEqual(index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative=None)['state'],'complete')
        self.assertNotIn('projects/alpha/src/old.rs',self.paths('old.rs'))

    def test_root_validation_and_does_not_follow_external_symlink(self):
        for bad in ('../Git','G:/projects','projects/../Git','projects//alpha'):
            with self.subTest(bad=bad),self.assertRaises(ValueError):
                index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative=bad)
        with self.assertRaises(ValueError):
            index.refresh_tree(self.drive,self.db,volume_id='wrong-volume',relative='projects')
        dest=Path(self.tmp.name)/'external'
        dest.mkdir()
        (dest/'secret.txt').write_text('SECRET')
        link=self.drive/'projects/alpha/external'
        try:
            os.symlink(dest,link,target_is_directory=True)
        except (OSError,NotImplementedError):
            self.skipTest('symlink creation unavailable')
        result=index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative='projects/alpha')
        self.assertEqual(result['state'],'complete')
        self.assertNotIn('projects/alpha/external/secret.txt',self.paths('secret.txt'))
        self.assertEqual((dest/'secret.txt').read_text(),'SECRET')

    def test_idempotent_second_pass_and_missing_root_requires_parent(self):
        first=index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative='projects/alpha')
        self.assertEqual(first['state'],'complete')
        second=index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative='projects/alpha')
        self.assertEqual(second['state'],'complete')
        self.assertEqual(second['added'],0)
        self.assertEqual(second['removed'],0)
        self.assertEqual(second['changed'],0)
        with closing(sqlite3.connect(self.db)) as conn:
            exact = conn.execute('SELECT COUNT(*) FROM entries').fetchone()[0]
        self.assertEqual(index.index_status(self.db,self.drive,volume_id=self.vid)['entries'],exact)

    def test_deleted_queued_child_is_reconciled_through_parent(self):
        target = self.drive/'projects/alpha/src'
        stop=[False]
        part=index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative='projects/alpha',
                                progress=lambda _:stop.__setitem__(0,True),cancelled=lambda:stop[0])
        self.assertEqual(part['state'],'paused')
        for child in target.iterdir(): child.unlink()
        target.rmdir()
        resumed=index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative=None)
        self.assertEqual(resumed['state'],'complete')
        self.assertNotIn('projects/alpha/src',self.paths('projects/alpha/src'))
        self.assertEqual(index.index_status(self.db,self.drive,volume_id=self.vid)['pending_directories'],0)

    def test_unreadable_nested_directory_keeps_prior_metadata_and_reports_gap(self):
        from unittest import mock
        inner=self.drive/'projects/alpha/src'
        original=index.os.scandir
        def blocked(path):
            if Path(path)==inner: raise PermissionError('fixture blocked')
            return original(path)
        with mock.patch.object(index.os,'scandir',side_effect=blocked):
            result=index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative='projects/alpha')
        self.assertEqual(result['state'],'complete')
        self.assertEqual(result['unreadable'],1)
        self.assertIn('projects/alpha/src/old.rs',self.paths('old.rs'))
        gaps=index.query_access_gaps(self.db,volume_id=self.vid,query='refresh_scandir')['rows']
        self.assertTrue(any(x['path']=='projects/alpha/src' for x in gaps))

    def test_link_replacement_drops_stale_descendants_without_following(self):
        target=self.drive/'projects/alpha/src'
        destination=Path(self.tmp.name)/'outside-link'
        destination.mkdir()
        (destination/'secret.txt').write_text('secret')
        for item in target.iterdir(): item.unlink()
        target.rmdir()
        try:
            os.symlink(destination,target,target_is_directory=True)
        except (OSError,NotImplementedError):
            self.skipTest('symlink creation unavailable')
        result=index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative='projects/alpha')
        self.assertEqual(result['state'],'complete')
        self.assertNotIn('projects/alpha/src/old.rs',self.paths('old.rs'))
        self.assertNotIn('projects/alpha/src/secret.txt',self.paths('secret.txt'))
        self.assertEqual((destination/'secret.txt').read_text(),'secret')

    def test_transient_access_gap_clears_on_successful_retry(self):
        inner = self.drive/'projects/alpha/src'
        original = index.os.scandir
        def blocked(path):
            if Path(path) == inner:
                raise PermissionError('injected temporary denial')
            return original(path)
        with mock.patch.object(index.os, 'scandir', side_effect=blocked):
            first=index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative='projects/alpha')
        self.assertEqual(first['unreadable'], 1)
        self.assertEqual(index.query_access_gaps(self.db,volume_id=self.vid,query='refresh_scandir')['total'],1)
        second=index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative='projects/alpha')
        self.assertEqual(second['unreadable'],0)
        self.assertEqual(index.query_access_gaps(self.db,volume_id=self.vid,query='refresh_scandir')['total'],0)
        self.assertEqual(index.index_status(self.db,self.drive,volume_id=self.vid)['errors'],0)

    def test_windows_owner_detection_uses_no_os_kill(self):
        replacement=types.SimpleNamespace(name='nt',kill=mock.Mock(side_effect=AssertionError('must not signal')))
        with mock.patch.object(index,'os',replacement), mock.patch.object(index,'_windows_owner_alive',return_value=True) as probe:
            self.assertTrue(index._owner_alive(12345))
            probe.assert_called_once_with(12345)
            replacement.kill.assert_not_called()

    def test_windows_handle_probe_is_non_signalling_and_closed(self):
        class Fn:
            def __init__(self, result): self.result=result;self.calls=[]
            def __call__(self,*args): self.calls.append(args); return self.result
        class API:
            def __init__(self, handle, wait):
                self.OpenProcess=Fn(handle)
                self.WaitForSingleObject=Fn(wait)
                self.CloseHandle=Fn(True)
        alive=API(123,258)  # WAIT_TIMEOUT: still active
        self.assertTrue(index._windows_owner_alive(42,kernel32=alive,last_error=lambda:0))
        self.assertEqual(len(alive.CloseHandle.calls),1)
        self.assertEqual(alive.OpenProcess.calls[0][0],0x101000)  # query+wait only; no terminate right
        gone=API(123,0)  # WAIT_OBJECT_0: terminated
        self.assertFalse(index._windows_owner_alive(42,kernel32=gone,last_error=lambda:0))
        self.assertEqual(len(gone.CloseHandle.calls),1)
        invalid=API(None,258)
        self.assertFalse(index._windows_owner_alive(42,kernel32=invalid,last_error=lambda:87))
        denied=API(None,258)
        self.assertTrue(index._windows_owner_alive(42,kernel32=denied,last_error=lambda:5))

    def test_removed_queued_descendants_pruned_during_parent_reconciliation(self):
        nested=self.drive/'projects/alpha/src/nested/level2'
        nested.mkdir(parents=True)
        (nested/'will_disappear').write_text('gone')
        # Include the new descendants in the baseline so the old queue can contain them.
        index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative='projects/alpha')
        stop=[False]
        partial=index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative='projects/alpha',
                                   progress=lambda _:stop.__setitem__(0,True),cancelled=lambda:stop[0])
        self.assertEqual(partial['state'],'paused')
        import shutil
        shutil.rmtree(self.drive/'projects/alpha/src')
        resumed=index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative=None)
        self.assertEqual(resumed['state'],'complete')
        self.assertEqual(resumed['pending'],0)
        with closing(sqlite3.connect(self.db)) as conn:
            stale=conn.execute("SELECT COUNT(*) FROM recursive_refresh_queue WHERE path LIKE 'projects/alpha/src/%'").fetchone()[0]
        self.assertEqual(stale,0)
        self.assertNotIn('projects/alpha/src/nested/level2/will_disappear',self.paths('will_disappear'))

    def test_replaced_folder_clears_obsolete_host_access_gap(self):
        from unittest import mock
        host = self.drive / 'projects/alpha/src'
        original = index.os.scandir
        def blocked(path):
            if Path(path) == host:
                raise PermissionError('temporary fixture denial')
            return original(path)
        with mock.patch.object(index.os, 'scandir', side_effect=blocked):
            first = index.refresh_directory(self.drive, self.db, volume_id=self.vid,
                                            relative='projects/alpha/src')
        self.assertIn('unreadable', first['refresh'])
        self.assertEqual(index.query_access_gaps(self.db, volume_id=self.vid,
                         query='refresh_scandir')['total'], 1)
        for child in host.iterdir():
            child.unlink()
        host.rmdir()
        host.write_text('now a file')
        index.refresh_directory(self.drive, self.db, volume_id=self.vid,
                                relative='projects/alpha')
        self.assertEqual(index.query_access_gaps(self.db, volume_id=self.vid,
                         query='refresh_scandir')['total'], 0)
        self.assertNotIn('projects/alpha/src/old.rs', self.paths('old.rs'))
        self.assertEqual(self.paths('projects/alpha/src')['projects/alpha/src']['type'], 'file')

    def test_resumed_initial_scan_clears_recovered_directory_access_gap(self):
        # Replay one previously blocked directory without clearing the index.
        with closing(sqlite3.connect(self.db)) as conn:
            conn.execute("INSERT INTO errors(path,operation,message) VALUES(?,?,?)",
                         ('projects/alpha','scandir','prior temporary denial'))
            conn.execute("UPDATE directories SET finished=0 WHERE path='projects/alpha'")
            conn.execute("UPDATE scan_state SET state='paused',completed_utc=NULL WHERE id=1")
            conn.commit()
        result = index.run_scan(self.drive, self.db, volume_id=self.vid)
        self.assertEqual(result['state'], 'complete')
        self.assertEqual(index.query_access_gaps(self.db, volume_id=self.vid,
                         query='prior temporary denial')['total'], 0)
        self.assertEqual(index.query_access_gaps(self.db, volume_id=self.vid,
                         query='scandir')['total'], 0)
        with closing(sqlite3.connect(self.db)) as conn:
            physical=conn.execute('SELECT COUNT(*) FROM entries').fetchone()[0]
        self.assertEqual(result['entries'], physical)

    def test_explicit_abandon_only_drops_queue_and_preserves_prior_entries(self):
        before=self.paths()
        part=index.refresh_tree(self.drive,self.db,volume_id=self.vid,relative='projects/alpha',cancelled=lambda:True)
        self.assertEqual(part['state'],'paused')
        outcome=index.abandon_recursive_refresh(self.db,volume_id=self.vid)
        self.assertEqual(outcome['state'],'abandoned')
        self.assertEqual(self.paths(),before)
        self.assertEqual(index.recursive_refresh_status(self.db,volume_id=self.vid)['pending'],0)
        with self.assertRaisesRegex(RuntimeError,'No unfinished'):
            index.abandon_recursive_refresh(self.db,volume_id=self.vid)

if __name__=='__main__': unittest.main()
