#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Derived from cdj3k-emu (MIT OR Apache-2.0); see ATTRIBUTION.md.
#
# build.sh: build the emulator's boot files.
#
#   ./build.sh              everything: kernel, modules, tools, and the boot image
#   ./build.sh --no-image   only our own parts (kernel, modules, shim, tools,
#                           dropbear); needs nothing from a deck
#   ./build.sh --image-only only the boot image, reusing build/docker-out/ from an
#                           earlier build (yours, or copied from another machine:
#                           those files are all ours, built from this repo)
#   ./build.sh --clean      also remove the Docker build cache afterwards
#
# Inputs:
#   build/initramfs-work-pulled/initramfs.cpio.gz
#                                    YOUR deck's system, from tools/az-emu-pull.py.
#                                    This is the ONLY input that comes from the deck's
#                                    firmware, and it never leaves your machine.
#   everything else                  built here from source (docker/Dockerfile)
#
# Outputs (all under build/, which git ignores):
#   Image                       our Linux 6.6 kernel
#   initramfs-patched.cpio.gz   your deck's system + the emulator patches
#   az-mnt.img                  the emulated eMMC (settings + cache), made once
#   usb.img                     an empty emulated USB stick, made once
#   ssh/az-emu_ed25519          the SSH key for THIS install (made once)
#
# Needs: Docker (Docker Desktop, colima or OrbStack), ssh-keygen, Python 3
# (python3, python or the Windows py launcher, whichever runs).
set -euo pipefail
REPO="$(cd "$(dirname "$0")" && pwd)"
BUILD="${AZ_EMU_BUILD:-$REPO/build}"   # AZ_EMU_BUILD: put the build on another disk
DOCKER_OUT="$BUILD/docker-out"
PULLED="$BUILD/initramfs-work-pulled/initramfs.cpio.gz"
KEY="$BUILD/ssh/az-emu_ed25519"
NEED_GB="${AZ_EMU_NEED_GB:-}"

NO_IMAGE=0; CLEAN=0; IMAGE_ONLY=0
for arg in "$@"; do
    case "$arg" in
        --no-image) NO_IMAGE=1 ;;
        --clean)    CLEAN=1 ;;
        --image-only) IMAGE_ONLY=1 ;;
        -h|--help)  sed -n '5,29p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $arg (try --help)" >&2; exit 1 ;;
    esac
done

say()  { printf '\n==> %s\n' "$*"; }
# source_hash PATCH_DIRS|GUEST_DIRS: the hash of those sources (tools/azemu.py).
source_hash() {
    "${PY[@]}" -c "import sys; sys.path.insert(0, sys.argv[1]); import azemu
print(azemu.source_hash(*getattr(azemu, sys.argv[2])))" "$REPO/tools" "$1"
}
fail() { printf '\nERROR: %s\n' "$*" >&2; exit 1; }

# Git Bash on Windows rewrites /paths in the arguments of Windows programs. That is
# what python.exe needs (it gets G:\... for /g/...), but it would mangle Docker's
# "G:/x:/src:ro" volume specs, so ONLY the docker commands below run with
# MSYS_NO_PATHCONV=1 and get Windows paths from hostpath.
case "$(uname -s)" in
    MINGW*|MSYS*) hostpath() { (cd "$1" && pwd -W); } ;;
    *)            hostpath() { (cd "$1" && pwd); } ;;
esac

command -v docker >/dev/null || fail "Docker is not installed (see docs/requirements.md)."
docker info >/dev/null 2>&1 || fail "Docker is installed but not running. Start Docker Desktop (or colima) and try again."
command -v ssh-keygen >/dev/null || fail "ssh-keygen not found (it comes with OpenSSH)."
# Python 3: python3 on macOS/Linux; Windows installs give python and py instead (and
# may have a python3 that only opens the Microsoft Store), so test that each one runs.
PY=()
for cand in python3 python "py -3"; do
    if $cand -c 'import sys; sys.exit(sys.version_info < (3, 9))' >/dev/null 2>&1; then
        read -ra PY <<<"$cand"; break
    fi
done
(( ${#PY[@]} )) || fail "Python 3.9 or newer not found (tried python3, python and py -3)."
lf_rc=0; "${PY[@]}" "$REPO/tools/check-lf.py" >/dev/null || lf_rc=$?
case "$lf_rc" in
    0) ;;
    1) fail "CRLF line endings in the checkout (listed above)." ;;
    *) fail "tools/check-lf.py could not run (exit $lf_rc, error above)." ;;
esac

if [[ "$NO_IMAGE" == 0 && ! -f "$PULLED" ]]; then
    cat >&2 <<MSG

The emulator needs the system from YOUR deck, and it is not here yet:
    $PULLED

This repository ships no firmware. Pull the system from your own rooted
XDJ-AZ (firmware 2.00+) first:
    ${PY[*]} tools/az-emu-pull.py --host DECK-IP --key path/to/private-key

To build only our own parts now (kernel, modules, tools), run:
    ./build.sh --no-image
MSG
    exit 2
fi

mkdir -p "$BUILD"
# A first full build needs ~10 GB (Docker's kernel build); the image step alone ~2 GB.
[[ -n "$NEED_GB" ]] || { [[ "$IMAGE_ONLY" == 1 ]] && NEED_GB=2 || NEED_GB=10; }
free_gb=$("${PY[@]}" -c "import shutil,sys; print(shutil.disk_usage(sys.argv[1]).free // 2**30)" "$BUILD")
if (( free_gb < NEED_GB )) && [[ "${AZ_EMU_SKIP_DISK_CHECK:-0}" != 1 ]]; then
    fail "only ${free_gb} GB free where build/ lives; the build needs about ${NEED_GB} GB
       (the kernel build inside Docker plus the images). Free some space, or set
       AZ_EMU_SKIP_DISK_CHECK=1 if you know your Docker disk lives elsewhere."
fi
# Windows: Docker Desktop keeps its disk on the system drive (under %LOCALAPPDATA%), not
# where build/ lives, and the kernel build fills it by ~10 GB. Check that drive too.
case "$(uname -s)" in MINGW*|MSYS*)
    if [[ "$IMAGE_ONLY" == 0 && -n "${LOCALAPPDATA:-}" && "${AZ_EMU_SKIP_DISK_CHECK:-0}" != 1 ]]; then
        sys_gb=$("${PY[@]}" -c "import shutil,sys; print(shutil.disk_usage(sys.argv[1]).free // 2**30)" "$LOCALAPPDATA")
        (( sys_gb >= 15 )) || fail "only ${sys_gb} GB free on the drive with Docker Desktop's disk
       ($LOCALAPPDATA); building the kernel here needs about 15 GB there.
       Recommended on Windows: copy build/docker-out/ from a Mac build and run
       ./build.sh --image-only (see docs/windows.md)."
    fi ;;
esac

say "[1/4] SSH key for this install"
if [[ ! -f "$KEY" ]]; then
    mkdir -p "$(dirname "$KEY")"
    ssh-keygen -q -t ed25519 -N '' -C "xdj-az-emu $(date +%Y-%m-%d)" -f "$KEY"
    case "$(uname -s)" in   # Windows OpenSSH refuses a key others can read
        MINGW*|MSYS*) icacls "$(cygpath -w "$KEY")" /inheritance:r /grant:r "$USERNAME:(R)" >/dev/null ;;
    esac
    echo "    made $KEY (it never leaves this machine; the image gets the .pub)"
else
    echo "    using $KEY"
fi

say "[2/4] Kernel, modules, shim, tools and dropbear (docker/Dockerfile)"
if [[ "$IMAGE_ONLY" == 1 ]]; then
    for f in Image ep122_shim.so master_out.so movewin dropbear cfgd_aarch64 dummy_drv.so modules/subucom_virt.ko; do
        [[ -f "$DOCKER_OUT/$f" ]] || fail "--image-only needs build/docker-out/$f from an earlier ./build.sh"
    done
    echo "    reusing build/docker-out/ (--image-only)"
else
    echo "    The first build downloads Linux and compiles it: 10-20 minutes on Apple"
    echo "    Silicon, much longer on an x86_64 host (arm64 emulation)."
    rm -rf "$DOCKER_OUT"
    MSYS_NO_PATHCONV=1 docker buildx build --platform linux/arm64 --target artifacts \
        --output "type=local,dest=$(hostpath "$BUILD")/docker-out" \
        -f "$(hostpath "$REPO")/docker/Dockerfile" "$(hostpath "$REPO")"
    # Which guest sources these were built from, for the image's build stamp.
    source_hash GUEST_DIRS > "$DOCKER_OUT/guest.hash"
fi
cp "$DOCKER_OUT/Image" "$BUILD/Image"
echo "    build/Image: $(du -h "$BUILD/Image" | cut -f1)"

if [[ "$NO_IMAGE" == 1 ]]; then
    say "Done (--no-image). Our parts are in build/docker-out/ and build/Image."
    echo "    Pull your deck's system with tools/az-emu-pull.py, then run ./build.sh."
    exit 0
fi

say "[3/4] Boot image from your deck's system"
# The image's build stamp (/etc/az-emu-build): the hash of this checkout's patches, and
# of the guest sources build/docker-out/ came from. tools/az-emu-health.py compares it
# with the checkout at every start and says when the image is older.
stamp="patches $(source_hash PATCH_DIRS)
guest $(cat "$DOCKER_OUT/guest.hash" 2>/dev/null || echo unknown)"
# The host's native alpine: this step only unpacks, edits files and repacks, it
# never runs an aarch64 program, so no emulation is needed on an x86_64 host.
MSYS_NO_PATHCONV=1 docker run --rm \
    -e AZ_EMU_ANY_FIRMWARE="${AZ_EMU_ANY_FIRMWARE:-0}" -e AZ_EMU_STAMP="$stamp" \
    -v "$(hostpath "$REPO"):/src:ro" -v "$(hostpath "$BUILD"):/build" \
    alpine:3.19 sh /src/initramfs-patch/build-image.sh

say "[4/4] Disks"
# 14 GB, the smallest whole size that settles: the reserve partition (/mnt) gets the
# disk less 0.7 GiB for the other partitions; ext4 keeps 5% of it for root; the deck
# program's five track caches take 9.5 GiB (5 x 2042626048 bytes); and the deck's
# emmc-fs.sh reformats /mnt at every boot when less than 2 GiB is left (patch 32). At
# 14 GB about 2.9 GiB is left; at 13 GB, 1.9. Sparse, so it costs the host only what
# the guest writes. The boot scripts enlarge an older, smaller image.
if [[ ! -f "$BUILD/az-mnt.img" ]]; then
    gb="${AZ_EMU_MNT_GB:-14}"
    case "$(uname -s)" in
        MINGW*|MSYS*)
            # Python's truncate() WRITES ZEROS on Windows, which would allocate the whole
            # image. Make it sparse instead: create it, mark it sparse, then extend with
            # SetLength (which allocates nothing once the sparse flag is set).
            win="$(hostpath "$BUILD")/az-mnt.img"
            : > "$BUILD/az-mnt.img"
            fsutil sparse setflag "$win" >/dev/null
            powershell.exe -NoProfile -Command \
                "\$f=[IO.File]::Open('$win','Open','ReadWrite'); \$f.SetLength([int64]$gb*1GB); \$f.Close()"
            ;;
        *)
            "${PY[@]}" -c "import sys; f=open(sys.argv[1],'wb'); f.truncate(int(sys.argv[2])<<30)" "$BUILD/az-mnt.img" "$gb"
            ;;
    esac
    echo "    made build/az-mnt.img (${gb} GB, sparse; partitioned on first boot)"
else
    echo "    keeping build/az-mnt.img (your emulator's settings and installed mods)"
fi
[[ -f "$BUILD/usb.img" ]] || "$REPO/tools/make-usb.sh" 256 "$BUILD/usb.img"

if [[ "$CLEAN" == 1 ]]; then
    docker builder prune -f >/dev/null && echo "    Docker build cache removed"
fi

say "Build complete"
cat <<DONE
    Kernel:  build/Image
    Image:   build/initramfs-patched.cpio.gz
    Next:    macOS: build QEMU once (qemu/build.sh), then open AZ-EMU.app.
             Windows: set AZ_QEMU to qemu-system-aarch64.exe, then run az-preview
             or tools/az-boot-win.ps1 (docs/windows.md).
DONE
