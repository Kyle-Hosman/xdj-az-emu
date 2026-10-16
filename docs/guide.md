# Develop on the emulator

From a fresh checkout to your own mod running in the emulator. The emulator runs the
XDJ-AZ's own deck program with the same AZ-MODS core a deck runs, so a build can be
tried in seconds instead of copying it to a USB and power-cycling.

No firmware ships with it: you copy the system from **your own** rooted XDJ-AZ once.
Everything else is built from source in this repository.

> [!NOTE]
> **Supported:** a Mac with **Apple
> Silicon** (the main platform) or a **Windows** PC (slower, see
> [Windows](windows.md)). Intel Macs and Linux are covered in
> [Requirements](requirements.md).
>
> The commands below say `python3`, as on a Mac. On Windows, type `python` instead.

## How it fits together

```
your deck ──(SSH, once)──> build/initramfs-work-pulled/   the deck's system with its AZ-MODS
                                     │                    loader, stays on your computer
this repository ──./build.sh──> build/Image + build/initramfs-patched.cpio.gz
                                     │
                         AZ-EMU.app ─┴─> QEMU ──> the deck program, its screen in a window
                                                     ▲
                    the deck's USB loader, at every boot, from the USB stick's MODS/
                                                     ▲
             build/usb.img (the emulated stick) or a real stick, written by
             tools/az-mods-fetch.py  (the published AZ-MODS set, signature checked),
             tools/az-push-mod.py    (your mod, or your own core build),
             or the AZ-MODS app      (a real stick)
```

The emulator loads mods exactly as a deck does: from `MODS/` on the USB stick, through
the deck's own loader, at every boot. The tools write the USB and let the loader
stage the set, so the deck's storage never holds an AZ-MODS core the USB does not
carry: an older build left there is replaced from the USB at the next boot.

## 1. Install the tools

On a Mac, in Terminal (skip what you already have). First the Xcode
command line tools (compilers, `git`, `swift`). It opens a dialog; let the install finish before
the next block:

```sh
xcode-select --install
```

Then Homebrew, QEMU's build packages and Rust:

```sh
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
eval "$(/opt/homebrew/bin/brew shellenv)" # Homebrew on PATH in this shell; the installer
                                          # prints the line that keeps it there
brew install glib pixman libslirp ninja pkgconf python@3.11
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"                      # Rust on PATH in this shell
```

Docker: install [Docker Desktop](https://www.docker.com/products/docker-desktop/) or
[OrbStack](https://orbstack.dev) and start it, or use colima as in
[Requirements](requirements.md#mac-apple-silicon-recommended). `docker buildx version`
must print a version. The first build needs about **15 GB** of free disk space.

Then get the repository:

```sh
git clone https://github.com/Kyle-Hosman/xdj-az-emu.git
cd xdj-az-emu
```

Every command below runs in this folder. What each tool is for, and what other
computers need: [Requirements](requirements.md). Windows: [Windows](windows.md).

## 2. Copy the system from your deck

You need a deck with AZ-MODS installed and root SSH access to it. The AZ-MODS desktop
app turns SSH on from a USB stick it has set up:

1. Put the USB in your computer and open the AZ-MODS app.
2. Under the USB's **Optional** settings, tick **SSH access** (marked *Developer*).
   The first time, the app creates a key pair on your computer, writes the public key
   to the USB, and shows where the private key is. **Keep the private key:** it is
   the only way in.
3. Put the USB in the deck and power the deck on. SSH starts at boot when the USB
   has it turned on.
4. With AZ-MODS 1.2.3 and later, the deck then asks **Remote access** with the key's
   fingerprint, once no deck has a track loaded. **Allow** records the key on the deck;
   Cancel leaves it waiting, and the deck asks again at its next start.
5. Find the deck's IP address (your router's list of devices, or the deck's network
   settings) and check that you can log in. The app keeps the private key in
   `.azmods/ssh/` in your home folder:

   ```sh
   ssh -i ~/.azmods/ssh/xdjaz_ecdsa root@DECK-IP
   ```

   Access is key-only; there is no password. On Windows, the key file must be readable
   only by you (see [Windows](windows.md#windows-specific-notes)).

To turn SSH off again, untick **SSH access** in the app; the next power-on with that
USB turns it off.

Then copy the system, with the same IP address and private key:

```sh
python3 tools/az-emu-pull.py --host DECK-IP --key ~/.azmods/ssh/xdjaz_ecdsa
```

The tool only reads from the deck, and stops if the deck program restarts while it
works. It writes `build/initramfs-work-pulled/initramfs.cpio.gz`, after checking that
the deck program in the copy is the one your deck runs, and scanning it for anything
that must never be in an image, such as keys. If the scan finds anything, it writes
nothing and exits with an error.

The copy keeps the AZ-MODS **loader** that the AZ-MODS install added to your deck
(firmware 2.01): `az-usbload`, its service, and the drop-in that hands the mods to the
deck program. The emulator runs it as the deck does, so mods load from the USB stick.
The tool prints `AZ-MODS loader: kept (8 entries)`. A deck without AZ-MODS (stock 2.00)
has no loader; the emulator then still runs, but loads mods by copying them into the
deck's storage, which is not how a deck does it, and `./build.sh` warns about it.


## 3. Build

```sh
./build.sh          # kernel, virtual hardware, tools, and the boot image
qemu/build.sh       # QEMU itself (once; this takes a while)
tools/build-app.sh  # AZ-EMU.app, the control app (once)
```

The first `./build.sh` downloads Linux and compiles it inside Docker, which takes 10
to 20 minutes; later builds reuse that work. It also creates:

- `build/ssh/az-emu_ed25519`, an SSH key made just for this install. The emulator
  accepts only this key (no password logins), on `127.0.0.1:2222`, which nothing
  outside your computer can reach.
- `build/az-mnt.img`, the emulated deck storage, 14 GB (`AZ_EMU_MNT_GB` sets another
  size). Your settings and installed mods live here and survive restarts. `./build.sh`
  never replaces it. The deck program keeps five track caches of about 2 GB each on it,
  so the emulator enlarges an older, smaller image at start. The file is sparse: it
  takes only the space the guest has written, up to about 10 GB as tracks play, and
  space the guest frees goes back to your disk (on a Mac).
- `build/usb.img`, an empty emulated USB stick.

`./build.sh` stops with a clear message if the pulled system is missing or is not
firmware 2.00+. Its image step prints whether the loader is in the image:

```
[image] AZ-MODS loader: present (release 2.01); mods load from the USB stick through it
```

On Windows, or with a `build/docker-out/` copied from another computer (the kernel
build needs about 10 GB of disk), `./build.sh --image-only` makes only the boot image.

## 4. Run

```sh
open AZ-EMU.app
```

AZ-EMU starts the emulator in the background and shows the deck's screen. The first
start takes up to a minute; the deck program then draws its SOURCE screen. Click the
screen to touch it. The small button at the screen's top-right corner (in the full
deck, the screen housing's) switches between the screen only and the full deck with
its controls.

Closing AZ-EMU stops the emulator it started: QEMU is asked to quit, which writes out
its disk caches, and is stopped after 6 seconds if it has not. For the deck's system
this is a power cut, with no shutdown: run `tools/az-ssh sync` first if the deck just
saved settings you want to keep. The same happens however AZ-EMU ends, a force quit or
a crash included: the emulator it started stops with it.

AZ-EMU opened while an emulator started by `tools/az-boot-m1.sh` runs attaches to it,
and closing it then leaves that emulator running. A second AZ-EMU opened while one is
open neither starts nor attaches to an emulator; its screen says the other window has
it.

## 5. The USB stick and the loader

### The USB

`./build.sh` made `build/usb.img`, an empty emulated USB stick, and AZ-EMU plugs it into
USB1 at every start. The mods go into its `MODS/` folder, laid out exactly as the
AZ-MODS app writes a USB: `azmod.so`, `azstem.so`, `azmod-pngpatch/`, `deck/` (the
loader's own newer copy, its helpers and its boot hooks) and `azmods.cfg` (the settings).

To fill it with the newest published set, with the emulator running:

```sh
python3 tools/az-mods-fetch.py
```

This downloads the set and checks it as the AZ-MODS app does: nothing is written unless
the release signature matches the AZ-MODS release key and every file matches the signed
list. It writes the set to the USB's `MODS/`, turns the developer switch off (see
step 6), and runs the deck's loader against the USB (below). The **MOD MENU** row
then appears on the SOURCE screen, as on a deck.

From then on, every start of AZ-EMU runs the same check in the background and writes a
newer release to the USB when there is one. The log is
`/tmp/az-emu-m1/mods-install.log` on a Mac and `%TEMP%\az-emu-win\mods-install.log` on
Windows. To skip it, start the emulator with `AZ_EMU_NOMODS=1` (on a Mac:
`open --env AZ_EMU_NOMODS=1 AZ-EMU.app`).

**A real USB** works too. A USB the AZ-MODS app wrote is the most faithful test:

- Copy it into an image, which the emulator then uses as its USB:

  ```sh
  tools/make-usb.sh 2048 build/usb.img      # the size in MB must hold everything
  docker run --rm -v "$PWD/build":/out -v /path/to/stick:/src:ro alpine:3.19 sh -c \
    'apk add --no-cache mtools >/dev/null && cd /src && mcopy -s -o -i /out/usb.img * ::/'
  ```

  In Git Bash on Windows, prefix `MSYS_NO_PATHCONV=1` and use `"$(pwd -W)/build"`,
  or the container sees no `/out`.

- Or plug the USB itself in: unmount it (`diskutil unmountDisk /dev/diskN`), then
  start the emulator from a terminal with `AZ_USB_FILE=/dev/rdiskN tools/az-boot-m1.sh`
  and open AZ-EMU from that terminal. The emulated deck reads and writes the USB as a
  deck does. `az-mods-fetch.py` and `az-push-mod.py` only install to the emulator's own
  USB image, so a real USB needs AZ-MODS and your mod on it already (from the desktop
  app or by copying them over).

### What happens at boot

As on a deck:

1. The deck program starts first, with the set the loader staged last time (from
   `/home/root/settings/az-usbload/`, on the emulated deck storage `build/az-mnt.img`),
   or stock when there is none.
2. The loader (`az-usbload.service`) then picks the USB, hands over to the newer
   loader in the USB's `MODS/deck/`, and copies the USB's set into the stage. From
   AZ-MODS 1.2.3 on it loads only what the set's signed trust pins name.
3. If the USB's set differs from what the deck program loaded:
   - a deck program **running mods** is not restarted: the Mod Menu's **Reload** row
     says **"Reload to apply."**, and Reload (or the next boot) loads the new set
     (loader from AZ-MODS 1.2.3 on; with 1.2.2 and older the loader restarts the deck
     program once, early in the boot);
   - a deck program running **stock** (the first boot with mods) is restarted once,
     early in the boot, onto the new set.

So a change to the USB while the emulator runs applies at the next boot, as on a
deck: restart the emulator (or `tools/az-ssh reboot`), and press Reload when the Mod
Menu says "Reload to apply.". The tools do the same for you: `az-mods-fetch.py` and
`az-push-mod.py --core` run the loader against the USB at once and then reload.

The loader's log is `/tmp/az-usbload.log` (this boot) and
`/home/root/settings/az-usbload/usbload.log` (kept across boots). Read the files, not
the journal: the emulator's journal keeps less than a minute.

```sh
tools/az-ssh cat /tmp/az-usbload.log
```

The first boot with the published set on the USB (shortened; `L` stands for
`LD_PRELOAD=/home/root/settings/az-usbload/azmod.so:/home/root/settings/az-usbload/azstem.so`):

```
[usbload 03:30:07] start (firmware loader; cached first start: EP147 loaded [])
[usbload 03:30:07] using USB1 stock mount /media/usb/sda1
[usbload 03:30:07] deck: using the stick's MODS/deck (set 1.2.3)
[usbload 03:30:07] chain: handing over to deck/az-usbload
[usbload 03:30:07] start: CHAINED loader from MODS/deck (set 1.2.3)
[usbload 03:30:07] add azmod.so          (keep azmod.so when the files are already staged)
[usbload 03:30:07] add azstem.so
[usbload 03:30:07] update azmod-pngpatch/
[usbload 03:30:08] desired [L] != loaded [stock]; env written for next boot
[usbload 03:30:08] hook 10-sshgate.sh: exit 0 after 0.1s
[usbload 03:30:49] restart gate satisfied after 41.4s (system=degraded, EP147 age 30.1s; ...)
[usbload 03:30:50] daemon-reload + restart EP147 (pid 2811) to apply [L] now
[usbload 03:30:51] EP147 back after 0.5s (pid 2811 -> 6622, HuiProcessor up); re-announcing USB
```

The last line says what it decided: `same as loaded` (nothing to do), `EP147 back
after ...` (it restarted the deck program onto the new set), or `NOT restarting` (the
set is staged and waits for Reload):

```
[usbload 03:36:47] update azstem.so
[usbload 03:36:48] restart policy: EP147 is running mods [L]; the stick's set [L] is staged -> NOT restarting (the Mod Menu's Reload row says Reload to apply; the next boot loads it)
```

A set the loader refuses (not signed, and the developer switch off) shows as:

```
[usbload 03:34:41] trust: deck/ has no genuinely signed trust.pins -> no deck script runs and no .so loads this boot
[usbload 03:34:41] trust: azmod.so is not pinned by a genuinely signed set -> not loaded
[usbload 03:34:41] no verified mods in MODS/ -> desired: none (stock)
```

The stage keeps its files; the env says stock, so the next Reload or boot starts the
deck program without mods.

## 6. Load your mod

> [!NOTE]
> Community Mods load in AZ-MODS **1.2.3 and later**. On an older core, `az-push-mod.py`
> stops with "The AZ-MODS core on the emulator has no Community Mods".

Build your mod with the SDK as usual, then:

```sh
python3 tools/az-push-mod.py path/to/beat-counter/
```

The folder needs `azmod.json` and your `.so` (beside it or in `out/`); `icon.png` is
copied too when present. The tool copies them to the emulator's Community Mods folder,
turns the mod on, approves exactly that build, restarts the deck program and prints the
load result. (A deck loads only mod builds it has approved; a mod copied in any other
way waits at "New mod found: allow?" until you press Allow.) For example:

```
copied beat-counter to /home/root/settings/az-usbload/community/beat-counter/, enabled and approved it
EP147 restarted in 18 s with beat-counter
load result: (the core's line for your mod, as in Mod Menu > Community Mods > Installed)
```

On a deck, a Community Mod lives on the USB in `MODS/community-mods/<id>/`
(`azmod.json`, `<id>.so`, `icon.png`, `assets/`, and `data/`, the mod's `data_dir()`,
kept across updates), and `community-mods.<id> = on` in `MODS/azmods.cfg` turns it on.
The deck's loader rebuilds the installed Community Mods from the USB at every boot, and
`assets_dir()` and `data_dir()` look there. To install a build by hand, copy the mod
into `MODS/community-mods/<id>/` and add `community-mods.<id> = on` to
`MODS/azmods.cfg`. `az-push-mod.py` also copies the mod, `assets/` included, to the
emulated USB; on firmware without the loader, add `--stick` for that copy:

```sh
python3 tools/az-push-mod.py path/to/beat-counter/ --stick
```

Change your code, rebuild, run the same command again. Files are uploaded as
`<name>.new` and renamed over the old ones, so a loaded library is never changed under
the running program. A push counts as a deliberate restart: a build suspended after a
crash gets a fresh start, and quick pushes in a row never trip the core's start-time
safety check.

Your log works exactly as on a deck:

```sh
tools/az-ssh 'touch /home/root/settings/azmod-logs.on'
tools/az-ssh 'tail -f /tmp/azmod-community/beat-counter.log'
```

Restart the deck program with `tools/az-ssh 'systemctl restart EP147'`; a non-zero
`tools/az-ssh 'systemctl show EP147 -p NRestarts'` means it restarted on its own (a
crash). Anything else you would do over SSH on a deck works here with `tools/az-ssh`
in place of `ssh root@DECK-IP` (see also the SDK's
[Testing on your deck](https://github.com/Kyle-Hosman/az-mods-sdk/blob/main/docs/04-testing.md)).

### Your own build of the AZ-MODS core

A deck's loader loads only a set the AZ-MODS release key signed (AZ-MODS 1.2.3 and
later). Your build is not signed, so it needs the deck's **developer switch**, the file
`/home/root/settings/azmods-unsigned.on`. It lives on the deck's own settings storage,
never on the USB (a USB cannot turn it on; only root on the deck can), and on the
emulator that is `build/az-mnt.img`, so it survives restarts. Without it the loader
refuses the build (`trust: deck/ has no genuinely signed trust.pins` in its log) and
the stage keeps what it had; with it the loader logs
`trust: DEVELOPER OVERRIDE (... present): deck scripts and .so run UNVERIFIED` at every
boot.

One command does all of it:

```sh
python3 tools/az-push-mod.py --core out/azmod.so out/azstem.so
```

It writes your build to the USB's `MODS/`, turns the developer switch on, runs the
deck's loader against the USB, reloads, and prints the md5 of the `azmod.so` the deck
program has mapped. By hand, the same steps are:

```sh
tools/az-ssh 'touch /home/root/settings/azmods-unsigned.on'    # the developer switch
tools/az-ssh 'az-emu-modsinstall azmod.so' < out/azmod.so      # MODS/azmod.so on the stick
tools/az-ssh 'az-emu-modsinstall azstem.so' < out/azstem.so
tools/az-ssh reboot      # the power cycle; then Mod Menu > Reload says "Reload to apply."
```

Press **Reload now** there. That is the loader from AZ-MODS 1.2.3 on (the USB's
`MODS/deck/`); a USB with the 1.2.2 set restarts the deck program onto your build by
itself during the boot. (On a deck: copy the files to `MODS/` on the
USB with your computer, put it back, and power the deck on.)

Your build stays until you run `az-mods-fetch.py` again, which writes the published set
back to the USB and turns the developer switch off. Start the emulator with
`AZ_EMU_NOMODS=1` from then on, or the next start's background check does that for
you. On a deck, the same build from a USB needs the same switch:
`touch /home/root/settings/azmods-unsigned.on` over SSH on that deck.

## 7. Drive it from scripts

Every control can also be pressed from the command line, for repeatable tests:

```sh
python3 tools/az-drive.py panel browse       # the BROWSE button
python3 tools/az-drive.py knob cw 3          # turn the browse knob 3 detents
python3 tools/az-drive.py padmode 1 hotcue   # HOT CUE mode on deck 1
python3 tools/az-drive.py pad 1 5            # pad 5 on deck 1
python3 tools/az-drive.py tap 1280 800       # touch the middle of the screen
python3 tools/az-drive.py shot my-test       # save a screenshot
```

The full list is in [Controls](controls.md).

## 8. Run the test suite

`tools/az-emu-test.py` boots the emulator headless and runs these stages in order.
Each carries a control, a step whose result must differ, so a check that cannot see a
failure fails instead of passing:

| Stage | Checks | Control |
|---|---|---|
| `instrument` | the crash detector works | a crash must be seen: a crash of the deck program in a no-mods boot (up to `--control-boots` tries, default 3), or else one caused on purpose (`kill -SEGV`) |
| `boot` | a cold boot: for 150 s EP147 keeps one process, never restarts, and the serial log shows no crash; then the health check (`tools/az-emu-health.py`): the image matches this checkout, the deck storage holds the track caches, the OneLibrary key flag exists | the `instrument` stage for the crash check; an 8 GB deck storage, not enlarged, must fail the health check |
| `mods` | EP147 has loaded exactly the expected `azmod.so` and `azstem.so` (the installed ones, or `--azmod`/`--azstem`), then runs 150 s without exiting | md5 of the libraries the process mapped, not of the files |
| `controls` | panel controls change their screen region | the browse knob turned past the end of the list changes nothing |
| `stems` | on a playing "Basic" track, the stem pads mute and unmute the deck | with the STEMS master off, the same pads do not silence it |
| `drumroll` | SLIP LOOP + pad 8 arms the drum roll; holding the X-PAD fires hits on the beat grid; pad 8 again disarms | the same X-PAD hold after disarming fires no hits |
| `fourdeck` | the 4 DECK tab shows deck 3 and deck 4 panels that are not blank, and 2 DECK goes back | a blank deck 3 or deck 4 panel fails |
| `modmenu` | every Mod Menu page has its own title, and BACK closes the menu | revisiting the first page gives the identical title |
| `themes` | Day, AZ-MODS, Day on the Themes page: the menu darkens, then returns | a tap on the THEME label changes nothing |

A failed `boot` stops the run. Touch steps check the screen and re-tap when a busy
guest drops a touch; retries are listed in the evidence line. The `drumroll` and
`themes` stages restore every setting and flag they change.

Quit AZ-EMU (or pass `--take-over` to stop a running emulator), install AZ-MODS
(step 5), then:

```sh
python3 tools/az-emu-test.py                              # the installed set
python3 tools/az-emu-test.py --azmod out/azmod.so --azstem out/azstem.so   # your build
python3 tools/az-emu-test.py --stages boot,controls,modmenu   # some stages only
```

Each stage prints PASS or FAIL with its evidence, or NOT RUN when its instrument cannot
work with what it was given; the exit code is non-zero if any stage failed, and the
summary lists the NOT RUN stages. `drumroll` counts hits from a log that release builds
leave out, so with the published set or a build of the public source it reports NOT RUN.
`--azmod`/`--azstem` are pushed by the `mods` stage: include it when you give them. Testing your own build puts the installed set back afterwards. A full
run takes a while; on Windows, set `AZ_QEMU` first.

### The test USB image

The `stems` and `drumroll` stages play a track from the emulated USB. The suite
boots with `--usb IMAGE`, or else `build/usb-test.img` if it exists, or else
`build/usb.img`. The USB must hold:

- **A rekordbox library** (export it from rekordbox to the USB, so it has its
  `PIONEER/` folder). The suite opens USB > Local Library > TRACK and taps the third
  row, so the track **"Basic"** must sort third by title (for example after "Alone"
  and "Alright").
- **Stems for it** in `STEMS/<file name>/`, where `<file name>` is the track's audio
  file name without its extension (for `Basic.mp3`, the folder `STEMS/Basic/`). The
  folder holds four `.ogg` files whose names contain `vocal`, `inst` (or `other`),
  `bass` and `drum`, for example `vocals.ogg`, `instruments.ogg`, `bass.ogg` and
  `drums.ogg`. Names are matched without regard to case.
- **Drum roll samples** in `DRUM ROLLS/Bank 1/` (the bank the Drum Roll page selects):
  `.wav` or `.ogg` files whose names contain `kick`, `snare`, `clap` and `hihat` (or
  `hat`).

The AZ-MODS desktop app writes `STEMS/` and `DRUM ROLLS/`, so the simplest source is
a real USB prepared by rekordbox and AZ-MODS. Copy it into an image (the size in MB
must hold everything):

```sh
tools/make-usb.sh 2048 build/usb-test.img
docker run --rm -v "$PWD/build":/out -v /path/to/stick:/src:ro alpine:3.19 sh -c \
  'apk add --no-cache mtools >/dev/null && cd /src && mcopy -s -o -i /out/usb-test.img * ::/'
```

(In Git Bash on Windows, prefix `MSYS_NO_PATHCONV=1` and use `"$(pwd -W)/build"`.)

### Proving the suite can fail

`tools/test-fixtures/` has deliberately broken libraries: an `azmod.so` that crashes
EP147 and an `azstem.so` with no stems engine. Building them needs only an aarch64
GCC; with Docker:

```sh
docker run --rm --platform=linux/arm64 -v "$PWD":/r -w /r ubuntu:18.04 sh -c \
  "apt-get update -qq && apt-get install -y -qq gcc libc6-dev >/dev/null && \
   mkdir -p build/test-fixtures && cd tools/test-fixtures && \
   gcc -O2 -fPIC -shared -o ../../build/test-fixtures/crashmod.so crashmod.c && \
   gcc -O2 -fPIC -shared -o ../../build/test-fixtures/nostems.so nostems.c"
```

Then `--azmod build/test-fixtures/crashmod.so` must fail `mods` (and the stages after
it), and `--azstem build/test-fixtures/nostems.so` must fail `stems` only. See
[tools/test-fixtures/README.md](../tools/test-fixtures/README.md).

## Music

The emulated USB (`build/usb.img`) starts empty. Copy music into a USB image as in
"The test USB image" above, or on a Mac share a real USB with `tools/az-macusb.sh`.

A USB exported from rekordbox browses as on a deck: PLAYLIST, ARTIST, ALBUM, KEY and
the other library views, as well as FOLDER. Without a rekordbox export it browses by
FOLDER only. See
[Troubleshooting](troubleshooting.md#playlists-and-the-rekordbox-library)
if the library views stay empty.

## Audio output

The emulated deck's output goes to one of three places, chosen when the emulator
starts:

| Setting | Where the sound goes | Use it for |
|---|---|---|
| `off` (default) | nowhere: the stream is consumed and discarded | UI work and scripted tests |
| `speakers` | your computer's default output device | listening to what your mod does |
| `file` or `file:<name.wav>` | a 16-bit stereo 44.1 kHz WAV file in `audio-out-captures/` (named by date and time, or as you say; an absolute path is used as given) | measuring your mod's output: levels, timing, a before/after diff |

- **Mac:** `AZ_AUDIO=speakers tools/az-boot-m1.sh`, or for the app
  `open --env AZ_AUDIO=file AZ-EMU.app`.
- **Windows:** `powershell -ExecutionPolicy Bypass -File tools\az-boot-win.ps1 -Audio speakers`,
  or `-Audio file`;
  for the app, set `$env:AZ_AUDIO = 'speakers'` in PowerShell before starting
  `az-preview.exe`. Under Windows' software emulation the sound is slow and
  choppy, and so are `file` recordings; use the sample-exact capture below instead
  (see [Windows](windows.md)).

What you hear is an approximation of the XDJ-AZ's master: the four decks mixed with
the channel faders, trim, crossfader (with each deck's crossfader assign) and master
level; EQ and effects are not modeled. On the XDJ-AZ the DSP does the mixing, which
the emulator does not have, so the emulator mixes the decks itself from the real
controls, with curves that are close to the deck's but not measured (see
[Audio levels](how-it-works.md#audio-levels)). AZ-EMU starts with every channel fader,
the master LEVEL and the crossfader at the middle and the TRIM knobs at their center,
so a deck is heard as soon as it plays; move them as on the deck. (Without AZ-EMU,
the emulated panel keeps the positions it was captured with, faders and master down,
and the mix is silent.)

A recording grows about 176 KB/s (about 635 MB an hour) for as long as the emulator
runs. QEMU leaves the WAV header's length fields at 0, which some players refuse, so
run `python3 tools/az-wav-fix.py <file.wav>` before you open it (it also works on a
recording that is still growing). `audio-out-captures/` is ignored by git; delete recordings when you are done.

### Sample-exact capture

`file:` records on the host's clock, so when the emulator runs slower than real time
(software emulation on Windows does) the recording has gaps where the deck fell
behind. `tools/az-capture.py` taps the decks inside the emulator instead: every sample the
deck program renders, in order, at any speed, for all four decks before the mixer. The
WAV has 8 channels, 32-bit at 44.1 kHz: deck 1 left, deck 1 right, deck 2 left, deck 2
right, and so on to deck 4 right. With `start --mix` it has 10: channels 9 and 10 are
the approximate mix described above.

```
python3 tools/az-capture.py start          # writes audio-out-captures/capture-<date_time>.wav
python3 tools/az-capture.py start --mix    # the same, plus the mix as channels 9 and 10
python3 tools/az-capture.py stop
```

The deck channels are the deck program's own samples, about 1.4 MB a second (1.76 MB
with the mix), streamed straight to your computer (nothing is kept inside the emulator), so a capture
can run as long as your disk has room.
It works with any audio output setting, including `off`.

`off` still consumes the audio on purpose: with no consumer, the deck program renders
silence and stems do not play. `AZ_AUDIODEV`, when set, is passed to QEMU's
`-audiodev` unchanged and overrides `AZ_AUDIO`.

## What the emulator cannot do

- **Real-time audio on Windows:** see [Windows](windows.md).
- **Stems** for tracks whose stem files are not on the emulated USB.
- **Installing from a repository** (Mod Menu > Community Mods > **Browse** and
  **Repositories**) has not been tested on the emulator. The downloader comes with the
  loader on the USB (AZ-MODS 1.2.3 and later). Load your mod with `az-push-mod.py`,
  and try the install from your published repository on a deck.

## When something goes wrong

See [Troubleshooting](troubleshooting.md). The most common fixes:

- `build.sh` says the system file is missing, or was pulled without its loader: step 2.
- AZ-EMU shows a black screen: wait a minute; the first start is slow. Then check
  `tools/az-ssh 'systemctl status EP147'`.
- A tool says the emulator is unreachable: AZ-EMU (or `tools/az-boot-m1.sh`) is not
  running.
