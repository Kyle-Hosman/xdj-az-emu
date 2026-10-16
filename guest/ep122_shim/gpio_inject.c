// SPDX-License-Identifier: GPL-3.0-only
/*
 * gpio_inject.c - host->shim GPIO injection bridge.
 *
 * The browse encoder + panel buttons reach EP147 over /dev/gpiodrv (see
 * docs/how-it-works.md). The shim models per-GPIO levels in
 * g_gpio_level[]; this reader thread lets the HOST drive them.
 *
 * A thread reads 2-byte {gpio, level} commands from the cdj3k.gpio
 * virtio-serial port (host connects to its ctrl.sock-style socket) and:
 *   - sets g_gpio_level[gpio] = level (0/1),
 *   - writes a burst of wake bytes to g_gpiodrv_wr so any gpiodrv read()
 *     blocked on the shared pipe returns and re-reads the (new) level.
 *
 * Encoder rotation = the host sends a quadrature A/B sequence; a button press =
 * {gpio, 0} then {gpio, 1} (active-low) or vice-versa.
 */
#include "ep122_shim.h"
#include <pthread.h>
#include <fcntl.h>
#include <time.h>

#define GPIO_PORT "/dev/virtio-ports/cdj3k.gpio"

/* read exactly n bytes (virtio-serial is a byte stream) */
static int read_exact(int fd, unsigned char *buf, int n) {
    int got = 0;
    while (got < n) {
        ssize_t r = sys_read(fd, buf + got, n - got);
        if (r <= 0) return -1;
        got += (int)r;
    }
    return 0;
}

static void *gpio_reader(void *arg) {
    (void)arg;
    int fd = -1;
    for (;;) {
        if (fd < 0) {
            fd = (int)syscall(SYS_openat, AT_FDCWD, GPIO_PORT, O_RDONLY, 0);
            if (fd < 0) {
                struct timespec ts = { 0, 200000000L }; /* 200ms; nanosleep isn't intercepted */
                nanosleep(&ts, NULL);
                continue;
            }
            DBG("gpio_inject: opened %s fd=%d\n", GPIO_PORT, fd);
        }
        unsigned char cmd[2];
        if (read_exact(fd, cmd, 2) < 0) {
            /* r==0 = no host client connected to gpio.sock yet (virtio-serial
             * returns EOF immediately). Do NOT close/reopen: that thrashes the
             * exclusive port. Sleep and retry the read; it delivers once the
             * host connects. */
            struct timespec ts = { 0, 100000000L }; /* 100ms */
            nanosleep(&ts, NULL);
            continue;
        }
        unsigned char gpio = cmd[0];
        unsigned char level = cmd[1] ? 1 : 0;
        if (gpio < GPIO_MAX)
            g_gpio_level[gpio] = level;
        /* Wake gpiodrv reads: all gpiodrv fds dup one pipe, so write a small
         * burst so the fds EP147 is polling all wake and re-read their levels. */
        if (g_gpiodrv_wr >= 0) {
            unsigned char w[8] = { 1, 1, 1, 1, 1, 1, 1, 1 };
            sys_write(g_gpiodrv_wr, w, sizeof(w));
        }
        if (g_debug)
            DBG("gpio_inject: gpio %u <- level %u\n", gpio, level);
    }
    return NULL;
}

/* Spawn the reader lazily (on first /dev/gpiodrv open), NOT from a library
 * constructor: pthread_create at LD_PRELOAD load time is unreliable (libpthread
 * may not be initialized yet). By the time EP147 opens gpiodrv it is running. */
void gpio_inject_start(void) {
    static int started; /* guarded by the atomic below */
    if (__atomic_test_and_set(&started, __ATOMIC_ACQ_REL))
        return;
    pthread_t t;
    if (pthread_create(&t, NULL, gpio_reader, NULL) == 0)
        pthread_detach(t);
}
