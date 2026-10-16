#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Patch 37 (AZ): re-announce mounted USB media to EP147 after an EP147 restart.
#
# The same re-announce runs on a real deck. EP147 learns
# about media ONLY from the /proc/udev_usb{1,2} mailbox that device-mount.sh
# writes at plug time. On an EP147 (re)start the USB stays mounted but EP147
# never re-reads it and drops it from the Source list, forcing an unplug/replug.
# usb-renotify.sh re-writes that mailbox. An ExecStartPost runs it on every
# EP147 start with --until-ack: it announces each mounted stick, then checks
# that EP147 took it (EP147 holds a file under the mount point, normally
# PIONEER/rekordbox/exportLibrary.db) and re-announces with backoff for up to
# 90 s. Every attempt is logged to the journal (usb-renotify) and
# /tmp/usb-renotify.log.
#
# Measured on the emulator (udev_usb1.ko):
# - a message written before EP147 opens the mailbox is queued and read on
#   open, so a late EP147 still gets it;
# - EP147's StorageMntDetct thread reads udev_usb1, then udev_usb2, and on the
#   emulator each read blocks until a message arrives. After one usb1 message
#   it waits on usb2, so a second usb1 message is not read by that EP147: it
#   stays queued and is consumed by the NEXT EP147 start, where it would hide a
#   missing announcement. The script therefore only writes while the stick is
#   not taken and stops once EP147 has read the mailbox.
#
# The firmware 2.01 loader's post-start path (az-usbload --post-start) calls
# this script without arguments: that is a one-shot announce of every stick
# EP147 has not taken.
#
# emu note: the emu only creates /proc/udev_usb1 (bus topology differs from the
# device, so udevadm won't resolve 1-1/2-1) -> the busid case falls back to the
# first existing mailbox. Identical behavior to the AZ, where udevadm resolves.
set -euo pipefail
: "${ROOTFS:?ROOTFS must be set by dispatcher}"

SCRIPTS_DIR="$ROOTFS/home/root/scripts"
install -d "$SCRIPTS_DIR"
cat > "$SCRIPTS_DIR/usb-renotify.sh" <<'SHEOF'
#!/bin/sh
# usb-renotify.sh - re-announce already-mounted USB media to EP147 after an
# EP147 (re)start. See patch 37-usb-renotify.sh for why.
#
#   usb-renotify.sh              announce, once, every mounted stick EP147 has
#                                not taken (the firmware loader's post-start
#                                path calls it this way)
#   usb-renotify.sh --until-ack  announce, then poll until EP147 has taken every
#                                stick, re-announcing with backoff (1, 2, 4, 4 ... s)
#                                for up to AZ_RENOTIFY_MAX seconds (default 90)
#
# Taken = EP147 holds a file open under the stick's mount point (outside MODS/,
# where the preloaded mods poll). EP147 opens PIONEER/rekordbox/exportLibrary.db
# when it accepts the mount message and keeps it open; while the stick is not
# announced it holds nothing there and BROWSE has no source.
#
# Every attempt is logged to the journal (tag usb-renotify) and to $LOG.
LOG=${AZ_RENOTIFY_LOG:-/tmp/usb-renotify.log}
MAX=${AZ_RENOTIFY_MAX:-90}

up() { cut -d' ' -f1 /proc/uptime; }
T0=$(up)
log() {
    msg="+$(awk -v a="$T0" -v b="$(up)" 'BEGIN{printf "%.1f", b-a}')s $*"
    echo "[$(date '+%H:%M:%S')] $msg" >> "$LOG" 2>/dev/null
    logger -t usb-renotify "$msg" 2>/dev/null
    return 0
}
ep147() { pidof EP147 2>/dev/null | awk '{print $1}'; }
fds() { ls -l /proc/"$1"/fd 2>/dev/null | sed -n 's/.* -> //p'; }

# taken PID MOUNTPOINT: print the file EP147 holds under the mount point
taken() {
    f=$(fds "$1" | grep -F "$2/" | grep -vF "$2/MODS/" | head -1)
    [ -n "$f" ] && echo "$f"
}

# moved_on PID BOX: EP147's storage thread reads the mailboxes in turn (usb1,
# then usb2, ...) and keeps only the one it is waiting on open. If it has
# another one open and not BOX, it has read BOX since it opened it. On the
# emulator its read blocks until a message arrives, so a later message to BOX
# is not read until another mailbox gets one: re-announcing cannot reach it.
moved_on() {
    open=$(fds "$1" | grep -E '^/proc/udev_usb[12]$')
    [ -n "$open" ] && ! echo "$open" | grep -qx "$2"
}

# box_of DEV: the /proc/udev_usbN mailbox for that stick's port
box_of() {
    busid=$(udevadm info --query=path --name="$1" 2>/dev/null | grep -oE '/[12]-1/' | head -1 | tr -d /)
    case "$busid" in
        1-1) echo /proc/udev_usb1 ;;
        2-1) echo /proc/udev_usb2 ;;
        *)   ls /proc/udev_usb1 /proc/udev_usb2 2>/dev/null | head -1 ;;
    esac
}

# pass: one look at every mounted stick; announce those not taken. Sets LEFT
# to how many are still not taken.
SENT=" "
pass() {
    P=$(ep147); LEFT=0
    for mp in /media/usb/sd*; do
        [ -d "$mp" ] || continue
        line=$(mount | grep " $mp ")
        [ -n "$line" ] || continue
        dev=$(echo "$line" | awk '{print $1}')
        if [ -n "$P" ] && f=$(taken "$P" "$mp"); then
            log "attempt $ATTEMPT: $mp taken by EP147 $P ($f)"
            continue
        fi
        LEFT=$((LEFT + 1))
        if [ -z "$P" ]; then
            log "attempt $ATTEMPT: $mp not announced, EP147 is not running"
            continue
        fi
        BOX=$(box_of "$dev")
        if [ -z "$BOX" ] || [ ! -e "$BOX" ]; then
            log "attempt $ATTEMPT: $mp ($dev) has no mailbox, not announcing"
            continue
        fi
        case "$SENT" in *" $mp "*)
            if moved_on "$P" "$BOX"; then
                log "attempt $ATTEMPT: EP147 $P read $BOX but holds nothing under $mp yet; not re-announcing (it is waiting on another mailbox)"
                continue
            fi ;;
        esac
        fs=$(echo "$line" | sed -E 's/.*type ([^ ]+).*/\1/')
        base=$(echo "$dev" | sed -E 's#/dev/(sd[a-h]).*#\1#')
        ro=$(cat /sys/block/$base/ro 2>/dev/null || echo 0)
        if fds "$P" | grep -qx "$BOX"; then st="EP147 $P has it open"; else st="EP147 $P has not opened it yet, queued"; fi
        echo -n "mount $mp $fs protect:$ro" > "$BOX"
        SENT="$SENT$mp "
        log "attempt $ATTEMPT: announced $mp ($dev, $fs, protect:$ro) -> $BOX ($st)"
    done
}

ATTEMPT=1
if [ "${1:-}" != --until-ack ]; then
    log "one-shot announce"
    pass
    exit 0
fi

log "start: announcing until EP147 has taken every mounted stick (max ${MAX}s)"
delay=1
while :; do
    pass
    if [ "$LEFT" -eq 0 ]; then
        log "done: every mounted stick is taken (attempt $ATTEMPT)"
        exit 0
    fi
    if awk -v a="$T0" -v b="$(up)" -v m="$MAX" 'BEGIN{exit !(b-a>=m)}'; then
        log "GAVE UP: $LEFT stick(s) still not taken after ${MAX}s"
        exit 1
    fi
    # backoff: a slow EP147 gets time to open the library before the next look
    sleep "$delay"
    ATTEMPT=$((ATTEMPT + 1))
    delay=$((delay * 2)); [ "$delay" -gt 4 ] && delay=4
done
SHEOF
chmod 755 "$SCRIPTS_DIR/usb-renotify.sh"

DROP_IN_DIR="$ROOTFS/etc/systemd/system/EP147.service.d"
install -d "$DROP_IN_DIR"
cat > "$DROP_IN_DIR/20-usb-renotify.conf" <<'SVCEOF'
[Service]
# Re-announce mounted USB media to EP147 after it (re)starts, until EP147 has
# taken each stick (see patch 37).
ExecStartPost=-/bin/sh -c '/home/root/scripts/usb-renotify.sh --until-ack </dev/null >/dev/null 2>&1 &'
SVCEOF
chmod 644 "$DROP_IN_DIR/20-usb-renotify.conf"

echo "  [37] usb-renotify.sh + EP147.service.d/20-usb-renotify.conf installed (USB re-announced until EP147 takes it)"
