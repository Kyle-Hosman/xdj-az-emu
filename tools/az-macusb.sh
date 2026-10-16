#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-only
# az-macusb.sh: use a REAL USB stick plugged into the Mac as the emulator's USB1.
#
#   tools/az-macusb.sh TEST-USB     the stick's volume name under /Volumes
#   tools/az-macusb.sh off          back to the emulator's own build/usb.img
#
# How: the Mac's /Volumes is shared into the guest over 9p (opt-in:
# `touch build/macusb.on`, then restart AZ-EMU so az-boot-m1.sh adds the device).
# The guest mounts it and bind-mounts the stick's folder at /media/usb/sda, where
# EP147 looks for USB1, then EP147 is restarted and told a stick is there. File
# level, so no raw-disk open: that needs root and can block QEMU on a macOS privacy
# prompt. Folders and the rekordbox library views both browse as on build/usb.img.
# Keep the stick mounted in macOS the whole time and do not open it in rekordbox
# while the emulator uses it (two writers on one exportLibrary.db).
set -euo pipefail
cd "$(dirname "$0")/.."
SSH=(tools/az-ssh)
VOL="${1:?usage: az-macusb.sh <VolumeName> | off}"

if [ "$VOL" = off ]; then
  "${SSH[@]}" 'systemctl stop EP147.service; sync; umount /media/usb/sda 2>/dev/null; mount /dev/sda /media/usb/sda && systemctl start EP147.service'
  sleep 25; "${SSH[@]}" 'printf "mount /media/usb/sda vfat protect:0" > /proc/udev_usb1'
  echo "USB1 is build/usb.img again"; exit 0
fi

[ -d "/Volumes/$VOL" ] || { echo "/Volumes/$VOL not found: is the stick mounted in macOS?" >&2; exit 1; }
ps -o command= -p "$(pgrep -f qemu-system-aarch64 | head -1)" | grep -q mount_tag=macvol || {
  echo "the emulator has no /Volumes share: touch build/macusb.on and restart AZ-EMU" >&2; exit 1; }

"${SSH[@]}" "set -e
# Images older than patch 25's BusID pin: Xorg will not start with the share present. Pin it and start X.
if ! pidof Xorg >/dev/null; then
  C=/etc/X11/xorg.conf.d/20-modesetting.conf; P=\$(readlink -f /sys/class/drm/card0/device)
  sed -i '/BusID/d' \$C
  sed -i \"s|    Driver      \\\"modesetting\\\"|&\\n    BusID       \\\"platform:\$P\\\"|\" \$C
  systemctl restart x11-only.service; sleep 5; pidof Xorg >/dev/null || { echo 'Xorg did not start' >&2; exit 1; }
fi
mkdir -p /mnt/macvol
mountpoint -q /mnt/macvol || mount -t 9p -o trans=virtio,version=9p2000.L,cache=mmap,msize=512000 macvol /mnt/macvol
test -d '/mnt/macvol/$VOL'
systemctl stop EP147.service; sync
umount /media/usb/sda 2>/dev/null || true
mount --bind '/mnt/macvol/$VOL' /media/usb/sda
systemctl start EP147.service"
until "${SSH[@]}" 'pidof EP147' >/dev/null 2>&1; do sleep 3; done
sleep 25   # EP147 must be up before it can take the announcement
"${SSH[@]}" 'printf "mount /media/usb/sda vfat protect:0" > /proc/udev_usb1'
sleep 8
echo "USB1 = /Volumes/$VOL (browse it from SOURCE on the emulator)"
