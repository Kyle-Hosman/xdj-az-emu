# Windows

Setup for a Windows PC (x86_64). It uses the same image, tools and control app as a
Mac, and runs slower: see "What to expect".

## What to expect

**No acceleration, by design.** The deck's system is 64-bit ARM (aarch64), and a
hardware hypervisor such as Windows Hypervisor Platform (WHPX) only runs guests of the
host's own architecture. QEMU therefore runs the guest with **TCG**, its software CPU
translator (`-accel tcg,thread=multi`). QEMU needs neither WHPX nor Hyper-V; turning
them on changes nothing.

**Speed.** Give it real cores: a desktop with 6 or more cores runs it well, and a small
virtual machine is far slower.

- **Works:** the screen, menus, the Mod Menu, touch, every button and knob, screenshots,
  loading your mod and reading its log.
- **Audio is slow and choppy.** Software emulation runs the deck program slower than real
  time, so `-Audio speakers` plays with constant gaps and `-Audio file`
  records the same gaps. To collect a mod's output on Windows, use
  `tools/az-capture.py`: it records every sample the deck renders, however slowly the
  guest runs (see [Audio output](guide.md#audio-output)). Judge how playback, stems and
  timing sound on a Mac or a deck.

**CPUs.** The guest always gets 6 CPUs (`-smp 6`): the deck program pins its threads to
specific CPU numbers and fails without them. Each guest CPU runs on its own host
thread, so a 6-core/12-thread PC has room for it plus Windows. Leave `-Smp` at 6.

## What to install

Everything installs as a normal user with no reboot, except Docker Desktop (and the
QEMU installer, if you use it rather than extracting it): Docker Desktop's installer
needs an administrator, and it runs on WSL 2, which needs the Windows feature "Virtual
Machine Platform" and virtualization turned on in the PC's firmware (BIOS/UEFI). If
that feature was off, Windows needs one restart after turning it on. If Docker Desktop
still says virtualization is unavailable while it is on in the firmware, run
`bcdedit /set hypervisorlaunchtype auto` in an administrator prompt and restart again.
QEMU itself needs none of this.

| What | Why | Notes |
|---|---|---|
| **Git for Windows** ([git-scm.com](https://git-scm.com/download/win)) | the checkout, and Git Bash to run `build.sh` | |
| **Python 3.9 or newer** ([python.org](https://www.python.org/downloads/windows/)) | the tools | `build.sh` finds `python` or the `py` launcher, so no `python3` is needed; where the guide says `python3`, type `python` |
| **OpenSSH client** | the tools talk to the guest over SSH | built into Windows 10/11 (`C:\Windows\System32\OpenSSH`). The Python tools always use this one, even from Git Bash |
| **QEMU for Windows**, 64-bit (11.1.0 is the tested version) | runs the emulator | `qemu-w64-setup-<date>.exe` from [qemu.weilnetz.de/w64](https://qemu.weilnetz.de/w64/): run it (it installs to `C:\Program Files\qemu`), or extract it with 7-Zip into any folder. The folder must hold `qemu-system-aarch64.exe`. It runs unpatched: this repository's QEMU patches are for the Mac build only |
| **Docker Desktop** ([docker.com](https://www.docker.com/products/docker-desktop/)) | builds the boot image from your deck's system (and, optionally, the kernel) | only while building; it can stay closed otherwise. See "Why Docker Desktop" below |
| **Rust** ([rustup.rs](https://rustup.rs), `rustup-init.exe`, the default MSVC toolchain) | builds the control app | the MSVC toolchain needs Visual Studio's C++ build tools; `rustup-init.exe` offers to install them. Optional: everything also works without the app, see "Without the app" |

The pulled system and the build need about 5 GB on the checkout's disk (about 15 GB
if you also build the kernel on Windows), plus up to about 10 GB as the emulated deck
storage fills.

## 1. Get the repository

In **Git Bash**, on a disk with room:

```sh
cd /g/                       # any disk with space; G: here
git clone https://github.com/Kyle-Hosman/xdj-az-emu.git
cd xdj-az-emu
python tools/check-lf.py     # must print "line endings OK"
```

`.gitattributes` keeps LF line endings for everything that runs in Linux, even with
`core.autocrlf=true`; `check-lf.py` and `build.sh` stop if a CRLF slipped in anyway.

To keep the build somewhere other than `build/` inside the checkout, set
`AZ_EMU_BUILD` (for example `export AZ_EMU_BUILD=/g/az-emu-build` in Git Bash, and
`$env:AZ_EMU_BUILD = 'G:\az-emu-build'` in PowerShell) in every shell you use.

## 2. Copy the system from your deck

Turn on SSH access to your deck with the AZ-MODS app first
([guide, step 2](guide.md#2-copy-the-system-from-your-deck)). The app keeps the private
key in `.azmods\ssh\` in your user folder. In **PowerShell**, in the checkout:

```powershell
cd G:\xdj-az-emu
python tools\az-emu-pull.py --host DECK-IP --key "$env:USERPROFILE\.azmods\ssh\xdjaz_ecdsa"
```

It writes `build\initramfs-work-pulled\initramfs.cpio.gz`, using Windows' own SSH
client. If SSH says `UNPROTECTED PRIVATE KEY FILE`, see
[SSH keys](#windows-specific-notes) below.

## 3. Bring the emulator's own parts

The emulator's own parts (kernel, modules, shim, tools, dropbear) do not depend on
your deck. Two ways to get them:

- **Copy them from a Mac or Linux build.** They are built from this repository only, so
  they are the same on every machine. Copy that machine's `build/docker-out/` folder
  into your `build/` (keep the name), then use `./build.sh --image-only` in step 4. This
  needs Docker only for a short, light step. To pack it on a Mac without the hidden
  `._*` files macOS adds, run this in the Mac's `build/` folder:
  `COPYFILE_DISABLE=1 tar --no-xattrs -czf docker-out.tgz docker-out`
- **Or build them here:** `./build.sh` compiles Linux inside Docker. On an x86_64 PC
  Docker runs the ARM compiler through emulation, which works but can take an hour or
  more and uses about 10 GB of Docker's disk. That disk is on your system drive (C:)
  wherever the checkout is, so `build.sh` stops unless C: has about 15 GB free. To
  build with less room on C:, move Docker's disk to a roomier drive first (Docker
  Desktop: Settings > Resources > Advanced > Disk image location), then run
  `AZ_EMU_SKIP_DISK_CHECK=1 ./build.sh` (the check only looks at C:, so it would
  still stop).

## 4. Build

Start Docker Desktop, then in **Git Bash**, in the checkout:

```sh
./build.sh --image-only      # with build/docker-out/ copied from another machine
# or: ./build.sh             # builds everything here (slow)
```

Success ends with "Build complete" and these files in `build/`: `Image`,
`initramfs-patched.cpio.gz`, `az-mnt.img`, `usb.img`, and `ssh/az-emu_ed25519`.
You can close Docker Desktop afterwards.

## 5. Run

In **PowerShell**, in the checkout, tell the tools where QEMU is (once per shell, or set
it as a user environment variable):

```powershell
cd G:\xdj-az-emu
$env:AZ_QEMU = 'C:\Program Files\qemu\qemu-system-aarch64.exe'   # or your folder
```

**With the app.** Build it once, then start it:

```powershell
cargo build --release -p az-preview
.\target\release\az-preview.exe
```

It starts QEMU in the background (no QEMU window) and shows the deck's screen. The app
runs `tools\az-boot-win.ps1` for you and passes its environment on, so every setting
that script reads from an environment variable works with the app too: set them in the
same PowerShell before starting it.

```powershell
$env:AZ_AUDIO = 'speakers'          # or 'file' (audio-out-captures\); default off
$env:AZ_EMU_BUILD = 'G:\az-emu-build' # if your build folder is elsewhere
$env:AZ_USB_FILE = 'G:\sticks\test.img' # another stick image
.\target\release\az-preview.exe
```

Closing the app stops the QEMU it started, as on a Mac (see
[Run](guide.md#4-run)); the QEMU it starts also stops if the app itself is ended or
crashes.

**Without the app.** Start QEMU with its own window:

```powershell
powershell -ExecutionPolicy Bypass -File tools\az-boot-win.ps1
```

It prints `qemu pid N ...` and returns; QEMU keeps running. Stop it with the window's
close button, or `Stop-Process -Id N`. Both end QEMU at once, so run `tools/az-ssh sync`
(in Git Bash) first: settings the deck wrote in the last few seconds are otherwise lost
(the guest's disks are cached).

Success: the serial log (`%TEMP%\az-emu-win\serial.log`) shows the Linux boot within
seconds, and the deck's SOURCE screen follows. Once the SOURCE screen shows, check from
PowerShell:

```powershell
python tools\az-drive.py shot first-screen     # prints the PNG's path
```

A screenshot answers as soon as QEMU runs, so one taken before the deck program draws
its screen is black.

## 6. Install AZ-MODS and load your mod

Every start of the emulator already downloads, verifies and installs the newest
published AZ-MODS set in the background (log: `%TEMP%\az-emu-win\mods-install.log`), so
`az-mods-fetch.py` here confirms it ("already downloaded and verified", then
"CURRENT: ...") or installs it if that check could not run. Same commands as on a Mac,
from PowerShell or Git Bash:

```powershell
python tools\az-mods-fetch.py                  # "EP147 runs mods ..." or "CURRENT: ..." = success
python tools\az-push-mod.py path\to\my-mod     # prints your mod's load result
python tools\az-drive.py panel browse          # press BROWSE
```

`tools/az-ssh` (a shell in the guest) runs in Git Bash.

Your mod needs a core with Community Mods (AZ-MODS 1.2.3 or later); on an older core
`az-push-mod.py` stops and says so. To test your own core build, push it with
`python tools\az-push-mod.py --core path\to\azmod.so path\to\azstem.so` (it goes on the
emulated USB and turns on the deck's developer switch, as in
[the guide](guide.md#your-own-build-of-the-az-mods-core)), then start the emulator with
`-NoMods` (or with `$env:AZ_EMU_NOMODS = '1'` set before starting the app) from then on;
otherwise the next boot puts the published set back on the USB.

## Why Docker Desktop for the build

The boot image is made by the same `./build.sh` on every computer: a small Linux
container unpacks your deck's system, applies the emulator patches and packs it again.
It never runs ARM code. Docker Desktop keeps that
one code path for every host; a plain WSL 2 setup would need its own instructions.

You can also skip building on Windows: build on a Mac and copy `build/Image`,
`build/initramfs-patched.cpio.gz` and `build/ssh/` to the PC.

## Windows-specific notes

- **The emulated deck storage** (`build/az-mnt.img`) is enlarged to 14 GB at start, as on
  a Mac, and marked sparse first. QEMU's Windows build cannot pass the guest's discards
  to the file, so space the guest frees is not returned: the file only grows, up to
  about 10 GB as tracks play. To shrink it, stop the emulator and delete it (it is made
  again; reinstall your mods with `tools/az-mods-fetch.py`).
- **Ports** are bound to 127.0.0.1 only. Tools use `127.0.0.1`, never `localhost`,
  which Windows may resolve to the IPv6 address first.
- **SSH clients.** The Python tools (`az-drive.py`, `az-mods-fetch.py`, `az-push-mod.py`,
  `az-emu-pull.py`) always run Windows' own OpenSSH, from PowerShell or Git Bash.
  `tools/az-ssh` is a Git Bash script and uses Git's own `ssh`, which works better for an
  interactive shell in Git Bash's window.
- **SSH keys must be readable only by you.** Windows' SSH client refuses a key other
  users can read. `build.sh` restricts the key it makes in `build/ssh/`. For a key you
  copied in, or your deck's key (`--key` of `az-emu-pull.py`) when SSH says
  `UNPROTECTED PRIVATE KEY FILE` and `Permission denied (publickey)`, run in PowerShell:

  ```powershell
  icacls path\to\key /inheritance:r /grant:r "${env:USERNAME}:R"
  ```

  Copying a `build/` folder to another place (`AZ_EMU_BUILD`) copies
  `build/ssh/az-emu_ed25519` without its restriction, so Windows' OpenSSH then refuses
  it (the Python tools fail; `tools/az-ssh` with Git's `ssh` still works). Run the
  `icacls` line above on the copied key, or rebuild to make a fresh one.
- **Audio** is off by default; `-Audio speakers` plays through Windows' default output
  (DirectSound) and `-Audio file:<path.wav>` records it. See
  [Audio output](guide.md#audio-output).
- **One emulator at a time**: the script refuses to start a second QEMU.
- **Measuring a setting**: `python tools/az-emu-bench-win.py <label>` boots the guest
  headless once and records boot times and the screen refresh rate to
  `%TEMP%\az-emu-win\bench.jsonl` (options are in the script's header).
