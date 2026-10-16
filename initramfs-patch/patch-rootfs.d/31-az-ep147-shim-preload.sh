#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Patch 31: how EP147 (the deck program) gets its mods on the emulator.
#
# As on a deck. A deck on firmware 2.01 runs AZ-MODS through the firmware's USB
# loader: az-usbload mirrors the stick's MODS/ into /home/root/settings/az-usbload/
# (the stage, on the settings partition), loads only what the set's signed trust pins
# name (or anything, with the developer switch settings/azmods-unsigned.on), and
# EP147.service.d/10-azmod.conf preloads the stage through az-preload.env. The
# emulator keeps all of that, so a set on the emulated stick runs exactly the path it
# runs on a deck, and adds only its own shim, ep122_shim.so (virtio-gpu/DRM quirks, USB
# bind writes, clock and link timing), and master_out.so (the four decks, mixed from the
# panel's controls, to the host's audio output, plus the capture and the panel's level
# meters). systemd cannot join LD_PRELOAD across drop-ins, so both are appended in
# apl_start.sh, after the loader's mods; master_out.so comes last so it hears the deck
# audio after any mod has processed it.
#
# Firmware without the loader (no /usr/sbin/az-usbload): EP147 preloads the stage
# directly, and the tools install there.
set -euo pipefail
: "${ROOTFS:?ROOTFS must be set by dispatcher}"

DROP_IN_DIR="$ROOTFS/etc/systemd/system/EP147.service.d"
install -d "$DROP_IN_DIR"
STAGE=/home/root/settings/az-usbload

if [[ -f "$ROOTFS/usr/sbin/az-usbload" && -f "$DROP_IN_DIR/10-azmod.conf" ]]; then
    rm -f "$DROP_IN_DIR/10-qemu.conf"
    a="$ROOTFS/home/root/scripts/apl_start.sh"
    [[ -f "$a" ]] || { echo "  !! no $a" >&2; exit 1; }
    { head -1 "$a"
      echo 'export LD_PRELOAD="${LD_PRELOAD:+$LD_PRELOAD:}/home/root/ep122_shim.so:/home/root/master_out.so"   # emulator shim and audio out, after the mods'
      tail -n +2 "$a"; } > "$a.new"
    chmod 755 "$a.new"; mv "$a.new" "$a"
    echo "stick" > "$ROOTFS/etc/az-emu-mods.loader"
    echo "  -> mods from the USB stick through the firmware loader; apl_start.sh appends ep122_shim.so, master_out.so"
else
    cat > "$DROP_IN_DIR/10-qemu.conf" <<SVCEOF
[Service]
Environment=LD_PRELOAD=$STAGE/azmod.so:/home/root/ep122_shim.so:$STAGE/azstem.so:/home/root/master_out.so
SVCEOF
    chmod 644 "$DROP_IN_DIR/10-qemu.conf"
    echo "  -> no USB loader in this firmware: EP147 preloads the stage directly"
fi
