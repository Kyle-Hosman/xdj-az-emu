#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Unit tests for tools/azemu.py that need no emulator (run in CI's repo job).

    python3 tools/test_azemu.py
"""
import importlib.util
import os
import subprocess
import sys
import tempfile
import time
import unittest

TOOLS = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, TOOLS)
import azemu  # noqa: E402

_spec = importlib.util.spec_from_file_location("azhealth", os.path.join(TOOLS, "az-emu-health.py"))
health = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(health)


class TrackCacheFailures(unittest.TestCase):
    """The deck program's track-cache failure is matched however the errno reads, so the
    health check and the smoke test catch it on the published build, not only on the
    emulator (the published build prints EINVAL, not ENOSPC)."""

    def test_einval_line_trips(self):
        # The exact line the published build repeats every ~2 s on a full /mnt.
        log = ("[   13.2] apl_start.sh[381]: Exception : Invalid argument : TrackFileCache\n"
               "[   15.3] apl_start.sh[381]: Exception : Invalid argument : TrackFileCache\n")
        self.assertEqual(azemu.track_cache_failures(log), 2)

    def test_emulator_enospc_form_trips(self):
        # The emulator's form: the marker and ENOSPC on one line still matches.
        self.assertEqual(
            azemu.track_cache_failures("ERR on TrackFileCache ENOSPC\n"), 1)

    def test_clean_journal_does_not_trip(self):
        self.assertEqual(
            azemu.track_cache_failures("Started EP147.service\nHuiProcessor up\n"), 0)

    def test_enospc_alone_does_not_trip(self):
        # An ENOSPC that is not about the track cache must not be counted.
        self.assertEqual(azemu.track_cache_failures("write failed: ENOSPC\n"), 0)

    def test_errors_are_honored(self):
        for word in azemu.TRACK_CACHE_ERRORS:
            self.assertEqual(
                azemu.track_cache_failures(f"{word} : TrackFileCache\n"), 1, word)


class WatcherStopsWhenEmulatorCloses(unittest.TestCase):
    """The health watcher must not outlive AZ-EMU: on Windows a leftover watcher held
    health.log open and reported a new boot's title from stale data."""

    def setUp(self):
        self._real = azemu.emu_running
        self._sleep = health.time.sleep
        health.time.sleep = lambda *_: None   # no real waiting in the test

    def tearDown(self):
        azemu.emu_running = self._real
        health.time.sleep = self._sleep

    def test_sleep_returns_false_at_once_when_qemu_gone(self):
        azemu.emu_running = lambda: False
        self.assertFalse(health.sleep_while_running(600))

    def test_sleep_returns_true_while_qemu_runs(self):
        azemu.emu_running = lambda: True
        self.assertTrue(health.sleep_while_running(0.01))

    def test_sleep_stops_when_qemu_disappears(self):
        calls = iter([True, True, False])
        azemu.emu_running = lambda: next(calls)
        self.assertFalse(health.sleep_while_running(600))


@unittest.skipIf(os.name == "nt", "the unix pid check; the Windows boot script has its own")
class StopPreviousWatcher(unittest.TestCase):
    """A boot stops the previous watcher by its pid file, but never a program that has
    since been given the same pid."""

    def spawn(self, *argv):
        p = subprocess.Popen(list(argv), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.addCleanup(lambda: (p.poll() is None and p.kill(), p.wait()))
        time.sleep(0.3)
        return p

    def pid_file(self, pid):
        fd, path = tempfile.mkstemp()
        with os.fdopen(fd, "w") as f:
            f.write(str(pid))
        self.addCleanup(lambda: os.path.exists(path) and os.remove(path))
        return path

    def test_unrelated_process_is_not_killed(self):
        other = self.spawn("sleep", "60")
        path = self.pid_file(other.pid)
        self.assertFalse(health.stop_previous(path))
        time.sleep(0.3)
        self.assertIsNone(other.poll(), "an unrelated process was killed")
        self.assertFalse(os.path.exists(path), "the stale pid file was kept")

    def test_previous_watcher_is_stopped(self):
        watcher = self.spawn(sys.executable, "-c", "import time; time.sleep(60)",
                             "az-emu-health.py")
        path = self.pid_file(watcher.pid)
        self.assertTrue(health.stop_previous(path))
        self.assertIsNotNone(watcher.wait(timeout=5))
        self.assertFalse(os.path.exists(path))

    def test_missing_or_garbage_file(self):
        self.assertFalse(health.stop_previous("/nonexistent/health.pid"))
        self.assertFalse(health.stop_previous(self.pid_file("not a pid")))


if __name__ == "__main__":
    unittest.main()
