// SPDX-License-Identifier: GPL-3.0-only
/*
 * master_out.so - the deck's audio, for the host's audio output and the panel's meters.
 *
 * EP147 writes its audio with snd_pcm_writei to one 10-channel S32_LE stream
 * at 44.1 kHz, the D810's inputs: channels 0/1 are deck 1, 2/3 deck 2, 4/5 deck
 * 3 and 6/7 deck 4 (8/9 stayed silent in every test). Each pair is a deck's
 * output before the mixer: trim, EQ, the faders and the master level are applied
 * after this stream, by the DSP, so no master mix exists in it. That stream never
 * reaches QEMU's audio backend. This library copies it into a ring on EP147's
 * audio thread, and a writer thread plays channels 0/1 (deck 1) to hw:0,2, the
 * virtio_snd module's 2-channel output leg, which QEMU sends to its -audiodev
 * (off, speakers or a WAV file).
 *
 * The host output is not the XDJ-AZ's master: that mix happens in the DSP, which
 * the emulator does not have. It is an approximation built from the real controls
 * (see "the mixer" below): the four decks, each through its channel fader and trim
 * and the crossfader as each deck is assigned, then the master level. EQ, color
 * FX and Beat FX are not modeled.
 *
 * Audio-thread rules: the snd_pcm_writei hook does no I/O, allocation, locking
 * or system calls; it copies samples and publishes an index. Everything else
 * runs on the writer thread.
 *
 * Levels: the writer thread measures every channel's peak and RMS over blocks
 * of LEVEL_BLOCK frames and publishes them in a shared page, LEVEL_PAGE, which
 * the guest's subucom forwarder sends to the host with the panel's LED frames.
 * The page is read without locks: seq is odd while a block is being written.
 *
 * Capture: while the FIFO /tmp/master_out.capture exists and has a reader, every
 * frame EP147 renders is also written to it, unpadded, as raw 32-bit PCM: the four
 * decks before the mixer (deck 1 left, deck 1 right, deck 2 left, ... deck 4
 * right), and, when CAPTURE_MIX exists as the capture starts, the mix as two more
 * channels.
 * tools/az-capture.py reads it over SSH into a WAV on the host, so nothing is kept
 * in the guest. Unlike the host's audio output, this is sample-exact however fast
 * the host runs the guest.
 *
 * libasound is already loaded in EP147, so its functions are resolved with
 * dlsym and this library needs no ALSA headers or link-time dependency.
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <time.h>
#include <sys/stat.h>
#include <stdint.h>
#include <stdio.h>
#include <math.h>
#include <string.h>
#include <sys/types.h>
#include <unistd.h>

typedef struct snd_pcm snd_pcm_t;
typedef struct snd_pcm_hw_params snd_pcm_hw_params_t;
typedef long snd_pcm_sframes_t;
typedef unsigned long snd_pcm_uframes_t;

#define SND_PCM_STREAM_PLAYBACK       0
#define SND_PCM_FORMAT_S32_LE         10
#define SND_PCM_ACCESS_RW_INTERLEAVED 3

#define MIX_CHANNELS 10        /* EP147's output stream */
#define OUT_DEVICE   "hw:0,2"  /* virtio_snd's 2-channel host output */
#define RATE         44100
#define RING_FRAMES  16384     /* power of two, about 0.37 s */
#define CHUNK        256       /* frames per write on the writer thread */
#define LEVEL_BLOCK  1024      /* frames per published level, about 23 ms */
#define LEVEL_PAGE   "/dev/shm/az-levels"
#define CAPTURE_FLAG "/tmp/master_out.capture"
#define CAPTURE_INFO "/tmp/master_out.capture.txt"
#define CAPTURE_MIX  "/tmp/master_out.capture.mix"
#define DECKS        4         /* channels 0-7: a stereo pair per deck */
#define MIXER_BUS    "/dev/subucom_spi3.0"
#define MIXER_TX     "/dev/shm/az-mixer-tx"  /* published by subucom_forwarder */
#define MIXER_POLL_NS 20000000L              /* read the controls every 20 ms */

static _Atomic(snd_pcm_t *) g_mix;           /* EP147's 10-channel stream */
static int32_t g_ring[RING_FRAMES * MIX_CHANNELS];  /* EP147's frames, as written */
static _Atomic unsigned g_w, g_r;            /* free-running frame indices */
static _Atomic unsigned g_c;                 /* capture's read index */
static _Atomic unsigned g_dropped;           /* frames lost to a full ring */
static _Atomic int g_active;                 /* set in EP147 only */

static snd_pcm_sframes_t (*real_writei)(snd_pcm_t *, const void *, snd_pcm_uframes_t);

/* Setup thread: remember which stream is the 10-channel mix. */
int snd_pcm_hw_params(snd_pcm_t *pcm, snd_pcm_hw_params_t *params)
{
    static int (*next)(snd_pcm_t *, snd_pcm_hw_params_t *);
    static int (*get_channels)(const snd_pcm_hw_params_t *, unsigned int *);
    static int (*stream_of)(snd_pcm_t *);
    if (!next) {
        next = dlsym(RTLD_NEXT, "snd_pcm_hw_params");
        get_channels = dlsym(RTLD_DEFAULT, "snd_pcm_hw_params_get_channels");
        stream_of = dlsym(RTLD_DEFAULT, "snd_pcm_stream");
    }
    int rc = next(pcm, params);
    if (rc == 0 && atomic_load(&g_active) && get_channels && stream_of) {
        unsigned int ch = 0;
        if (get_channels(params, &ch) == 0 && ch == MIX_CHANNELS &&
            stream_of(pcm) == SND_PCM_STREAM_PLAYBACK)
            atomic_store_explicit(&g_mix, pcm, memory_order_release);
    }
    return rc;
}

/* EP147's audio thread: copy the stream into the ring, nothing else. */
snd_pcm_sframes_t snd_pcm_writei(snd_pcm_t *pcm, const void *buffer, snd_pcm_uframes_t size)
{
    if (!real_writei)
        real_writei = dlsym(RTLD_NEXT, "snd_pcm_writei");
    if (buffer && pcm == atomic_load_explicit(&g_mix, memory_order_acquire)) {
        const int32_t *p = buffer;
        unsigned w = atomic_load_explicit(&g_w, memory_order_relaxed);
        unsigned r = atomic_load_explicit(&g_r, memory_order_acquire);
        unsigned c = atomic_load_explicit(&g_c, memory_order_acquire);
        if (w - c > w - r)
            r = c;  /* the reader furthest behind bounds the ring */
        if (size > RING_FRAMES - (w - r))
            atomic_fetch_add_explicit(&g_dropped, (unsigned)(size - (RING_FRAMES - (w - r))),
                                      memory_order_relaxed);
        unsigned n = size < RING_FRAMES - (w - r) ? (unsigned)size : RING_FRAMES - (w - r);
        while (n) {  /* at most two copies: up to the end of the ring, then from its start */
            unsigned i = w % RING_FRAMES, k = n < RING_FRAMES - i ? n : RING_FRAMES - i;
            memcpy(&g_ring[(size_t)i * MIX_CHANNELS], p, (size_t)k * MIX_CHANNELS * sizeof *p);
            p += (size_t)k * MIX_CHANNELS;
            w += k;
            n -= k;
        }
        atomic_store_explicit(&g_w, w, memory_order_release);
    }
    return real_writei(pcm, buffer, size);
}

static int (*pcm_open)(snd_pcm_t **, const char *, int, int);
static int (*pcm_close)(snd_pcm_t *);
static int (*hw_malloc)(snd_pcm_hw_params_t **);
static void (*hw_free)(snd_pcm_hw_params_t *);
static int (*hw_any)(snd_pcm_t *, snd_pcm_hw_params_t *);
static int (*hw_access)(snd_pcm_t *, snd_pcm_hw_params_t *, int);
static int (*hw_format)(snd_pcm_t *, snd_pcm_hw_params_t *, int);
static int (*hw_channels)(snd_pcm_t *, snd_pcm_hw_params_t *, unsigned int);
static int (*hw_rate)(snd_pcm_t *, snd_pcm_hw_params_t *, unsigned int, int);
static int (*hw_buffer)(snd_pcm_t *, snd_pcm_hw_params_t *, snd_pcm_uframes_t *);
static int (*hw_apply)(snd_pcm_t *, snd_pcm_hw_params_t *);

static int resolve_alsa(void)
{
#define SYM(var, name) (var = dlsym(RTLD_DEFAULT, name)) != NULL
    return SYM(pcm_open, "snd_pcm_open") && SYM(pcm_close, "snd_pcm_close") &&
           SYM(hw_malloc, "snd_pcm_hw_params_malloc") && SYM(hw_free, "snd_pcm_hw_params_free") &&
           SYM(hw_any, "snd_pcm_hw_params_any") &&
           SYM(hw_access, "snd_pcm_hw_params_set_access") &&
           SYM(hw_format, "snd_pcm_hw_params_set_format") &&
           SYM(hw_channels, "snd_pcm_hw_params_set_channels") &&
           SYM(hw_rate, "snd_pcm_hw_params_set_rate") &&
           SYM(hw_buffer, "snd_pcm_hw_params_set_buffer_size_near") &&
           (hw_apply = dlsym(RTLD_NEXT, "snd_pcm_hw_params")) != NULL;
#undef SYM
}

/*
 * Open and configure the output leg; NULL when it is not available yet. Only
 * the format and a buffer of about 120 ms are asked for: the device fixes its
 * own period size, and asking for a different one makes the setup fail.
 */
static snd_pcm_t *open_out(void)
{
    snd_pcm_t *h = NULL;
    snd_pcm_hw_params_t *hw = NULL;
    snd_pcm_uframes_t frames = RATE * 120 / 1000;
    if (pcm_open(&h, OUT_DEVICE, SND_PCM_STREAM_PLAYBACK, 0) != 0)
        return NULL;
    int ok = hw_malloc(&hw) == 0 &&
             hw_any(h, hw) >= 0 &&
             hw_access(h, hw, SND_PCM_ACCESS_RW_INTERLEAVED) == 0 &&
             hw_format(h, hw, SND_PCM_FORMAT_S32_LE) == 0 &&
             hw_channels(h, hw, 2) == 0 &&
             hw_rate(h, hw, RATE, 0) == 0 &&
             hw_buffer(h, hw, &frames) == 0 &&
             hw_apply(h, hw) == 0;
    if (hw)
        hw_free(hw);
    if (!ok) {
        pcm_close(h);
        return NULL;
    }
    return h;
}

/* --- the mixer (writer thread only) -------------------------------------- */

/*
 * The controls come from the mixer bus frame the deck program reads (the panel's
 * input; reading it takes nothing from the deck program) and the settings from the
 * last frame the deck program wrote to that bus. Offsets, as the deck program's
 * own control objects read them (each a 10-bit value: a high byte and two low bits
 * packed four to a byte, the first control in bits 7-6):
 *   channel faders 1-4: 0x0a-0x0d, low bits 0x0e;  crossfader: 0x0f, low bits in
 *   bits 1-0 of 0x10;  trim of channel n: 0x24 + 5(n-1), low bits 4 bytes on;
 *   master level: 0x38, low bits 0x3c
 * Written by the deck program: crossfader assign 0x06 (two bits per deck, deck 1 in
 * bits 7-6; 0 THRU, 1 A, 2 B); 0x08 bits 1-0 the channel fader curve and bits 3-2 the
 * crossfader curve, as chosen in MIXER SETTINGS.
 *
 * The curves below are approximations of the panel's settings, not measurements.
 * Every control has a dead zone of 1/64 of its travel at each end, so a fader at
 * the bottom or a crossfader at one end closes fully, as on a mixer:
 *   channel fader (gain, x = 0..1 from the bottom): the three MIXER SETTINGS
 *     choices, left to right, x^3, x and 1 - (1 - x)^3;
 *   crossfader (x = 0 full A .. 1 full B; the B side mirrors A): sharp, A at full
 *     gain until the last 1/32 of the travel; medium, full to the middle and then
 *     linear to 0; smooth, cos(x * pi / 2), constant power;
 *   trim: 0 dB at the center, +9 dB at the top (linear in dB), (2x)^2 below the
 *     center down to silence;
 *   master level: x^2, 0 dB at the top.
 */
#define FADER_HI    0x0a
#define FADER_LO    0x0e
#define XFADER_HI   0x0f
#define XFADER_LO   0x10
#define TRIM_HI     0x24
#define TRIM_STRIDE 5
#define MASTER_HI   0x38
#define MASTER_LO   0x3c
#define TX_ASSIGN   0x06
#define TX_CURVES   0x08

struct mixer_tx_page {
    _Atomic uint32_t seq;
    _Atomic uint32_t frame[152 / 4];
};

static int g_mixer_fd = -1;
static struct mixer_tx_page *g_mixer_tx;
static float g_gain[DECKS];      /* each deck's gain into the mix, master level included */

/* A 10-bit control, 0..1 past the dead zones: the high byte, and its two bits of
 * the low-bits byte. */
static float control(const uint8_t *f, int hi, int lo, int shift)
{
    unsigned v = (unsigned)f[hi] << 2 | ((f[lo] >> shift) & 3);
    float x = (v / 1023.0f - 1.0f / 64) * 64.0f / 62.0f;
    return x < 0.0f ? 0.0f : x > 1.0f ? 1.0f : x;
}

static float fader_gain(float x, int curve)
{
    float y = 1.0f - x;
    switch (curve) {
    case 0: return x * x * x;
    case 2: return 1.0f - y * y * y;
    default: return x;
    }
}

/* One side of the crossfader: the gain of a deck assigned to A at position x. */
static float xfader_side(float x, int curve)
{
    float g;
    switch (curve) {
    case 2: g = (1.0f - x) * 32.0f; break;                 /* sharp */
    case 1: g = (1.0f - x) * 2.0f; break;                  /* medium */
    default: return cosf(x * 1.5707963f);                  /* smooth */
    }
    return g < 0.0f ? 0.0f : g > 1.0f ? 1.0f : g;
}

static float trim_gain(float x)
{
    if (x >= 0.5f)
        return powf(10.0f, 9.0f * (x - 0.5f) * 2.0f / 20.0f);
    return (2.0f * x) * (2.0f * x);
}

/* The settings byte at `at` of the deck program's last mixer write, or -1. */
static int mixer_tx_byte(int at)
{
    if (!g_mixer_tx) {
        int fd = open(MIXER_TX, O_RDONLY | O_CLOEXEC);
        struct stat st;
        if (fd < 0)
            return -1;
        if (fstat(fd, &st) == 0 && st.st_size >= (off_t)sizeof *g_mixer_tx) {
            void *m = mmap(NULL, sizeof *g_mixer_tx, PROT_READ, MAP_SHARED, fd, 0);
            if (m != MAP_FAILED)
                g_mixer_tx = m;
        }
        close(fd);
        if (!g_mixer_tx)
            return -1;
    }
    for (int tries = 0; tries < 4; tries++) {
        uint32_t seq = atomic_load_explicit(&g_mixer_tx->seq, memory_order_acquire);
        if (seq == 0 || (seq & 1))
            continue;
        uint32_t w = atomic_load_explicit(&g_mixer_tx->frame[at / 4], memory_order_relaxed);
        atomic_thread_fence(memory_order_acquire);
        if (atomic_load_explicit(&g_mixer_tx->seq, memory_order_relaxed) == seq)
            return (w >> (8 * (at % 4))) & 0xff;
    }
    return -1;
}

/* Read the controls now and then, and work out each deck's gain. Until the mixer
 * bus has been read the mix is silent. Without the deck program's settings (an old
 * forwarder) every deck counts as THRU with the middle curves, the factory
 * settings. */
static void mixer_poll(void)
{
    static struct timespec last;
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    if ((now.tv_sec - last.tv_sec) * 1000000000L + (now.tv_nsec - last.tv_nsec) < MIXER_POLL_NS)
        return;
    last = now;
    if (g_mixer_fd < 0 && (g_mixer_fd = open(MIXER_BUS, O_RDONLY | O_CLOEXEC)) < 0)
        return;
    uint8_t f[152];
    if (read(g_mixer_fd, f, sizeof f) < 0x40)
        return;
    int assign = mixer_tx_byte(TX_ASSIGN), curves = mixer_tx_byte(TX_CURVES);
    if (assign < 0)
        assign = 0;
    int fcurve = curves < 0 ? 1 : curves & 3, xcurve = curves < 0 ? 1 : (curves >> 2) & 3;
    float x = control(f, XFADER_HI, XFADER_LO, 0);
    float master = control(f, MASTER_HI, MASTER_LO, 6);
    master *= master;
    for (int d = 0; d < DECKS; d++) {
        float g = fader_gain(control(f, FADER_HI + d, FADER_LO, 6 - 2 * d), fcurve) *
                  trim_gain(control(f, TRIM_HI + TRIM_STRIDE * d, TRIM_HI + TRIM_STRIDE * d + 4, 6));
        switch ((assign >> (6 - 2 * d)) & 3) {
        case 1: g *= xfader_side(x, xcurve); break;
        case 2: g *= xfader_side(1.0f - x, xcurve); break;
        }
        g_gain[d] = g * master;
    }
}

/* Mix n frames from ring frame r into out (stereo), each deck's gain moving from
 * ramp[] to the current gains over the n frames so control changes do not click. */
static void mix_frames(float ramp[DECKS], unsigned r, unsigned n, int32_t *out, size_t stride)
{
    float step[DECKS];
    for (int d = 0; d < DECKS; d++)
        step[d] = n ? (g_gain[d] - ramp[d]) / n : 0.0f;
    for (unsigned i = 0; i < n; i++) {
        const int32_t *f = &g_ring[(size_t)((r + i) % RING_FRAMES) * MIX_CHANNELS];
        float l = 0.0f, rr = 0.0f;
        for (int d = 0; d < DECKS; d++) {
            float g = ramp[d] + step[d] * (i + 1);
            l += g * (float)f[2 * d];
            rr += g * (float)f[2 * d + 1];
        }
        out[i * stride] = l >= 2147483520.0f ? INT32_MAX : l <= -2147483648.0f ? INT32_MIN : (int32_t)l;
        out[i * stride + 1] = rr >= 2147483520.0f ? INT32_MAX : rr <= -2147483648.0f ? INT32_MIN : (int32_t)rr;
    }
    for (int d = 0; d < DECKS; d++)
        ramp[d] = g_gain[d];
}

/* --- capture (writer thread only) --------------------------------------- */
static int g_cap = -1;                       /* the FIFO, write end */
static unsigned long long g_cap_frames, g_cap_lost;
static unsigned g_cap_channels;              /* 8, or 10 with the mix */
static float g_cap_ramp[DECKS];
static unsigned g_cap_dropped0;

static void capture_info(const char *state)
{
    FILE *f = fopen(CAPTURE_INFO, "w");
    if (!f)
        return;
    fprintf(f, "state=%s\nchannels=%u\nframes=%llu\nseconds=%.3f\ndropped=%llu\n", state,
            g_cap_channels, g_cap_frames,
            g_cap_frames / (double)RATE,
            g_cap_lost + (atomic_load_explicit(&g_dropped, memory_order_relaxed) - g_cap_dropped0));
    fclose(f);
}

/* Follow the FIFO: open it once a reader is there, close it when it is removed. */
static void capture_poll(void)
{
    struct stat st;
    int want = stat(CAPTURE_FLAG, &st) == 0 && S_ISFIFO(st.st_mode);
    if (want && g_cap < 0) {
        int fd = open(CAPTURE_FLAG, O_WRONLY | O_NONBLOCK);
        if (fd < 0)
            return;  /* no reader yet */
        fcntl(fd, F_SETPIPE_SZ, 1 << 20);
        g_cap = fd;
        g_cap_channels = 2 * DECKS + (access(CAPTURE_MIX, F_OK) == 0 ? 2 : 0);
        memcpy(g_cap_ramp, g_gain, sizeof g_cap_ramp);
        g_cap_frames = g_cap_lost = 0;
        g_cap_dropped0 = atomic_load_explicit(&g_dropped, memory_order_relaxed);
        atomic_store_explicit(&g_c, atomic_load_explicit(&g_w, memory_order_acquire),
                              memory_order_release);
        capture_info("recording");
    } else if (!want && g_cap >= 0) {
        close(g_cap);
        g_cap = -1;
        capture_info("done");
    } else if (g_cap >= 0) {
        capture_info("recording");
    }
}

/* Write every frame since the last call; with no capture, just keep up. A frame
 * the pipe cannot take is counted as lost, never waited for. */
static void capture_drain(void)
{
    unsigned w = atomic_load_explicit(&g_w, memory_order_acquire);
    unsigned c = atomic_load_explicit(&g_c, memory_order_relaxed);
    while (g_cap >= 0 && c != w) {
        unsigned i = c % RING_FRAMES;
        unsigned n = w - c;
        if (n > RING_FRAMES - i)
            n = RING_FRAMES - i;  /* up to the end of the ring, then wrap */
        size_t frame = (size_t)g_cap_channels * 4;
        if (n > 4096 / frame)
            n = 4096 / frame;     /* at most 4096 bytes: a pipe write that is all or nothing */
        static int32_t st[4096 / 4];
        for (unsigned f = 0; f < n; f++)
            memcpy(&st[f * g_cap_channels], &g_ring[(size_t)(i + f) * MIX_CHANNELS], 2 * DECKS * 4);
        if (g_cap_channels > 2 * DECKS)
            mix_frames(g_cap_ramp, c, n, &st[2 * DECKS], g_cap_channels);
        ssize_t k = write(g_cap, st, n * frame);
        if (k < 0 && errno == EPIPE) {  /* the reader went away (SIGPIPE is blocked here) */
            close(g_cap);
            g_cap = -1;
            capture_info("done");
            break;
        }
        if (k < 0 && errno != EAGAIN) {
            close(g_cap);
            g_cap = -1;
            capture_info("done");
            break;
        }
        if (k < (ssize_t)frame) {
            g_cap_lost += n;
            c += n;
            continue;
        }
        g_cap_frames += (unsigned long long)k / frame;
        g_cap_lost += n - (unsigned)(k / frame);
        c += n;
    }
    atomic_store_explicit(&g_c, w, memory_order_release);
}

/* --- levels (writer thread only) ----------------------------------------- */

/* The shared page. Peaks and RMS are float bit patterns, as a fraction of full
 * scale (2^31). A reader copies the page and keeps the copy only when seq was
 * even and unchanged on both sides of it. */
struct level_page {
    _Atomic uint32_t seq;
    _Atomic uint32_t peak[MIX_CHANNELS];
    _Atomic uint32_t rms[MIX_CHANNELS];
};

static struct level_page *g_lv;
static unsigned g_m;                         /* the level reader's index */
static uint32_t g_lv_peak[MIX_CHANNELS];
static double g_lv_sum[MIX_CHANNELS];
static unsigned g_lv_n;

static void level_open(void)
{
    int fd = open(LEVEL_PAGE, O_RDWR | O_CREAT | O_CLOEXEC, 0644);
    if (fd < 0)
        return;
    if (ftruncate(fd, sizeof *g_lv) == 0) {
        void *m = mmap(NULL, sizeof *g_lv, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        if (m != MAP_FAILED)
            g_lv = m;
    }
    close(fd);
}

static uint32_t float_bits(float f)
{
    uint32_t u;
    memcpy(&u, &f, sizeof u);
    return u;
}

static void level_publish(void)
{
    uint32_t seq = atomic_load_explicit(&g_lv->seq, memory_order_relaxed);
    atomic_store_explicit(&g_lv->seq, seq + 1, memory_order_relaxed);
    atomic_thread_fence(memory_order_release);
    for (int ch = 0; ch < MIX_CHANNELS; ch++) {
        float rms = sqrtf((float)(g_lv_sum[ch] / g_lv_n)) / 2147483648.0f;
        atomic_store_explicit(&g_lv->peak[ch], float_bits(g_lv_peak[ch] / 2147483648.0f),
                              memory_order_relaxed);
        atomic_store_explicit(&g_lv->rms[ch], float_bits(rms), memory_order_relaxed);
        g_lv_peak[ch] = 0;
        g_lv_sum[ch] = 0;
    }
    atomic_store_explicit(&g_lv->seq, seq + 2, memory_order_release);
    g_lv_n = 0;
}

/* Measure every frame EP147 wrote since the last call. The ring is not held for
 * this reader: one that has fallen half a ring behind skips ahead instead. */
static void level_feed(void)
{
    if (!g_lv)
        return;
    unsigned w = atomic_load_explicit(&g_w, memory_order_acquire);
    if (w - g_m > RING_FRAMES / 2)
        g_m = w - RING_FRAMES / 2;
    for (; g_m != w; g_m++) {
        const int32_t *f = &g_ring[(size_t)(g_m % RING_FRAMES) * MIX_CHANNELS];
        for (int ch = 0; ch < MIX_CHANNELS; ch++) {
            int64_t v = f[ch];
            uint32_t a = (uint32_t)(v < 0 ? -v : v);
            if (a > g_lv_peak[ch])
                g_lv_peak[ch] = a;
            g_lv_sum[ch] += (double)(v * v);
        }
        if (++g_lv_n == LEVEL_BLOCK)
            level_publish();
    }
}

/*
 * Writer thread. It always writes a full chunk, padding with silence when the
 * ring is short, so the output stream never underruns while the deck is idle.
 *
 * A failed write (an underrun, typically when the host runs the guest slower
 * than real time) closes the stream and opens it again: a stream recovered in
 * place on this device can stay stopped for good, while a fresh one starts
 * cleanly. What was queued in the ring is dropped so the sound does not lag
 * behind the deck afterwards. When nothing on the host consumes the stream every
 * write fails at once, so after a few failures in a row it pauses between tries.
 */
static void *writer(void *arg)
{
    (void)arg;
    /* A capture reader can vanish mid-write; EPIPE must not become a SIGPIPE that
     * ends EP147. The signal is thread-directed, so blocking it here is enough. */
    sigset_t pipe_sig;
    sigemptyset(&pipe_sig);
    sigaddset(&pipe_sig, SIGPIPE);
    pthread_sigmask(SIG_BLOCK, &pipe_sig, NULL);
    if (!real_writei)
        real_writei = dlsym(RTLD_NEXT, "snd_pcm_writei");
    if (!resolve_alsa() || !real_writei) {
        fprintf(stderr, "master_out: libasound not found; no host audio\n");
        return NULL;
    }
    static int32_t buf[CHUNK * 2];
    static float ramp[DECKS];
    snd_pcm_t *h = NULL;
    int failures = 0;
    unsigned loops = 0;
    atomic_store(&g_c, atomic_load(&g_w));
    g_m = atomic_load(&g_w);
    level_open();
    for (;;) {
        if (loops++ % 64 == 0)  /* every 0.4 s playing to the host, 1.3 s without */
            capture_poll();
        mixer_poll();
        capture_drain();
        level_feed();
        if (!h) {
            /* The sound card may not be up yet, or the host has no audio output.
             * Keep the ring moving for capture and levels, and try again about
             * every 100 ms. */
            h = loops % 5 == 0 ? open_out() : NULL;
            if (!h) {
                atomic_store_explicit(&g_r, atomic_load_explicit(&g_w, memory_order_acquire),
                                      memory_order_release);
                usleep(20000);
                continue;
            }
            atomic_store_explicit(&g_r, atomic_load_explicit(&g_w, memory_order_acquire),
                                  memory_order_release);
        }
        unsigned w = atomic_load_explicit(&g_w, memory_order_acquire);
        unsigned r = atomic_load_explicit(&g_r, memory_order_relaxed);
        unsigned take = w - r < CHUNK ? w - r : CHUNK;
        mix_frames(ramp, r, take, buf, 2);
        memset(buf + take * 2, 0, (CHUNK - take) * 2 * sizeof buf[0]);
        if (take)
            atomic_store_explicit(&g_r, r + take, memory_order_release);
        if (real_writei(h, buf, CHUNK) < 0) {
            pcm_close(h);
            h = NULL;
            if (++failures >= 3)
                usleep(250000);
            continue;
        }
        failures = 0;
    }
    return NULL;
}

/* LD_PRELOAD reaches every process in EP147's service; only EP147 is tapped. */
__attribute__((constructor))
static void master_out_init(void)
{
    char comm[32] = {0};
    FILE *f = fopen("/proc/self/comm", "r");
    if (f) {
        if (fgets(comm, sizeof comm, f))
            comm[strcspn(comm, "\n")] = 0;
        fclose(f);
    }
    if (strcmp(comm, "EP147") != 0)
        return;
    atomic_store(&g_active, 1);
    pthread_t t;
    if (pthread_create(&t, NULL, writer, NULL) == 0)
        pthread_detach(t);
}
