#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Patch 02: KEY-ONLY root SSH for the emulator's tools.
#
# Every tool in this repository talks to the guest over SSH (127.0.0.1:2222 on the
# host, forwarded to the guest's port 22 by QEMU, nothing else can reach it). The
# deck's 2.x firmware ships no SSH server, so the image gets OUR dropbear, built
# from the pinned upstream release in docker/Dockerfile, and the public key that
# build.sh generated for this install. There is no password login of any kind:
# dropbear runs with -s, root's password is left exactly as the firmware has it.
#
# SCHED_RR 50: the deck program's startup runs real-time threads with RT
# throttling off, and a normal-priority dropbear can be starved for minutes while
# it starts. 50 sits above its audio thread and below its MCU bus thread.
set -euo pipefail
: "${ROOTFS:?ROOTFS must be set by dispatcher}"
: "${PATCH_ASSETS_DIR:?PATCH_ASSETS_DIR must be set by dispatcher}"

install -D -m 755 "$PATCH_ASSETS_DIR/dropbear" "$ROOTFS/usr/sbin/az-emu-dropbear"
install -D -m 644 "$PATCH_ASSETS_DIR/dropbear.LICENSE" "$ROOTFS/usr/share/doc/az-emu-dropbear/LICENSE"

# Both candidate root homes: passwd says /home/root, some tools look in /root.
for H in home/root root; do
    install -d -m 700 "$ROOTFS/$H/.ssh"
    install -m 600 "$PATCH_ASSETS_DIR/authorized_keys" "$ROOTFS/$H/.ssh/authorized_keys"
done
# dropbear refuses keys under a home directory others can write to.
chmod go-w "$ROOTFS/home/root" "$ROOTFS/root" 2>/dev/null || true
# No host key is baked into the image: dropbear -R makes one at first connection,
# in RAM, so every boot (and every install) has its own. Remove any that a rootfs
# might carry. The 2.x firmware has no /etc/dropbear at all, and dropbear -R
# cannot create the directory itself ("Couldn't read or generate hostkey").
rm -f "$ROOTFS"/etc/dropbear/dropbear_*_host_key
install -d -m 700 "$ROOTFS/etc/dropbear"

cat > "$ROOTFS/etc/systemd/system/az-emu-dropbear.service" <<'UNIT'
[Unit]
Description=Emulator SSH (key-only dropbear, host 127.0.0.1:2222 -> guest :22)
After=network.target

[Service]
# -s no password logins, -R make host keys on first use (they live in RAM, so
# each boot gets new ones; the tools do not pin them), -E log to stderr.
ExecStart=/usr/sbin/az-emu-dropbear -F -E -s -R -p 22
Restart=always
RestartSec=2
CPUSchedulingPolicy=rr
CPUSchedulingPriority=50

[Install]
WantedBy=multi-user.target
UNIT
mkdir -p "$ROOTFS/etc/systemd/system/multi-user.target.wants"
ln -sf ../az-emu-dropbear.service "$ROOTFS/etc/systemd/system/multi-user.target.wants/az-emu-dropbear.service"

# The deck's loader runs the stick's boot hooks, and the SSH hook (MODS/deck/boot.d/
# 10-sshgate.sh) manages root's authorized_keys as on a deck: it deletes them when the stick
# does not turn SSH on, and replaces them with the stick's key when it does. On a deck that
# is right; here it would lock the tools out. So the emulator keeps its key in
# /etc/az-emu/authorized_keys and puts it back into both files whenever either .ssh
# directory changes, beside any key the hook installed.
install -D -m 600 "$PATCH_ASSETS_DIR/authorized_keys" "$ROOTFS/etc/az-emu/authorized_keys"
chmod 700 "$ROOTFS/etc/az-emu"
az_put 755 "$ROOTFS/usr/sbin/az-emu-sshkey" <<'SH'
#!/bin/sh
# az-emu-sshkey: make sure the emulator's own key is in root's authorized_keys (patch 02)
K=/etc/az-emu/authorized_keys
[ -s "$K" ] || exit 0
for H in /home/root /root; do
    f="$H/.ssh/authorized_keys"
    mkdir -p "$H/.ssh" && chmod 700 "$H/.ssh"
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        grep -qxF "$line" "$f" 2>/dev/null && continue
        printf '%s\n' "$line" >> "$f" && echo "az-emu-sshkey: restored the emulator's key in $f"
    done < "$K"
    chmod 600 "$f"
done
exit 0
SH
cat > "$ROOTFS/etc/systemd/system/az-emu-sshkey.service" <<'UNIT'
[Unit]
Description=Emulator SSH: keep the emulator's key in root's authorized_keys

[Service]
Type=oneshot
ExecStart=/usr/sbin/az-emu-sshkey
UNIT
cat > "$ROOTFS/etc/systemd/system/az-emu-sshkey.path" <<'UNIT'
[Unit]
Description=Emulator SSH: watch root's .ssh directories (the stick's SSH hook rewrites them)

[Path]
PathChanged=/home/root/.ssh
PathChanged=/root/.ssh
Unit=az-emu-sshkey.service

[Install]
WantedBy=multi-user.target
UNIT
ln -sf ../az-emu-sshkey.path "$ROOTFS/etc/systemd/system/multi-user.target.wants/az-emu-sshkey.path"

# A firmware dropbear unit (older firmware) would fight ours for port 22: keep it off.
rm -f "$ROOTFS/etc/systemd/system/multi-user.target.wants/dropbear.service" \
      "$ROOTFS/etc/systemd/system/multi-user.target.wants/dropbear.socket"
echo "  -> az-emu-dropbear.service: key-only SSH with this install's key (no passwords)"
