#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Patch 41: az-emu-modsinstall, how the tools put mods where a deck gets them: on the stick.
#
#   az-emu-modsinstall stick          the stick mount (the one with MODS/ first)
#   az-emu-modsinstall NAME < file    write MODS/NAME on the stick (.new + rename, synced)
#   az-emu-modsinstall rm NAME        remove MODS/NAME from the stick
#
# Only the stick is written. The deck's USB loader (az-usbload) then stages the set from the
# stick, as on a deck, so the stage can never hold anything the stick does not carry.
set -euo pipefail
: "${ROOTFS:?ROOTFS must be set by dispatcher}"
az_put 755 "$ROOTFS/usr/sbin/az-emu-modsinstall" <<'SH'
#!/bin/sh
stick() {
  m=""
  for mp in $(awk '$2 ~ "^/media/usb/" {print $2}' /proc/mounts); do
    [ -d "$mp/MODS" ] && { echo "$mp"; return 0; }
    [ -z "$m" ] && m=$mp
  done
  [ -n "$m" ] && { echo "$m"; return 0; }
  return 1
}
rw() { awk -v m="$1" '$2==m && $4 ~ /(^|,)ro(,|$)/ {f=1} END{exit !f}' /proc/mounts && mount -o remount,rw "$1"; return 0; }
case "$1" in
  stick) stick ;;
  rm)
    M=$(stick) || { echo "az-emu-modsinstall: no USB stick mounted" >&2; exit 1; }
    rw "$M"; rm -f "$M/MODS/$2"; sync ;;
  *)
    M=$(stick) || { echo "az-emu-modsinstall: no USB stick mounted" >&2; exit 1; }
    rw "$M"; mkdir -p "$M/MODS" || exit 1
    cat > "$M/MODS/$1.new" && mv -f "$M/MODS/$1.new" "$M/MODS/$1" || exit 1
    sync
    echo "$1 $(md5sum "$M/MODS/$1" | cut -d' ' -f1) stick=$M" ;;
esac
SH
echo "  -> az-emu-modsinstall installed"
