#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Patch 34 (AZ): run movewin to reposition EP147's window to (0,0).
#
# EP147 places its 1280x800 window at (-640,-400) (center at screen origin),
# which only works when the DSI panel is physically rotated (device: xrandr
# --rotate left). QEMU virtio-gpu can't do that rotation, so we run on a native
# 1280x800 landscape screen and move EP147's window to (0,0) so the full UI is
# visible. movewin is a tiny X client (guest/movewin/movewin.c) staged into
# /usr/bin by build.sh.
set -euo pipefail
: "${ROOTFS:?ROOTFS must be set by dispatcher}"

az_put 644 "$ROOTFS/etc/systemd/system/az-movewin.service" <<'UNIT'
[Unit]
Description=AZ emu: move EP147 window to (0,0) (landscape geometry fix)
After=EP147.service x11-only.service

[Service]
Type=simple
Environment=DISPLAY=:0
ExecStart=/usr/bin/movewin
Restart=always
RestartSec=2

[Install]
WantedBy=graphical.target
UNIT

install -d "$ROOTFS/etc/systemd/system/graphical.target.wants"
ln -sf ../az-movewin.service \
    "$ROOTFS/etc/systemd/system/graphical.target.wants/az-movewin.service"
echo "  -> az-movewin.service installed (moves EP147 window to 0,0)"
