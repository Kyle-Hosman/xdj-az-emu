# Attribution

## cdj3k-emu

xdj-az-emu started as a fork of **cdj3k-emu**, a CDJ-3000 emulator:

- Project: https://github.com/nsaintot/cdj3k-emu
- Copyright (c) 2026 The cdj3k-emu authors
- License: MIT OR Apache-2.0, at your option (texts in
  [LICENSES/MIT-cdj3k-emu.txt](LICENSES/MIT-cdj3k-emu.txt) and
  [LICENSES/Apache-2.0.txt](LICENSES/Apache-2.0.txt))
- Based on upstream commit `cf19e92dfe6917d6bfb5712aa7000c261dc55cf8`
  (also recorded in [.upstream-base-commit](.upstream-base-commit)). The fork
  started from `d1a25ef188a01729c3637b6c2e9d7d1bebb69f99`; the upstream changes
  from there to `cf19e92` that apply to the XDJ-AZ are merged in.

This file is the record of what came from upstream. Thank you to the cdj3k-emu
authors: the QEMU machine, the LD_PRELOAD shim, the MCU bus bridge and the initramfs
patch system are theirs, and they made the XDJ-AZ port possible.

The parts of cdj3k-emu that only serve the CDJ-3000 (its desktop app, its Rust
crates, its firmware install wizard, its CDJ documentation and assets) are not
included, except for the front-panel drawing code that `az-preview/src/parts.rs`
ports from its desktop app (listed below).

### Files under upstream's license

These keep upstream's license, as their SPDX header says. That is `MIT OR
Apache-2.0`, except for the GPL-2.0 carve-outs upstream's NOTICE names (its kernel
modules, kernel patches and QEMU patches), which stay `GPL-2.0-or-later`:

- `.clang-format`, `.clang-tidy`
- `az-preview/src/parts.rs`: the jog wheel, the tempo slider, the browse rotary and
  the jog feel knob, with the ring and gear primitives they share, ported from
  upstream commit `17d24f12fac6fc2f741bb12197e62bc53966630f` and adapted to the
  XDJ-AZ panel. It ports, from `crates/cdj3k-emu-ui/src/app/ui/`:
  `draw_jog.rs`, `draw_jog/statics.rs`, `draw_jog/overlays.rs` and
  `draw_jog/geometry.rs` (the jog rings, grip ring, platter and position mark, the
  jog display's outline and its curved labels); `draw_primitives.rs` (the double rings,
  the gear, indicator and tick ring); `slate/cdj3k/draw_right/nav_rotary.rs` (the
  browse rotary and its lit ring); `slate/cdj3k/draw_right/tempo.rs` and
  `slate/cdj3k/draw_right/statics.rs` (the tempo slider). The adaptations are
  offered under upstream's license too.
- `guest/cfgd/cfgd.c` (as of `d1a25ef`, with upstream's later fix to `emit()`)
- `guest/ep122_shim/jog_drm.c`
- `guest/ep122_shim/jog_shm.c`: with upstream's later fix to the jog framebuffer
  registry (`47ff48b`), ported to this file
- `guest/ep122_shim/clock.c`, `jog.c`, `link.c`: changed only to stop overwriting the
  error code of a failed system call; that fix is offered under upstream's license too.
  `clock.c` also reads the clock through the vDSO, ported from upstream (`e6abf66`)
- `guest/subucom/live.c`
- `initramfs-patch/repack.sh` (changed only in a usage comment: the container image's
  name)
- `initramfs-patch/patch-rootfs.d/`: `06-eth0-dhcp.sh` (as of `d1a25ef`; upstream
  has since removed it), `09-pre-setting-ordering.sh`, `10-usb-drive-mount.sh`,
  `16-xorg-modesetting-conf.sh`, `19-usb-external.sh`, `22-vanilla-kernel-fixups.sh`,
  `23-absent-hardware.sh`, `27-udev-usb1.sh`; and `08-insmod-subucom-virt.sh` and
  `17-insmod-virtio-snd.sh`, changed only to leave out a module parameter that our
  modules do not take (upstream's `model=` and `period_frames=`), a change offered
  under upstream's license too
- `guest/kernel-patches/*.patch` (GPL-2.0-or-later)
- `guest/modules/udev_usb1/*`, `guest/modules/Makefile`, `guest/modules/subucom_virt/Makefile`,
  `guest/modules/virtio_snd/Makefile` (GPL-2.0-or-later)
- `qemu/patches/*.patch` (GPL-2.0-or-later)

### Files changed from cdj3k-emu

These carry a header line "Derived from cdj3k-emu", with the upstream file's license.
Our changes are licensed GPL-3.0-only (the kernel modules stay GPL-2.0-only); the
upstream copyright notice above applies to the upstream portions:

- `build.sh`, `docker/Dockerfile`, `guest/Makefile`, `Cargo.toml`, `qemu/build.sh`
- `guest/ep122_shim/ep122_shim.c`, `ep122_shim.h`, `fd.c`, `syscalls.c`, `syscalls_io.c`
- `guest/subucom/forwarder.c`
- `guest/modules/subucom_virt/subucom_virt.c` (its service-mode hold follows upstream's
  `5fc3e66` and `cf19e92`), `guest/modules/virtio_snd/virtio_snd.c`,
  `guest/kernel-patches/cdj3k-emu-kernel.config` (GPL-2.0-only)
- `initramfs-patch/patch-rootfs.sh` and patches `01`, `11`, `20`, `21`, `25`
- `tools/az-boot-m1.sh`, `tools/az-boot-win.ps1` (written for the XDJ-AZ after
  upstream's boot script)

Everything else is new in this project.

## Other third-party code

- **suspend-usb-device**, Copyright (C) 2009-2016 Yan Li, GPLv3. A modified copy is
  embedded in `initramfs-patch/patch-rootfs.d/20-unbind-usb-device.sh` with its
  original copyright and acknowledgements.
- **dropbear** SSH server (https://matt.ucc.asn.au/dropbear/), MIT-style license.
  Downloaded from its official release at build time (version and SHA-256 pinned in
  `docker/Dockerfile`); its license file is installed beside it in the image.
- **Linux** (https://kernel.org), GPL-2.0. Downloaded at build time at a pinned
  commit; `guest/modules/uinput` builds the kernel's own `uinput.c`.
- **QEMU** (https://www.qemu.org), GPL-2.0-or-later. Downloaded at build time at a
  pinned commit (macOS); Windows uses the official QEMU build.
- **xserver-xorg-video-dummy** (Ubuntu package), MIT. Downloaded at build time,
  SHA-256 pinned.
- Rust crates used by AZ-EMU (`eframe`, `egui`, `image`, `uds_windows`) are
  downloaded by Cargo and keep their own licenses (MIT and/or Apache-2.0).
