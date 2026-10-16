#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Patch 35 (AZ): make the QEMU virtio-tablet usable as the panel input.
#
# The host adds a `virtio-tablet-pci` device so the emulator's control-surface
# UI can inject touches via QMP input-send-event.
#
# The device reports ABS_X/ABS_Y *and* BTN_LEFT, but NOT INPUT_PROP_DIRECT
# (see /sys/class/input/eventN/device/properties == 0). Tagging it
# ID_INPUT_TOUCHSCREEN (an earlier attempt) makes libinput open it on the
# touchscreen path with no pointer capability, then DROP every BTN_LEFT event:
#   (EE) libinput bug: Event for missing capability CAP_POINTER on device
#        "QEMU Virtio Tablet"
# i.e. no tap ever reached EP147. Classifying it as a pointer instead gives it
# CAP_POINTER, so absolute motion + button events flow through to X/EP147.
# Verified at the device layer: an injected abs(x,y) emits `motion a[0]=..`
# + `button press 1` (xinput test).
set -euo pipefail
: "${ROOTFS:?ROOTFS must be set by dispatcher}"

az_put 644 "$ROOTFS/etc/udev/rules.d/70-virtio-touchscreen.rules" <<'RULES'
SUBSYSTEM=="input", ATTRS{name}=="QEMU Virtio Tablet", ENV{ID_INPUT_TOUCHSCREEN}="", ENV{ID_INPUT_MOUSE}="1"
RULES

echo "  -> 70-virtio-touchscreen.rules installed (virtio-tablet -> absolute pointer)"
