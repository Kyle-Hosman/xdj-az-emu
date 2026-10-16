#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Derived from cdj3k-emu (MIT OR Apache-2.0); see ATTRIBUTION.md.
# initramfs-patch/patch-rootfs.sh
#
# Dispatcher: runs every script in patch-rootfs.d/ (in numeric order).
#
# Usage:
#   ./patch-rootfs.sh <path/to/initramfs-root>
#
# Each script in patch-rootfs.d/ receives these environment variables:
#   ROOTFS           - path to the extracted initramfs rootfs (from $1)
#   PATCH_ASSETS_DIR - path to this directory (contains cfgd_aarch64 binary from tools/)
#   APP_UNIT         - the player application's systemd unit, detected from the
#                      rootfs: EP147.service on the XDJ-AZ
#   APP_NAME         - APP_UNIT without the .service suffix (the process name)
#   APP_SLUG         - the model's short name: xdjaz on the XDJ-AZ; the patches
#                      kept from cdj3k-emu also know cdj3k, cdj3kx and cdj1500x

set -euo pipefail

ROOTFS="${1:?Usage: $0 <initramfs-root>}"
export ROOTFS
export PATCH_ASSETS_DIR="$(cd "$(dirname "$0")" && pwd)"

# Steps that order a service before the app, or install a drop-in for it, use
# APP_UNIT, so the patches kept from cdj3k-emu serve the XDJ-AZ unchanged.
APP_UNIT="$(cd "$ROOTFS/etc/systemd/system" 2>/dev/null && ls EP1[0-9][0-9].service 2>/dev/null | head -n1 || true)"
export APP_UNIT="${APP_UNIT:-EP147.service}"
export APP_NAME="${APP_UNIT%.service}"
case "$APP_UNIT" in
    EP147.service) export APP_SLUG="xdjaz" ;;
    EP145.service) export APP_SLUG="cdj3kx" ;;
    EP166.service) export APP_SLUG="cdj1500x" ;;
    *)             export APP_SLUG="cdj3k" ;;
esac

PATCH_D="$PATCH_ASSETS_DIR/patch-rootfs.d"

echo "=== Patching initramfs rootfs at: $ROOTFS (app unit: $APP_UNIT) ==="
echo ""

shopt -s nullglob
scripts=("$PATCH_D"/[0-9]*.sh)

if [[ ${#scripts[@]} -eq 0 ]]; then
    echo "ERROR: no patch scripts found in $PATCH_D" >&2
    exit 1
fi

# az_put MODE PATH <<'EOF' ... : write stdin to PATH (parents created) with MODE. Replaces
# GNU `install -Dm<mode> /dev/stdin PATH`, which BSD install refuses with "Inappropriate
# file type or format" (kept portable although the patches now run in a Linux container).
az_put() { mkdir -p "$(dirname "$2")" && cat > "$2" && chmod "$1" "$2"; }
export -f az_put
for script in "${scripts[@]}"; do
    echo "--- $(basename "$script") ---"
    bash "$script"
    echo ""
done

echo "=== All patches applied ==="
