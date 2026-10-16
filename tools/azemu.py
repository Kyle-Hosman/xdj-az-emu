# SPDX-License-Identifier: GPL-3.0-only
"""Shared helpers for the xdj-az-emu tools: where things live, and SSH into the guest.

Every tool that talks to the running emulator goes through `ssh()` here, so there is
exactly one place that knows the port, the address and the key.

- SSH is key-only. `./build.sh` generates a key pair for THIS install in
  build/ssh/ and bakes the public half into the image. There is no password login.
- The guest's SSH port is forwarded to 127.0.0.1:2222 (IPv4 on purpose: "localhost"
  can resolve to ::1 first, and QEMU's forward is IPv4 only).
- The automation QMP socket is 127.0.0.1:4446. 4445 belongs to AZ-EMU.app; QEMU allows
  one client per QMP socket, so tools never use 4445.
"""
import os
import re
import subprocess
import sys
import time

WIN = os.name == "nt"
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BUILD = os.environ.get("AZ_EMU_BUILD", os.path.join(REPO, "build"))
SSH_KEY = os.environ.get("AZ_EMU_SSH_KEY", os.path.join(BUILD, "ssh", "az-emu_ed25519"))
SSH_PORT = int(os.environ.get("AZ_EMU_SSH_PORT", "2222"))
QMP = ("127.0.0.1", int(os.environ.get("QMP_PORT", "4446")))
# Per-boot sockets: az-boot-m1.sh uses /tmp/az-emu-m1, az-boot-win.ps1 %TEMP%\az-emu-win.
SOCK_DIR = (os.path.join(os.environ.get("TEMP", "."), "az-emu-win") if WIN else "/tmp/az-emu-m1")
CTRL_SOCK = os.path.join(SOCK_DIR, "ctrl.sock")
# Windows: always Windows' own OpenSSH, whatever the shell's PATH says (Git Bash puts
# Git's ssh first). build.sh sets the key's permissions the way that client checks them.
_WIN_SSH = os.path.join(os.environ.get("SystemRoot", r"C:\Windows"), "System32", "OpenSSH", "ssh.exe")
SSH_EXE = _WIN_SSH if WIN and os.path.isfile(_WIN_SSH) else "ssh"


def _ssh_base():
    if not os.path.isfile(SSH_KEY):
        sys.exit(f"No emulator SSH key at {SSH_KEY}.\n"
                 "Run ./build.sh first: it creates a key for this install and puts it in the image.")
    null = "NUL" if WIN else "/dev/null"
    return [SSH_EXE, "-i", SSH_KEY, "-p", str(SSH_PORT),
            "-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes",
            "-o", "StrictHostKeyChecking=no", "-o", f"UserKnownHostsFile={null}",
            "-o", "LogLevel=ERROR", "-o", "ConnectTimeout=20", "root@127.0.0.1"]


def ssh(cmd, data=None, timeout=120):
    """Run `cmd` in the guest. Returns (returncode, stdout text). rc -1 = timed out."""
    try:
        r = subprocess.run(_ssh_base() + [cmd], input=data, capture_output=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return -1, ""
    return r.returncode, r.stdout.decode(errors="replace")


def wait_for_guest(seconds):
    """Wait until SSH answers and EP147 (the deck's program) is running."""
    t0 = time.time()
    while ssh("pidof EP147", timeout=30)[0] != 0:
        if time.time() - t0 > seconds:
            return False
        time.sleep(3)
    return True


def emu_running():
    """True while this emulator's QEMU runs on the host. A transient error is treated as
    running, so a watcher never stops on a hiccup."""
    if WIN:
        try:
            out = subprocess.run(["tasklist", "/FI", "IMAGENAME eq qemu-system-aarch64.exe", "/NH"],
                                 capture_output=True, text=True, timeout=20).stdout
            return "qemu-system-aarch64" in out
        except (OSError, subprocess.SubprocessError):
            return True
    try:
        return subprocess.run(["pgrep", "-f", "qemu-system-aarch64"],
                              stdout=subprocess.DEVNULL, timeout=20).returncode == 0
    except (OSError, subprocess.SubprocessError):
        return True


def restart_ep147(wait=180, check=None):
    """Restart the deck's program and wait until it runs again (and `check`, a guest
    shell test, passes). Returns the seconds it took, or None on timeout."""
    ssh("systemctl restart EP147.service", timeout=60)
    t0 = time.time()
    test = "pidof EP147 >/dev/null" + (f" && {check}" if check else "")
    while time.time() - t0 < wait:
        time.sleep(3)
        if ssh(test, timeout=30)[0] == 0:
            return time.time() - t0
    return None


def push_file(local, remote, mode="755"):
    """Copy a file into the guest the safe way: write `remote`.new, then mv over.

    Never write over a library the running program has loaded: changing a loaded
    file in place changes its code under the program and crashes it. A rename
    leaves the old file alive for the process that has it mapped."""
    with open(local, "rb") as f:
        data = f.read()
    d = os.path.dirname(remote)
    rc, _ = ssh(f"mkdir -p '{d}' && cat > '{remote}.new' && chmod {mode} '{remote}.new' && "
                f"mv -f '{remote}.new' '{remote}'", data=data)
    return rc == 0


def guest_md5(paths):
    rc, out = ssh("md5sum " + " ".join(f"'{p}'" for p in paths) + " 2>/dev/null")
    have = {}
    for line in out.splitlines():
        h, _, n = line.partition("  ")
        have[n.strip()] = h
    return have


# ---- the deck's USB loader (images pulled from a deck with AZ-MODS, firmware 2.01) ----
STAGE = "/home/root/settings/az-usbload"   # the loader's stage on the settings partition
UNSIGNED = "/home/root/settings/azmods-unsigned.on"   # the deck's developer switch
LOADER_LOG = "/tmp/az-usbload.log"
# the loader's last line for a run: what it decided about EP147
# The crash-loop guard's lines that mean it acted (mods disabled, stock staged, or tripped
# during the apply). Its other lines, such as the core-dump limit it writes before an apply,
# are progress, not a decision.
CRASH_LOOP = r"crash-loop guard(: .*(disabling mods|crash-looping)| tripped)"
LOADER_DECISIONS = ("EP147 back after", "same as loaded", "NOT now", "NOT restarting",
                    "did NOT come back", CRASH_LOOP)
MAPPED = ('P=""; for p in /proc/[0-9]*; do case "$(readlink $p/exe 2>/dev/null)" in */pdj/EP147) P=${p#/proc/};; esac; done; '
          '[ -n "$P" ] || exit 1; for so in azmod azstem; do r=$(grep -E "/$so(-core)?\\.so$" /proc/$P/maps | head -1 | cut -d" " -f1); '
          'if [ -n "$r" ]; then echo "$so $(md5sum /proc/$P/map_files/$r | cut -d" " -f1)"; else echo "$so none"; fi; done')


def has_loader():
    """True when the image boots the deck's USB loader (patch 31 writes the marker)."""
    return ssh("test -e /etc/az-emu-mods.loader", timeout=30)[0] == 0


def mapped():
    """{azmod, azstem}: md5 of what the running EP147 has mapped ("none" when not)."""
    rc, out = ssh(MAPPED, timeout=30)
    return dict(l.split() for l in out.splitlines() if len(l.split()) == 2) if rc == 0 else {}


def loader_decision(seconds=240):
    """Wait for the loader's decision line in this boot's log. None = no decision in time."""
    pat = "|".join(LOADER_DECISIONS)
    t0 = time.time()
    while time.time() - t0 < seconds:
        rc, out = ssh(f"grep -E '{pat}' {LOADER_LOG} 2>/dev/null | tail -1", timeout=30)
        if out.strip():
            return out.strip()
        time.sleep(3)
    return None


def run_loader(seconds=240):
    """Run the deck's loader against the stick now, the emulator's stand-in for the power
    cycle a deck needs after its stick changed, and return its decision line (None: none in
    time). The loader is the deck's own; it reads the stick, checks the set and writes the
    stage and the env exactly as at boot."""
    ssh(f"rm -f {LOADER_LOG}; systemctl restart az-usbload.service", timeout=60)
    return loader_decision(seconds)


def apply_staged(decision):
    """What a user does after the loader ran: on "Reload to apply." (a staged set the running
    EP147 has not loaded), press Reload, which restarts EP147. Returns a short note, or exits
    when the loader reports a failure."""
    if decision is None:
        sys.exit(f"the deck's USB loader made no decision within 240 s (tools/az-ssh cat {LOADER_LOG})")
    if "did NOT come back" in decision or re.search(CRASH_LOOP, decision):
        sys.exit("the loader reports: " + decision)
    refused = ssh(f"grep -E 'no genuinely signed trust.pins' {LOADER_LOG} | tail -1", timeout=30)[1].strip()
    if refused:
        print("loader:", refused)
    if "NOT restarting" in decision or "NOT now" in decision:
        pend = ssh("cat /run/az-reload-pending 2>/dev/null | head -1", timeout=30)[1].strip()
        print(f"the set is staged and waits for a Reload (/run/az-reload-pending: {pend or 'absent'}); "
              "reloading now, as the Mod Menu's Reload does (daemon-reload, restart EP147)")
        ssh("systemctl daemon-reload", timeout=60)
        if restart_ep147() is None:
            sys.exit("EP147 did not come back within 180 s after the Reload")
        return "reloaded"
    return "applied by the loader"


# ---- whether the image matches this checkout, and whether the guest is healthy -----
# The image's build stamp: build.sh writes it into the rootfs as "patches <hash>" (this
# checkout's initramfs-patch/) and "guest <hash>" (the guest/ and docker/ sources
# build/docker-out/ was built from; "unknown" for a docker-out older than the stamp).
IMAGE_STAMP = "/etc/az-emu-build"
PATCH_DIRS = ("initramfs-patch",)
GUEST_DIRS = ("guest", "docker")
# The deck program preallocates five track caches on the reserve partition (/mnt),
# LocalFileCache0-4.bin at 2042626048 bytes each (9.5 GiB together, the same on a deck's
# 28.5 GB partition), and the deck's emmc-fs.sh reformats that partition at every boot
# when less than 2 GiB of it is free. Less than this much free means it cannot settle.
MNT_MIN_FREE_KB = 2 * 1024 * 1024
TRACK_CACHES = 5 * 2042626048
# When the reserve partition is full the deck program fails to write its track cache and
# repeats the failure every ~2 s. The errno in the line is not fixed: the published build
# prints "Exception : Invalid argument : TrackFileCache" (EINVAL), the emulator shows
# "on TrackFileCache" with "ENOSPC". So a failure line is matched by the TrackFileCache
# marker together with any of these error words, never by ENOSPC alone.
TRACK_CACHE_ERRORS = ("Exception", "Invalid argument", "ENOSPC", "No space")
ONELIBRARY_FLAG = "/home/root/settings/azmod-onelibrarykey.on"
MNT_IMG = os.environ.get("AZ_MNT_IMG", os.path.join(BUILD, "az-mnt.img"))


def source_hash(*dirs):
    """A hash of the files under `dirs` (relative to the repository): their paths and
    contents, line endings normalized, so a Windows checkout hashes as a Mac one does.
    Build outputs and caches are skipped."""
    import hashlib
    h = hashlib.sha256()
    for top in dirs:
        for root, subdirs, files in os.walk(os.path.join(REPO, top)):
            subdirs[:] = sorted(d for d in subdirs
                                if not d.startswith(".") and d not in ("__pycache__", "out"))
            for name in sorted(files):
                if name.startswith(".") or name.endswith(".pyc"):
                    continue
                path = os.path.join(root, name)
                rel = os.path.relpath(path, REPO).replace(os.sep, "/")
                with open(path, "rb") as f:
                    data = f.read().replace(b"\r\n", b"\n")
                h.update(rel.encode() + b"\0" + str(len(data)).encode() + b"\0" + data)
    return h.hexdigest()[:16]


def stamp_problem(stamp):
    """What is wrong with an image whose build stamp reads `stamp` (the file's text,
    "" when it has none), in words with the fix; None when it matches this checkout."""
    have = dict(line.split(None, 1) for line in stamp.splitlines() if len(line.split()) == 2)
    if have.get("guest") != source_hash(*GUEST_DIRS):
        return "the emulator image is older than this checkout: run ./build.sh"
    if have.get("patches") != source_hash(*PATCH_DIRS):
        return "the emulator image is older than this checkout: run ./build.sh --image-only"
    return None


def track_cache_failures(text):
    """Count the deck program's track-cache failure lines in `text`, whatever the errno
    reads: a line that names TrackFileCache and carries one of [`TRACK_CACHE_ERRORS`].
    A clean log (no such line) returns 0."""
    return sum(1 for line in text.splitlines()
               if "TrackFileCache" in line and any(e in line for e in TRACK_CACHE_ERRORS))


def host_space_problem(image=MNT_IMG):
    """The disk image is sparse: it grows on the host as the deck program writes its
    track caches, up to about their size. A line with the fix when the host disk holding
    `image` cannot take the rest of that plus 1 GiB; None when it can."""
    import shutil
    try:
        used = os.stat(image).st_blocks * 512 if not WIN else 0
        free = shutil.disk_usage(os.path.dirname(os.path.abspath(image))).free
    except (OSError, AttributeError):
        return None
    need = max(0, TRACK_CACHES - used) + (1 << 30)
    if os.environ.get("AZ_EMU_HOST_NEED_GB"):   # for tests on a short disk
        need = int(float(os.environ["AZ_EMU_HOST_NEED_GB"]) * 2**30)
    if free >= need:
        return None
    return (f"only {free / 2**30:.1f} GB free on the disk holding {os.path.basename(image)}; "
            f"the emulated deck storage can grow by {need / 2**30:.1f} GB as tracks play: "
            "free some space")


def health(serial_log=None):
    """What is wrong with the running emulator, as short lines that each end with the
    fix ([] when nothing is). `serial_log`: the host's copy of the guest console, which
    outlives the guest journal's short window."""
    rc, out = ssh(f"cat {IMAGE_STAMP} 2>/dev/null; echo ==; "
                  "df -k /mnt | awk 'NR==2 {print $2, $4}'; echo ==; "
                  "journalctl -b --no-pager 2>/dev/null | grep -F TrackFileCache | "
                  "grep -c -E '" + "|".join(TRACK_CACHE_ERRORS) + "'; echo ==; "
                  f"ls {ONELIBRARY_FLAG} {ONELIBRARY_FLAG[:-3]}.off 2>/dev/null; echo ==; "
                  "pidof EP147", timeout=60)
    if rc not in (0, 1):
        return ["the emulator does not answer over SSH: quit AZ-EMU and open it again"]
    parts = (out.split("==\n") + [""] * 5)[:5]
    stamp, mnt, nospace, flag, pid = (p.strip() for p in parts)
    problems = []
    image = stamp_problem(stamp)
    if image:
        problems.append(image)
    try:
        size_kb, free_kb = (int(v) for v in mnt.split())
    except ValueError:
        size_kb, free_kb = 0, 0
    if serial_log:
        try:
            with open(serial_log, "rb") as f:
                nospace_host = track_cache_failures(f.read().decode(errors="replace"))
        except OSError:
            nospace_host = 0
    else:
        nospace_host = 0
    full = size_kb and free_kb < MNT_MIN_FREE_KB
    if full or int(nospace or 0) or nospace_host:
        fix = ("quit AZ-EMU and open it again (it enlarges build/az-mnt.img)"
               if size_kb < 14 * 1024 * 1024 else
               "stop the emulator and delete build/az-mnt.img (made again at the next start)")
        problems.append(f"the emulated deck storage is full ({free_kb // 1024} MB free of "
                        f"{size_kb // 1024} MB; the deck program cannot write its track cache): {fix}")
    if not flag:
        problems.append("the OneLibrary key flag is missing, so the stick's library cannot "
                        "open: run ./build.sh --image-only")
    if not pid:
        problems.append("the deck program is not running: quit AZ-EMU and open it again")
    host = host_space_problem()
    if host:
        problems.append(host)
    # The title bar shows them in this order and may cut the last ones short: what
    # breaks the emulator now comes before what is older or may run short later.
    problems.sort(key=lambda p: 0 if "storage is full" in p or "not running" in p else
                  1 if "OneLibrary" in p else 2)
    return problems
