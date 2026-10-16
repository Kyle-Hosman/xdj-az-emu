#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Patch 32 (AZ): partition the virtio-blk disk as the AZ eMMC so the device's
# own emmc-fs.sh mounts setting(p7)->/home/root/settings and reserve(p8)->/mnt.
#
# az-boot-m1.sh attaches a virtio-blk disk (build/az-mnt.img, 14 GB or more); the
# base kernel enumerates it as /dev/mmcblk1 (matching the firmware's hardcoded eMMC
# path). Unpartitioned, emmc-fs.sh's `[[ -b /dev/mmcblk1p7/p8 ]]` guards fail, /mnt
# stays on the RAM initramfs, and EP147's 2GB LocalFileCache*.bin OOM-kills it. This
# service GPT-partitions the disk before emmc-fs.service; emmc-fs.sh then mkfs.ext4 +
# mounts p7/p8.
#
# p8 must hold EP147's five preallocated track caches, LocalFileCache0-4.bin at
# 2042626048 bytes each (9.5 GiB together, whatever the free space), and emmc-fs.sh
# reformats p8 at every boot when less than 2 GiB of it is free. On a smaller p8 the
# caches never fit: EP147 loops on ENOSPC (TrackFileCache) and never brings up the
# library. The boot scripts extend an older, smaller disk image; on the next boot this
# service grows p8 into the new space (same start, p1-p7 untouched) and its file system
# with it.
set -euo pipefail
: "${ROOTFS:?ROOTFS must be set by dispatcher}"

az_put 755 "$ROOTFS/home/root/scripts/az-emmc-partition.sh" <<'SH'
#!/bin/sh
DEV=/dev/mmcblk1
[ -b "$DEV" ] || { echo "az-emmc-partition: $DEV absent, skipping"; exit 0; }
# Already partitioned (p8 present)? keep it (it persists across reboots), but grow p8
# when the disk has grown.
if [ -b "${DEV}p8" ]; then
  /sbin/sgdisk -e "$DEV" >/dev/null 2>&1   # the backup GPT to the disk's (new) end
  start=$(/sbin/sgdisk -i 8 "$DEV" | awk '/^First sector:/ {print $3}')
  end=$(/sbin/sgdisk -i 8 "$DEV" | awk '/^Last sector:/ {print $3}')
  last=$(/sbin/sgdisk -p "$DEV" | sed -n 's/.*last usable sector is \([0-9]*\).*/\1/p')
  # Grow only for more than 64 MiB (131072 sectors) of new space.
  if [ -n "$start" ] && [ -n "$end" ] && [ -n "$last" ] && [ "$last" -gt $((end + 131072)) ]; then
    echo "az-emmc-partition: growing ${DEV}p8 (sectors $start-$end) to the disk's end ($last)"
    /sbin/sgdisk -d 8 -n "8:$start:0" -c 8:reserve -t 8:8300 "$DEV" >/dev/null 2>&1
    /sbin/partprobe "$DEV" 2>/dev/null || true
    /sbin/blockdev --rereadpt "$DEV" 2>/dev/null || true
    want=$((last - start + 1))   # sectors the new p8 spans (GPT may round a little)
    psz="/sys/class/block/$(basename "$DEV")p8/size"
    # resize2fs grows nothing if it runs before the kernel sees the new size:
    # wait for /sys to report it (up to 10 s) before resizing.
    i=0
    while [ "$i" -lt 20 ]; do
      [ "$(cat "$psz" 2>/dev/null || echo 0)" -ge $((want - 4096)) ] && break
      sleep 0.5; i=$((i + 1))
    done
    # p8 holds only the deck program's caches and logs: if its file system cannot be
    # grown, emmc-fs.sh makes a new one, as the deck does when p8 runs low.
    # e2fsck exits 1 or 2 when it corrected something (a journal left by a power-off,
    # say): only 4 and above mean the file system cannot be used.
    grown=0
    /sbin/e2fsck -fy "${DEV}p8" >/dev/null 2>&1; rc=$?
    if [ "$rc" -lt 4 ] && /sbin/resize2fs "${DEV}p8" >/dev/null 2>&1; then
      # Confirm the file system actually grew into the partition: resize2fs can
      # "succeed" against a stale size and leave it small. Compare the fs size (block
      # count x block size, in 512-B sectors) with the partition, allowing 128 MiB.
      info=$(/sbin/tune2fs -l "${DEV}p8" 2>/dev/null)
      bc=$(echo "$info" | sed -n 's/^Block count: *//p')
      bs=$(echo "$info" | sed -n 's/^Block size: *//p')
      have=$(cat "$psz" 2>/dev/null || echo 0)
      if [ -n "$bc" ] && [ -n "$bs" ] && [ "$((bc * (bs / 512)))" -ge $((have - 262144)) ]; then
        grown=1
      fi
    fi
    if [ "$grown" = 1 ]; then
      echo "az-emmc-partition: ${DEV}p8 file system grown"
    else
      echo "az-emmc-partition: ${DEV}p8 file system not grown; emmc-fs.sh will make a new one"
      dd if=/dev/zero of="${DEV}p8" bs=1M count=1 2>/dev/null
    fi
  else
    echo "az-emmc-partition: ${DEV}p8 exists, keeping"
  fi
  exit 0
fi
echo "az-emmc-partition: creating AZ eMMC layout on $DEV"
# sizes: uboot/trust/resource 4M, update 128M, boota/bootb 256M, setting 64M,
# reserve = rest. Names cosmetic; emmc-fs.sh only needs p7/p8 to exist.
/sbin/sgdisk -og "$DEV" >/dev/null 2>&1
/sbin/sgdisk \
  -n1:0:+4M   -c1:uboot   -n2:0:+4M   -c2:trust    -n3:0:+4M  -c3:resource \
  -n4:0:+128M -c4:update  -n5:0:+256M -c5:boota     -n6:0:+256M -c6:bootb \
  -n7:0:+64M  -c7:setting -n8:0:0     -c8:reserve   "$DEV" >/dev/null 2>&1
/sbin/partprobe "$DEV" 2>/dev/null || true
sleep 1
echo "az-emmc-partition: done ($(ls ${DEV}p* 2>/dev/null | tr '\n' ' '))"
SH

az_put 644 "$ROOTFS/etc/systemd/system/az-emmc-partition.service" <<'UNIT'
[Unit]
Description=AZ emu: partition virtio-blk as eMMC (p7 setting, p8 reserve)
DefaultDependencies=no
After=systemd-udevd.service
Before=emmc-fs.service pre-setting.service EP147.service
ConditionPathExists=/dev/mmcblk1

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/home/root/scripts/az-emmc-partition.sh

[Install]
WantedBy=multi-user.target
UNIT

# Space the deck program frees on /mnt goes back to the host: the boot script gives the
# disk discard=unmap, and /mnt is mounted with online discard. emmc-fs.sh (the deck's)
# mounts it plainly and may have just made a new file system, so this runs after it:
# the file system's default mount options get discard, and /mnt is remounted with it.
az_put 644 "$ROOTFS/etc/systemd/system/emmc-fs.service.d/20-az-discard.conf" <<'UNIT'
[Service]
ExecStartPost=-/bin/sh -c '/sbin/tune2fs -o discard /dev/mmcblk1p8 >/dev/null 2>&1; /bin/mount -o remount,discard /mnt'
UNIT

install -d "$ROOTFS/etc/systemd/system/multi-user.target.wants"
ln -sf ../az-emmc-partition.service \
    "$ROOTFS/etc/systemd/system/multi-user.target.wants/az-emmc-partition.service"
# remove the old vda-based unit if a previous build left it
rm -f "$ROOTFS/etc/systemd/system/az-mnt-storage.service" \
      "$ROOTFS/etc/systemd/system/multi-user.target.wants/az-mnt-storage.service" \
      "$ROOTFS/home/root/scripts/az-mnt-storage.sh"
echo "  -> az-emmc-partition.service installed (GPT p1..p8; emmc-fs mounts p7/p8)"
