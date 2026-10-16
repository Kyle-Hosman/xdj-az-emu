#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-only
# az-usb2.sh: plug a SECOND USB stick into the running emulator, as USB2.
#
#   tools/az-usb2.sh [image]      default: build/usb2.img (made empty with
#                                 tools/make-usb.sh if missing)
#   tools/az-usb2.sh announce     re-announce after an EP147 restart
#
# How: QMP hot-plug of a usb-storage device on ehci.0 (port 4446,
# the automation port; 4445 is AZ-EMU.app's), the guest mounts it at
# /media/usb/sdb, and one message on /proc/udev_usb2 tells EP147, exactly as the
# deck's mount script does for port 2. /proc/udev_usb2 comes from udev_usb1.ko.
# EP147 consumes one announcement per read, and forgets USB2 on every restart:
# run `announce` again after each restart.
set -euo pipefail
cd "$(dirname "$0")/.."
SSH=(tools/az-ssh)
announce(){ "${SSH[@]}" "printf 'mount /media/usb/sdb vfat protect:0' > /proc/udev_usb2" && echo "announced USB2"; }
if [ "${1:-}" = announce ]; then announce; exit 0; fi

IMG="$(cd "$(dirname "${1:-build/usb2.img}")" && pwd)/$(basename "${1:-build/usb2.img}")"
[ -e "$IMG" ] || { tools/make-usb.sh 256 "$IMG"; }

if ! "${SSH[@]}" 'test -e /proc/udev_usb2'; then
  echo "guest has no /proc/udev_usb2: the emulator image is older than this checkout; run ./build.sh" >&2
  exit 1
fi

python3 - "$IMG" <<'PY'
import socket, json, sys
s = socket.create_connection(('127.0.0.1', 4446)); f = s.makefile('rw')
def rd():
    while True:
        m = json.loads(f.readline())
        if 'event' not in m: return m
def cmd(c, a=None):
    f.write(json.dumps({'execute': c, **({'arguments': a} if a else {})}) + '\n'); f.flush()
    r = rd(); print(c, 'ok' if 'return' in r else r); return r
rd(); cmd('qmp_capabilities')
cmd('blockdev-add', {'driver': 'raw', 'node-name': 'usb2', 'file': {'driver': 'file', 'filename': sys.argv[1]}})
cmd('device_add', {'driver': 'usb-storage', 'bus': 'ehci.0', 'drive': 'usb2', 'id': 'usbstick2'})
PY
for i in $(seq 1 20); do "${SSH[@]}" 'mount | grep -q " /media/usb/sdb "' && break; sleep 1; done
announce
