#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# The emulator opens a stick's rekordbox library the way a deck does.
#
# A deck derives the OneLibrary key for PIONEER/rekordbox/exportLibrary.db from its
# provisioned state. The emulator has none, so EP147's decrypt fails, the stick
# browses as folders only, and the failed connection keeps exportLibrary.db open:
# every USB STOP then waits out all 20 polls of the eject script (~26 s instead of
# the deck's 2-3 s).
#
# The AZ-MODS core supplies the key when EP147 passes an empty one, gated by the flag
# /home/root/settings/azmod-onelibrarykey.on. This drop-in creates the flag before
# every EP147 start, so the library mounts and a USB STOP releases it as on a deck.
#
# To test the folders-only path, create /home/root/settings/azmod-onelibrarykey.off.
# Without AZ-MODS there is no key provider, and the stick browses as folders only.
# Emulator image only: never create the flag on a deck.
set -euo pipefail
: "${ROOTFS:?ROOTFS must be set by dispatcher}"

DROP_IN_DIR="$ROOTFS/etc/systemd/system/EP147.service.d"
install -d "$DROP_IN_DIR"
cat > "$DROP_IN_DIR/20-onelibrary-key.conf" <<'SVCEOF'
[Service]
ExecStartPre=/bin/sh -c '[ -e /home/root/settings/azmod-onelibrarykey.off ] || touch /home/root/settings/azmod-onelibrarykey.on'
SVCEOF
chmod 644 "$DROP_IN_DIR/20-onelibrary-key.conf"
echo "  -> EP147.service.d/20-onelibrary-key.conf: the OneLibrary key flag on (unless azmod-onelibrarykey.off)"
