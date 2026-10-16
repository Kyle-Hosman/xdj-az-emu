// SPDX-License-Identifier: GPL-3.0-only
// Derived from cdj3k-emu (MIT OR Apache-2.0); see ATTRIBUTION.md.
/*
 * syscalls_ioctl.c - interposed ioctl(2). Dispatches DRM master/render-node
 * fds to handle_drm_ioctl (jog.c) and the subucom virtual SPI fd to its own
 * handler; everything else falls through to the real libc ioctl.
 */

 #include "ep122_shim.h"

int ioctl(int fd, unsigned long request, ...) {
    va_list ap; va_start(ap, request);
    void *arg = va_arg(ap, void *);
    va_end(ap);

    /* DRM fd */
    if (is_drm_fd(fd))
        return handle_drm_ioctl(fd, request, arg);

    /* ALSA sequencer fd - stub the MIDI client lifecycle, discard the rest */
    if (is_seq_fd(fd)) {
        if (request == ALSA_SEQ_IOCTL_PVERSION) {
            /* snd_seq_open() checks major version: must be 1 or it closes fd
             * and returns error → seq==NULL → snd_seq_nonblock() asserts.
             * SND_SEQ_PROTOCOL_VERSION = (1<<16)|(0<<8)|2 = 0x010002 */
            if (arg) *(int *)arg = 0x010002;
            DBG("alsa seq(fd=%d, PVERSION) -> 0x010002\n", fd);
        } else if (request == ALSA_SEQ_IOCTL_CLIENT_ID) {
            /* Return EP122's real client number from the hardware: 128 */
            if (arg) *(int *)arg = 128;
            DBG("alsa seq(fd=%d, CLIENT_ID) -> 128\n", fd);
        }
        /* All other seq ioctls (CREATE_PORT, SUBSCRIBE_PORT, SET_CLIENT_INFO,
         * etc.) succeed silently.  EP122 checks return value, not output
         * structs for most of these. */
        return 0;
    }

    /* GPIODRV fd - accept any ioctl silently (e.g. GPIO pin configure) */
    if (is_gpiodrv_fd(fd)) {
        DBG("gpiodrv ioctl fd=%d request=0x%lx arg=%p\n", fd, (unsigned long)request, arg);
        return 0;
    }

    /* HIDG fd - accept any ioctl silently.
     *   Without this, ioctl() falls through to sys_ioctl(pipe_fd, …)
     *   which returns ENOTTY.  EP122's USB gadget manager interprets
     *   ENOTTY as a gadget initialization failure and may show
     *   "USB Error. Remove the device." */
    if (is_hidg_fd(fd))
        return 0;

    /* All subucom ioctls pass through to subucom_virt.ko */
    long r = sys_ioctl(fd, request, arg);
    if (g_debug && r < 0)
        DBG("ioctl FAIL fd=%d request=0x%lx ret=%ld errno=%d\n",
            fd, (unsigned long)request, r, errno);
    return r;
}

/* ------------------------------------------------------------------ */
/* read                                                               */
/* ------------------------------------------------------------------ */
ssize_t read(int fd, void *buf, size_t count) {
    /* --- DRM fd: synthesise a DRM_EVENT_FLIP_COMPLETE when a flip is pending.
     *   EP122's render loop calls drmHandleEvent(fd, ctx) which internally calls
     *   read() expecting a 32-byte drm_event_vblank.  The real card0 fd has no
     *   actual flip events because SETPLANE is faked.  We return a synthetic
     *   packet when inject_drm_flip_event() has been called (counter > 0);
     *   otherwise EAGAIN (non-blocking, caller should poll first). */
    if (is_drm_fd(fd)) {
        /* Per-fd flip event: only synthesise for fds that have a flip slot. */
        jog_flip_slot_t *rslot = flip_slot_for(fd);
        if (!rslot) { errno = EAGAIN; return -1; }
        int pending = __atomic_exchange_n(&rslot->pending, 0, __ATOMIC_ACQUIRE);
        if (!pending || count < 32) { errno = EAGAIN; return -1; }
        static uint32_t g_rd_seq = 0;
        uint8_t ev[32];
        memset(ev, 0, sizeof(ev));
        uint32_t type = 2u, length = 32u; /* DRM_EVENT_FLIP_COMPLETE */
        uint32_t seq  = __atomic_fetch_add(&g_rd_seq, 1, __ATOMIC_RELAXED);
        uint64_t ud   = __atomic_load_n(&rslot->user_data, __ATOMIC_ACQUIRE);
        memcpy(ev + 0,  &type,   4);
        memcpy(ev + 4,  &length, 4);
        memcpy(ev + 8,  &ud,     8);
        memcpy(ev + 24, &seq,    4);
        memcpy(buf, ev, 32);
        return 32;
    }

    /* --- ALSA seq fd: O_NONBLOCK - no incoming MIDI in QEMU --- */
    if (is_seq_fd(fd)) {
        errno = EAGAIN;
        return -1;
    }

    /* --- HIDG fd: no USB host connected → EAGAIN (not EOF).
     *   read() returning 0 = EOF would cause EP122 to close the fd and
     *   crash its HID receive thread.  EAGAIN tells it "no data yet". */
    if (is_hidg_fd(fd)) {
        errno = EAGAIN;
        return -1;
    }

    /* --- GPIODRV fd: the pipe drives WHEN a read completes (poll wake /
     *   blocking); we substitute WHAT it returns = the level of the GPIO this fd
     *   was lseek'd to (real driver returns gpio_get_value(offset)). The host bridge
     *   (gpio_inject.c) sets g_gpio_level[] + writes a wake byte to g_gpiodrv_wr. */
    if (is_gpiodrv_fd(fd)) {
        ssize_t r = sys_read(fd, buf, count);
        if (r > 0 && count >= 1) {
            long gpio = gpiodrv_get_off(fd);
            unsigned char level = (gpio >= 0 && gpio < GPIO_MAX) ? g_gpio_level[gpio] : 1;
            ((unsigned char *)buf)[0] = level;
            r = 1;
            if (g_debug)
                DBG("gpiodrv read fd=%d gpio=%ld -> level %u\n", fd, gpio, level);
        }
        return r;
    }

    /* --- All other fds (including subucom_virt.ko): passthrough --- */
    return sys_read(fd, buf, count);
}

/* ------------------------------------------------------------------ */
/* lseek - gpiodrv uses the file offset as the GPIO number. The stub fd is a    */
/* pipe (unseekable), so we must record the offset ourselves and report success */
/* instead of letting it fail ESPIPE (which silently broke every panel read).   */
/* ------------------------------------------------------------------ */
off_t lseek(int fd, off_t offset, int whence) {
    if (is_gpiodrv_fd(fd)) {
        if (whence == SEEK_SET)
            gpiodrv_set_off(fd, (long)offset);
        return offset;
    }
    return sys_lseek(fd, offset, whence);
}
/* EP147 (glibc 2.29) selects the panel GPIO via lseek64 on the gpiodrv fd. We
 * MUST intercept it too: otherwise it hits the unseekable pipe (ESPIPE) and the
 * GPIO is never bound (reads come back gpio=-1 and the panel never arms). */
off_t lseek64(int fd, off_t offset, int whence) {
    return lseek(fd, offset, whence);
}

/* ------------------------------------------------------------------ */
/* write - intercept HIDG fds to silently discard USB HID frames      */
/* ------------------------------------------------------------------ */
/*
 * EP122 writes 64-byte HID reports to hidg0 at regular intervals.
 * We return count immediately without writing to the pipe (which would
 * fill up since nobody drains the write-end).
 */
ssize_t write(int fd, const void *buf, size_t count) {
    if (is_hidg_fd(fd)) {
        DBG("hidg0 write fd=%d count=%zu bytes[0..3]=%02x %02x %02x %02x\n", fd, count,
            count > 0 ? ((const unsigned char*)buf)[0] : 0,
            count > 1 ? ((const unsigned char*)buf)[1] : 0,
            count > 2 ? ((const unsigned char*)buf)[2] : 0,
            count > 3 ? ((const unsigned char*)buf)[3] : 0);
        (void)buf;
        return (ssize_t)count;
    }
    if (is_seq_fd(fd)) {
        /* Silently discard MIDI events - no f_midi gadget in QEMU.
         * EP122 sends MIDI to USB host via sequencer; we drop it. */
        (void)buf;
        return (ssize_t)count;
    }
    return sys_write(fd, buf, count);
}


/* ------------------------------------------------------------------ */
/* fcntl - passthrough with failure logging (JUCE async audio setup)  */
/* ------------------------------------------------------------------ */
#include <fcntl.h>
int fcntl(int fd, int cmd, ...) {
    va_list ap; va_start(ap, cmd);
    void *arg = va_arg(ap, void *);
    va_end(ap);
    long r = syscall(SYS_fcntl, fd, cmd, arg);
    if (r < 0) {
        int e = (int)-r;
        if (g_debug) DBG("fcntl FAIL fd=%d cmd=%d arg=%p errno=%d\n", fd, cmd, arg, e);
        errno = e;
        return -1;
    }
    return (int)r;
}
