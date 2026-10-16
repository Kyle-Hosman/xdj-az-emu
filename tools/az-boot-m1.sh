#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Derived from cdj3k-emu (MIT OR Apache-2.0); see ATTRIBUTION.md.
# az-boot-m1.sh - boot the emulated XDJ-AZ on macOS (Apple Silicon, HVF).
# AZ-EMU.app runs this itself (headless). Run it by hand for a QEMU window:
#   tools/az-boot-m1.sh            (AZ_DISPLAY=none for headless)
#
# Screen: virtio-gpu at 1280x800 with `edid=off`. EP147 (JUCE) picks its UI scale
# from the display's reported physical size: the deck's panel reports 0 mm (1x),
# while virtio-gpu with EDID reports 126 mm and EP147 renders at 2x. With EDID off
# it renders at 1x, so screen coordinates match a deck pixel for pixel.
#   QMP:   127.0.0.1:4445 belongs to AZ-EMU.app, 4446 to scripts. QEMU allows one
#          client per QMP socket, so scripts must leave 4445 free.
#   SSH:   tools/az-ssh (key-only, 127.0.0.1:2222)
#   Serial log: $SERIAL_LOG
#   AZ_USB_FILE: boot with another stick image, or a real stick as a raw device:
#     unmount it (`diskutil unmountDisk /dev/diskN`), then
#     `AZ_USB_FILE=/dev/rdiskN tools/az-boot-m1.sh`. Apps started from Finder do
#     not inherit shell variables, so set it in a terminal and start az-preview
#     there. (tools/az-macusb.sh shares a mounted stick without raw access.)
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
QEMU="${AZ_QEMU:-$REPO/qemu/install/bin/qemu-system-aarch64}"
B="${AZ_EMU_BUILD:-$REPO/build}"
SOCK="/tmp/az-emu-m1"; mkdir -p "$SOCK"
# Every bus starts at its idle frame on a new boot, so the control state that
# az-drive.py and AZ-EMU.app keep here must start from idle too; a stale knob
# counter would turn the first knob frame into a multi-detent jump.
rm -f "$SOCK/knob.ctr" "$SOCK/deck1.state" "$SOCK/deck2.state" "$SOCK/mixer.state"
SERIAL_LOG="${SERIAL_LOG:-$SOCK/serial.log}"
MEM=4038197248   # the deck's RAM, 64 KiB-aligned
for f in "$QEMU" "$B/Image" "${AZ_INITRD:-$B/initramfs-patched.cpio.gz}" \
         "${AZ_MNT_IMG:-$B/az-mnt.img}" "${AZ_USB_FILE:-$B/usb.img}"; do
  if [ ! -e "$f" ]; then
    echo "az-boot-m1: missing $f" >&2
    case "$f" in
      "$QEMU") echo "  Build QEMU once: qemu/build.sh" >&2 ;;
      *initramfs*) echo "  Pull your deck's system (tools/az-emu-pull.py), then run ./build.sh" >&2 ;;
      *) echo "  Run ./build.sh" >&2 ;;
    esac
    exit 2
  fi
done
# The deck storage needs AZ_MNT_MIN_GB (default 14, see build.sh): the deck program's
# five track caches take 9.5 GiB of its reserve partition, and the deck reformats that
# partition at boot when less than 2 GiB is left, so a smaller disk never settles
# (ENOSPC on TrackFileCache, no library). An older, smaller image is enlarged here
# (sparse: it costs only what the guest writes); the guest grows its reserve partition
# into the new space and keeps the settings partition as it is (patch 32). 0 = leave it.
MNT_IMG="${AZ_MNT_IMG:-$B/az-mnt.img}"
MIN_GB="${AZ_MNT_MIN_GB:-14}"
if [ -f "$MNT_IMG" ] && [ "$MIN_GB" -gt 0 ] && [ "$(stat -f %z "$MNT_IMG")" -lt $((MIN_GB << 30)) ]; then
  echo "az-boot-m1: enlarging $MNT_IMG to $MIN_GB GB (sparse) so the deck storage fits the deck program's caches" >&2
  python3 -c "import sys; f=open(sys.argv[1], 'r+b'); f.truncate(int(sys.argv[2]) << 30)" "$MNT_IMG" "$MIN_GB"
fi
# The image grows on the host as the guest writes, up to about the caches' 9.5 GiB.
# Say so now if this disk cannot take that (the health check repeats it in the title).
python3 "$REPO/tools/az-emu-health.py" --host-only --image "$MNT_IMG" >&2 || true
# AZ_TESTMODE=1 boots EP147TestMode (subucom_virt inject_testmode=1, held until the first LED frame)
EXTRA_CMDLINE=""
# Audio output: AZ_AUDIO=off (default) | speakers | file:<path.wav>
#   off       the stream is consumed and discarded (wav into /dev/null). It must be
#             consumed: with QEMU's "none" backend EP147 renders silence and stems break.
#   speakers  the Mac's current default output (CoreAudio).
#   file      a new WAV in audio-out-captures/, named by date and time.
#   file:...  a 16-bit stereo WAV of everything the deck plays, for measuring your
#             mod's output. It grows about 176 KB/s (about 635 MB an hour) while QEMU
#             runs; tools/az-wav-fix.py fills in its header. Delete it when you are done.
# AZ_AUDIODEV, when set, is passed to QEMU's -audiodev as-is and overrides AZ_AUDIO.
AZ_AUDIO="${AZ_AUDIO:-off}"
case "$AZ_AUDIO" in
  off)      AUDIODEV="wav,id=audio0,path=/dev/null" ;;
  speakers) AUDIODEV="coreaudio,id=audio0" ;;
  file|file:*)
    WAV="${AZ_AUDIO#file}"; WAV="${WAV#:}"
    [ -n "$WAV" ] || WAV="$(date +%Y-%m-%d_%H-%M-%S).wav"
    case "$WAV" in /*) ;; *) WAV="$REPO/audio-out-captures/$WAV" ;; esac
    case "$WAV" in *,*) echo "az-boot-m1: AZ_AUDIO file path cannot contain a comma: $WAV" >&2; exit 3 ;; esac
    if ! mkdir -p "$(dirname "$WAV")"; then echo "az-boot-m1: cannot create $(dirname "$WAV")" >&2; exit 3; fi
    echo "az-boot-m1: recording audio to $WAV (about 635 MB an hour while QEMU runs)" >&2
    AUDIODEV="wav,id=audio0,path=$WAV" ;;
  *) echo "az-boot-m1: AZ_AUDIO must be off, speakers, file or file:<path.wav> (got '$AZ_AUDIO')" >&2; exit 3 ;;
esac
AUDIODEV="${AZ_AUDIODEV:-$AUDIODEV}"
[ -n "${AZ_TESTMODE:-}" ] && EXTRA_CMDLINE=" subucom_testmode"
: > "$SERIAL_LOG"
# Opt-in (touch build/macusb.on): share the Mac's /Volumes for tools/az-macusb.sh.
# Xorg starts with the extra PCI device only on an image with 25-xorg-headless.sh's
# BusID pin, so older images must not get it.
MACVOL_ARGS=()
[ -e "$B/macusb.on" ] && MACVOL_ARGS=(-fsdev "local,id=macvol,path=/Volumes,security_model=none" \
  -device "virtio-9p-pci,fsdev=macvol,mount_tag=macvol")
# AZ_INITRD / AZ_MNT_IMG: boot another image with its OWN settings disk.
# Acceleration: HVF on Apple Silicon (the guest is aarch64, like the host). An Intel
# Mac cannot accelerate an aarch64 guest, so it falls back to TCG (slow, untested), with
# the Windows script's TCG settings: the deck's own core (cortex-a72) and nokaslr, which
# keeps the kernel from turning on KPTI there (tools/az-boot-win.ps1 has the numbers).
if [ "$(uname -m)" = arm64 ]; then ACCEL=(-accel hvf -cpu host)
else ACCEL=(-accel tcg,thread=multi -cpu cortex-a72); EXTRA_CMDLINE="$EXTRA_CMDLINE nokaslr"; fi
# Every boot checks that the CURRENT published mods set is installed and installs it
# if not (tools/az-mods-fetch.py, signature-checked; nothing happens when it is
# current). It waits in the background for the guest; log: $SOCK/mods-install.log.
# When it cannot confirm the installed set is the newest (offline, say), its
# "AZ-MODS: ..." line is copied to this terminal, and AZ-EMU.app shows it in its
# title bar. AZ_EMU_NOMODS=1 skips it: use that while you test your OWN core build
# (az-push-mod.py --core), which this would otherwise replace.
rm -f "$SOCK/mods-status.txt" "$SOCK/health-status.txt"
# Every boot also checks the guest's health once the deck program runs (tools/az-emu-
# health.py): an image older than this checkout, a full deck storage, a missing
# OneLibrary key flag. AZ-EMU.app shows what it finds in its title bar. A watcher from a
# previous AZ-EMU holds health.log, so stop it first (it also exits on its own once its
# QEMU is gone). Only a pid that is still that watcher is stopped; a stale pid file is
# just removed, since its number may now belong to an unrelated program.
[ -f "$SOCK/health.pid" ] && python3 "$REPO/tools/az-emu-health.py" --stop-previous "$SOCK/health.pid"
( trap '' HUP
  python3 "$REPO/tools/az-emu-health.py" --wait 300 --watch 10 >"$SOCK/health.log" 2>&1 </dev/null ) &
if [ "${AZ_EMU_NOMODS:-}" != 1 ]; then
  ( trap '' HUP
    case "${AZ_USB_FILE:-}" in /dev/*) export AZ_EMU_STICK=real ;; *) export AZ_EMU_STICK=emulated ;; esac
    python3 "$REPO/tools/az-mods-fetch.py" --wait 300 2>&1 </dev/null |
      tee "$SOCK/mods-install.log" | grep --line-buffered '^AZ-MODS:' >&2 ) &
fi
exec "$QEMU" \
  -machine virt "${ACCEL[@]}" -smp 6 -m "${MEM}B" \
  -kernel "$B/Image" \
  -initrd "${AZ_INITRD:-$B/initramfs-patched.cpio.gz}" \
  -append "root=/dev/ram0 rdinit=/init loglevel=7 nowatchdog console=ttyAMA0,115200 systemd.journald.forward_to_console=1 snd_dummy.index=1 dummy.numdummies=0${EXTRA_CMDLINE}" \
  -display "${AZ_DISPLAY:-cocoa,show-cursor=on}" \
  -netdev "user,id=net0,hostfwd=tcp:127.0.0.1:2222-:22" \
  -device "virtio-net-device,netdev=net0" \
  -serial "file:$SERIAL_LOG" \
  -qmp "tcp:127.0.0.1:4445,server=on,wait=off" \
  -qmp "tcp:127.0.0.1:4446,server=on,wait=off" \
  -device virtio-rng-device \
  -audiodev "$AUDIODEV" \
  -device "virtio-sound-device,audiodev=audio0,streams=2" \
  -device "virtio-gpu-device,id=gpu0,xres=1280,yres=800,edid=off,max_outputs=1" \
  -device "virtio-tablet-pci,id=tablet0" \
  -drive "file=$MNT_IMG,if=none,id=mnt0,format=raw,cache=writeback,discard=unmap,detect-zeroes=unmap" \
  -device "virtio-blk-device,drive=mnt0" \
  -drive "file=${AZ_USB_FILE:-$B/usb.img},if=none,id=usbdrv,format=raw,cache=writeback" \
  -device "usb-ehci,id=ehci" \
  -device "usb-storage,bus=ehci.0,drive=usbdrv" \
  -device "virtio-serial-device,max_ports=8" \
  -chardev "socket,id=vmain,path=$SOCK/main.sock,server=on,wait=off" \
  -device "virtserialport,chardev=vmain,name=cdj3k.main" \
  -chardev "socket,id=vsub,path=$SOCK/sub.sock,server=on,wait=off" \
  -device "virtserialport,chardev=vsub,name=cdj3k.sub" \
  -chardev "socket,id=vctrl,path=$SOCK/ctrl.sock,server=on,wait=off" \
  -device "virtserialport,chardev=vctrl,name=cdj3k.ctrl" \
  -chardev "socket,id=vgpio,path=$SOCK/gpio.sock,server=on,wait=off" \
  -device "virtserialport,chardev=vgpio,name=cdj3k.gpio" \
  `# the Mac's /Volumes, read-write, for tools/az-macusb.sh: a REAL stick plugged into the Mac,
   # mounted in the guest where EP147 looks for USB. File-level, so no raw-disk open (which
   # needs root and can block on macOS privacy prompts). On PCI on purpose: as a virtio-mmio
   # device (-virtfs) it stopped Xorg starting ("Cannot run in framebuffer mode").` \
  ${MACVOL_ARGS[@]+"${MACVOL_ARGS[@]}"}
