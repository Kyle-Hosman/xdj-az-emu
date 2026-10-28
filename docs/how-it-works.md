# How it works

What the emulator replaces on a real deck, and where each piece lives in this
repository.

## The machine

QEMU's `virt` machine (64-bit ARM) with the XDJ-AZ's 6 CPUs and 3.76 GB of RAM:

| Real deck | Emulator |
|---|---|
| Rockchip RK3399 | QEMU `virt`, HVF on Apple Silicon, TCG elsewhere |
| the deck's Linux 4.4 kernel | our Linux 6.6 LTS build (`docker/Dockerfile`, pinned commit, `guest/kernel-patches/`) |
| the deck's root filesystem | the same filesystem, pulled from your deck and patched (`initramfs-patch/`) |
| 1280x800 DSI panel | `virtio-gpu` at 1280x800 with EDID off, so the deck program picks the same 1x scale as on the deck |
| touch screen | `virtio-tablet`, classified as an absolute pointer |
| eMMC | `build/az-mnt.img` (14 GB or more, sparse, with discard) as `/dev/mmcblk1`, partitioned like the deck on first boot (p7 = settings, p8 = cache); p8 grows when the image does |
| USB sticks | `usb-storage` on an EHCI controller, from `build/usb.img` |
| front-panel MCUs (4 SPI buses) | the `subucom_virt` kernel module, fed by the host |
| D810 audio interface (10 channels) | our `virtio_snd` module plus `virtio-sound`; `master_out.so` mixes the four decks from the real mixer controls (an approximation of the DSP's mix) for the host's audio output: speakers, a WAV file or nowhere ([Audio output](guide.md#audio-output)), and measures every deck's level for the panel's meters |

The deck program (EP147) runs unmodified. Two things are preloaded into it: the
emulator shim (`guest/ep122_shim/`, which smooths over differences between the deck's
kernel and ours: DRM ioctls, USB bind writes, timing) and, once installed, the AZ-MODS
core.

## Front-panel buses

The deck's controls come from microcontrollers on four SPI buses, which the deck
program reads as `/dev/subucom_spi{1,2,3,5}.0`. Every frame is 128 bytes, protected by
a CRC-16/X-25 (polynomial 0x8408 reflected, init 0xFFFF, final XOR 0xFFFF).

| Bus | Carries | CRC |
|---|---|---|
| spi1 ("erp") | both decks' transport buttons, pad modes, pads, deck select, a few unit-wide buttons | over bytes 0-27, stored little-endian at 28-29 |
| spi2 / spi5 | deck 2 / deck 1: jog wheel (touch, position, velocity), tempo fader | per-deck layout |
| spi3 (mixer) | panel buttons, browse knob, mixer knobs and faders | over bytes 0-95, stored at 96-97 |

`subucom_virt` replays idle frames captured from a real deck, and accepts overrides
from the host: a frame is one bus-id byte plus 152 bytes (128 real plus padding), sent
over the `cdj3k.ctrl` virtio-serial port (`ctrl.sock` on the host). The module
re-stamps the CRC. The guest's `subucom_forwarder` bridges that port to the module, in
both directions: everything the deck program writes to a panel bus comes back the same
way, which is how AZ-EMU lights the pads and buttons. The module keeps the latest write
of each bus (about 19 a second each) and hands them out in turn, so a busy bus never
hides another's; each comes back as one bus-id byte, the write zero-padded to 152 bytes
(a deck bus writes 152, the erp and mixer buses 128), and the write's real length.
Exact bytes and bits for every control are in `tools/az-drive.py` and
`az-preview/src/controls.rs`.

The erp bus's 128-byte frame (spi1) carries the deck lights: bytes 0x0e + 4j are the
RGB of pad-mode button j, 0x1e + 4k the RGB of pad k (deck 1), in linear light: the
deck program dims a light by scaling all three bytes by one factor (HOT CUE's button is
98 ff 9a lit, 07 0c 07 dim). White is not three equal bytes, and differs by kind of
light: 98 ff 9a for a pad-mode button, b3 eb 5a for a pad (an idle SLIP LOOP pad 7 or 8;
an empty HOT CUE pad is the same, dimmed: 08 0b 04). AZ-EMU scales each channel against
its kind's white, then converts to sRGB for the screen, so those show white and their
dimmed forms gray, and other colors keep their hue (`WHITE_PADMODE`, `WHITE_PAD` in
`controls.rs`). 0x08-0x0c hold 2-bit
levels (0 off, 1 dim, 2 on) for buttons such as PLAY, CUE, SLIP and QUANTIZE (the table
is `DECK_LEDS` in `controls.rs`); deck 2 is the same layout 0x38 further on; a
CRC-16/X-25 over bytes 0-0x77 sits at 0x78.

The deck buses (spi2, spi5) also carry messages to the jog displays, one at a time in
a slot at 0x4c: a present flag (0x4c), a sequence number (0x4e; a repeated frame repeats
it), the display (0x50: 0x10 left, 0x20 right), the message id (0x51), then a piece
number from 1 (0x52), the number of pieces (0x54) and 58 bytes of data (0x56). When a
deck loads a track the deck program sends a few 0x2d (the display resets; the last
one carries the track's length in milliseconds, 3 bytes little-endian after one byte),
0x3f, 0x30 and 0x40, then the track's overview waveform (0x2c), and, if the track has
artwork, the artwork as a JPEG in 0x33 pieces (the first starts with the image's
length, 3 bytes little-endian). The overview is one byte (0x90 on every track seen),
then 7 bytes a column: an RGB565 color (little-endian), two zero bytes, the column's
height, the height of its brighter lower part, and a zero; about 600 columns (145
pieces) for an analyzed track, empty columns after the last. Measured: two tracks'
columns drew the same shapes as the deck program's own overview of each, and not each
other's. A 0x2c piece can come before the last 0x2d of a load.

The deck program sends no pixels for these displays: on a deck, each deck's own
microcontroller draws them from these messages. AZ-EMU draws the display's outline as
the deck has it (a wide window with round ends, the artwork's circle bulging above and
below it), the artwork in that circle until the track's overview waveform arrives,
then, as the deck usually shows it, the waveform along the bottom of the window with
the playing position, on a gray background in place of the artwork, and the SLIP,
VINYL, SYNC and MASTER labels around it, lit from the buttons' LEDs (`JogDisplay` and
`parse_overview` in `controls.rs`). No message seen says which of these views the
deck's display is in, so AZ-EMU shows the waveform whenever it has one: a track with
no analysis sends none and keeps its artwork.
The pad-mode screen and the hot cue markers the deck draws in the window are not
drawn. The messages come only with a load, so an AZ-EMU that attaches to a running
emulator shows a deck's display once that deck loads a track. Each piece rides one
deck-bus write, a few hundred of them at about 19 writes a second, and a piece the
host misses drops that copy until the next one, so a cover can take a minute or more
to appear after a load. Each deck bus carries two status records, named by byte 8:
0x31 is deck 1 on spi5 and 0x41 deck 2 on spi2 (0x11 and 0x21, the other two, read "no
track" in every capture, so they are most likely decks 3 and 4). Their bytes 0x24 on
carry the elapsed time: minutes,
seconds, milliseconds (u16); `bf ff ff` when no track is loaded.

## Audio levels

The deck program writes its audio as one 10-channel stream, the D810's inputs: channels
0/1 are deck 1, 2/3 deck 2, 4/5 deck 3, 6/7 deck 4. Channels 8/9 (zero-based indexing)
feed the LINK CUE headphone monitoring path, as verified on physical XDJ-AZ hardware
running firmware 2.01 with AZ-MODS 1.2.5. A 1 kHz test tone injected into channels
8/9 of EP147's existing ALSA output stream was audible in headphones with LINK CUE
enabled and inaudible with LINK CUE disabled. The test tone was not present on the
MASTER, BOOTH or REC outputs during testing. These findings establish the observed
routing of channels 8/9 under the tested conditions.
Each pair is the deck's output before the mixer: trim, EQ, the faders, the crossfader
and the master level are applied after it, by the DSP, so the stream holds no master
mix (measured: playing one deck alone moves only its pair; that deck's channel
fader at the bottom, its LOW knob at either end, or the master LEVEL at zero leaves the
pair unchanged; setting every mixer byte to either end moves channels 0/1 by at most
2 dB). `master_out.so` copies the
stream into a ring on the deck program's audio thread, and its writer thread measures
each channel's peak and RMS over 1,024-frame blocks (about 23 ms) into a shared page,
`/dev/shm/az-levels`. `subucom_forwarder` sends each new block to the host as a frame
from bus id 4: a sequence number, then ten peaks and ten RMS values as little-endian
floats (fractions of full scale). AZ-EMU drives the CH 1-4 level meters from them,
with standard PPM ballistics (IEC 60268-10 Type II: instant rise, 24 dB fall in 2.8 s)
and 0 dB on the meters at -18 dBFS (EBU R68), both assumptions rather than
measurements of an XDJ-AZ (`meter.rs`). The MASTER meter stays dark: the deck program
never computes the master mix, and the emulator's own mix (below) is an approximation.

For the host's audio output `master_out.so` mixes the four decks itself, on its writer
thread: each deck through its channel fader and trim, then the crossfader as the deck is
assigned, then the master level. It reads the controls the way the deck program does:
the mixer bus frame (opening `/dev/subucom_spi3.0` read-only, which takes nothing from
the deck program) every 20 ms, and the settings the deck program writes to the mixer
(crossfader assign, curves) from `/dev/shm/az-mixer-tx`, where `subucom_forwarder`
keeps the deck program's latest mixer-bus write. The curves are approximations, given
in `master_out.c`; EQ, color FX and Beat FX are not modeled. `tools/az-capture.py`
records the four decks before the mixer, and with `--mix` the mix as well.

## The mixer bus

Every analog mixer control is a 10-bit value: a high byte, and two low bits packed four
controls to a byte (the first control in bits 7-6). Found by setting one byte at a time
and reading back, from the deck program's memory, the value of its own control object
for each named control (`huiid::Mixer::Ch<n>::fader()`, `Knobs::masterLevel()` and so
on):

| Bytes (low bits) | Controls |
|---|---|
| 0x0a-0x0d (0x0e) | channel faders 1-4 |
| 0x0f (0x10, bits 1-0) | crossfader |
| 0x24-0x27 (0x28), 0x29-0x2c (0x2d), 0x2e-0x31 (0x32), 0x33-0x36 (0x37) | channel 1, 2, 3 and 4: TRIM, HI, MID, LOW each |
| 0x38-0x3b (0x3c) | master LEVEL, HI, MID, LOW |
| 0x3d-0x40 (0x41) | channel 1-4 COLOR |
| 0x45-0x50 | Beat FX LEVEL/DEPTH, BOOTH, headphones MIX and LEVEL, Sound Color FX PARAMETER (0x4e) |

The deck program's mixer-bus writes carry its MIXER SETTINGS: crossfader assign at 0x06
(two bits per deck, deck 1 in bits 7-6: 0 THRU, 1 A, 2 B), the channel fader curve in
bits 1-0 of 0x08 and the crossfader curve in bits 3-2.

The browse encoder and a few panel buttons also exist as GPIO lines (`/dev/gpiodrv`
on the deck: the file offset is the GPIO number and `read()` returns that pin's
level). The shim models those levels, and `gpio_inject.c` lets the host drive them
over the `cdj3k.gpio` port.

## Patches to the deck's system

`initramfs-patch/patch-rootfs.d/` runs in order at build time, inside a Linux container.
Among them: stub the display-bridge I2C check, load our kernel modules, create
`/tmp/testmode=off` (the MCU query that normally writes it is not emulated), stand in
for the absent Wi-Fi module, configure Xorg for virtio-gpu and leave it unpinned, partition the emulated eMMC, move the deck program's window to
(0,0) (the real panel is rotated), set up ALSA for the 10-channel device, re-announce
USB sticks after a restart, widen the deck program's restart limits, start our
key-only SSH server, and set what the deck program preloads.

## Where mods live

Exactly where a deck keeps them, on the emulator's persistent settings partition:

- `/home/root/settings/az-usbload/azmod.so`, `azstem.so`: the AZ-MODS core
- `/home/root/settings/azmod-pngpatch/`: its image data
- `/home/root/settings/az-usbload/community/<id>/`: Community Mods (on the USB they are in
  `MODS/community-mods/<id>/`, turned on and off in `MODS/azmods.cfg`)
- `/home/root/settings/azmod-community/approved`: the approved build (SHA-256) of each
  Community Mod

They get there exactly as on a deck. `tools/az-emu-pull.py` keeps the AZ-MODS loader
that a deck with AZ-MODS (firmware 2.01) carries: `/usr/sbin/az-usbload`,
`az-usbload.service`, `/usr/sbin/az-usbpick`, `/home/root/scripts/usb-renotify.sh`,
`/etc/az-mods-release` and `EP147.service.d/10-azmod.conf`, and patch 31 leaves them in
place. At every boot EP147 starts first with the set staged last time
(`10-azmod.conf` reads `az-preload.env` from the stage); then `az-usbload.service`
picks the USB, chain-loads the newer loader in the USB's `MODS/deck/`, copies
`MODS/` into the stage, runs the USB's boot hooks (`MODS/deck/boot.d/`) and writes
`az-preload.env`. From AZ-MODS 1.2.3 on, the USB's loader loads only what the set's
signed trust pins name, or anything with the developer switch
`/home/root/settings/azmods-unsigned.on`, and never restarts an EP147 that already runs
mods: it writes `/run/az-reload-pending` and the Mod Menu says "Reload to apply.". Its
log is `/tmp/az-usbload.log`. The emulator adds only its own libraries, appended to
EP147's `LD_PRELOAD` in `apl_start.sh`, after the loader's.

`tools/az-mods-fetch.py` writes the signed published set to the emulated USB,
`tools/az-push-mod.py --core` writes your build there and turns the developer switch on;
both then run `az-usbload.service` against the USB (the emulator's stand-in for the
power cycle a deck needs) and reload. A real USB (`AZ_USB_FILE=/dev/...`) gets
nothing from these tools: the emulated deck uses it as a deck would, reading and writing it,
with whatever AZ-MODS set and mods it already carries. A system without the
loader (a stock 2.00 deck) gets `EP147.service.d/10-qemu.conf` instead, which preloads
the stage directly, and the tools copy the set there.
