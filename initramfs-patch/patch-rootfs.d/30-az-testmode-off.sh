#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Patch 30 (AZ): create /tmp/testmode=off at boot so apl_start.sh launches EP147.
#
# On real hardware, subucom_read (called from pre-setting.sh) queries the
# front-panel MCU and writes /tmp/testmode. The emulated subucom does not answer
# that query, and apl_start.sh HARD-GATES the app launch on the file existing
# ("can't read /tmp/testmode" -> no EP147). This installs a tmpfiles entry that
# creates /tmp/testmode=off early.
set -euo pipefail
: "${ROOTFS:?ROOTFS must be set by dispatcher}"

install -d "$ROOTFS/etc/tmpfiles.d"
cat > "$ROOTFS/etc/tmpfiles.d/az-testmode.conf" <<'TMPEOF'
#Type Path           Mode UID GID Age Argument
f     /tmp/testmode  0644 root root -  off
TMPEOF
echo "  -> /etc/tmpfiles.d/az-testmode.conf (creates /tmp/testmode=off)"
