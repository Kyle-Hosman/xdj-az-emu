# Troubleshooting

Messages and symptoms by stage, with the fix for each.

## Building

**"The emulator needs the system from YOUR deck, and it is not here yet."**
`./build.sh` needs `build/initramfs-work-pulled/initramfs.cpio.gz` from
`tools/az-emu-pull.py`. To build only the emulator's own parts before that, run
`./build.sh --no-image`.

**"this deck program ... is not firmware 2.00 or 2.01."**
The system you pulled is from another firmware. Update the deck to 2.00 (or install
AZ-MODS, which brings 2.01) and pull again.

**"Docker is installed but not running."** Start Docker Desktop, OrbStack, or
`colima start`. If Docker Desktop on Windows says it is already running but
`docker info` hangs, its engine is stopped: `docker desktop restart`.

**Docker has no `buildx` command** (`'buildx' is not a docker command`, or
`unknown command: docker buildx`; usually with colima). Docker does not find the Buildx
plugin Homebrew installed: link it as in
[Requirements](requirements.md#mac-apple-silicon-recommended).

**"only N GB free where build/ lives."** Free some space. The kernel build happens
inside Docker's own disk, which on a Mac lives in your home folder. If you are sure
your Docker disk is elsewhere, `AZ_EMU_SKIP_DISK_CHECK=1 ./build.sh`.

**"CRLF line endings found in files that must be LF."** A Windows editor or a
`core.autocrlf` setting changed line endings after checkout. Re-clone, or convert the
listed files back to LF.

**The kernel build fails while downloading Linux.** The build tries kernel.org and
then a GitHub mirror of the same release (the commit is pinned, so either gives the
same source). Check your network and run `./build.sh` again; finished steps are reused.

**`qemu/build.sh` fails in `configure`.** Install the Homebrew packages listed in
[Requirements](requirements.md). QEMU's build needs Python 3.11 or 3.12; set
`QEMU_PYTHON=/path/to/python3.11` if several are installed.

## Starting

**AZ-EMU opens but the screen stays black.** The first start takes up to a minute (on
Windows with few cores, a few minutes). If it stays black:

```sh
tools/az-ssh 'systemctl status EP147 --no-pager'
tools/az-ssh 'journalctl -u EP147 -n 60 --no-pager'
```

The QEMU serial console is in `/tmp/az-emu-m1/serial.log` (Mac) or
`%TEMP%\az-emu-win\serial.log` (Windows).

**"az-boot-m1: missing ..."** It names the file and the command that makes it.

**The title bar names a problem.** At every start AZ-EMU checks the emulator
(`tools/az-emu-health.py`, log: `health.log` in the socket folder) and puts what it
finds in the title bar, each with its fix:

- *the emulator image is older than this checkout*: run `./build.sh --image-only` (or
  `./build.sh` when it says so), then start the emulator again.
- *the emulated deck storage is full*: quit AZ-EMU and open it again; it enlarges
  `build/az-mnt.img`. If the storage is already large, stop the emulator and delete
  `build/az-mnt.img` (it is made again; reinstall your mods with
  `tools/az-mods-fetch.py`).
- *only N GB free on the disk holding az-mnt.img*: free space on that disk. The
  emulated deck storage grows by up to about 10 GB as tracks play.
- *the OneLibrary key flag is missing*: run `./build.sh --image-only`.
- *the deck program is not driving the panel*, or *is not running*: quit AZ-EMU and
  open it again; if it repeats, read the serial log (above).

Run `python3 tools/az-emu-health.py` any time for the same check.

**Every panel light flashes and the library never opens.** The deck program could not
create its track caches: the emulated deck storage is too small or full. The serial
log repeats a `TrackFileCache` failure (its errno reads `Invalid argument` on a
released build, `ENOSPC` on some) every two seconds. Images made before the
default was raised were 8 GB, too small for the five caches (about 10 GB together).
Quit AZ-EMU and open it again: the boot script enlarges `build/az-mnt.img` to 14 GB and
the guest grows its storage into it, keeping your settings and mods.

**No OneLibrary: the USB shows only folders, no playlists or library.** The emulator
needs the OneLibrary key flag, `/home/root/settings/azmod-onelibrarykey.on`, which the
image creates at every start of the deck program. Without it, the image predates that
step: run `./build.sh --image-only` and start the emulator again. (If you created
`azmod-onelibrarykey.off` on purpose, delete it.)

**macOS asks whether AZ-EMU may find devices on local networks.** AZ-EMU talks only to
the emulator on your own computer (local sockets and 127.0.0.1), so it works with
either answer; **Don't Allow** is enough.

**A tool says the emulator is unreachable on 127.0.0.1:2222.** The emulator is not
running, or is still booting (SSH answers a few seconds after Linux starts). Start
AZ-EMU, or `tools/az-boot-m1.sh`, and try again. `python3 tools/az-mods-fetch.py
--wait 300` waits for a booting emulator.

**"No emulator SSH key ... Run ./build.sh first."** The tools use the key `build.sh`
made for this install. If you deleted `build/ssh/`, run `./build.sh` again: it makes a
new key and a new image with it.

**AZ-EMU shows "the emulator did not start".** The boot script stopped; the message
gives its error (on a Mac, the full log is `qemu.log` in the socket folder,
`/tmp/az-emu-m1`).

**AZ-EMU shows "AZ-EMU is already open."** Another AZ-EMU window has the emulator;
QEMU takes one app at a time. Use that window, or close it and open AZ-EMU again.
On a Mac, right after an AZ-EMU closes, is force quit or crashes, the emulator it started
takes up to 6 seconds to stop, and an AZ-EMU opened in that time says this too: open it
again a moment later.

**AZ-EMU attached to an old emulator.** AZ-EMU reuses an emulator that is already
running (one started by `tools/az-boot-m1.sh` or `tools\az-boot-win.ps1`) instead of
starting a second one. Stop that QEMU, then open AZ-EMU again.

**Ports 2222, 4445 or 4446 are taken.** Another emulator (or another program) uses
them. Only one emulator can run at a time.

**The deck program restarts once, early in the boot.** Read
`tools/az-ssh cat /tmp/az-usbload.log` first: on the first boot with a new set (the deck
program started stock), the loader restarts it once on purpose and logs
`EP147 back after ...`. Any other restart is a crash: a `SIGSEGV` banner from
`apl_start.sh` in the serial log (`/tmp/az-emu-m1/serial.log` on a Mac), and
`systemctl show EP147 -p NRestarts` above 0 (a restart by the loader, the Mod Menu or
`systemctl restart` sets it back to 0, so read the serial log too). An intermittent
crash on the deck program's first start can occur on the emulator even with no AZ-MODS
core loaded; systemd starts it again and nothing is lost. If it happens with your mod
loaded, rule your mod out first: `mods = off` in `MODS/azmods.cfg` on the USB (with
`AZ_EMU_NOMODS=1`, so the background check leaves the USB alone) boots the deck program
without mods.

## Mods

**"Reload to apply." in the Mod Menu.** The USB carries a different set from the one
the deck program loaded (the loader staged it at boot, see "What happens at boot" in
the [guide](guide.md#5-the-usb-stick-and-the-loader)). Press Reload, or restart the
emulator.

**The mods did not change after I changed the USB.** The loader reads the USB at
boot, as on a deck. Restart the emulator, then press Reload when the Mod Menu says
"Reload to apply.". `tools/az-ssh cat /tmp/az-usbload.log` says what it found.

**The loader's log says `no genuinely signed trust.pins`.** The set on the USB is not
signed by the AZ-MODS release key (your own build, or a modified set) and the developer
switch is off, so the loader loaded nothing from it. For your own build, use
`az-push-mod.py --core` or turn the switch on (guide step 6); otherwise put a published
set back with `az-mods-fetch.py`.

**`./build.sh` says the system was pulled without its loader.** It was pulled by an
older `az-emu-pull.py`, which removed the deck's AZ-MODS loader. Pull it again.

**"AZ-MODS: could not check for updates (...)"** in the terminal, or in AZ-EMU's title
bar. Every boot checks the mods channel for a newer published set; this boot could
not (no network, the channel unreachable, or the newest set failed its signature
check), so the emulator keeps the set it names, which may be out of date. Fix the
cause, then run `python3 tools/az-mods-fetch.py`. The full log is
`/tmp/az-emu-m1/mods-install.log` (Mac) or `%TEMP%\az-emu-win\mods-install.log`
(Windows).

**`az-mods-fetch.py` says it refused the newest set.** The download did not match the AZ-MODS release
signature or the signed file list. Nothing was installed. Try again later; if it
persists, report it, since it means the download was damaged or tampered with.

**My mod is not listed after `az-push-mod.py`.** Check, in order:

1. The AZ-MODS core is installed (`python3 tools/az-mods-fetch.py --check`) and is
   version 1.2.3 or later (Community Mods start there).
2. The load result printed by `az-push-mod.py`, or Mod Menu > Community Mods >
   Installed. The reasons are the same as on a deck, see "Testing on your deck".
3. `tools/az-ssh 'cat /tmp/azmod-community/loaded'`.

**"New mod found: allow?" appears for my mod.** The build was copied in without
`az-push-mod.py`, which approves the exact build it pushes. Press Allow, or push it
with the tool.

**The deck program keeps restarting after I pushed my mod.** Your mod probably crashes
it. The core's crash protection works here as on a deck. To turn your mod off, set
`community-mods.MY-ID = off` in `MODS/azmods.cfg` on the emulated USB:

1. Copy the USB's settings file to your computer:

   ```sh
   tools/az-ssh 'cat "$(az-emu-modsinstall stick)/MODS/azmods.cfg"' > azmods.cfg
   ```

2. In `azmods.cfg`, change `community-mods.MY-ID = on` to `community-mods.MY-ID = off`.
3. Write it back to the USB and restart the emulator:

   ```sh
   tools/az-ssh 'az-emu-modsinstall azmods.cfg' < azmods.cfg
   tools/az-ssh reboot
   ```

**Start completely fresh.** Stop the emulator and delete `build/az-mnt.img`; the next
`./build.sh` makes a new, empty one (your settings and the loader's stage go with it;
the mods themselves are on the USB, and the loader stages them again at the next
boot).

## Playlists and the rekordbox library

The library views (PLAYLIST, ARTIST, ALBUM and so on) read the encrypted database that
rekordbox exports to the USB (`PIONEER/rekordbox/exportLibrary.db`). The emulator has
no deck provisioning to derive its key from, so before every start of the deck program
it creates `/home/root/settings/azmod-onelibrarykey.on`, which tells the AZ-MODS core to
supply the key. This switch is for the emulator only; never create it on a deck.

If the library views stay empty:

- **The USB has no rekordbox export.** Export the library to the USB from rekordbox
  (it writes the `PIONEER/` folder). Without one, browse by FOLDER.
- **The switch was turned off.** `azmod-onelibrarykey.off` in the same folder turns it
  off; delete that file and restart the emulator.
- **The deck program started without AZ-MODS** (for example with all mods turned off).
  The key comes from the AZ-MODS core, so without it the USB browses by folder.
- **The image was built before this switch existed.** Rebuild it with `./build.sh`.

**BROWSE stays empty with a "Waiting..." box on a slow or busy computer.** The USB
shows under SOURCE, but BROWSE lists nothing and no track will load, even after the
computer is idle again. The deck program asks for the USB's library once, and when the
computer is starved for CPU (software emulation, or a heavy game running at the same
time) that request times out and the deck program does not ask again. It is a limit of
running the deck program on a slow host, not of the USB or of AZ-MODS: it occurs with
the 1.2.2 and the 1.2.3 cores alike, mostly right after the deck program restarts.

To recover, close what is loading the computer, then restart the deck program:

```
tools/az-ssh 'systemctl restart EP147'
```

If BROWSE is still empty after that, quit and start the emulator again. To make it rare,
give the emulator a computer that is not busy with something else, and use hardware
acceleration where your system offers it (see [requirements](requirements.md)).

## Audio

No sound by default: the emulator starts with audio output `off`, so tests stay quiet
and nothing grows on disk. To hear it or record it, see
[Audio output](guide.md#audio-output).

- **A recording is empty (44 bytes):** QEMU was built before
  `qemu/patches/13-virtio-snd-bypass-coreaudio-only.patch`. Run `qemu/build.sh` again.
- **A recording will not open, or plays nothing:** run
  `python3 tools/az-wav-fix.py <file.wav>`; QEMU leaves the header's lengths at 0.

