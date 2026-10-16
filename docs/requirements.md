# Requirements

## Your deck

- An **XDJ-AZ on firmware 2.00+**. Other firmware is refused by `./build.sh`.
- **SSH access** turned on with the AZ-MODS desktop app, so `tools/az-emu-pull.py` can
  copy the deck's system ([guide, step 2](guide.md#2-copy-the-system-from-your-deck)).
  You do this once; the emulator never needs the deck again.

## Mac (Apple Silicon, recommended)

| What | Why | How |
|---|---|---|
| macOS on an M1 or newer | the guest is 64-bit ARM like your Mac, so it runs at full speed (Apple's Hypervisor framework) | |
| Xcode command line tools | compilers, `git`, `swift` for the app icon | `xcode-select --install` |
| Homebrew | the packages below | [brew.sh](https://brew.sh) |
| Homebrew packages | QEMU's build | `brew install glib pixman libslirp ninja pkgconf python@3.11` |
| Docker with Buildx | builds the Linux kernel, the guest tools and the boot image | Docker Desktop or OrbStack (both include Buildx), or colima (below) |
| Rust | AZ-EMU.app | [rustup.rs](https://rustup.rs), the default stable toolchain (1.88 or newer) |
| Python 3.9 or newer | the tools | the `python3` of the Xcode command line tools; QEMU's build uses Homebrew's `python3.11` |
| about 15 GB free disk | Docker's kernel build (about 10 GB of Docker's disk), QEMU (about 1.5 GB), the Rust build (about 1.5 GB), the boot files | |
| about 10 GB more in use | the emulated deck storage, `build/az-mnt.img` | sparse: it grows as the deck program fills its track caches |

Docker with colima:

```sh
brew install colima docker docker-buildx
mkdir -p ~/.docker/cli-plugins
ln -sfn "$(brew --prefix)/lib/docker/cli-plugins/docker-buildx" ~/.docker/cli-plugins/docker-buildx
colima start
docker buildx version          # prints a version once Docker finds Buildx
```

colima's virtual disk grows during the kernel build and does not shrink again on its
own, so check your free space before the first build.

## Windows PC

Supported, with limits: see [Windows](windows.md). An aarch64 guest on an x86_64 PC runs
under QEMU's software emulation (TCG), which is slower than on a Mac: the screen, menus
and controls work, and live audio has gaps.

## Intel Mac

Not tested. The Mac's hypervisor accelerates only a guest of its own architecture, so
on an Intel Mac `tools/az-boot-m1.sh` uses software emulation (TCG) with the Windows
settings (`-cpu cortex-a72`, `nokaslr`) and the same limits as on Windows. Docker
builds the arm64 parts through emulation, which works but takes much longer. Reports
of how it runs are welcome.

## Linux

Not supported yet. The Docker build works on Linux, and QEMU is available there, but
there is no boot script or app build for a Linux host. On an arm64 Linux machine with
KVM, a port of `tools/az-boot-m1.sh` would replace `-accel hvf` with `-accel kvm` and
the Cocoa display with GTK or SDL. Contributions are welcome.
