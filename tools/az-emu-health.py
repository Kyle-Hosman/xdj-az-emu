#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""az-emu-health.py - check the running emulator for the problems that break it quietly.

    python3 tools/az-emu-health.py                 check now, print what is wrong
    python3 tools/az-emu-health.py --wait 300      first wait for a booting emulator
    python3 tools/az-emu-health.py --watch 10      check again every minute for 10 minutes

It looks for:
  - too little free space on the host disk for the emulated deck storage to grow into
    (--host-only checks only this, without an emulator; --image says which image);
  - an emulator image older than this checkout (the build stamp ./build.sh writes);
  - a full deck storage: the deck program preallocates five track caches (9.5 GiB) on
    the reserve partition and fails on it ("TrackFileCache", the errno varies) when
    they do not fit,
    so the library and the panel never come up;
  - a missing OneLibrary key flag (the stick's library cannot open);
  - the deck program not running.

AZ_EMU_HOST_NEED_GB sets the free space the host check wants (for a test on a short
disk); everything else is checked as usual.

Each problem is one line ending with its fix. They are written to health-status.txt in
the socket folder, one "WARN <line>" each, which AZ-EMU.app shows in its title bar; the
file is removed when nothing is wrong. Exit status: 0 healthy, 1 a problem, 2 no emulator.
The boot scripts run it in the background at every start (log: health.log there). It
writes its pid to health.pid and stops as soon as the emulator's QEMU is gone, so it
never outlives a closed AZ-EMU (which would hold the log open and report stale state).
"""
import argparse
import os
import signal
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import azemu  # noqa: E402

STATUS_FILE = os.path.join(azemu.SOCK_DIR, "health-status.txt")
SERIAL_LOG = os.path.join(azemu.SOCK_DIR, "serial.log")
PID_FILE = os.path.join(azemu.SOCK_DIR, "health.pid")
# The deck program allocates its caches within its first minute; check after that.
SETTLE_S = 60


def write_pid():
    try:
        os.makedirs(azemu.SOCK_DIR, exist_ok=True)
        with open(PID_FILE, "w") as f:
            f.write(str(os.getpid()))
    except OSError:
        pass


def clear_pid():
    try:
        if os.path.exists(PID_FILE):
            os.remove(PID_FILE)
    except OSError:
        pass


def is_watcher(pid):
    """True only when `pid` is a running az-emu-health.py. A pid file can outlive the
    process that wrote it, and the OS can hand that number to an unrelated program."""
    try:
        args = subprocess.run(["ps", "-p", str(int(pid)), "-o", "args="],
                              capture_output=True, text=True, timeout=10).stdout
    except (OSError, ValueError, subprocess.SubprocessError):
        return False
    return "az-emu-health.py" in args


def stop_previous(pid_file=None):
    """Stop the watcher a previous AZ-EMU left behind, but only if the pid in its pid
    file is still that watcher; otherwise the stale file is just removed. Returns True
    when a watcher was stopped."""
    pid_file = pid_file or PID_FILE
    try:
        with open(pid_file) as f:
            pid = int(f.read().strip())
    except (OSError, ValueError):
        pid = None
    stopped = False
    if pid and pid != os.getpid() and is_watcher(pid):
        try:
            os.kill(pid, signal.SIGTERM)
            stopped = True
        except OSError:
            pass
    try:
        os.remove(pid_file)
    except OSError:
        pass
    return stopped


def sleep_while_running(seconds):
    """Sleep up to `seconds` in short steps, returning early (False) the moment the
    emulator's QEMU has gone; True when the whole time passed and it is still up."""
    end = time.time() + seconds
    while time.time() < end:
        if not azemu.emu_running():
            return False
        time.sleep(min(5.0, max(0.0, end - time.time())))
    return azemu.emu_running()


def record(problems):
    """Tell AZ-EMU.app; a failure here never matters."""
    try:
        if problems:
            os.makedirs(azemu.SOCK_DIR, exist_ok=True)
            with open(STATUS_FILE, "w") as f:
                f.writelines(f"WARN {p}\n" for p in problems)
        elif os.path.exists(STATUS_FILE):
            os.remove(STATUS_FILE)
    except OSError:
        pass


def check():
    problems = azemu.health(serial_log=os.environ.get("SERIAL_LOG", SERIAL_LOG))
    stamp = time.strftime("%H:%M:%S")
    for p in problems or ["healthy"]:
        print(f"{stamp} {p}", flush=True)
    record(problems)
    return problems


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--wait", type=int, default=0, metavar="S",
                    help="wait up to S seconds for the guest and its deck program first")
    ap.add_argument("--watch", type=int, default=0, metavar="MIN",
                    help="check again every minute for MIN minutes")
    ap.add_argument("--host-only", action="store_true",
                    help="check only the host's free space for the disk image")
    ap.add_argument("--stop-previous", nargs="?", const=PID_FILE, metavar="PIDFILE",
                    help="stop a watcher left by a previous AZ-EMU (only if its pid is still one)")
    ap.add_argument("--image", default=azemu.MNT_IMG, help="the disk image (default: %(default)s)")
    args = ap.parse_args()
    if args.stop_previous:
        stop_previous(args.stop_previous)
        return 0
    if args.host_only:
        problem = azemu.host_space_problem(args.image)
        if problem:
            print("warning:", problem, flush=True)
        return 1 if problem else 0
    write_pid()
    try:
        if args.wait:
            t0 = time.time()
            while azemu.ssh("pidof EP147", timeout=30)[0] != 0:
                if not azemu.emu_running():
                    return 2   # the emulator was closed before the deck program came up
                if time.time() - t0 > args.wait:
                    print("no emulator answered within", args.wait, "s", flush=True)
                    return 2
                time.sleep(3)
            if not sleep_while_running(SETTLE_S):
                return 0
        problems = check()
        for _ in range(args.watch):
            if not sleep_while_running(60):
                break   # the emulator was stopped: nothing more to watch
            problems = check()
        return 1 if problems else 0
    finally:
        clear_pid()


if __name__ == "__main__":
    sys.exit(main())
