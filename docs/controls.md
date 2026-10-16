# Controls

How to work every control of the emulated deck: by mouse in AZ-EMU, or from scripts
with `tools/az-drive.py`.

## In AZ-EMU

- **Screen:** click to touch (a click is a tap; there is no swipe).
- **Screen only / full deck:** the small button at the top-right corner of the screen
  (in the full deck, of the screen's housing, level with the screen tabs) switches
  between just the screen and the whole deck with its controls. AZ-EMU starts
  in the screen-only view; start it with `AZ_FULL=1` (on a Mac:
  `open --env AZ_FULL=1 AZ-EMU.app`) to open on the whole deck.
- **Buttons, pads, pad modes, LOAD, the panel buttons:** click them. A click is held
  for at least 0.1 s so the deck program always sees it; hold the mouse down for a
  long press.
- **Browse knob:** drag around it to turn, click its center to push.
- **Jog wheels:** drag around the wheel. The grip ring and the white position mark
  on the outer band turn with it; the jog display in the center stays upright.
- **Tempo faders:** drag.
- **X-PAD** (the touch strip on the Beat FX panel): press or drag along it.
- **Mixer and Beat FX knobs:** drag up or down. Beat FX TIME is an endless encoder:
  each step of the drag is one detent. MIC 1 and MIC 2 LEVEL and JOG FEEL send
  nothing: on a deck they are an analog gain and a mechanical tension, not on the
  panel bus.
- **What the mixer controls do here:** every mixer button and knob sends what a deck's
  panel sends. On a deck the mixer hardware acts on most of them, and the emulator
  has none: the EQ, COLOR, BOOTH and headphone knobs, the Sound Color FX and the Beat
  FX are not in its sound (see [Audio output](guide.md#audio-output)), the CUE and
  effect buttons light no LED, and the screen's BEAT FX panel does not follow BEAT,
  TIME, TAP, AUTO/TAP, FX QUANTIZE or the channel buttons, because a deck shows there
  the state its mixer reports back.
- **Pad colors:** the pads show the colors the deck program sets, read from the same
  LED data a real deck's pads get, converted from the LEDs' linear light to screen
  color (see [How it works](how-it-works.md)).

## From the command line: `tools/az-drive.py`

Every control can be pressed from a script, which makes tests repeatable. It works
while AZ-EMU is open.

| Command | Does |
|---|---|
| `tap X Y` | touch the screen at X Y (in 2560x1600 units: a screenshot pixel times two) |
| `shot NAME` | screenshot to `NAME.png` in the shots folder (path printed) |
| `seq 'X Y' 'X Y' ...` | taps in a row, a screenshot after each |
| `panel NAME [S]` | `source` `browse` `taglist` `playlist` `search` `menu` `shortcut` (held 1 s), `back` `tagtrack` `trackfilter` `load1`..`load4` `mixersettings` (0.15 s). S sets the hold. A long `back` closes a whole menu |
| `menu` | a SHORT press of MENU: the current screen's menu (`panel menu` is a long press, which opens UTILITY) |
| `knob cw\|ccw [N]` | turn the browse knob N detents |
| `knobclick`, `knobhold [S]` | push the knob, or hold it down for S seconds |
| `press DECK NAME [S] [shift]` | any deck button on deck 1 or 2, for example `play` `cue` `pad1` `sync` `loopin` `beatjump+` `temporange` `quantize` (`buttons` lists them all). Add `shift` to hold that deck's SHIFT |
| `padmode DECK MODE` | `hotcue` `beatloop` `sliploop` `beatjump` |
| `pad DECK N` | performance pad 1-8 |
| `load DECK` | the LOAD button for deck 1-4 |
| `tempo DECK PCT` | move deck 1 or 2's tempo fader to PCT percent (for example `6.1` or `-3`, on the +-10% range) |
| `jog DECK scratch\|hold [STEP]` | touch and turn a jog wheel, or touch and hold it |
| `mixer NAME\|BYTE VALUE` | set a mixer control's high byte to 0-255: `fader1`-`fader4` `xfader` `master` `trim1` `hi1` `mid1` `low1` ... (`buttons`). Kept until the next boot |
| `fxselect N`, `fxon` | Beat FX SELECT position 0-15; press Beat FX ON/OFF |
| `xpad POS [S]` | touch the X-PAD strip at POS 0-127 (left to right) for S seconds; the drum roll needs the deck armed first (SLIP LOOP + pad 8) |
| `buttons` | list every named control |
| `waveform` | go to the WAVEFORM view |
| `source` | go to the SOURCE screen from anywhere |

Tips:

- Check the result with a screenshot (`shot`) rather than assuming a tap landed:
  screen layouts move when rows are added.
- The browse knob position, the jog position, the tempo faders and the mixer levels
  are remembered between commands, so a `knob` followed by `knobclick` behaves like a
  real hand on the knob, and a jog turn does not move the tempo fader. The knob and
  the mixer levels are shared with AZ-EMU (`knob.ctr` and `mixer.state` in the
  socket folder: `/tmp/az-emu-m1` on a Mac, `%TEMP%\az-emu-win` on Windows), so a
  script keeps the faders and the master where AZ-EMU set them.
  Every boot starts them from idle, and AZ-EMU then sets its own starting positions.

## Jog displays

In the full deck view, each jog wheel's center shows what the deck program sends that deck's jog display: the
track's artwork, then its overview waveform with the playing position once that has
arrived. Two clicks on a deck's SHIFT within 400 ms switch that display between the
artwork and the waveform, as a double press does on the deck; SHIFT's own state is left
as it was. The display fills in only when a deck loads a track, so an AZ-EMU that
attaches to a running emulator shows it after the next load.

## Other tools

| Tool | Does |
|---|---|
| `tools/az-ssh [COMMAND]` | a root shell in the emulator, or one command |
| `tools/az-emu-pull.py --host IP --key KEY` | copy the system from your deck (read-only on the deck) |
| `tools/az-mods-fetch.py [--check] [--release TAG]` | put the published AZ-MODS set on the emulated USB, checked against its signature; `--check` only reports CURRENT or STALE |
| `tools/az-push-mod.py DIR` | load your mod; `--core azmod.so azstem.so` loads your own core build |
| `tools/az-emu-test.py` | the test suite ([guide](guide.md#8-run-the-test-suite)) |
| `tools/az-emu-health.py` | the health check AZ-EMU shows in its title bar |
| `tools/az-capture.py start [--mix]`, `stop` | sample-exact recording of the four decks ([guide](guide.md#sample-exact-capture)) |
| `tools/az-wav-fix.py FILE.wav` | fill in a recording's WAV header so players open it |
| `tools/az-usb2.sh [IMAGE]` | plug a second USB stick (USB2) into the running emulator |
| `tools/az-macusb.sh VOLUME` | (Mac) use a real USB plugged into your Mac as USB1; `off` to go back. Needs `touch build/macusb.on` and an AZ-EMU restart first |
| `tools/make-usb.sh [MB] [FILE]` | make an empty USB image (default 256 MB, `build/usb.img`) |
| `tools/az-boot-probe.py SERIAL_LOG OUT_DIR` | time a boot: serial milestones plus a screenshot every 20 s |
| `tools/az-emu-bench-win.py LABEL` | (Windows) one measured headless boot per setting |

## Ports

| Port (127.0.0.1 only) | Used by |
|---|---|
| 2222 | SSH into the guest (key-only) |
| 4445 | AZ-EMU's QMP connection. Leave it alone: QEMU allows one client per QMP port, and AZ-EMU needs it for the screen and the controls |
| 4446 | QMP for scripts and tools |
