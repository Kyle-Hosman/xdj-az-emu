#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""az-emu-test.py - the emulator test suite: boot, mods, controls, stems.

    python3 tools/az-emu-test.py [--azmod PATH --azstem PATH] [--stages LIST]
                                 [--usb IMG] [--control-boots N] [--take-over]

Each stage prints PASS or FAIL with the line of evidence it is based on, or NOT RUN
when its instrument cannot work with what it was given (the stage says why). The exit
code is non-zero if any stage failed; NOT RUN is listed in the summary and does not fail
the run. --azmod/--azstem are pushed by the mods stage, so include it when you give them. Every stage carries its own control, so
an instrument that cannot see a failure fails instead of passing:

  instrument  Boot a no-mods copy of the image (derived here, no Docker) up to
              --control-boots times (default 3) and look for a crash of EP147 (an
              intermittent SIGSEGV on the first EP147 start, so it may not show). If none
              of the boots shows one, the last one crashes EP147 on purpose
              (kill -SEGV), and the crash detector must see that, or it is declared
              broken.
  boot        Cold-boot the normal image; for 150 s EP147 keeps one pid, systemd
              NRestarts stays 0, and the HOST serial log (which outlives journald's
              ~50 s window and, unlike NRestarts, is not reset by a manual restart)
              shows no SIGSEGV and no "Main process exited". Then the health check of
              tools/az-emu-health.py: the image matches this checkout, /mnt has 2 GiB
              free with the deck program's track caches in place, no TrackFileCache
              failure (any errno) in the serial log or the journal, and the OneLibrary key
              flag exists. Control: an 8 GB image booted with AZ_MNT_MIN_GB=0 (so it is
              not enlarged) must fail it.
  mods        With --azmod/--azstem: push that pair, restart EP147, and require that
              the live EP147 has MAPPED exactly those bytes (md5 of
              /proc/<pid>/map_files), then 150 s with no exit. Without them: the
              mapped azmod/azstem must match the installed files. The installed pair
              lives on the persistent settings disk, so a pushed pair is saved first
              and put back when the run ends.
  controls    Drive the panel and compare screen regions: every step must change its
              region, and the deliberate no-op (the browse knob turned past the end of
              the SOURCE list) must change NOTHING.
  stems       Load "Basic" (a track with stems on the test USB) on deck 1 and play it.
              With all four stems on the deck must be audible; muting all four with the
              pads must silence it while it keeps playing; unmuting brings it back.
              Control: with the STEMS master off the same pad presses must NOT silence
              the deck. Read from azstem's /tmp/azstem.probestat (ctrl, play_S, peakL).
  drumroll    With a track playing (it loads "Basic" if needed): SLIP LOOP + pad 8
              arms the roll (the pad strip and the Beat FX panel change), holding the
              X-PAD fires hits that azstem logs locked to the beat grid (gap = one
              subdivision); pad 8 again disarms (the strip changes back), and the same
              X-PAD hold then fires NO hits (the control). It needs azmod's and
              azstem's logs, which run only if settings/azmod-logs.on existed when
              EP147 started: it sets the flags, restarts EP147, and restores the flags
              exactly as it found them. It also needs DRUM ROLLS samples on the stick.
  fourdeck    The 4 DECK tab changes the deck panels, the deck 3 and deck 4 panels are
              not blank, and 2 DECK changes them back.
  modmenu     Opens the Mod Menu and visits every page: each page title differs from
              the one before and is not blank, revisiting the first page gives the
              identical title (the control), and BACK closes the menu.
  themes      Day -> AZ-MODS -> Day on the Themes page: the menu's mean brightness
              drops, then returns; a tap on the page's THEME label changes nothing.
              azmod.cfg is put back afterwards, so the theme is left as found.

Touch steps LOOK at the screen and re-tap when a busy TCG guest drops a touch; every
retry is listed in that stage's evidence line.

Needs: no emulator running (--take-over stops one), a built image (./build.sh), the
AZ-MODS set installed (tools/az-mods-fetch.py) unless --azmod/--azstem are given, and
for the stems and drumroll stages a USB image with the "Basic" track, its stems and
DRUM ROLLS samples (--usb; default build/usb-test.img if it exists, else
build/usb.img; docs/guide.md describes the image). On Windows, AZ_QEMU must point
at qemu-system-aarch64.exe. Standard library only; the panel actions come from
tools/az-drive.py. A full run takes about 20 minutes (longer under TCG).
"""
import argparse
import gzip
import hashlib
import importlib.util
import json
import os
import re
import socket
import subprocess
import sys
import time

TOOLS = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, TOOLS)
import azemu  # noqa: E402

_spec = importlib.util.spec_from_file_location("azdrive", os.path.join(TOOLS, "az-drive.py"))
azd = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(azd)

WIN = azemu.WIN
BUILD = azemu.BUILD
SOCK = azemu.SOCK_DIR
SERIAL = os.path.join(SOCK, "serial.log")
STAGE = "/home/root/settings/az-usbload"      # where the installed azmod/azstem live
SAVED = f"{STAGE}/.az-emu-test-saved"
WATCH_S = 150
PRELOAD_CONF = "etc/systemd/system/EP147.service.d/10-qemu.conf"   # image without the loader
LOADER_CONF = "etc/systemd/system/EP147.service.d/10-azmod.conf"   # the deck's loader (2.01)
LOADER_UNIT = "etc/systemd/system/az-usbload.service"
STAGES = ("instrument", "boot", "mods", "controls", "stems", "drumroll", "fourdeck", "modmenu", "themes")


class Fail(Exception):
    pass


results = []


def report(stage, ok, evidence):
    """ok is True (PASS), False (FAIL) or None (NOT RUN: the stage's instrument cannot
    work with what it was given, so it says so instead of reporting a result)."""
    results.append((stage, ok))
    word = "NOT RUN" if ok is None else "PASS" if ok else "FAIL"
    print(f"{word} {stage}: {evidence}", flush=True)


def log(msg):
    print(f"  .. {msg}", flush=True)


# ---- guest -----------------------------------------------------------------------
def guest(cmd, timeout=60):
    return azemu.ssh(cmd, timeout=timeout)[1].strip()


def guest_up():
    return azemu.ssh("true", timeout=30)[0] == 0


def ep147_state():
    """(NRestarts, MainPID) of EP147.service, or None if unreadable."""
    out = guest("systemctl show -p NRestarts -p MainPID EP147.service")
    kv = dict(line.split("=", 1) for line in out.splitlines() if "=" in line)
    try:
        return int(kv["NRestarts"]), int(kv["MainPID"])
    except (KeyError, ValueError):
        return None


def serial_counts(start=0):
    """(SIGSEGV count, EP147 exit count) in the host serial log from byte `start`."""
    try:
        with open(SERIAL, "rb") as f:
            f.seek(start)
            data = f.read()
    except OSError:
        return 0, 0
    return data.count(b"SIGSEGV"), data.count(b"EP147.service: Main process exited")


def serial_size():
    try:
        return os.path.getsize(SERIAL)
    except OSError:
        return 0


# ---- emulator lifecycle ------------------------------------------------------------
def emu_running():
    if WIN:
        out = subprocess.run(["tasklist", "/FI", "IMAGENAME eq qemu-system-aarch64.exe"],
                             capture_output=True, text=True).stdout
        return "qemu-system-aarch64" in out
    return subprocess.run(["pgrep", "-f", "qemu-system-aarch64"],
                          stdout=subprocess.DEVNULL).returncode == 0


def qmp(cmd):
    with socket.create_connection(azemu.QMP, timeout=10) as s:
        f = s.makefile("rw")
        f.readline()
        for c in ({"execute": "qmp_capabilities"}, cmd):
            f.write(json.dumps(c) + "\n")
            f.flush()
            while True:
                line = f.readline()
                if not line:
                    return None
                r = json.loads(line)
                if "return" in r or "error" in r:
                    break
        return r


def stop_emu():
    """Quit QEMU. The settings disk (mods, azmod.cfg, flags) is cached writeback in the
    guest and a QMP quit does not flush it, so sync first or the last changes (a
    restore, a removed flag) are lost and come back on the next boot."""
    if not emu_running():
        return
    if guest_up():
        guest("sync", 60)
    try:
        qmp({"execute": "quit"})
    except OSError:
        pass
    for _ in range(30):
        if not emu_running():
            return
        time.sleep(1)
    if WIN:
        subprocess.run(["taskkill", "/IM", "qemu-system-aarch64.exe", "/F"], capture_output=True)
    else:
        subprocess.run(["pkill", "-f", "qemu-system-aarch64"])
    time.sleep(3)


def start_emu(usb, initrd=None):
    """Cold-boot headless; return once EP147 is active. The boot script truncates
    serial.log, so every count afterwards is this boot's. AZ_EMU_NOMODS keeps the
    boot script from installing the published set behind the test's back."""
    env = dict(os.environ, AZ_DISPLAY="none", AZ_EMU_NOMODS="1", AZ_USB_FILE=usb)
    if initrd:
        env["AZ_INITRD"] = initrd
    if WIN:
        cmd = ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File",
               os.path.join(TOOLS, "az-boot-win.ps1"), "-Display", "none", "-NoMods"]
        r = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=120)
        if "qemu pid" not in r.stdout:
            raise Fail(f"az-boot-win.ps1 did not start QEMU: {(r.stdout + r.stderr).strip()[-300:]}")
    else:
        subprocess.Popen(["bash", os.path.join(TOOLS, "az-boot-m1.sh")], env=env,
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL, start_new_session=True)
    t0 = time.time()
    while not guest_up():
        if time.time() - t0 > 600:
            raise Fail("no SSH 600 s after boot")
        time.sleep(5)
    while guest("systemctl is-active EP147.service") != "active":
        if time.time() - t0 > 900:
            raise Fail("EP147 never became active")
        time.sleep(3)
    for f in ("knob.ctr", "deck1.state", "deck2.state", "mixer.state"):
        try:
            os.remove(os.path.join(SOCK, f))  # az-drive state from a previous boot
        except OSError:
            pass
    azd._app_up = False  # the harness boots headless: no app holds ctrl.sock
    return time.time() - t0


def watch(seconds=WATCH_S, serial_from=0):
    """Sample EP147 for `seconds`. Returns (clean, evidence)."""
    first = ep147_state()
    pids = set()
    last = first
    t_end = time.time() + seconds
    while time.time() < t_end:
        time.sleep(10)
        st = ep147_state()
        if st:
            last = st
            pids.add(st[1])
    segv, exits = serial_counts(serial_from)
    nrestarts = last[0] if last else -1
    clean = segv == 0 and exits == 0 and nrestarts == 0 and len(pids) == 1
    return clean, (f"{seconds}s: SIGSEGV={segv} exits={exits} NRestarts={nrestarts} "
                   f"pids={sorted(pids)}")


# ---- the no-mods control image --------------------------------------------------------
def _cpio_records(raw):
    """Yield (name, header_offset, data_offset, end_offset, filesize) of a newc cpio."""
    off = 0
    while off + 110 <= len(raw):
        h = raw[off:off + 110]
        if h[:6] not in (b"070701", b"070702"):
            raise ValueError(f"not newc cpio at {off}")
        namesize = int(h[94:102], 16)
        filesize = int(h[54:62], 16)
        name = raw[off + 110:off + 110 + namesize - 1].decode("utf-8", "replace")
        data = (off + 110 + namesize + 3) & ~3
        end = (data + filesize + 3) & ~3
        yield name, off, data, end, filesize
        if name == "TRAILER!!!":
            return
        off = end


def nomods_initrd():
    """The normal image with EP147's preload cut to the emulator's own libraries, so
    neither azmod nor azstem loads even though they are installed. Cached next to
    the image. An image with the deck's USB loader (firmware 2.01): its drop-in reads no
    env file and its service does nothing, so the loader neither preloads nor restarts
    EP147 onto the stick's set. Without the loader: the preload line is cut to the shim."""
    src = os.path.join(BUILD, "initramfs-patched.cpio.gz")
    dst = os.path.join(BUILD, "initramfs-nomods-test.cpio.gz")
    if os.path.exists(dst) and os.path.getmtime(dst) >= os.path.getmtime(src):
        return dst
    raw = gzip.decompress(open(src, "rb").read())
    names = {name.lstrip("./") for name, *_ in _cpio_records(raw)}
    if PRELOAD_CONF in names:
        edits = {PRELOAD_CONF: (r"(?m)^Environment=LD_PRELOAD=.*$",
                                "Environment=LD_PRELOAD=/home/root/ep122_shim.so")}
    elif LOADER_CONF in names and LOADER_UNIT in names:
        edits = {LOADER_CONF: (r"(?m)^EnvironmentFile=.*$", "EnvironmentFile=-/nonexistent-az-emu-test"),
                 LOADER_UNIT: (r"(?m)^ExecStart=.*$", "ExecStart=/bin/true")}
    else:
        raise Fail(f"neither {PRELOAD_CONF} nor {LOADER_CONF} + {LOADER_UNIT} in {src}")
    out, done = bytearray(), set()
    for name, h, d, e, size in _cpio_records(raw):
        name = name.lstrip("./")
        if name not in edits:
            out += raw[h:e]
            continue
        rx, repl = edits[name]
        new, n = re.subn(rx, repl, raw[d:d + size].decode())
        if n != 1:
            raise Fail(f"{name}: expected one line matching {rx}, found {n}")
        body = new.encode()
        hdr = bytearray(raw[h:d])
        hdr[54:62] = b"%08X" % len(body)
        out += hdr + body + b"\0" * ((4 - len(body) % 4) % 4)
        done.add(name)
    if done != set(edits):
        raise Fail(f"not edited in {src}: {sorted(set(edits) - done)}")
    out += b"\0" * ((512 - len(out) % 512) % 512)
    tmp = dst + ".tmp"
    with open(tmp, "wb") as f:
        f.write(gzip.compress(bytes(out), 6))
    os.replace(tmp, dst)
    return dst


# ---- stages ----------------------------------------------------------------------------
def induce_crash():
    """Crash EP147 for real (SIGSEGV) and read the HOST serial log for it. Returns
    (seen, evidence): seen means the deck program's crash banner names that pid and
    systemd logged the main process exiting."""
    pid = guest("pidof EP147").split()[0] if guest("pidof EP147") else ""
    if not pid:
        return False, "no EP147 pid to crash"
    start = serial_size()
    guest(f"kill -SEGV {pid}")
    time.sleep(20)
    with open(SERIAL, "rb") as f:
        f.seek(start)
        tail = f.read()
    banner = any(b"SIGSEGV" in line and f"[{pid}]".encode() in line for line in tail.splitlines())
    exited = b"EP147.service: Main process exited" in tail
    return banner and exited, (f"kill -SEGV {pid}: SIGSEGV banner for pid {pid} "
                               f"{'seen' if banner else 'MISSING'}, main process exit "
                               f"{'seen' if exited else 'MISSING'}")


def stage_instrument(n, usb):
    """The crash detector must see a crash. Preferred proof: an EP147 crash on its first
    start in one of n no-mods boots. It is intermittent and timing dependent, so
    it may never fire; if none of the n boots shows it, the last one crashes EP147 on
    purpose and the detector must see that instead. A clean boot reading clean is the negative half,
    proven by the boot stage."""
    img = nomods_initrd()
    for i in range(1, n + 1):
        stop_emu()
        start_emu(usb, img)
        time.sleep(60)  # the crash lands ~11 s into the first EP147 start
        preload = guest("tr '\\0' '\\n' < /proc/$(pidof EP147)/environ | grep ^LD_PRELOAD=")
        segv, exits = serial_counts()
        log(f"no-mods boot {i}: SIGSEGV={segv} exits={exits} ({preload})")
        if "azmod" in preload:
            raise Fail(f"the no-mods image still preloads azmod: {preload}")
        if segv and exits:
            report("instrument", True, f"first-start crash seen: no-mods boot {i}/{n} shows an EP147 crash: "
                   f"SIGSEGV={segv} exits={exits}: the crash detector can see a crash")
            return
    seen, ev = induce_crash()
    report("instrument", seen, (f"induced SIGSEGV seen (no first-start crash in {n} no-mods boots): {ev}; the "
                                "crash detector can see a crash") if seen else
           (f"no first-start crash in {n} no-mods boots and the induced crash was not detected: {ev}; "
            "a clean boot below would not mean anything"))


def stage_boot(usb):
    stop_emu()
    took = start_emu(usb)
    clean, ev = watch()
    problems = azemu.health(serial_log=SERIAL)
    mnt = guest("df -h /mnt | awk 'NR==2 {print $4 \" free of \" $2}'; "
                "ls /mnt/LocalFileCache*.bin 2>/dev/null | wc -l")
    free, caches = (mnt.splitlines() + ["?", "?"])[:2]
    ev += f"; /mnt {free}, {caches.strip()} track caches; health: " + (
        "; ".join(problems) if problems else "ok")
    report("boot", clean and not problems, f"cold boot (EP147 active after {took:.0f} s), {ev}")


def md5(path):
    return hashlib.md5(open(path, "rb").read()).hexdigest()


def mapped_md5(lib):
    """md5 of the bytes the live EP147 has mapped for lib.so (map_files), "" if none.
    An empty answer is read again (three tries): a failed or timed-out SSH call also
    returns "", and must not be reported as "not mapped"."""
    cmd = (f"P=$(pidof EP147 | cut -d' ' -f1); r=$(grep '/{lib}\\.so$' /proc/$P/maps | head -1 | cut -d' ' -f1); "
           f"[ -n \"$r\" ] && md5sum /proc/$P/map_files/$r | cut -d' ' -f1")
    for i in range(3):
        out = guest(cmd)
        if out:
            return out
        time.sleep(3)
    return ""


def save_installed(libs):
    """Keep the installed pair aside before a test pair replaces it."""
    files = " ".join(f"{STAGE}/{lib}.so" for lib in libs)
    guest(f"mkdir -p {SAVED} && for f in {files}; do [ -f $f ] && cp -p $f {SAVED}/; done; true")


def restore_installed():
    """Put back what save_installed() kept (rename over, never write a mapped file in
    place) and restart EP147 on it."""
    out = guest(f"[ -d {SAVED} ] || exit 0; for f in {SAVED}/*.so; do [ -f \"$f\" ] || continue; "
                f"cp -p \"$f\" {STAGE}/.restore.tmp && mv -f {STAGE}/.restore.tmp {STAGE}/$(basename \"$f\") "
                f"&& echo restored $(basename \"$f\"); done; rm -rf {SAVED}; sync", 120)
    if out:
        guest("systemctl reset-failed EP147.service")  # a crash loop may have hit the start limit
        azemu.restart_ep147()
        log(f"{out.replace(chr(10), ', ')}; EP147 restarted on the installed set")


def stage_mods(pair):
    if pair:
        save_installed(pair)
        for lib, path in pair.items():
            if not azemu.push_file(path, f"{STAGE}/{lib}.so"):
                raise Fail(f"push {path} -> {STAGE}/{lib}.so failed")
        want = {lib: md5(path) for lib, path in pair.items()}
        start = serial_size()
        guest("systemctl reset-failed EP147.service; systemctl restart EP147.service", 90)
        t0 = time.time()
        while guest("systemctl is-active EP147.service") != "active" and time.time() - t0 < 120:
            time.sleep(3)
        time.sleep(5)
        got = {lib: mapped_md5(lib) for lib in want}
        clean, ev = watch(serial_from=start)
        # A manual restart zeroes NRestarts, so here it is a real count since then.
        ok = clean and got == want
        report("mods", ok, f"mapped {', '.join(f'{k}={got[k][:8]}' for k in got)} "
               f"want {', '.join(f'{k}={want[k][:8]}' for k in want)}; after restart {ev}")
    else:
        files = {lib: guest(f"md5sum {STAGE}/{lib}.so | cut -d' ' -f1") for lib in ("azmod", "azstem")}
        got = {lib: mapped_md5(lib) for lib in files}
        ok = all(files[k] and files[k] == got[k] for k in files)
        report("mods", ok, f"mapped {got} vs installed {files} (no --azmod/--azstem: the installed set)")


# Screen regions in screendump pixels (1280x800).
SOURCE_LIST = (90, 74, 640, 345)
DECK1_PADS = (10, 480, 630, 555)
DECK1_TEMPO = (360, 610, 520, 700)
WHOLE = (0, 70, 1280, 800)


def screen(name):
    path = os.path.join(SOCK, f"test-{name}.ppm").replace("\\", "/")
    qmp({"execute": "screendump", "arguments": {"filename": path}})
    time.sleep(0.5)
    data = open(path, "rb").read()
    m = re.match(rb"P6\s+(\d+)\s+(\d+)\s+(\d+)\s", data)
    if not m:
        raise Fail(f"bad screendump {path}")
    w, h = int(m.group(1)), int(m.group(2))
    return w, h, data[m.end():m.end() + w * h * 3]


def changed(a, b, region):
    """Fraction of pixels in `region` that differ between two screens."""
    w, _, pa = a
    _, _, pb = b
    x0, y0, x1, y1 = region
    diff = total = 0
    for y in range(y0, y1, 2):
        o0, o1 = (y * w + x0) * 3, (y * w + x1) * 3
        ra, rb = pa[o0:o1], pb[o0:o1]
        for i in range(0, len(ra), 6):
            total += 1
            if ra[i:i + 3] != rb[i:i + 3]:
                diff += 1
    return diff / max(total, 1)


def stage_controls():
    steps = []

    def step(name, action, region, expect_change):
        before = screen(f"{name}-a")
        action()
        time.sleep(2.0)
        after = screen(f"{name}-b")
        frac = changed(before, after, region)
        ok = (frac > 0.002) if expect_change else (frac == 0.0)
        steps.append((name, ok, frac, expect_change))
        log(f"{name}: {frac:.4f} of region changed ({'want change' if expect_change else 'want NONE'})")

    goto_source()
    time.sleep(2)
    azd.knob("cw", 3)  # to the end of the SOURCE list
    time.sleep(1.5)
    step("knob-past-end (no-op)", lambda: azd.knob("cw", 1), SOURCE_LIST, False)
    step("knob-ccw", lambda: azd.knob("ccw", 1), SOURCE_LIST, True)
    step("knob-cw", lambda: azd.knob("cw", 1), SOURCE_LIST, True)
    step("waveform", azd.waveform_view, WHOLE, True)
    step("padmode-beatloop", lambda: azd.padmode(1, "beatloop"), DECK1_PADS, True)
    step("padmode-hotcue", lambda: azd.padmode(1, "hotcue"), DECK1_PADS, True)
    step("tempo-minus3", lambda: azd.tempo(1, -3), DECK1_TEMPO, True)
    step("tempo-zero", lambda: azd.tempo(1, 0), DECK1_TEMPO, True)
    bad = [s for s in steps if not s[1]]
    ev = ", ".join(f"{n}={f:.3f}" for n, _, f, _ in steps)
    report("controls", not bad, ("all steps as expected: " if not bad else
                                 f"{len(bad)} wrong ({', '.join(s[0] for s in bad)}): ") + ev)


# Touch coordinates (2560x1600, the space az-drive's tap uses) on the 2.0x firmware.
COORD = {
    "usb_source": (714, 230),            # SOURCE list row 1: the USB stick
    "local_library": (1477, 400),        # right panel: "Local Library"
    "popup": (1280, 1100),               # empty list area: dismisses the OneLibrary popup
    "modmenu_row": (380, 590),           # SOURCE list: MOD MENU when it is row 3 (fallback only)
    "modmenu_stems_page": (600, 334),    # opened Mod Menu, left column: "Stems"
    "modmenu_stems_enable": (2474, 354), # Stems page: "Enable" checkbox
}


def tap(key_or_xy):
    """Touch the screen. Each QMP command gets its own short connection: QEMU serves
    one client per QMP socket, and screen() needs the same one in between."""
    x, y = COORD[key_or_xy] if isinstance(key_or_xy, str) else key_or_xy
    ax, ay = round(x / azd.W * 0x7FFF), round(y / azd.H * 0x7FFF)
    qmp({"execute": "input-send-event", "arguments": {"events": [
        {"type": "abs", "data": {"axis": "x", "value": ax}},
        {"type": "abs", "data": {"axis": "y", "value": ay}},
        {"type": "btn", "data": {"button": "left", "down": True}}]}})
    time.sleep(0.08)
    qmp({"execute": "input-send-event", "arguments": {"events": [
        {"type": "btn", "data": {"button": "left", "down": False}}]}})


def tap_row(key):
    """A browse-list row: one tap selects it and shows its panel on the right (a
    second tap would open it, e.g. the USB stick's folder view)."""
    tap(key)


def stems_on():
    """The STEMS master: char 0 of line 1 of azmod.cfg (azmod persists it)."""
    return mod_enabled(0)


def _usb_tile_blue():
    """The SOURCE screen's blue USB tile at the top left (x 20-80, y 84-128). The
    opened Mod Menu has a gray back-arrow box there instead."""
    w, _, px = screen("modmenu-check")
    blue = n = 0
    for y in range(84, 128, 4):
        for x in range(20, 80, 4):
            o = (y * w + x) * 3
            blue += px[o + 2] - px[o]
            n += 1
    return blue / n >= 30


def goto_source():
    """Get to the SOURCE screen and confirm it. az-drive's goto_source() (BROWSE, then SOURCE)
    is right from the deck views, but before anything was browsed BROWSE opens "Please
    select a source", where SOURCE alone is the way back; so try both and LOOK."""
    for i in range(6):
        if i:
            time.sleep(5)
        if i % 2:
            azd.panel("source")
            time.sleep(2.0)
        else:
            azd.goto_source()
        if _usb_tile_blue():
            return
    raise Fail(f"could not get to the SOURCE screen (screens in {SOCK}, test-modmenu-check.ppm)")


def modmenu_row_xy():
    """The MOD MENU row on the SOURCE screen, found by LOOKING: its row carries the orange
    AZ-MODS icon in the left icon column. The row's position depends on how many sources
    the deck lists (USB1 only puts it second, a second USB or a link player third), so a
    fixed coordinate misses it on some setups. Falls back to COORD["modmenu_row"]."""
    w, h, px = screen("source-rows")
    rows = []
    for y in range(60, min(h, 700), 2):
        hits = 0
        for x in range(8, 80, 4):
            o = (y * w + x) * 3
            r, g, b = px[o], px[o + 1], px[o + 2]
            if r > 200 and 90 < g < 190 and b < 80:
                hits += 1
        if hits >= 3:
            rows.append(y)
    if not rows:
        return COORD["modmenu_row"]
    sy = azd.H / h
    return (COORD["modmenu_row"][0], round((min(rows) + max(rows)) / 2 * sy))


def mod_menu_open():
    """True if the Mod Menu is up; meaningful only after the SOURCE screen was
    confirmed, since other views are not blue there either."""
    return not _usb_tile_blue()


def open_mod_menu():
    """Open the Mod Menu from SOURCE. First make sure SOURCE is really on screen (the
    deck can be slow to follow BROWSE/SOURCE while it plays), then tap the MOD MENU row
    and LOOK: a tap opens the menu, or only selects the row first."""
    goto_source()
    for i in range(6):   # a deck that is playing under TCG drops many touches
        if i and i % 2 == 0:
            goto_source()
        tap(modmenu_row_xy())
        time.sleep(2.0)
        if mod_menu_open():
            log(f"Mod Menu opened by tap {i + 1}")
            return
        # With the row selected, the preview says "tap again or press the knob to open":
        # the knob press is the other documented way in, and does not depend on a
        # second touch being delivered.
        if i % 2 == 1:
            azd.knobclick()
            time.sleep(2.0)
            if mod_menu_open():
                log(f"Mod Menu opened by the knob press after tap {i + 1} "
                    "(a second tap on the selected row did not open it)")
                return
    raise Fail("the Mod Menu did not open after 6 taps on its SOURCE row "
               f"(screens in {SOCK}, test-modmenu-check.ppm)")


def mod_enabled(cfg_index):
    """A mod's Enable state: char cfg_index of line 1 of azmod.cfg (0 = Stems,
    6 = Drum Roll). No azmod.cfg yet means every mod is at its default, off."""
    line = guest("head -1 /home/root/settings/azmod.cfg 2>/dev/null")
    return len(line) > cfg_index and line[cfg_index] == "1"


def set_mod_enable(name, page_y, cfg_index, on):
    """Mod Menu > <page> > Enable; azmod.cfg is the evidence. A deck that plays under
    TCG drops some touches, so tap the checkbox, wait long enough for a late tap to
    land, read azmod.cfg, and only tap again if the state is still wrong (never twice
    unchecked)."""
    if mod_enabled(cfg_index) != on:
        open_mod_menu()
        tap((600, page_y))
        time.sleep(2.0)
        for _ in range(6):
            if mod_enabled(cfg_index) == on:
                break
            if not mod_menu_open():
                open_mod_menu()
                tap((600, page_y))
                time.sleep(2.0)
            tap("modmenu_stems_enable")   # every mod page has Enable at the same spot
            time.sleep(4.0)
        screen(f"{name}-{'on' if on else 'off'}")  # evidence if it failed
        azd.panel("back")
        time.sleep(1.5)
        if mod_menu_open():          # BACK from a mod page may leave the menu up
            azd.panel("back")
            time.sleep(1.5)
    goto_source()
    if mod_enabled(cfg_index) != on:
        raise Fail(f"could not turn {name} {'on' if on else 'off'} in 6 taps "
                   f"(azmod.cfg unchanged; screen in {SOCK}, test-{name}-*.ppm)")


def set_stems_master(on):
    """Mod Menu > Stems > Enable (the STEMS master)."""
    set_mod_enable("stems-master", COORD["modmenu_stems_page"][1], 0, on)


def deck0():
    """(ctrl, frames, play_S, peakL) of deck 1 from azstem's probestat, or None."""
    line = guest("grep '^deck0 ' /tmp/azstem.probestat")
    m = re.search(r"ctrl=0x([0-9a-f]+) frames=(\d+) play_S=(-?\d+) peakL=([0-9.]+)", line)
    if not m:
        return None
    return int(m.group(1), 16), int(m.group(2)), int(m.group(3)), float(m.group(4))


def sample(n=3, gap=1.5):
    out = []
    for _ in range(n):
        time.sleep(gap)
        s = deck0()
        if s:
            out.append(s)
    return out


def stem_pads():
    """Put deck 1's pads in BEAT LOOP mode, where (with STEMS on) pads 1-4 are the stem
    toggles. Pressing BEAT LOOP while already in it switches to BEAT LOOP 2, so go
    through HOT CUE first, and press it only once per stage."""
    azd.padmode(1, "hotcue")
    time.sleep(0.6)
    azd.padmode(1, "beatloop")
    time.sleep(0.6)


STEM_RETRIES = []


def set_all_stems(on):
    """Press stem pads 1-4 on deck 1 (VOCAL / MELODY / BASS / DRUMS; the pads must
    already be in BEAT LOOP mode, stem_pads()) until its stem mask (probestat ctrl,
    low nibble) is all on or all off. An injected pad press is occasionally dropped,
    so a pad whose bit did not move is pressed again (up to twice), and every retry
    is reported."""
    want = 0x0F if on else 0x00
    for n in (1, 2, 3, 4):
        azd.pad(1, n)
        time.sleep(0.3)
    for _ in range(2):
        time.sleep(1.5)
        s = deck0()
        if not s or not s[0] & 0x80:
            return  # stems not engaged (the control): nothing to converge
        mask = s[0] & 0x0F
        if mask == want:
            return
        for n in (1, 2, 3, 4):
            if bool(mask >> (n - 1) & 1) != on:
                STEM_RETRIES.append(f"stem {n} {'on' if on else 'off'}")
                azd.pad(1, n)
                time.sleep(0.3)


def mute_all():
    set_all_stems(False)


def track_view_open():
    """True if the browser shows the TRACK view: its category tile on the left
    (x 0-100, y 212-280) is the blue selected one."""
    w, _, px = screen("trackview-check")
    blue = n = 0
    for y in range(220, 272, 4):
        for x in range(10, 90, 4):
            o = (y * w + x) * 3
            blue += px[o + 2] - px[o]
            n += 1
    return blue / n > 60


def load_basic():
    """SOURCE -> USB -> Local Library -> TRACK view, row 3 ("Basic"), LOAD 1, then wait
    (up to 120 s: loading is slow under TCG) until azstem has deck 1's stems, and PLAY.
    Returns the deck0 probestat sample that showed the load, or None."""
    # Soon after a boot the deck may not have recognized the stick yet (its row reads
    # "USB1" and opens nothing), and the first browse mounts the library and may raise
    # the OneLibrary popup a few seconds later, which swallows the next tap. So walk the
    # whole path from SOURCE and look; retry until the TRACK view is really up.
    for attempt in range(5):
        if attempt:
            time.sleep(15)
        goto_source()
        tap_row("usb_source")
        time.sleep(1.8)
        tap_row("local_library")
        time.sleep(3.0)
        tap("popup")
        time.sleep(1.5)
        tap((100, 480))  # the TRACK category
        time.sleep(3.0)
        if track_view_open():
            break
    else:
        raise Fail("the TRACK view did not open in 5 tries (screens in "
                   f"{SOCK}, test-trackview-check.ppm)")
    tap((1400, 520))  # third row: Alone, Alright, Basic
    time.sleep(1.5)
    azd.load(1)
    loaded, t0 = None, time.time()
    while time.time() - t0 < 120:
        time.sleep(5)
        s = deck0()
        if s and s[0] & 0x80 and s[1] > 0:
            loaded = s
            break
    azd.press(1, "play")
    time.sleep(6)
    return loaded


ONELIBRARY_FLAG = "/home/root/settings/azmod-onelibrarykey.on"
_flag_created = False


def enable_onelibrary_key(usb):
    """azmod's emulator-only OneLibrary key interposer, gated by a flag file: without
    it the deck program cannot decrypt a stick's exportLibrary.db and browses the stick
    as folders only. Refused unless EP147 runs with the emulator shim (never on a
    deck). The deck program mounts a stick's library once per boot (an EP147 restart
    does not mount it again), and the flag lives on the persistent settings disk, so a
    newly created flag is followed by a cold boot."""
    global _flag_created
    if not guest("pidof EP147"):
        raise Fail("EP147 is not running, so there is nothing to test")
    preload = guest("grep -c ep122_shim /proc/$(pidof EP147)/environ")
    if preload.strip() in ("", "0"):
        raise Fail("EP147 does not preload ep122_shim.so: not the emulator, refusing to "
                   f"create {ONELIBRARY_FLAG}")
    if guest(f"[ -e {ONELIBRARY_FLAG} ] && echo yes") == "yes":
        return
    guest(f"touch {ONELIBRARY_FLAG} && sync")
    _flag_created = True
    stop_emu()
    took = start_emu(usb)
    time.sleep(90)  # the deck UI ignores navigation for the first minute or two of a boot
    log(f"created {ONELIBRARY_FLAG}; cold boot so the stick's library mounts with it "
        f"(EP147 active after {took:.0f} s)")


def disable_onelibrary_key():
    """Remove the flag again if this run created it."""
    global _flag_created
    if _flag_created:
        guest(f"rm -f {ONELIBRARY_FLAG} && sync")
        _flag_created = False
        log(f"removed {ONELIBRARY_FLAG}")


def stage_stems(usb):
    enable_onelibrary_key(usb)
    set_stems_master(True)
    loaded = load_basic()
    stem_pads()
    # Release builds keep no /tmp/azstem.log; a dev build's line names the track.
    name = guest("grep -a 'stems loaded d0' /tmp/azstem.log 2>/dev/null | tail -1")
    if not loaded or (name and "Basic" not in name):
        why = ""
        with open(SERIAL, "rb") as f:   # this boot's log: the boot script truncates it
            if b"cannnot decypt database" in f.read():   # sic: the deck program's spelling
                why = (f"; the deck could not decrypt the stick's exportLibrary.db even with "
                       f"{ONELIBRARY_FLAG} (is the installed azmod one with the emulator key "
                       "interposer?), so it browses the stick as folders only")
        raise Fail(f"Basic's stems did not load on deck 1 within 120 s (probestat deck0: "
                   f"{deck0()}; log: {name or 'none'}){why}")
    log(f"Basic loaded on deck 1: probestat ctrl=0x{loaded[0]:02x} frames={loaded[1]}")
    on = sample()
    mute_all()
    time.sleep(2)
    muted = sample()
    set_all_stems(True)  # unmute
    time.sleep(2)
    back = sample()
    log(f"stems: all on {on}; muted {muted}; unmuted {back}")
    audible = lambda ss: bool(ss) and min(s[3] for s in ss) > 0.005  # noqa: E731
    silent = lambda ss: bool(ss) and max(s[3] for s in ss) == 0.0  # noqa: E731
    moving = lambda ss: len(ss) > 1 and ss[-1][2] > ss[0][2]  # noqa: E731
    engaged = lambda ss: bool(ss) and all(s[0] & 0x80 and s[1] > 0 for s in ss)  # noqa: E731
    ok_on = engaged(on) and audible(on) and moving(on)
    ok_mute = silent(muted) and moving(muted)
    ok_back = audible(back)

    # Control: STEMS master off, same pad presses: the deck must stay audible.
    set_stems_master(False)
    azd.waveform_view()
    time.sleep(3)
    mute_all()
    time.sleep(2)
    ctl = sample()
    set_stems_master(True)  # leave the emulator as found
    ok_ctl = bool(ctl) and audible(ctl) and not engaged(ctl)

    def fmt(ss):
        if not ss:
            return "no probestat"
        return "peak " + "/".join(f"{x[3]:.3f}" for x in ss) + f" ctrl=0x{ss[-1][0]:02x}"
    report("stems", ok_on and ok_mute and ok_back and ok_ctl,
           f"all on {fmt(on)}; muted {fmt(muted)} "
           f"(play_S {'advancing' if moving(muted) else 'STOPPED'}); unmuted {fmt(back)}; "
           f"CONTROL stems master off, same pads: {fmt(ctl)}"
           + (f"; pad presses retried: {', '.join(STEM_RETRIES)}" if STEM_RETRIES else ""))


def region_stats(scr, region):
    """(mean luminance, standard deviation) of a screen region."""
    w, _, px = scr
    x0, y0, x1, y1 = region
    vals = []
    for y in range(y0, y1, 3):
        row = px[(y * w + x0) * 3:(y * w + x1) * 3]
        vals += [(row[i] * 3 + row[i + 1] * 6 + row[i + 2]) / 10 for i in range(0, len(row), 9)]
    mean = sum(vals) / max(len(vals), 1)
    sd = (sum((v - mean) ** 2 for v in vals) / max(len(vals), 1)) ** 0.5
    return mean, sd


def track_playing():
    a = deck0()
    time.sleep(1.5)
    b = deck0()
    return bool(a and b and b[2] > a[2])


TAP_RETRIES = []


def _retries():
    """The retries of the stage that just ran, for its evidence line."""
    out = f"; retried: {', '.join(TAP_RETRIES)}" if TAP_RETRIES else ""
    TAP_RETRIES.clear()
    return out


def tap_until(xy, name, region, base, threshold=0.002, tries=3):
    """Tap and LOOK: re-tap (a busy TCG guest drops touches) until `region` differs
    from the screen `base` by more than `threshold`. Returns (screen, fraction)."""
    for _ in range(tries):
        tap(xy)
        time.sleep(1.5)
        scr = screen(name)
        frac = changed(base, scr, region)
        if frac > threshold:
            return scr, frac
        TAP_RETRIES.append(name)
    return scr, frac


def restart_ep147():
    guest("systemctl reset-failed EP147.service; systemctl restart EP147.service", 90)
    t0 = time.time()
    while guest("systemctl is-active EP147.service") != "active" and time.time() - t0 < 120:
        time.sleep(3)
    time.sleep(60)  # the deck UI ignores navigation for the first minute after a start


DECK1_STRIP = (0, 480, 640, 560)    # deck 1's pad-mode strip (waveform view)
BEATFX = (1090, 90, 1270, 430)      # the Beat FX panel
DECK_PANELS = (0, 670, 1280, 800)   # the deck panels along the bottom
DECK3_PANEL = (0, 690, 640, 800)    # 4 DECK view: deck 3 (left) and deck 4 (right)
DECK4_PANEL = (640, 690, 1280, 800)
TAB_2DECK, TAB_4DECK = (426, 66), (684, 66)
LOG_FLAGS = "/home/root/settings/azmod-logs.on /home/root/settings/azstem.drumhits"
DRUMROLL_PAGE_Y, DRUMROLL_CFG = 444, 6   # Mod Menu row 2; azmod.cfg line 1, char 6


def drumhits():
    return guest("grep -ac 'drumhit:' /tmp/azstem.log 2>/dev/null")


def tile8():
    """azmod's last pad-8 tile state ("green" = armed, "red" = not), or ""."""
    out = guest("grep -ah 'TILE8 g=' /tmp/azmod-drumtile.log* 2>/dev/null | tail -1")
    m = re.search(r"idx=(\w+)", out)
    return m.group(1) if m else ""


def stage_drumroll(usb):
    """azmod and azstem log only if azmod-logs.on existed when EP147 started, so set
    the log flags (emulator only), restart EP147 (which also starts the roll
    disarmed), and put the flags back exactly as found at the end. The Drum Roll mod
    itself is switched on for the stage if it was off, and off again afterwards."""
    # The hits are read from azstem's log, and a release azstem compiles its log lines
    # out: with one, no hit could ever be seen, so the stage would fail whatever the
    # roll does. Say so instead, and name the build that can run it.
    if guest(f"grep -ac 'drumhit:' {STAGE}/azstem.so 2>/dev/null") in ("", "0"):
        report("drumroll", None, "this azstem.so is a release build: it writes no hit log, so "
               "the hits cannot be counted. Expected with the published AZ-MODS set and with "
               "builds of the public source, which are release builds")
        return

    enable_onelibrary_key(usb)   # loading "Basic" needs the stick's library
    drum_was_on = mod_enabled(DRUMROLL_CFG)
    had = guest(f"for f in {LOG_FLAGS}; do [ -e $f ] && echo $f; done").split()
    flags_set = False
    try:   # everything that changes a setting is inside, so a failure still restores
        set_mod_enable("drumroll", DRUMROLL_PAGE_Y, DRUMROLL_CFG, True)
        guest(f"touch {LOG_FLAGS}; rm -f /tmp/azmod-drumtile.log*")
        flags_set = True
        restart_ep147()
        if not track_playing() and not load_basic():
            raise Fail(f"Basic did not load on deck 1 (probestat deck0: {deck0()})")
        azd.waveform_view()
        time.sleep(1.5)
        azd.padmode(1, "hotcue")      # from HOT CUE, SLIP LOOP lands on its first page
        time.sleep(1.2)
        azd.padmode(1, "sliploop")
        time.sleep(1.5)
        before = screen("dr-before")
        azd.pad(1, 8)                 # arm
        time.sleep(2.0)
        t_armed = tile8()
        if t_armed != "green":        # an injected pad press is occasionally dropped
            TAP_RETRIES.append("pad 8 arm")
            azd.pad(1, 8)
            time.sleep(2.0)
            t_armed = tile8()
        armed = screen("dr-armed")
        strip, fx = changed(before, armed, DECK1_STRIP), changed(before, armed, BEATFX)
        n0 = int(drumhits() or 0)
        azd.xpad(0, hold=4.0, period=0.05)   # rate zone 0 = one beat
        time.sleep(2.5)
        lines = guest("grep -a 'drumhit:' /tmp/azstem.log | tail -n +%d" % (n0 + 1)).splitlines()
        subs = [(float(m.group(1)), int(m.group(2))) for m in
                (re.search(r"sub=(\d+).*GAP (\d+)", ln) for ln in lines) if m]
        full = [g for sub, g in subs if g and abs(g - sub) <= 0.01 * sub]
        doubles = [g for sub, g in subs if g and g < 0.05 * sub]
        azd.pad(1, 8)                 # disarm
        time.sleep(2.0)
        t_after = tile8()
        if t_after != "red":
            TAP_RETRIES.append("pad 8 disarm")
            azd.pad(1, 8)
            time.sleep(2.0)
            t_after = tile8()
        after = screen("dr-after")
        revert = changed(armed, after, DECK1_STRIP)
        n1 = int(drumhits() or 0)
        azd.xpad(0, hold=3.0, period=0.05)
        time.sleep(2.5)
        n2 = int(drumhits() or 0)
    finally:
        if flags_set:
            gone = [f for f in LOG_FLAGS.split() if f not in had]   # put the flags back as found
            if gone:
                guest("rm -f " + " ".join(gone) + " && sync")
            restart_ep147()
        if not drum_was_on and mod_enabled(DRUMROLL_CFG):
            set_mod_enable("drumroll", DRUMROLL_PAGE_Y, DRUMROLL_CFG, False)
    ok = (strip > 0.01 and fx > 0.01 and t_armed == "green" and len(lines) >= 2 and len(full) >= 1
          and revert > 0.01 and t_after == "red" and n2 == n1)
    report("drumroll", ok,
           f"arm: tile8 {t_armed or 'none'}, strip {strip:.3f}, Beat FX {fx:.3f} changed; "
           f"X-PAD held: {len(lines)} hits, {len(full)} gaps = one subdivision"
           + (f", {len(doubles)} extra hits <5% of a subdivision after a beat" if doubles else "")
           + f"; disarm: tile8 {t_after or 'none'}, strip {revert:.3f} changed; "
           f"CONTROL X-PAD held after disarm: {n2 - n1} hits" + _retries())


def stage_fourdeck():
    azd.waveform_view()
    time.sleep(1.5)
    two = screen("4d-two")
    four, c_in = tap_until(TAB_4DECK, "4d-four", DECK_PANELS, two, threshold=0.05)
    back, c_out = tap_until(TAB_2DECK, "4d-back", DECK_PANELS, four, threshold=0.05)
    sd3, sd4 = region_stats(four, DECK3_PANEL)[1], region_stats(four, DECK4_PANEL)[1]
    ok = c_in > 0.05 and c_out > 0.05 and sd3 > 8 and sd4 > 8
    report("fourdeck", ok, f"4 DECK: panels {c_in:.3f} changed, deck 3 / deck 4 panel contrast "
           f"{sd3:.1f} / {sd4:.1f} (blank < 8); 2 DECK: {c_out:.3f} changed back" + _retries())


MENU_PAGES = [334, 444, 554, 664, 774, 884, 994, 1104, 1214, 1324]  # left column rows (tap y)
PAGE_TITLE = (650, 80, 1270, 132)
MENU_LIST = (0, 140, 640, 690)
FULL_TITLE = (100, 80, 640, 132)   # a full-screen page's own title, where MOD MENU was
FULL_SCREEN = 0.6   # menu column changed this much: a full-screen page (a right-pane page < 0.2)


def leave_full_screen(i, scr, opened, notes, bad):
    """Community Mods (cores with community mods) opens FULL SCREEN over the menu, so
    the next rows' taps would land on it. Check its own title is drawn, then BACK must
    bring the menu back, with the row's summary in the right pane. Returns that screen."""
    sd = region_stats(scr, FULL_TITLE)[1]
    back = scr
    for _ in range(3):
        azd.panel("back")
        time.sleep(1.5)
        back = screen(f"mm-{i}-back")
        if changed(scr, back, MENU_LIST) > 0.01:
            break                    # BACK did something: judge that, never press again blind
        TAP_RETRIES.append(f"mm-{i}-back")   # nothing moved: the press was dropped
    menu_back = changed(opened, back, MENU_LIST) <= FULL_SCREEN
    notes.append(f"page {i + 1} opens full screen (title contrast {sd:.1f}), "
                 f"BACK {'returns to the menu' if menu_back else 'did NOT return to the menu'}")
    if sd < 5 or not menu_back:
        bad.append(f"page {i + 1} full screen (title contrast {sd:.1f}, menu back {menu_back})")
    return back


def visit_page(i, y, prev):
    """Tap menu row i and return the screen. If the Mod Menu is found CLOSED (the SOURCE
    screen is up: under TCG a late or stray touch can close it, seen on 1.2.2 before row
    7), reopen it and tap the row again, rather than reading SOURCE as page i."""
    for attempt in range(2):
        if i == 0:   # the menu may reopen on page 1 already: one tap, no change expected
            tap((600, y))
            time.sleep(1.5)
            scr = screen("mm-0")
        else:
            scr, _ = tap_until((600, y), f"mm-{i}", PAGE_TITLE, prev)
        if attempt or not _usb_tile_blue():
            return scr
        TAP_RETRIES.append(f"mm-{i}-reopen")
        open_mod_menu()
    return scr


def stage_modmenu():
    open_mod_menu()
    opened = screen("mm-open")
    titles, bad, notes = [], [], []
    prev = opened
    for i, y in enumerate(MENU_PAGES):
        scr = visit_page(i, y, prev)
        if changed(opened, scr, MENU_LIST) > FULL_SCREEN and not _usb_tile_blue():
            scr = leave_full_screen(i, scr, opened, notes, bad)
        titles.append(scr)
        prev = scr
    for i in range(len(titles)):
        sd = region_stats(titles[i], PAGE_TITLE)[1]
        diff = changed(titles[i - 1], titles[i], PAGE_TITLE) if i else 1.0
        if sd < 5 or diff < 0.002:
            bad.append(f"page {i + 1} (contrast {sd:.1f}, vs previous {diff:.3f})")
    notes = "".join(f"{n}; " for n in notes)
    first, _ = tap_until((600, MENU_PAGES[0]), "mm-again", PAGE_TITLE, titles[-1])
    again = changed(titles[0], first, PAGE_TITLE)
    azd.panel("back")
    time.sleep(1.5)
    closed = changed(opened, screen("mm-closed"), MENU_LIST)
    goto_source()
    ok = not bad and again == 0.0 and closed > 0.05
    report("modmenu", ok, f"{len(MENU_PAGES)} pages visited, "
           + (f"wrong: {'; '.join(bad)}; " if bad else "every title distinct and not blank; ")
           + notes + f"CONTROL first page revisited: {again:.4f} of its title changed; "
           f"BACK: menu area {closed:.3f} changed" + _retries())


THEME_DAY, THEME_AZMODS, THEME_LABEL = (1920, 714), (1920, 886), (1500, 596)
AZMOD_CFG = "/home/root/settings/azmod.cfg"


def stage_themes():
    """Day -> AZ-MODS -> Day. azmod.cfg is saved first and put back afterwards (EP147
    stopped while it is restored, since azmod may save on exit), so the theme the
    emulator had before the stage is the theme it has after it."""
    saved = guest(f"[ -f {AZMOD_CFG} ] && cp -p {AZMOD_CFG} /tmp/azmod.cfg.az-emu-test && echo saved")
    try:
        open_mod_menu()
        tap((600, MENU_PAGES[4]))  # Themes
        time.sleep(1.5)
        base = screen("th-base")
        day, _ = tap_until(THEME_DAY, "th-day", MENU_LIST, base, threshold=0.0)
        tap(THEME_LABEL)  # a label, not an option: must change nothing
        time.sleep(1.5)
        noop = changed(day, screen("th-noop"), MENU_LIST)
        dark, _ = tap_until(THEME_AZMODS, "th-dark", MENU_LIST, day, threshold=0.05)
        day2, _ = tap_until(THEME_DAY, "th-day2", MENU_LIST, dark, threshold=0.05)
        azd.panel("back")
        time.sleep(1.2)
    finally:
        if saved == "saved":
            guest(f"systemctl stop EP147.service; cp -p /tmp/azmod.cfg.az-emu-test {AZMOD_CFG}; "
                  "rm -f /tmp/azmod.cfg.az-emu-test; sync", 90)
            restart_ep147()
            log("azmod.cfg restored as found; EP147 restarted")
    l_day, l_dark, l_day2 = (region_stats(x, MENU_LIST)[0] for x in (day, dark, day2))
    ok = l_day - l_dark > 80 and abs(l_day2 - l_day) < 30 and noop == 0.0
    report("themes", ok, f"menu brightness Day {l_day:.0f} -> AZ-MODS {l_dark:.0f} -> Day {l_day2:.0f}; "
           f"CONTROL tap on the THEME label: {noop:.4f} changed" + _retries())


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--azmod")
    ap.add_argument("--azstem")
    ap.add_argument("--stages", default=",".join(STAGES))
    ap.add_argument("--usb", help="USB image to boot with (the stems stage needs \"Basic\" on it)")
    ap.add_argument("--control-boots", type=int, default=3)
    ap.add_argument("--take-over", action="store_true", help="stop a running emulator first")
    a = ap.parse_args()
    stages = [s.strip() for s in a.stages.split(",") if s.strip()]
    if unknown := set(stages) - set(STAGES):
        sys.exit(f"unknown stage(s): {', '.join(sorted(unknown))}")
    pair = {k: v for k, v in (("azmod", a.azmod), ("azstem", a.azstem)) if v}
    for p in pair.values():
        if not os.path.isfile(p):
            sys.exit(f"no such file: {p}")
    test_usb = os.path.join(BUILD, "usb-test.img")
    usb = a.usb or (test_usb if os.path.exists(test_usb) else os.path.join(BUILD, "usb.img"))
    if not os.path.isfile(usb):
        sys.exit(f"no such USB image: {usb}")
    if emu_running() and not a.take_over:
        sys.exit("an emulator is running; quit it (or pass --take-over)")
    if azd._app_running():
        sys.exit("the control app (AZ-EMU / az-preview) is open: quit it "
                 "(it holds the control socket and port 4445)")
    log(f"build {BUILD}, USB {usb}")

    try:
        for stage in stages:
            try:
                if stage == "instrument":
                    stage_instrument(a.control_boots, usb)
                elif stage == "boot":
                    stage_boot(usb)
                elif stage == "mods":
                    stage_mods(pair)
                elif stage == "controls":
                    stage_controls()
                elif stage == "stems":
                    stage_stems(usb)
                elif stage == "drumroll":
                    stage_drumroll(usb)
                elif stage == "fourdeck":
                    stage_fourdeck()
                elif stage == "modmenu":
                    stage_modmenu()
                elif stage == "themes":
                    stage_themes()
            except Fail as e:
                report(stage, False, str(e))
            except Exception as e:  # a harness bug is a failed run, never a pass
                report(stage, False, f"harness error: {type(e).__name__}: {e}")
            if stage == "boot" and not results[-1][1]:
                break  # nothing after a failed boot can be trusted
    finally:
        if emu_running() and guest_up():
            disable_onelibrary_key()
            if pair:
                restore_installed()
    failed = [s for s, ok in results if ok is False]
    notrun = [s for s, ok in results if ok is None]
    passed = len(results) - len(failed) - len(notrun)
    print(f"\n{passed}/{len(results)} stages passed"
          + (f"; FAILED: {', '.join(failed)}" if failed else "")
          + (f"; NOT RUN: {', '.join(notrun)}" if notrun else ""))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
