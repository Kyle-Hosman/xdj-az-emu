#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Patch 36 (AZ): asound.conf with a single 10ch 'outs' + a real pcm.!default.
#
# Two fixes for EP147's audio init (JUCE AudioDeviceManager):
#  1. 'outs' is a single 10ch hw device (virtio_snd exposes device 0 as 10ch),
#     avoiding the `type multi` plugin.
#  2. JUCE opens its INPUT as the ALSA "default" PCM. With no pcm.!default,
#     ALSA falls back to a dmix/dsnoop default whose snd_pcm_hw_params returns
#     EPERM ("Operation not permitted"), so audio never streams and the preview
#     mixer aborts (~every 25s). A `type plug` !default made JUCE's hw_params return EPERM; a plain
#     `type hw` !default fixes it and EP147's audio init succeeds.
set -euo pipefail
: "${ROOTFS:?ROOTFS must be set by dispatcher}"
az_put 644 "$ROOTFS/etc/asound.conf" <<'CONF'
pcm.outs {
	type hw
	card ALPHATHETAD810
	device 0
}

pcm.!default {
	type hw
	card ALPHATHETAD810
	device 0
}

ctl.!default {
	type hw
	card ALPHATHETAD810
}
CONF
echo "  -> /etc/asound.conf: single 10ch outs + pcm.!default (fixes capture EPERM)"
