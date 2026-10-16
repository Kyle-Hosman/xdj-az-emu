#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Widen EP147.service's restart-limit window.
#
# EP147.service ships with Restart=on-failure, but under systemd's default limit
# (5 starts in 10 s) a short crash loop (an init race, USB timing, a bad mod)
# exhausts it and leaves a black screen until someone restarts the emulator. This
# allows more attempts over 5 minutes, while a truly wedged guest still stops
# retrying instead of looping forever.
set -euo pipefail
: "${ROOTFS:?ROOTFS must be set by dispatcher}"

DROP_IN_DIR="$ROOTFS/etc/systemd/system/EP147.service.d"
install -d "$DROP_IN_DIR"
# StartLimitIntervalSec / StartLimitBurst are [Unit] directives (systemd >= 229);
# under [Service] systemd silently ignores them.
cat > "$DROP_IN_DIR/30-az-restart-policy.conf" <<'SVCEOF'
[Unit]
StartLimitIntervalSec=300
StartLimitBurst=20

[Service]
RestartSec=2s
SVCEOF
chmod 644 "$DROP_IN_DIR/30-az-restart-policy.conf"
echo "  -> EP147.service.d/30-az-restart-policy.conf installed (RestartSec=2s, StartLimitBurst=20/300s)"
