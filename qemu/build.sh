#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Derived from cdj3k-emu (MIT OR Apache-2.0); see ATTRIBUTION.md.
# qemu/build.sh
#
# macOS (Apple Silicon): clone QEMU at a pinned commit, apply qemu/patches/, and
# build qemu-system-aarch64 with HVF (hardware virtualization) and Cocoa.
# Needs Homebrew: brew install glib pixman libslirp ninja pkgconf python@3.11
# (Windows uses the official QEMU build instead; see docs/requirements.md.)
#
# Usage:
#   cd <repo-root>
#   bash qemu/build.sh
#
# Output:
#   qemu/src/        - QEMU source tree
#   qemu/build/      - build artifacts
#   qemu/install/    - installed binaries (qemu-system-aarch64 lives here)
#
# Re-running is idempotent: clone and patch steps are skipped if already done.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PATCHES_DIR="${SCRIPT_DIR}/patches"
SRC_DIR="${SCRIPT_DIR}/src"
BUILD_DIR="${SCRIPT_DIR}/build"
INSTALL_DIR="${SCRIPT_DIR}/install"

# Pinned to a master snapshot that includes the HVF in-kernel GIC support
# (Mohamed Mediouni's series). Stable QEMU 11.0.0 predates that work, so a
# master pin is required for the in-kernel vGIC.
QEMU_REF="ee7eb612be8f8886d48c1d0c1f1c65e495138f83"
QEMU_REPO="https://github.com/qemu/qemu.git"

# --------------------------------------------------------------------------
# 1. Clone
# --------------------------------------------------------------------------

need_fetch=0
if [ ! -d "${SRC_DIR}/.git" ]; then
    need_fetch=1
else
    current_sha="$(git -C "${SRC_DIR}" rev-parse HEAD 2>/dev/null || echo unknown)"
    if [ "${current_sha}" != "${QEMU_REF}" ]; then
        echo "==> QEMU src at ${current_sha:0:12} does not match required ${QEMU_REF:0:12}"
        echo "    Wiping ${SRC_DIR} and re-fetching."
        rm -rf "${SRC_DIR}" "${BUILD_DIR}"
        need_fetch=1
    fi
fi

if [ "${need_fetch}" = "1" ]; then
    echo "==> Fetching QEMU @ ${QEMU_REF} …"
    git init -q "${SRC_DIR}"
    git -C "${SRC_DIR}" remote add origin "${QEMU_REPO}"
    git -C "${SRC_DIR}" fetch --depth=1 origin "${QEMU_REF}"
    git -C "${SRC_DIR}" checkout -q FETCH_HEAD
    # No submodules needed for a minimal aarch64-softmmu+cocoa build.
else
    echo "==> Source tree already at ${QEMU_REF:0:12} - skipping fetch"
fi

# --------------------------------------------------------------------------
# 2. Apply patches
# --------------------------------------------------------------------------
# Every QEMU modification (overlay file replacements, ivshmem gating, virtio
# hot-swap fix, etc.) is captured as a unified-diff patch in qemu/patches/.
# Patches are pinned to the QEMU_REF above; if the SHA is bumped, regenerate
# them by editing src/ and running `git -C src diff > patches/NN-foo.patch`,
# then restore the SPDX line at the top (git apply ignores text before the diff).
#
# A failure to apply means upstream QEMU drifted under one of our anchors -
# fail loudly here so the user catches it before a half-patched src/ silently
# produces a binary missing features or behaving oddly.

if ! ls "${PATCHES_DIR}"/*.patch >/dev/null 2>&1; then
    echo "ERROR: no patches found in ${PATCHES_DIR}"
    exit 1
fi

# Check whether any patch is already applied to the current tree (idempotent
# re-runs after a successful build land here).  We probe with --reverse
# --check: if reversing applies cleanly, the patch is already in.
patch_changed=0
echo "==> Applying QEMU patches from $(basename "${PATCHES_DIR}")/"
for p in "${PATCHES_DIR}"/*.patch; do
    name="$(basename "$p")"
    if git -C "${SRC_DIR}" apply --reverse --check "$p" >/dev/null 2>&1; then
        echo "    [skip]   ${name} - already applied"
        continue
    fi
    if ! git -C "${SRC_DIR}" apply --check "$p" 2>/dev/null; then
        echo "    [FAIL]   ${name} - does not apply cleanly"
        echo
        echo "Upstream QEMU likely drifted under this patch's anchors."
        echo "Regenerate the patch:"
        echo "  - inspect: git -C ${SRC_DIR} apply --check ${p}"
        echo "  - rebase the change manually, then:"
        echo "      git -C ${SRC_DIR} diff -- <paths> > ${p}"
        exit 1
    fi
    git -C "${SRC_DIR}" apply "$p"
    echo "    [apply]  ${name}"
    patch_changed=1
done

# Force reconfigure when any patch landed - meson caches Kconfig results in
# build.ninja, so a stale build dir won't pick up newly-enabled symbols.
if [ "${patch_changed}" = "1" ] && [ -f "${BUILD_DIR}/build.ninja" ]; then
    echo "    invalidating ${BUILD_DIR}/build.ninja to trigger reconfigure"
    rm -f "${BUILD_DIR}/build.ninja"
fi

# --------------------------------------------------------------------------
# 3. Configure
#
# --disable-pvg: ParavirtualizedGraphics.framework (3D passthrough to the host
# via hw/display/apple-gfx.m). The guest never instantiates it, and the
# framework's PGTask_t is obsolete in the macOS 27 SDK, which fails the build
# under the option's `auto` default.
# --------------------------------------------------------------------------

mkdir -p "${BUILD_DIR}"

if [ ! -f "${BUILD_DIR}/build.ninja" ]; then
    echo "==> Configuring …"
    # configure must be invoked from the build directory:
    # it runs `meson setup "$PWD" "$source_path"` internally.
    (
        cd "${BUILD_DIR}"
        # QEMU's bundled pip/meson bootstrap breaks under Python 3.14 (the
        # Homebrew default). Prefer a 3.11/3.12; override with QEMU_PYTHON.
        # The fallback list is fixed; extend it if a host lacks all three.
        QEMU_PYTHON="${QEMU_PYTHON:-$(command -v python3.12 || command -v python3.11 || command -v python3)}"
        "${SRC_DIR}/configure" \
            --python="${QEMU_PYTHON}"   \
            --prefix="${INSTALL_DIR}"   \
            --target-list=aarch64-softmmu \
            --enable-hvf                \
            --enable-cocoa              \
            --disable-gtk               \
            --disable-sdl               \
            --disable-curses            \
            --disable-vnc               \
            --disable-fuse              \
            --disable-pvg               \
            --disable-dbus-display      \
            --disable-docs              \
            --disable-guest-agent       \
            --disable-plugins           \
            --enable-tools              \
            --disable-install-blobs     \
            --enable-trace-backends=nop \
            --enable-slirp              \
            --disable-werror            \
            --extra-cflags="-O2"
    )
else
    echo "==> Build already configured - skipping configure"
fi

# --------------------------------------------------------------------------
# 4. Build
# --------------------------------------------------------------------------

JOBS="${JOBS:-$(sysctl -n hw.logicalcpu 2>/dev/null || nproc)}"
echo "==> Building with ${JOBS} jobs …"
# `--enable-tools` configures a fleet of small helper tools (qemu-img,
# qemu-io, qemu-nbd, qemu-storage-daemon, qemu-edid, elf2dmp,
# ivshmem-client, ivshmem-server) for install; `meson install` aborts if
# any of them is missing, so build them all even though the .app only
# ships qemu-img.  They link fast, so the extra ninja work is negligible.
ninja -C "${BUILD_DIR}" -j"${JOBS}" \
    qemu-system-aarch64 \
    qemu-img qemu-io qemu-nbd storage-daemon/qemu-storage-daemon \
    qemu-edid contrib/elf2dmp/elf2dmp \
    contrib/ivshmem-client/ivshmem-client \
    contrib/ivshmem-server/ivshmem-server



# --------------------------------------------------------------------------
# 5. Install
# --------------------------------------------------------------------------

echo "==> Installing …"
# Use meson install --no-rebuild instead of `ninja install` - the latter
# pulls in test binaries (e.g. tests/audio/test-audio) that don't link
# virtio-snd.c.o and therefore can't resolve our coreaudio.m bypass refs.
# Meson is bundled inside the build dir's pyvenv (no system meson).
"${BUILD_DIR}/pyvenv/bin/meson" install -C "${BUILD_DIR}" --no-rebuild

echo ""
echo "Done. Binary at: ${INSTALL_DIR}/bin/qemu-system-aarch64"
