#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""az-drive.py - drive the emulated XDJ-AZ from the command line: touch, panel
buttons, the browse knob, deck buttons, pads, jog wheels and tempo faders.

    python3 tools/az-drive.py <command> [args]

  tap X Y                 tap the touch screen at pixel X Y (2560x1600 space,
                          i.e. screenshot pixels x2)
  shot NAME               save a screenshot to $AZ_SHOT_DIR/NAME.png
  seq 'X Y' 'X Y' ...     tap a sequence (0.8 s apart), screenshot after each
  panel NAME [SECONDS]    press a panel button (`buttons` lists them). The screen
                          tabs, SHORTCUT and MENU are held 1 s; the rest, BACK
                          included, 0.15 s (a long BACK closes a whole menu)
  menu                    SHORT press of MENU (the screen's context menu;
                          `panel menu` is a long press, which opens UTILITY)
  knob cw|ccw [N]         turn the browse knob N detents
  knobclick               push the browse knob
  knobhold [SECONDS]      hold the browse knob down
  press DECK NAME [SECONDS] [shift]
                          a deck button on deck 1|2 (`buttons` lists them), with
                          that deck's SHIFT held if you add `shift`
  padmode DECK MODE       hotcue|beatloop|sliploop|beatjump on deck 1|2
  pad DECK N              performance pad N (1-8) on deck 1|2
  load DECK               the LOAD button for deck 1-4
  tempo DECK PCT          set deck 1|2's tempo fader to PCT (for example 6.1 or -3)
  jog DECK scratch|hold [STEP]   touch and turn (or just touch) deck 1|2's jog wheel
  mixer NAME|BYTE VALUE   set a mixer control (0-255) and keep it: fader1 xfader ...
  fxselect N              Beat FX SELECT position 0-15
  fxon                    press Beat FX ON/OFF
  xpad POS [SECONDS]      touch the X-PAD strip at POS 0-127 (DRUM ROLL rate zone,
                          left to right); the deck must be armed (SLIP LOOP + pad 8)
  buttons                 list every named control
  waveform                go to the WAVEFORM view (BROWSE twice)
  source                  go to the SOURCE screen from any view

Every command works while AZ-EMU.app is open. Screenshots and taps use the
automation QMP socket (4446); AZ-EMU.app keeps 4445. Hardware controls go into
the guest's /dev/subucom_ctrl: directly over the ctrl.sock socket when the app is
closed, otherwise over SSH (the app holds the socket, and QEMU allows one client).

The bus frame layouts below were measured on a real deck: each control is a bit
or a field in one of the front-panel MCU frames, and the guest's subucom_virt
module re-stamps the frame's CRC. Each bus holds ONE frame that every new frame
replaces whole, so the state that must survive between commands (knob counter, jog
position, tempo, mixer levels) is kept in files next to ctrl.sock, and every boot
deletes them.
"""
import json
import os
import socket
import struct
import subprocess
import sys
import time
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import azemu  # noqa: E402

W, H = 2560, 1600
SHOT_DIR = os.environ.get("AZ_SHOT_DIR", os.path.join(azemu.SOCK_DIR, "shots"))

# ---- the three MCU buses ------------------------------------------------------
# A control frame on ctrl.sock is [bus id] + 152 bytes. Bus ids: 0 = spi1 (the
# "erp" bus: deck buttons and pads, both decks), 1 = spi2 (deck 2's jog/tempo),
# 2 = spi3 (mixer bus: panel buttons, browse knob), 3 = spi5 (deck 1's jog/tempo).
BUS_ERP, BUS_DECK2, BUS_MIXER, BUS_DECK1 = 0, 1, 2, 3
DECK_BUS = {1: BUS_DECK1, 2: BUS_DECK2}

# Idle frames captured from a real deck (128 real bytes, zero-padded to 152).
ERP_IDLE = [0, 1] + [0] * 22 + [1, 0, 0, 0, 57, 72] + [0] * 122
SPI2_IDLE = [2,1,0,0,0,0,83,0,0,0,0,0,255,255,0,0,117,2,6,2,205,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,38,1] + [0] * 112
SPI5_IDLE = [2,1,0,0,0,0,68,0,0,0,104,0,255,255,0,0,70,2,5,2,194,0,105,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,142,134] + [0] * 112
SPI3_IDLE = [3,1,2,1,1,1,8,0,0,0,0,0,0,0,0,124,2,0,0,0,0,0,16,0,0,0,0,0,0,255,0,0,0,0,128,0,123,128,128,128,64,123,128,128,128,0,128,128,128,128,192,128,128,128,128,0,0,128,128,128,0,128,128,128,128,0,128,128,128,42,2,255,0,128,0,192,128,121,128,0,0,0,0,0,1,0,10,0,176,4,244,1,0,88,0,0,82,17] + [0] * 54
assert len(ERP_IDLE) == len(SPI2_IDLE) == len(SPI5_IDLE) == len(SPI3_IDLE) == 152
# Byte 34 is the browse knob's 8-bit counter. The deck program reads a turn as the
# plain difference between two frames, so crossing 255 <-> 0 reads as a huge turn
# the other way. Every boot starts it at 128 (here, in AZ-EMU and in the guest's
# idle frame), which leaves 127 detents each way; knob() stops at the ends.
assert SPI3_IDLE[34] == 128
DECK_IDLE = {1: SPI5_IDLE, 2: SPI2_IDLE}

# ---- sending a frame ------------------------------------------------------------
_ctrl = None
_app_up = None


def _app_running():
    """AZ-EMU.app holds ctrl.sock while it runs. A connect() then does NOT fail: it
    sits in QEMU's accept backlog and every frame silently goes nowhere. So when the
    app is up, always take the SSH path."""
    global _app_up
    if _app_up is None:
        if azemu.WIN:
            out = subprocess.run(["tasklist", "/FI", "IMAGENAME eq az-preview.exe"],
                                 capture_output=True, text=True).stdout
            _app_up = "az-preview.exe" in out
        else:
            _app_up = subprocess.run(["pgrep", "-f", "AZ-EMU.app/Contents/MacOS/AZ-EMU|target/.*/az-preview"],
                                     stdout=subprocess.DEVNULL).returncode == 0
    return _app_up


def send(bus, frame152):
    """Send one frame. Keeps ONE socket connection open and reuses it: reconnecting
    per frame outruns QEMU's accept at ~20 frames/s and the socket then stays refused
    until QEMU restarts."""
    global _ctrl
    payload = bytes([bus] + list(frame152))
    if _app_running() or not hasattr(socket, "AF_UNIX"):
        azemu.ssh("cat > /dev/subucom_ctrl", data=payload, timeout=30)
        return
    for attempt in (0, 1):
        try:
            if _ctrl is None:
                _ctrl = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                _ctrl.connect(azemu.CTRL_SOCK)
            _ctrl.sendall(payload)
            return
        except OSError:
            try:
                if _ctrl:
                    _ctrl.close()
            except OSError:
                pass
            _ctrl = None
            if not attempt:
                time.sleep(0.2)
    azemu.ssh("cat > /dev/subucom_ctrl", data=payload, timeout=30)


# ---- deck buttons and pads (erp bus) -------------------------------------------
# Deck 2's fields are deck 1's plus 8. Byte 10: PLAY 0x02, CUE 0x04. Byte 13: pad
# modes (HOT CUE 0x01, BEAT LOOP 0x02, SLIP LOOP 0x04, BEAT JUMP 0x08). Byte 14:
# pads 1-8 (0x01..0x80). Byte 8/9 bit 0: DECK SELECT (primary / secondary deck).
ERP_BTN = {}
PAD_MODES = ("hotcue", "beatloop", "sliploop", "beatjump")
for _d, _b in ((1, 0), (2, 8)):
    ERP_BTN[(_d, "play")] = (10 + _b, 0x02)
    ERP_BTN[(_d, "cue")] = (10 + _b, 0x04)
    for _i, _m in enumerate(PAD_MODES):
        ERP_BTN[(_d, "padmode_" + _m)] = (13 + _b, 1 << _i)
    for _i in range(8):
        ERP_BTN[(_d, "pad%d" % (_i + 1))] = (14 + _b, 1 << _i)
    ERP_BTN[(_d, "decksel%d" % _d)] = (8 + _b, 0x01)
    ERP_BTN[(_d, "decksel%d" % (_d + 2))] = (9 + _b, 0x01)
# The other deck buttons (deck 1 bytes; deck 2 = +8), checked one by one on the
# emulator against the deck program's screen and LEDs.
DECK_BUTTONS = {
    "loopin": (0x08, 0x40), "loopout": (0x08, 0x20), "reloop": (0x09, 0x08),
    "call-": (0x0C, 0x20), "call+": (0x0C, 0x10), "delete": (0x0C, 0x40), "memory": (0x0C, 0x80),
    "loop4": (0x0C, 0x04), "loop8": (0x0C, 0x08),
    "beatjump-": (0x09, 0x40), "beatjump+": (0x09, 0x20),
    "slip": (0x08, 0x04), "slipreverse": (0x08, 0x80),
    "vinyl": (0x0C, 0x01), "jogmode": (0x0C, 0x02),
    "sync": (0x08, 0x02), "keysync": (0x09, 0x02), "master": (0x08, 0x10),
    "temporange": (0x09, 0x10), "temporeset": (0x09, 0x04), "mastertempo": (0x08, 0x08),
    "track-": (0x0B, 0x08), "track+": (0x0B, 0x04), "search-": (0x0B, 0x02), "search+": (0x0B, 0x01),
    "shift": (0x09, 0x80),
}
for _d, _b in ((1, 0), (2, 8)):
    for _n, (_byte, _m) in DECK_BUTTONS.items():
        ERP_BTN[(_d, _n)] = (_byte + _b, _m)
# QUANTIZE is one bit for the whole unit (it toggles both decks).
ERP_BTN[(1, "quantize")] = ERP_BTN[(2, "quantize")] = (0x0A, 0x01)


def press(deck, name, hold=0.10, shift=False):
    """Press a deck button. shift=True holds that deck's SHIFT across the press: SHIFT
    is a modifier, so its bit rides in the same frame, as on the deck."""
    byte, mask = ERP_BTN[(deck, name)]
    base = ERP_IDLE[:]
    if shift:
        sb, sm = ERP_BTN[(deck, "shift")]
        base[sb] |= sm
        send(BUS_ERP, base)
        time.sleep(0.08)                 # SHIFT down first
    f = base[:]
    f[byte] |= mask
    send(BUS_ERP, f)
    time.sleep(hold)
    send(BUS_ERP, base)
    if shift:
        time.sleep(0.08)
        send(BUS_ERP, ERP_IDLE)          # then SHIFT up


def padmode(deck, mode, hold=0.10):
    press(deck, "padmode_" + mode, hold)


def pad(deck, n, hold=0.10):
    press(deck, "pad%d" % n, hold)


# ---- deck bus: tempo fader and jog wheel -----------------------------------------
# One deck frame carries both the jog and the tempo fader, so each keeps the other's
# last value (deck<N>.state: "position tempo"). Without that, turning the jog snapped
# the tempo fader back to idle.
_DECK_STATE = os.path.join(azemu.SOCK_DIR, "deck%d.state")


def _deck_state(deck):
    try:
        pos, raw = map(int, open(_DECK_STATE % deck).read().split())
        return pos & 0xFFFF, raw & 0xFFFF
    except (OSError, ValueError):
        idle = DECK_IDLE[deck]
        return idle[10] | idle[11] << 8, idle[16] | idle[17] << 8


def _deck_state_set(deck, pos, raw):
    try:
        os.makedirs(azemu.SOCK_DIR, exist_ok=True)
        with open(_DECK_STATE % deck, "w") as f:
            f.write("%d %d" % (pos & 0xFFFF, raw & 0xFFFF))
    except OSError:
        pass


def _deck_frame(deck, touch, pos, vel, raw):
    """Byte 9 = jog touch (0x0f touched), 10-11 = jog position, 12-13 = jog
    velocity, 16-17 = tempo fader (all 16-bit little-endian)."""
    f = DECK_IDLE[deck][:]
    f[9] = touch
    f[10], f[11] = pos & 0xFF, (pos >> 8) & 0xFF
    f[12], f[13] = vel & 0xFF, (vel >> 8) & 0xFF
    f[16], f[17] = raw & 0xFF, raw >> 8
    return f


# Jog bytes 12-13 are a PERIOD, not a signed velocity (decoded from a real deck's
# capture): 0xFFFF = the platter is not moving, a small value = turning fast, never 0.
# A touched-then-released frame with 0 reads as an infinitely fast BACKWARD spin: the
# track runs back to its start and PLAY stops working until CUE. Direction is byte 9:
# 0x0F turning forward, 0x0B turning back, 0 released.
JOG_STOPPED = 0xFFFF
JOG_TURNING = 450        # ponytail: the real deck's median while turned by hand; no speed curve


def tempo(deck, pct):
    """Calibrated on the +-10% range against the deck's own readout:
    raw = 517 + 49.8 * pct."""
    raw = max(0, min(0xFFFF, int(round(517 + 49.8 * pct))))
    pos, _ = _deck_state(deck)
    for _ in range(4):
        send(DECK_BUS[deck], _deck_frame(deck, 0, pos, JOG_STOPPED, raw))
        time.sleep(0.05)
    _deck_state_set(deck, pos, raw)
    return raw


def jog(deck, action="scratch", step=-1500):
    """Touch and turn (scratch) or just touch (hold) the jog for about 0.75 s. The
    position is kept on release: a jump back reads as a fast spin the other way."""
    pos, raw = _deck_state(deck)

    def frame(touch, p, vel):
        return _deck_frame(deck, touch, p, vel, raw)
    for _ in range(25):
        if action == "scratch":
            pos = (pos + step) & 0xFFFF
            send(DECK_BUS[deck], frame(0x0F if step > 0 else 0x0B, pos, JOG_TURNING))
        else:
            send(DECK_BUS[deck], frame(0x0F, pos, JOG_STOPPED))
        time.sleep(0.03)
    send(DECK_BUS[deck], frame(0x00, pos, JOG_STOPPED))
    _deck_state_set(deck, pos, raw)


# ---- mixer bus: panel buttons and the browse knob ---------------------------------
# Byte 6 is a shared press latch (0x00 pressed, 0x08 idle). Byte 32: BROWSE 0x40,
# SOURCE 0x80, TAG LIST 0x20, PLAYLIST 0x10, SEARCH 0x08, MENU 0x04. Byte 33: BACK
# 0x80, TAG TRACK 0x40, SHORTCUT 0x10, LOAD 1-4 = 0x08/0x04/0x02/0x01. Byte 31 bit 0:
# browse knob push. Byte 34: browse knob position, an 8-bit ABSOLUTE counter.
PANEL = {"browse": (32, 0x40), "source": (32, 0x80), "taglist": (32, 0x20),
         "playlist": (32, 0x10), "search": (32, 0x08), "menu": (32, 0x04),
         "back": (33, 0x80), "tagtrack": (33, 0x40), "shortcut": (33, 0x10),
         "trackfilter": (33, 0x20),
         "load1": (33, 0x08), "load2": (33, 0x04), "load3": (33, 0x02), "load4": (33, 0x01),
         "mixersettings": (0x18, 0x10)}   # MIXER SETTINGS (gear): press again to close
LOAD_MASK = {1: 0x08, 2: 0x04, 3: 0x02, 4: 0x01}
# How long panel() holds a button. The screen tabs want about 1 s on the emulator,
# and SHORTCUT and MENU-as-UTILITY are hold gestures. BACK is not: a 1 s BACK is a
# long press, which closes a whole menu instead of going back one level.
PANEL_HOLD = {"source": 1.0, "browse": 1.0, "taglist": 1.0, "playlist": 1.0,
              "search": 1.0, "menu": 1.0, "shortcut": 1.0}
PANEL_SHORT_HOLD = 0.15

# Mixer controls by name -> the high byte the deck program reads them from (each is a
# 10-bit value; the low bits are left alone here). Found by reading the deck program's
# own control objects back while setting one byte at a time; see MX_AT in
# az-preview/src/controls.rs for the low bits. The deck program shows none of them on
# screen, so check their effect by ear, in the emulator's audio output, or in a mod.
MIXER_KNOBS = {"fader1": 0x0a, "fader2": 0x0b, "fader3": 0x0c, "fader4": 0x0d, "xfader": 0x0f,
               "trim1": 0x24, "hi1": 0x25, "mid1": 0x26, "low1": 0x27,
               "trim2": 0x29, "hi2": 0x2a, "mid2": 0x2b, "low2": 0x2c,
               "trim3": 0x2e, "hi3": 0x2f, "mid3": 0x30, "low3": 0x31,
               "trim4": 0x33, "hi4": 0x34, "mid4": 0x35, "low4": 0x36,
               "master": 0x38, "color1": 0x3d, "color2": 0x3e, "color3": 0x3f, "color4": 0x40,
               "scfxparam": 0x4e}
FX_BYTE = 0x16          # Beat FX: SELECT position (Gray code) in bits 4-7, ON/OFF bit 0
_MIXER_STATE = os.path.join(azemu.SOCK_DIR, "mixer.state")

# The knob counter must persist across invocations: every frame carries it, and a
# frame with a different value reads to the deck as a knob turn. Sending the idle
# value from an unrelated command would move the browse selection.
_KNOB_STATE = os.path.join(azemu.SOCK_DIR, "knob.ctr")


def _knob_get():
    try:
        return int(open(_KNOB_STATE).read().strip()) & 0xFF
    except (OSError, ValueError):
        return SPI3_IDLE[34]


def _knob_set(v):
    try:
        os.makedirs(os.path.dirname(_KNOB_STATE), exist_ok=True)
        with open(_KNOB_STATE, "w") as f:
            f.write(str(v & 0xFF))
    except OSError:
        pass


def _mixer_state():
    try:
        return {int(k): int(v) & 0xFF for k, v in json.load(open(_MIXER_STATE)).items()}
    except (OSError, ValueError, AttributeError):
        return {}


def _mixer(off=None, mask=0, pressed=False):
    """A mixer frame from the kept mixer levels and knob counter (a frame built from
    the idle one would snap every fader back to idle)."""
    f = SPI3_IDLE[:]
    for k, v in _mixer_state().items():
        if 0 <= k < len(f):
            f[k] = v
    f[34] = _knob_get()
    if pressed:
        f[6] = 0x00
        if off is not None:
            f[off] = mask
    return f


def _hold(frame, seconds, period=0.02):
    """Hold a press by RE-SENDING the pressed frame. One frame is a 1-2 frame pulse
    on the bus however long you sleep, and the deck samples such a pulse only some
    of the time."""
    end = time.time() + seconds
    while time.time() < end:
        send(BUS_MIXER, frame)
        time.sleep(period)
    send(BUS_MIXER, _mixer())          # always release


# X-PAD touch strip (DRUM ROLL). It rides the mixer bus: byte 0x1d is the 7-bit
# position (0-127, six rate zones left to right) and byte 0x1e a zone bitmap that is
# nonzero while a finger is down (measured on a real deck).
XPAD_POS, XPAD_ZONE = 0x1d, 0x1e
_XPAD_ZONE_BITS = [0x01, 0x03, 0x02, 0x06, 0x04, 0x0c, 0x08, 0x18, 0x10, 0x30, 0x20, 0x60, 0x40, 0xc0]


def xpad(pos, hold=0.30, period=0.01):
    """Touch the X-PAD at `pos` for `hold` seconds, re-sending the frame every
    `period` so the touch is seen across frames, then lift the finger."""
    f = _mixer(pressed=True)
    f[XPAD_POS] = pos & 0x7F
    f[XPAD_ZONE] = _XPAD_ZONE_BITS[min(len(_XPAD_ZONE_BITS) - 1, (pos & 0x7F) * len(_XPAD_ZONE_BITS) // 128)]
    end = time.time() + hold
    while time.time() < end:
        send(BUS_MIXER, f)
        time.sleep(period)
    send(BUS_MIXER, _mixer())


def mixer_set(values):
    """Set mixer bytes {byte: value}, keep them, and send the frame."""
    st = _mixer_state()
    st.update({int(k): int(v) & 0xFF for k, v in values.items()})
    try:
        os.makedirs(azemu.SOCK_DIR, exist_ok=True)
        with open(_MIXER_STATE, "w") as f:
            json.dump({str(k): v for k, v in st.items()}, f)
    except OSError:
        pass
    for _ in range(3):
        send(BUS_MIXER, _mixer())
        time.sleep(0.03)


def panel(name, hold=None):
    if hold is None:
        hold = PANEL_HOLD.get(name, PANEL_SHORT_HOLD)
    off, mask = PANEL[name]
    send(BUS_MIXER, _mixer(off, mask, True))
    time.sleep(hold)
    send(BUS_MIXER, _mixer())


def menu_short():
    """MENU is hold-sensitive: a short press (about 0.08 s, held) opens the current
    screen's context menu, a long press opens UTILITY."""
    off, mask = PANEL["menu"]
    _hold(_mixer(off, mask, True), 0.08, 0.01)


def load(deck, hold=0.12):
    send(BUS_MIXER, _mixer(33, LOAD_MASK[deck], True))
    time.sleep(hold)
    send(BUS_MIXER, _mixer())


def knob(direction="cw", detents=1, step_delay=0.12):
    ctr = _knob_get()
    step = 1 if direction == "cw" else -1
    for i in range(detents):
        if not 1 <= ctr + step <= 254:   # never wrap: see SPI3_IDLE[34]
            print("knob: at the end of its range; restart the emulator to recenter it",
                  file=sys.stderr)
            break
        ctr += step
        f = _mixer()
        f[34] = ctr
        f[6] = 0x00 if i % 2 == 0 else 0x08    # byte 6 bit 3 strobes per detent
        send(BUS_MIXER, f)
        time.sleep(step_delay)
    _knob_set(ctr)
    send(BUS_MIXER, _mixer())


def knobclick(hold=0.35):
    send(BUS_MIXER, _mixer(31, 0x01, True))
    time.sleep(hold)
    send(BUS_MIXER, _mixer())


def knobhold(seconds=1.5):
    _hold(_mixer(31, 0x01, True), seconds)


def waveform_view(settle=1.6):
    """From SOURCE (with tracks loaded) BROWSE twice lands on WAVEFORM; from BROWSE
    one more BROWSE toggles back to WAVEFORM. Do not press SOURCE while already on
    SOURCE: that opens an empty "Please select a source" screen."""
    panel("browse"); time.sleep(settle)
    panel("browse"); time.sleep(settle)


def goto_source():
    """Land on SOURCE from any view: BROWSE always lands on WAVEFORM, and SOURCE from
    WAVEFORM always lands on SOURCE."""
    panel("browse"); time.sleep(2.0)
    panel("source"); time.sleep(2.0)


# ---- touch and screenshots (QMP 4446) --------------------------------------------
def ppm_to_png(ppm, png):
    """QEMU's screendump writes PPM (P6). Convert with the standard library only."""
    data = open(ppm, "rb").read()
    parts, i = [], 0
    while len(parts) < 4:              # magic, width, height, maxval
        while data[i:i + 1].isspace():
            i += 1
        if data[i:i + 1] == b"#":
            i = data.index(b"\n", i) + 1
            continue
        j = i
        while not data[j:j + 1].isspace():
            j += 1
        parts.append(data[i:j])
        i = j
    i += 1
    w, h = int(parts[1]), int(parts[2])
    px = data[i:i + w * h * 3]
    raw = b"".join(b"\x00" + px[y * w * 3:(y + 1) * w * 3] for y in range(h))

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    with open(png, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b""))


class Qmp:
    def __init__(self):
        try:
            self.s = socket.create_connection(azemu.QMP, timeout=8)
        except OSError:
            sys.exit(f"No emulator on QMP {azemu.QMP[0]}:{azemu.QMP[1]}. Is it running?")
        self.s.settimeout(12)
        self.f = self.s.makefile("rw")
        self.f.readline()                       # greeting
        self.cmd({"execute": "qmp_capabilities"})

    def cmd(self, o):
        self.f.write(json.dumps(o) + "\n")
        self.f.flush()
        while True:                             # skip async events, return the reply
            r = json.loads(self.f.readline())
            if "return" in r or "error" in r:
                return r

    def tap(self, px, py):
        ax, ay = round(px / W * 0x7FFF), round(py / H * 0x7FFF)
        self.cmd({"execute": "input-send-event", "arguments": {"events": [
            {"type": "abs", "data": {"axis": "x", "value": ax}},
            {"type": "abs", "data": {"axis": "y", "value": ay}},
            {"type": "btn", "data": {"button": "left", "down": True}}]}})
        time.sleep(0.08)
        self.cmd({"execute": "input-send-event", "arguments": {"events": [
            {"type": "btn", "data": {"button": "left", "down": False}}]}})

    def shot(self, name):
        os.makedirs(SHOT_DIR, exist_ok=True)
        ppm = os.path.join(SHOT_DIR, name + ".ppm")
        png = os.path.join(SHOT_DIR, name + ".png")
        self.cmd({"execute": "screendump",
                  "arguments": {"filename": ppm.replace("\\", "/")}})
        time.sleep(0.5)
        ppm_to_png(ppm, png)
        os.remove(ppm)
        return png


def main():
    a = sys.argv[1:]
    if not a or a[0] in ("-h", "--help"):
        print(__doc__)
        return
    c = a[0]
    if c in ("tap", "shot", "seq"):
        q = Qmp()
        if c == "tap":
            q.tap(int(a[1]), int(a[2])); print("tapped", a[1], a[2])
        elif c == "shot":
            print(q.shot(a[1]))
        else:
            for i, pair in enumerate(a[1:]):
                x, y = map(int, pair.split()); q.tap(x, y); time.sleep(0.8)
                print("step", i, q.shot(f"seq{i}"))
    elif c == "panel":
        panel(a[1], float(a[2]) if len(a) > 2 else None); print("panel", a[1])
    elif c == "menu":
        menu_short(); print("menu (short press)")
    elif c == "knob":
        d = a[1] if len(a) > 1 else "cw"
        knob(d, int(a[2]) if len(a) > 2 else 1); print("knob", d)
    elif c == "knobclick":
        knobclick(); print("knob click")
    elif c == "knobhold":
        knobhold(float(a[1]) if len(a) > 1 else 1.5); print("knob held")
    elif c == "press":
        rest = a[3:]
        hold = float(rest[0]) if rest and rest[0] != "shift" else 0.10
        press(int(a[1]), a[2], hold, shift="shift" in rest)
        print("pressed deck", a[1], a[2], "with SHIFT" if "shift" in rest else "")
    elif c == "mixer":
        b = MIXER_KNOBS[a[1]] if a[1] in MIXER_KNOBS else int(a[1], 0)
        mixer_set({b: int(a[2], 0)}); print("mixer", a[1], "byte", b, "=", a[2])
    elif c == "fxselect":
        n = int(a[1]) % 16
        cur = _mixer_state().get(FX_BYTE, SPI3_IDLE[FX_BYTE])
        mixer_set({FX_BYTE: (cur & 0x0F) | ((n ^ (n >> 1)) << 4)}); print("fxselect", n)
    elif c == "fxon":
        f = _mixer(); f[FX_BYTE] |= 0x01
        send(BUS_MIXER, f); time.sleep(0.15); send(BUS_MIXER, _mixer()); print("fxon pressed")
    elif c == "xpad":
        xpad(int(a[1]), float(a[2]) if len(a) > 2 else 0.30); print("xpad", a[1])
    elif c == "buttons":
        print("deck (press DECK NAME):", " ".join(sorted({n for (_, n) in ERP_BTN})))
        print("panel (panel NAME):", " ".join(sorted(PANEL)))
        print("mixer (mixer NAME 0-255):", " ".join(sorted(MIXER_KNOBS)))
    elif c == "padmode":
        padmode(int(a[1]), a[2]); print("pad mode deck", a[1], a[2])
    elif c == "pad":
        pad(int(a[1]), int(a[2])); print("pad deck", a[1], a[2])
    elif c == "load":
        load(int(a[1])); print("load deck", a[1])
    elif c == "tempo":
        print("tempo deck", a[1], "raw", hex(tempo(int(a[1]), float(a[2]))))
    elif c == "jog":
        jog(int(a[1]), a[2] if len(a) > 2 else "scratch", int(a[3]) if len(a) > 3 else -1500)
        print("jog deck", a[1], a[2] if len(a) > 2 else "scratch")
    elif c == "waveform":
        waveform_view(); print("waveform view")
    elif c == "source":
        goto_source(); print("source screen")
    else:
        sys.exit(f"unknown command: {c} (try --help)")


if __name__ == "__main__":
    main()
