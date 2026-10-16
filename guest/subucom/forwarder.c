// SPDX-License-Identifier: GPL-3.0-only
// Derived from cdj3k-emu (MIT OR Apache-2.0); see ATTRIBUTION.md.
/*
 * subucom_forwarder - bidirectional bridge between /dev/subucom_ctrl and the
 * cdj3k.ctrl virtio-serial port.
 *
 *   LED  (guest→host): subucom_virt.ko writes MOSI frames to /dev/subucom_ctrl
 *                      → this process reads them → writes to vport → ctrl.sock
 *   CTRL (host→guest): ctrl.sock → vport → this process reads → /dev/subucom_ctrl
 *                      → subucom_virt.ko delivers MISO frames to EP122
 *   LEVELS (guest→host): master_out.so's level page (LEVEL_PAGE) → vport, as
 *                      frames from bus LEVEL_BUS, one per new measurement
 *   MIXER TX (guest):  the deck program's latest mixer-bus write is also kept in
 *                      MIXER_TX_PAGE, where master_out.so reads the settings the
 *                      deck program sends the mixer (crossfader assign, curves)
 *
 * Build (aarch64 static):
 *   aarch64-linux-gnu-gcc -O2 -static -o subucom_forwarder_aarch64 subucom_forwarder.c -lpthread
 */

#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <dirent.h>
#include <pthread.h>
#include <stdatomic.h>
#include <sys/mman.h>
#include <sys/stat.h>

#define CTRL_PATH   "/dev/subucom_ctrl"
#define PORT_NAME   "cdj3k.ctrl"
/* LED/MOSI frame guest->host: [0]=source bus id + the 152-byte payload (see
 * subucom_virt.c ctrl_read; the bus tag lets the host tell an erp pad-LED
 * write apart from a deck or mixer write; shorter writes are zero-padded). */
#define FRAME_SIZE  157  /* 1 bus tag + 152B MOSI frame + 4B real cmd.size */
/* AZ host->guest control frame: 1 bus-id byte + 152-byte MISO overlay.
 * subucom_virt.ko's ctrl_write parses [bus] + 152 and re-stamps the per-bus
 * CRC. (CDJ used a bare 64-byte frame; the AZ protocol is bus-tagged.) */
#define AZ_CTRL_SIZE 153

/* Levels: the page master_out.so publishes (see master_out.c), sent on as a
 * frame from a bus id no SPI bus uses: the sequence number, then each channel's
 * peak, then each channel's RMS, as little-endian floats (fractions of full
 * scale). */
#define LEVEL_PAGE     "/dev/shm/az-levels"
#define LEVEL_BUS      4
#define LEVEL_CHANNELS 10
#define LEVEL_POLL_US  10000

struct level_page {
    _Atomic uint32_t seq;
    _Atomic uint32_t peak[LEVEL_CHANNELS];
    _Atomic uint32_t rms[LEVEL_CHANNELS];
};

/* The latest mixer-bus write (bus id MIXER_BUS in the LED frames), for
 * master_out.so: a sequence number (odd while being written) and the frame. */
#define MIXER_TX_PAGE "/dev/shm/az-mixer-tx"
#define MIXER_BUS     2
#define MIXER_TX_SIZE 152

struct mixer_tx_page {
    _Atomic uint32_t seq;
    _Atomic uint32_t frame[MIXER_TX_SIZE / 4];
};

static struct mixer_tx_page *mixer_tx;

static void mixer_tx_open(void)
{
    int fd = open(MIXER_TX_PAGE, O_RDWR | O_CREAT | O_CLOEXEC, 0644);
    if (fd < 0)
        return;
    if (ftruncate(fd, sizeof *mixer_tx) == 0) {
        void *m = mmap(NULL, sizeof *mixer_tx, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        if (m != MAP_FAILED)
            mixer_tx = m;
    }
    close(fd);
}

static void mixer_tx_publish(const uint8_t *frame)
{
    uint32_t w[MIXER_TX_SIZE / 4];
    memcpy(w, frame, sizeof w);
    uint32_t seq = atomic_load_explicit(&mixer_tx->seq, memory_order_relaxed);
    atomic_store_explicit(&mixer_tx->seq, seq + 1, memory_order_relaxed);
    atomic_thread_fence(memory_order_release);
    for (size_t i = 0; i < MIXER_TX_SIZE / 4; i++)
        atomic_store_explicit(&mixer_tx->frame[i], w[i], memory_order_relaxed);
    atomic_store_explicit(&mixer_tx->seq, seq + 2, memory_order_release);
}

static int ctrl_rfd = -1;  /* O_RDONLY - read MOSI/LED frames */
static int ctrl_wfd = -1;  /* O_WRONLY - write MISO/CTRL frames */
static int vport_fd = -1;  /* O_RDWR  - virtio-serial port     */
/* Two threads send to the host; a frame must reach it whole. */
static pthread_mutex_t vport_lock = PTHREAD_MUTEX_INITIALIZER;

/* Find the /dev/vportNpM path for a virtio-serial port by name. */
static int find_vport(char *out, size_t out_sz)
{
    DIR *d = opendir("/sys/class/virtio-ports");
    if (!d)
        return -1;

    struct dirent *de;
    while ((de = readdir(d)) != NULL) {
        if (de->d_name[0] == '.')
            continue;

        char name_path[512];
        snprintf(name_path, sizeof(name_path),
                 "/sys/class/virtio-ports/%s/name", de->d_name);

        FILE *f = fopen(name_path, "r");
        if (!f)
            continue;

        char name[64] = {0};
        if (!fgets(name, sizeof(name), f)) {
            fclose(f);
            continue;
        }
        fclose(f);
        name[strcspn(name, "\n")] = '\0';

        if (strcmp(name, PORT_NAME) == 0) {
            snprintf(out, out_sz, "/dev/%s", de->d_name);
            closedir(d);
            return 0;
        }
    }
    closedir(d);
    return -1;
}

static int open_ctrl_r(void)
{
    int fd;
    while ((fd = open(CTRL_PATH, O_RDONLY)) < 0) {
        fprintf(stderr, "[subucom_forwarder] waiting for %s (r): %s\n",
                CTRL_PATH, strerror(errno));
        usleep(100000);
    }
    return fd;
}

static int open_ctrl_w(void)
{
    int fd;
    while ((fd = open(CTRL_PATH, O_WRONLY)) < 0) {
        fprintf(stderr, "[subucom_forwarder] waiting for %s (w): %s\n",
                CTRL_PATH, strerror(errno));
        usleep(100000);
    }
    return fd;
}

static int open_vport(const char *path)
{
    int fd;
    while ((fd = open(path, O_RDWR)) < 0) {
        fprintf(stderr, "[subucom_forwarder] waiting for %s: %s\n",
                path, strerror(errno));
        usleep(200000);
    }
    return fd;
}

static ssize_t read_exact(int fd, uint8_t *buf, size_t n)
{
    size_t got = 0;
    while (got < n) {
        ssize_t r = read(fd, buf + got, n - got);
        if (r < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        if (r == 0)
            return 0;
        got += (size_t)r;
    }
    return (ssize_t)got;
}

/* write() can return a short write (a partial byte count, not an error) on
 * a virtio-serial port fd, which a plain `write() < 0` check misses. A
 * short write here silently shifts every subsequent frame boundary on the
 * host's read_exact(FRAME_SIZE) side, splicing two frames into one garbage
 * frame forever (az-preview then decodes torn LED frames and the pad colors
 * flicker). Loop until all n bytes are sent. */
static ssize_t write_exact(int fd, const uint8_t *buf, size_t n)
{
    size_t sent = 0;
    while (sent < n) {
        ssize_t w = write(fd, buf + sent, n - sent);
        if (w < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        sent += (size_t)w;
    }
    return (ssize_t)sent;
}

/* LED thread: /dev/subucom_ctrl → vport (guest→host) */
static void *led_thread(void *arg)
{
    (void)arg;
    uint8_t frame[FRAME_SIZE];
    for (;;) {
        ssize_t r = read_exact(ctrl_rfd, frame, FRAME_SIZE);
        if (r <= 0) {
            fprintf(stderr, "[subucom_forwarder] ctrl read: %s\n",
                    r < 0 ? strerror(errno) : "EOF");
            exit(1);
        }

        if (frame[0] == MIXER_BUS && mixer_tx)
            mixer_tx_publish(frame + 1);
        pthread_mutex_lock(&vport_lock);
        ssize_t w = write_exact(vport_fd, frame, FRAME_SIZE);
        pthread_mutex_unlock(&vport_lock);
        if (w < 0)
            usleep(50000);
    }
    return NULL;
}

/* The level page, once master_out.so has made it; NULL until then. */
static struct level_page *map_levels(void)
{
    struct stat st;
    int fd = open(LEVEL_PAGE, O_RDONLY | O_CLOEXEC);
    if (fd < 0)
        return NULL;
    void *m = MAP_FAILED;
    if (fstat(fd, &st) == 0 && st.st_size >= (off_t)sizeof(struct level_page))
        m = mmap(NULL, sizeof(struct level_page), PROT_READ, MAP_SHARED, fd, 0);
    close(fd);
    return m == MAP_FAILED ? NULL : m;
}

/* Levels thread: each new measurement in the page → vport. */
static void *level_thread(void *arg)
{
    (void)arg;
    struct level_page *pg = NULL;
    uint32_t last = 0;
    for (;;) {
        if (!pg && !(pg = map_levels())) {
            sleep(1);
            continue;
        }
        uint32_t seq = atomic_load_explicit(&pg->seq, memory_order_acquire);
        if ((seq & 1) || seq == last) {
            usleep(LEVEL_POLL_US);
            continue;
        }
        uint32_t v[1 + 2 * LEVEL_CHANNELS];
        v[0] = seq;
        for (int ch = 0; ch < LEVEL_CHANNELS; ch++) {
            v[1 + ch] = atomic_load_explicit(&pg->peak[ch], memory_order_relaxed);
            v[1 + LEVEL_CHANNELS + ch] = atomic_load_explicit(&pg->rms[ch], memory_order_relaxed);
        }
        atomic_thread_fence(memory_order_acquire);
        if (atomic_load_explicit(&pg->seq, memory_order_relaxed) != seq)
            continue;  /* written while we copied it */
        last = seq;

        uint8_t frame[FRAME_SIZE] = { LEVEL_BUS };
        uint32_t size = sizeof v;
        memcpy(frame + 1, v, sizeof v);
        memcpy(frame + FRAME_SIZE - 4, &size, 4);
        pthread_mutex_lock(&vport_lock);
        write_exact(vport_fd, frame, FRAME_SIZE);
        pthread_mutex_unlock(&vport_lock);
        usleep(LEVEL_POLL_US);
    }
    return NULL;
}

int main(void)
{
    ctrl_rfd = open_ctrl_r();
    ctrl_wfd = open_ctrl_w();
    mixer_tx_open();

    char vport_path[512];
    while (find_vport(vport_path, sizeof(vport_path)) != 0) {
        fprintf(stderr, "[subucom_forwarder] waiting for virtio-serial port '%s'\n",
                PORT_NAME);
        sleep(1);
    }
    vport_fd = open_vport(vport_path);

    fprintf(stderr, "[subucom_forwarder] ready - %s ↔ %s\n", vport_path, CTRL_PATH);

    pthread_t t;
    if (pthread_create(&t, NULL, led_thread, NULL) != 0) {
        fprintf(stderr, "[subucom_forwarder] pthread_create: %s\n", strerror(errno));
        return 1;
    }
    pthread_detach(t);
    if (pthread_create(&t, NULL, level_thread, NULL) == 0)
        pthread_detach(t);

/* CTRL direction: vport → /dev/subucom_ctrl (host→guest).
 * AZ frames are bus-tagged 153-byte overlays ([bus] + 152B); the module
 * re-stamps the CRC. Unlike the CDJ path there is no power-off nibble to
 * sniff (that byte offset is meaningless in the AZ frame), so we just pass
 * each full frame straight through. */
    uint8_t frame[AZ_CTRL_SIZE];
    for (;;) {
        ssize_t r = read_exact(vport_fd, frame, AZ_CTRL_SIZE);
        if (r == 0) {
            /* host_connected=false: QEMU returns 0 immediately when no client
             * is connected to ctrl.sock.  Keep the vport open - closing and
             * reopening just re-triggers the same path and burns the guest_connected
             * slot unnecessarily.  Sleep and retry; the kernel will unblock once
             * cdj-ui connects and host_connected flips to true. */
            usleep(50000);
            continue;
        }
        if (r < 0) {
            /* Real I/O error - vport was unplugged or an interrupt caused a
             * permanent failure.  Reopen to recover. */
            fprintf(stderr, "[subucom_forwarder] vport read: %s - reopening\n",
                    strerror(errno));
            close(vport_fd);
            usleep(50000);
            vport_fd = open_vport(vport_path);
            continue;
        }

        if (write_exact(ctrl_wfd, frame, AZ_CTRL_SIZE) < 0) {
            fprintf(stderr, "[subucom_forwarder] ctrl write: %s - reopening\n",
                    strerror(errno));
            close(ctrl_wfd);
            ctrl_wfd = open_ctrl_w();
        }
    }
}
