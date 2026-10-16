# syntax=docker/dockerfile:1
# SPDX-License-Identifier: GPL-3.0-only
# Derived from cdj3k-emu (MIT OR Apache-2.0); see ATTRIBUTION.md.
#
# XDJ-AZ emulator guest build - Linux 6.6 LTS, aarch64/virt target.
# Runs natively on arm64 (Apple Silicon). On an x86_64 host Docker runs it
# through arm64 emulation, which works but is much slower.
#
# Produces all guest artifacts in /out/:
#   Image              - kernel image (PL011 console, virtio built-in)
#   modules/           - out-of-tree kernel modules (.ko)
#   ep122_shim.so      - LD_PRELOAD shim (glibc 2.17-compatible)
#   subucom_forwarder_aarch64  subucom_live_aarch64  cfgd_aarch64 - static tools
#   dummy_drv.so       - Xorg Video Driver ABI 24.0 for headless mode
#   movewin            - tiny X client that moves EP147's window to (0,0)
#   dropbear           - our own SSH server, built from the pinned upstream
#                        release (the emulator's automation channel)
#
# Single source of truth for guest user-space build flags is `guest/Makefile`.
# Two stages invoke it with different toolchains:
#   builder       (alpine, musl gcc)  → static tools (subucom_*, cfgd) + kernel + modules
#   shim-builder  (ubuntu, glibc gcc) → ep122_shim.so (must be glibc-linked
#                                       for LD_PRELOAD into EP122)

FROM arm64v8/alpine:3.19 AS builder

RUN apk add --no-cache \
    build-base \
    bc \
    flex \
    bison \
    perl \
    openssl-dev \
    lz4-dev \
    lz4-static \
    libdrm-dev \
    bash \
    wget \
    xz \
    git \
    file \
    python3 \
    rsync

# Clone Linux 6.6 LTS (shallow).  Pinned to a specific point release so
# the build is reproducible: re-running the same Dockerfile months apart
# produces the same kernel.  Bump deliberately to pick up CVE fixes.
# kernel.org first, then Greg KH's GitHub mirror of the same stable tree (some networks
# cannot reach git.kernel.org). The commit is pinned, so both give the same source.
# NO pipe here: `git clone ... | tail -3` would report a failed clone as success, Docker
# would cache that empty layer, and the next step would die on a missing /kernel.
ARG LINUX_TAG=v6.6.138
ARG LINUX_COMMIT=3b9f64db049687c0d38b4b3ef2f297f0642179af
RUN for url in https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git \
               https://github.com/gregkh/linux.git; do \
        rm -rf /kernel; \
        timeout 900 git -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=60 \
            clone --quiet --depth=1 --branch ${LINUX_TAG} "$url" /kernel && break; \
    done && \
    test "$(git -C /kernel rev-parse HEAD)" = "${LINUX_COMMIT}"

# Apply our Linux 6.6 kernel patches.  Each modification (Pioneer virtio_blk
# naming, synthetic DSI-2 jog output, dummy MIN_PERIOD_SIZE bump, virtgpu_kms
# VERSION_1 guard drop) lives as a unified-diff patch in guest/kernel-patches/.
# A failure to apply means upstream Linux drifted under our anchors - the
# build aborts here, signalling that the patch needs regeneration.
#
# The .config is the only non-patch artifact and is copied separately.
COPY guest/kernel-patches/ /tmp/kernel-patches/
RUN cp /tmp/kernel-patches/cdj3k-emu-kernel.config /kernel/.config && \
    cd /kernel && \
    for p in /tmp/kernel-patches/*.patch; do \
        echo "    [apply] $(basename $p)"; \
        git apply --check "$p" || { echo "FAIL: $p"; exit 1; }; \
        git apply "$p"; \
    done && \
    rm -rf /tmp/kernel-patches

# Build the kernel Image, keep it, then delete the object files in the SAME layer
# so they never land in Docker's storage (they are ~2 GB and nothing below needs
# them: out-of-tree modules only need `modules_prepare`). Keeps the build
# friendly to a small disk.
RUN cd /kernel && make ARCH=arm64 olddefconfig >/dev/null && \
    make ARCH=arm64 -j$(nproc) Image >/tmp/kbuild.log 2>&1 || { tail -40 /tmp/kbuild.log; exit 1; } ; \
    mkdir -p /out && cp arch/arm64/boot/Image /out/Image && \
    find . -name '*.o' -delete && rm -f vmlinux vmlinux.o arch/arm64/boot/Image* && \
    make ARCH=arm64 -j$(nproc) modules_prepare 2>&1 | tail -3

# arm64 defconfig has many =m drivers; building them all just to get Module.symvers
# is too slow.  Our modules only use core kernel symbols (_printk, spinlocks,
# cdev/class, ALSA PCM) that are unconditionally present in any 6.x kernel and
# CONFIG_MODVERSIONS is off in defconfig, so version CRC mismatches don't cause
# insmod failures at runtime.  KBUILD_MODPOST_WARN turns the "undefined symbol"
# modpost errors into warnings and lets the build produce usable .ko files.

# subucom_virt - virtual /dev/subucom_spi1.0 (the deck front-panel MCU buses)
COPY guest/modules/subucom_virt/ /subucom_virt/
RUN cd /kernel && make ARCH=arm64 -j$(nproc) KBUILD_MODPOST_WARN=1 M=/subucom_virt modules

# virtio_snd - custom virtio-sound PCM playback (out-of-tree, replaces SND_VIRTIO=n upstream)
COPY guest/modules/virtio_snd/ /virtio_snd/
RUN cd /kernel && make ARCH=arm64 -j$(nproc) KBUILD_MODPOST_WARN=1 M=/virtio_snd modules

# udev_usb1 - the /proc/udev_* media and gadget event channels (absent from the vanilla kernel)
COPY guest/modules/udev_usb1/ /udev_usb1/
RUN cd /kernel && make ARCH=arm64 -j$(nproc) KBUILD_MODPOST_WARN=1 M=/udev_usb1 modules

# uinput - /dev/uinput (the deck's goodix touch path). The
# emu config has CONFIG_INPUT_UINPUT off; build the kernel's own uinput.c out of tree
# instead of rebuilding the kernel. Its include of "../input-compat.h" is rewritten.
COPY guest/modules/uinput/ /uinput/
RUN cp /kernel/drivers/input/input-compat.h /uinput/ && \
    sed 's|\.\./input-compat\.h|input-compat.h|' /kernel/drivers/input/misc/uinput.c > /uinput/uinput.c && \
    cd /kernel && make ARCH=arm64 -j$(nproc) KBUILD_MODPOST_WARN=1 M=/uinput modules

# dummy_drv.so - Xorg Video Driver ABI 24.0 for headless mode.
# Ubuntu 20.04 focal arm64 ships Xorg 1.20.9 (ABI 24.0), matching Pioneer Xorg 1.20.4.
# Fetched over HTTPS and SHA-256 verified so a substituted upstream package
# can't silently end up in the bundle.
ARG DUMMY_DRV_DEB_SHA256=866876b535aedba516f3495c0b8bdf57abf3e65cf4351e38a45789973081ceab
RUN wget -q \
        "https://ports.ubuntu.com/ubuntu-ports/pool/main/x/xserver-xorg-video-dummy/xserver-xorg-video-dummy_0.3.8-1build3_arm64.deb" \
         -O /tmp/dummy.deb && \
    echo "${DUMMY_DRV_DEB_SHA256}  /tmp/dummy.deb" | sha256sum -c - && \
    cd /tmp && mkdir -p dummy_extract && cd dummy_extract && \
    ar x /tmp/dummy.deb && \
    tar -xJf data.tar.xz ./usr/lib/xorg/modules/drivers/dummy_drv.so && \
    ls -la /tmp/dummy_extract/usr/lib/xorg/modules/drivers/dummy_drv.so

# Static guest tools, driven by guest/Makefile so flags stay in one place.
# Alpine arm64v8's plain `gcc` is the musl-libc cross compiler we want for
# fully-static aarch64 binaries that don't depend on glibc.
COPY guest/ /tmp/guest/
RUN make -C /tmp/guest CC=gcc tools

# Collect everything except the shim into /out/. The shim is built in the
# glibc stage below because it has to be LD_PRELOAD-compatible with EP122
# (a Pioneer aarch64 binary dynamically linked against glibc 2.17). A
# musl-linked .so would NEEDED `libc.so` instead of `libc.so.6` and the
# glibc dynamic linker would refuse to load it.
RUN mkdir -p /out/modules && \
    cp /subucom_virt/subucom_virt.ko                                 /out/modules/ && \
    cp /virtio_snd/virtio_snd.ko                                     /out/modules/ && \
    cp /udev_usb1/udev_usb1.ko                                       /out/modules/ && \
    cp /uinput/uinput.ko                                             /out/modules/ && \
    cp /tmp/dummy_extract/usr/lib/xorg/modules/drivers/dummy_drv.so /out/ && \
    cp /tmp/guest/out/subucom_forwarder_aarch64                      /out/ && \
    cp /tmp/guest/out/subucom_live_aarch64                           /out/ && \
    cp /tmp/guest/out/cfgd_aarch64                                   /out/ && \
    echo "=== /out/ ===" && ls -la /out/ && echo "=== /out/modules/ ===" && ls -la /out/modules/

# ep122_shim.so: built against glibc 2.27 (Ubuntu 18.04) so it can
# LD_PRELOAD into Pioneer's glibc 2.17 EP122 process. Alpine's musl gcc
# produces a `.so` that lists `libc.so` (musl) in NEEDED; glibc's ld.so
# searches for `libc.so.6` and fails with
# `error while loading shared libraries: libc.so: cannot open shared
# object file`. Same Makefile, different toolchain.
FROM ubuntu:18.04 AS shim-builder
RUN apt-get update -qq && \
    apt-get install -y --no-install-recommends gcc libc6-dev make file libx11-dev curl ca-certificates bzip2 && \
    rm -rf /var/lib/apt/lists/*
COPY guest/ /tmp/guest/
RUN make -C /tmp/guest CC=gcc shim
# movewin: tiny X client that repositions EP147's window to (0,0). glibc-linked
# (like the shim) so it runs against the deck rootfs's libX11.
RUN gcc -O2 -o /tmp/guest/out/movewin /tmp/guest/movewin/movewin.c -lX11

# dropbear: our OWN SSH server, built from the pinned upstream release (MIT-style
# license, shipped beside it in the image). The deck's 2.x firmware has no SSH
# server, and the emulator's tools all talk to the guest over SSH, so the image
# carries this one. Keys only: built without PAM or zlib, and run with -s (no
# password logins). glibc 2.27 here is older than the deck's, so it runs there.
ARG DROPBEAR_VER=2024.86
ARG DROPBEAR_SHA256=e78936dffc395f2e0db099321d6be659190966b99712b55c530dd0a1822e0a5e
RUN cd /tmp && \
    curl -sfLO "https://matt.ucc.asn.au/dropbear/releases/dropbear-${DROPBEAR_VER}.tar.bz2" && \
    echo "${DROPBEAR_SHA256}  dropbear-${DROPBEAR_VER}.tar.bz2" | sha256sum -c - && \
    tar xjf dropbear-${DROPBEAR_VER}.tar.bz2 && cd dropbear-${DROPBEAR_VER} && \
    ./configure --disable-pam --disable-zlib --disable-lastlog --disable-utmp \
                --disable-utmpx --disable-wtmp --disable-wtmpx >/tmp/dropbear-configure.log 2>&1 && \
    make -j"$(nproc)" PROGRAMS=dropbear >/tmp/dropbear-make.log 2>&1 && \
    strip dropbear && mkdir -p /tmp/dropbear-out && \
    cp dropbear /tmp/dropbear-out/dropbear && cp LICENSE /tmp/dropbear-out/dropbear.LICENSE && \
    timeout 3 /tmp/dropbear-out/dropbear -F -E -s -R -p 2299 -r /tmp/hk >/tmp/dropbear-opt.log 2>&1; \
    if grep -qiE "invalid option|usage" /tmp/dropbear-opt.log; then cat /tmp/dropbear-opt.log; exit 1; fi

FROM scratch AS artifacts
COPY --from=builder      /out/                                /
COPY --from=shim-builder /tmp/guest/out/ep122_shim.so         /ep122_shim.so
COPY --from=shim-builder /tmp/guest/out/master_out.so         /master_out.so
COPY --from=shim-builder /tmp/guest/out/movewin               /movewin
COPY --from=shim-builder /tmp/dropbear-out/dropbear           /dropbear
COPY --from=shim-builder /tmp/dropbear-out/dropbear.LICENSE   /dropbear.LICENSE
