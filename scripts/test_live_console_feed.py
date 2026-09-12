"""CPU-only regression checks for optional live status publication."""
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import live_console_feed as feed


class StatusPublicationTests(unittest.TestCase):
    def test_failed_warning_sink_does_not_raise(self):
        with patch('builtins.print', side_effect=BrokenPipeError('closed log')):
            feed.report_status_warning('status destination is locked')
            feed.report_status_warning(None)

    @unittest.skipUnless(os.name == 'nt', 'requires Windows sharing semantics')
    def test_locked_status_keeps_old_snapshot_and_recovers_after_unlock(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            run = root / 'run'
            run.mkdir()
            with patch.object(feed.time, 'time', return_value=1):
                self.assertEqual(feed.publish_status(run, {'tick': 1}), [])
            destination = root / 'live-status.json'
            original = destination.read_bytes()
            kernel = ctypes.WinDLL('kernel32', use_last_error=True)
            kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD,
                wintypes.DWORD, wintypes.LPVOID, wintypes.DWORD, wintypes.DWORD,
                wintypes.HANDLE]
            kernel.CreateFileW.restype = wintypes.HANDLE
            kernel.CloseHandle.argtypes = [wintypes.HANDLE]
            # Allow reads/writes but deny deletion/replacement, as a transient
            # Windows reader can do. No mocked error is used for this case.
            handle = kernel.CreateFileW(str(destination), 0x80000000, 3,
                None, 3, 0x80, None)
            self.assertNotEqual(handle, ctypes.c_void_p(-1).value)
            try:
                with patch.object(feed.time, 'time', return_value=2):
                    failures = feed.publish_status(run, {'tick': 2})
                self.assertEqual(len(failures), 1)
                self.assertEqual(destination.read_bytes(), original)
                self.assertEqual(json.loads((run / 'status.json').read_text())['tick'], 2)
            finally:
                kernel.CloseHandle(handle)
            with patch.object(feed.time, 'time', return_value=3):
                self.assertEqual(feed.publish_status(run, {'tick': 3}), [])
            self.assertEqual(json.loads(destination.read_text()), {'tick': 3, 'published_ms': 3000})

    def test_persistent_storage_failure_does_not_raise_or_refresh_old_status(self):
        with tempfile.TemporaryDirectory() as directory:
            run = Path(directory) / 'run'
            run.mkdir()
            self.assertEqual(feed.publish_status(run, {'tick': 1}), [])
            before = (run.parent / 'live-status.json').read_bytes()
            with patch.object(feed.os, 'replace', side_effect=OSError('storage unavailable')):
                for tick in range(2, 12):
                    self.assertEqual(len(feed.publish_status(run, {'tick': tick})), 2)
            self.assertEqual((run.parent / 'live-status.json').read_bytes(), before)


if __name__ == '__main__':
    unittest.main()
