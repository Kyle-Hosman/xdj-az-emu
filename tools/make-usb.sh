#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#   tools/make-usb.sh [SIZE_MB] [OUTPUT]     default: 256 MB, build/usb.img
# Create an empty FAT32 stick image (label AZ_USB) for the emulator's USB port.
# Uses Docker + mtools, so no mounting or root is needed. Copy music, or a
# rekordbox export (PIONEER/ and Contents/), onto it with mcopy (docs/guide.md).
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
SIZE_MB="${1:-256}"
OUT="${2:-$REPO/build/usb.img}"
mkdir -p "$(dirname "$OUT")"
OUT="$(cd "$(dirname "$OUT")" && pwd)/$(basename "$OUT")"
NAME="$(basename "$OUT")"
# Git Bash on Windows: keep the volume path as a Windows path for Docker.
case "$(uname -s)" in MINGW*|MSYS*) export MSYS_NO_PATHCONV=1; DIR="$(cd "$(dirname "$OUT")" && pwd -W)";; *) DIR="$(dirname "$OUT")";; esac
docker run --rm -v "$DIR":/out alpine:3.19 sh -c "
  apk add --no-cache mtools >/dev/null 2>&1
  dd if=/dev/zero of=/out/$NAME bs=1M count=${SIZE_MB} 2>/dev/null
  mformat -i /out/$NAME -F -v AZ_USB ::
  mmd -i /out/$NAME ::/Contents
  printf 'AZ test USB\n' > /tmp/r.txt
  mcopy -i /out/$NAME /tmp/r.txt ::/Contents/readme.txt
  mdir -i /out/$NAME ::
"
echo "$OUT ready (${SIZE_MB}MB, label AZ_USB)"
