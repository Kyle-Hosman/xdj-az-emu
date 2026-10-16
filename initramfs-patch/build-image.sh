#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-only
# build-image.sh: runs INSIDE a Linux container (build.sh starts it). Turns the
# deck system you pulled from your own deck into the emulator's boot image.
#
#   /src     this repository (read-only)
#   /build   the build/ directory (read-write):
#              initramfs-work-pulled/initramfs.cpio.gz   from tools/az-emu-pull.py
#                                         (YOUR deck's system)
#              docker-out/                our kernel, modules, shim, tools, dropbear
#              ssh/az-emu_ed25519.pub     this install's public key
#            writes initramfs-patched.cpio.gz
#
# Everything happens on the container's own filesystem (unpack, patch, repack), so
# the host's filesystem never has to hold a Linux root tree: no case-insensitivity,
# symlink or ownership problems on macOS or Windows.
set -eu

IN=/build/initramfs-work-pulled/initramfs.cpio.gz
OUT=/build/initramfs-patched.cpio.gz
DO=/build/docker-out
PUB=/build/ssh/az-emu_ed25519.pub

apk add -q --no-cache bash cpio python3 coreutils findutils grep sed gzip >/dev/null

for f in "$IN" "$DO/Image" "$DO/ep122_shim.so" "$DO/master_out.so" "$DO/dropbear" "$PUB"; do
    [ -f "$f" ] || { echo "build-image: missing $f" >&2; exit 1; }
done

# Refuse an input that carries key material or files that must never be in an image
# (the same no-key scan az-emu-pull.py runs, with its planted controls).
echo "[image] scanning the pulled system for key material..."
python3 /src/tools/az-emu-pull.py --scan-only "$IN" >/tmp/scan.json || {
    cat /tmp/scan.json >&2
    echo "build-image: the pulled system contains files that must not go into an image (above)." >&2
    exit 1; }

ROOT=/tmp/rootfs
rm -rf "$ROOT"; mkdir -p "$ROOT"
echo "[image] unpacking your deck's system ($(du -h "$IN" | cut -f1))..."
( cd "$ROOT" && gzip -dc "$IN" | cpio -i --make-directories --preserve-modification-time 2>/dev/null ) || true
[ -x "$ROOT/home/root/pdj/EP147" ] || {
    echo "build-image: $IN does not contain the deck program (home/root/pdj/EP147)." >&2
    echo "             Pull it again with tools/az-emu-pull.py." >&2
    exit 1; }

# Firmware 2.00 and 2.01 only, recognized by the deck program's own revision string
# (home/root/pdj/app.rev), the way cdj3k-emu reads APP.REV. 2.01 is the AZ-MODS loader
# build of 2.00 and carries the same revision. AZ_EMU_ANY_FIRMWARE=1 skips the check (unsupported).
APP_REV=$(tr -d ' \r\n' < "$ROOT/home/root/pdj/app.rev" 2>/dev/null || echo unknown)
REL=$(cat "$ROOT/etc/az-mods-release" 2>/dev/null || echo "stock")
case "$APP_REV" in
    05c40500f) echo "[image] firmware 2.00/2.01 (app.rev $APP_REV, AZ-MODS release marker: $REL)" ;;
    *)  if [ "${AZ_EMU_ANY_FIRMWARE:-0}" = 1 ]; then
            echo "[image] WARNING: unknown firmware revision $APP_REV, continuing (AZ_EMU_ANY_FIRMWARE=1)"
        else
            echo "build-image: this deck program (app.rev $APP_REV) is not firmware 2.00 or 2.01." >&2
            echo "             The emulator supports XDJ-AZ firmware 2.00 and 2.01 only." >&2
            exit 1
        fi ;;
esac

# The deck's USB mod loader (firmware 2.01, the AZ-MODS release) loads the mods from the
# stick at every boot, as on the deck: patch 31 keeps it. Older versions of az-emu-pull.py
# removed it (the manifest then lists loader_entries_removed); such a system must be pulled
# again, or the emulator would load whatever the stage last held instead of the stick's set.
MAN=/build/initramfs-work-pulled/manifest.json
if [ -f "$MAN" ] && python3 -c 'import json,sys; sys.exit(0 if json.load(open(sys.argv[1])).get("loader_entries_removed") else 1)' "$MAN"; then
    echo "build-image: this system was pulled by an older tools/az-emu-pull.py, which removed the" >&2
    echo "             deck's AZ-MODS loader. Pull it again (same command, see docs/guide.md):" >&2
    echo "             the loader is what loads the mods from the USB stick." >&2
    exit 1
fi
if [ -f "$ROOT/usr/sbin/az-usbload" ] && [ -f "$ROOT/etc/systemd/system/EP147.service.d/10-azmod.conf" ]; then
    echo "[image] AZ-MODS loader: present (release $REL); mods load from the USB stick through it"
else
    echo "[image] WARNING: no AZ-MODS loader in this system (stock 2.00): the emulator will preload"
    echo "        the stage directly, which is NOT how a deck loads mods. Pull from a deck with"
    echo "        AZ-MODS installed (firmware 2.01) to load mods from the stick as a deck does."
fi

# Stage what the patch scripts install, in a writable copy of initramfs-patch/.
P=/tmp/patch
rm -rf "$P"; cp -a /src/initramfs-patch "$P"
mkdir -p "$P/vanilla-modules"
cp "$DO"/modules/*.ko "$P/vanilla-modules/"
cp "$DO/dummy_drv.so" "$DO/cfgd_aarch64" "$DO/dropbear" "$DO/dropbear.LICENSE" "$P/"
cp "$PUB" "$P/authorized_keys"

mkdir -p "$ROOT/usr/bin" "$ROOT/home/root"
install -m 755 "$DO/subucom_live_aarch64" "$ROOT/usr/bin/subucom_live_aarch64"
install -m 755 "$DO/subucom_forwarder_aarch64" "$ROOT/usr/bin/subucom_forwarder_aarch64"
install -m 755 "$DO/movewin" "$ROOT/usr/bin/movewin"
install -m 755 "$DO/ep122_shim.so" "$ROOT/home/root/ep122_shim.so"
install -m 755 "$DO/master_out.so" "$ROOT/home/root/master_out.so"

echo "[image] applying the emulator patches..."
bash "$P/patch-rootfs.sh" "$ROOT"

# build.sh's stamp: which patches and guest sources this image was built from.
printf '%s\n' "${AZ_EMU_STAMP:-patches unknown
guest unknown}" > "$ROOT/etc/az-emu-build"

echo "[image] repacking..."
mkdir -p /work
rm -rf /work/rootfs; ln -s "$ROOT" /work/rootfs
sh "$P/repack.sh"
mv /work/initramfs-patched.cpio.gz "$OUT"
echo "[image] wrote build/$(basename "$OUT") ($(du -h "$OUT" | cut -f1))"
