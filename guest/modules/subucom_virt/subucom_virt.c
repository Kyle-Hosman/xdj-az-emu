// SPDX-License-Identifier: GPL-2.0-only
// Derived from cdj3k-emu (GPL-2.0); see ATTRIBUTION.md.
/*
 * subucom_virt.c - Virtual subucom_spi character device for QEMU cdj3k-emu
 *
 * Creates /dev/subucom_spi1.0 (major 246, minor 0) under class "subucom_spiclass",
 *
 * Also creates /dev/subucom_ctrl for host communication:
 *   write(ctrl, [bus]+152B) → overlay control state on that bus (0=spi1 erp,
 *                             1=spi2 deck, 2=spi3 mixer, 3=spi5 deck); CRC is
 *                             recomputed in spi_read so the host sends bytes only
 *   write(ctrl, [bus])      → clear that bus;  write(ctrl, 0B) → clear all
 *   read(ctrl, 64 bytes)    → consume latest LED frame written by EP147
 *
 * Ioctl interface (/sys/module/subucom_spi/parameters/):
 *   0x80027003  TIMER_STATUS_READ   → returns u16 DEFtmstat (default 0)
 *   0x40027003  TIMER_STATUS_WRITE  → sets timer_status (starts transfer timer)
 *   0x80017001  BITS_PER_WORD_READ  → returns u8 bits_per_word (default 8)
 *   0x40017001  BITS_PER_WORD_WRITE → sets bits_per_word
 *   0x40107000  MOSI_TRANSFER       → EP122 sends 16-byte MOSI frame
 *   0x80047004  RX_BYTES_READ       → returns u32 rx frame size (64)
 *   0x40047004  RX_BYTES_WRITE      → sets rx frame size
 *   0x80047002  INTERVAL_READ       → returns u32 polling interval in µs (1176)
 *   0x40047002  INTERVAL_WRITE      → sets polling interval
 *
 * read() on spi1.0: blocks until MISO frame available (~850 Hz via hrtimer),
 * returns 64 bytes. If host injected a frame via ctrl, that frame is used;
 * otherwise the idle frame is returned.
 *
 * Specs:
 *   SPI: ff1d0000.spi, bus1 CS0, 3.2 MHz, Mode 3 (CPOL=1, CPHA=1)
 *   Device class: subucom_spiclass
 *   Major: 246, Minor: 0
 *   Module params: DEFtmstat=0, txmulti=1, txpart=1, SZbufsize=1036
 */

#include <linux/module.h>
#include <linux/kernel.h>
#include <linux/fs.h>
#include <linux/cdev.h>
#include <linux/device.h>
#include <linux/uaccess.h>
#include <linux/slab.h>
#include <linux/wait.h>
#include <linux/spinlock.h>
#include <linux/delay.h>   /* usleep_range */
#include <linux/sched.h>   /* schedule_timeout_interruptible */
#include <linux/poll.h>    /* poll_table, POLLIN, POLLRDNORM */
#include <linux/version.h>
#include "subucom_frames.h"  /* real captured per-bus idle frames */

#define DRV_NAME        "subucom_virt"
#define CTRL_DEV_NAME   "subucom_ctrl"
#define CLASS_NAME      "subucom_virt_class"

/* AZ (XDJ-AZ / EP147): the front panel exposes FOUR SPI sub-CPU buses, not one.
 * EP147 opens /dev/subucom_spi{1,2,3,5}.0. Each carries a 128-byte MISO frame
 * (CDJ-3000/EP122 used 64). Different buses CRC a different prefix length:
 *   spi1 "ErP" : CRC-16/X25 over the first 28 bytes, stored LE at [28],[29]
 *   spi3 "MC"  : CRC-16/X25 over the first 96 bytes, stored LE at [96],[97]
 *   spi2/spi5  : ErP-type panels (4-deck L/R) - CRC over 28 (best-known).
 * Derived by reversing /home/root/pdj/subucom_read (main @0x4006b0): 128-byte
 * reads, crc(buf,0x1c) for ErP and crc(buf,0x60) for MC, same poly 0x8408. */
#define N_SPI           4
static const char * const spi_dev_name[N_SPI] = {
    "subucom_spi1.0", "subucom_spi2.0", "subucom_spi3.0", "subucom_spi5.0",
};
/* CRC'd prefix length per bus (== byte offset where the 2-byte CRC is stored). */
/* Per-bus CRC prefix length {spi1 erp, spi2 deck, spi3 mixer, spi5 deck}.
 * Verified against captured real frames: X25 over these lengths reproduces the
 * baked CRC of every idle frame AND every jog frame (635 real frames, 100%). */
static int spi_crc_len[N_SPI] = { 28, 38, 96, 38 };

/* Dynamic major - alloc_chrdev_region picks a free slot at runtime */
static int subucom_major;
#define MINOR_CTRL      N_SPI          /* ctrl follows the N_SPI spi minors */
#define NDEVS           (N_SPI + 1)

/* MISO frame size and polling interval */
#define MISO_SIZE       152  /* real AZ subucom frame size (EP147 reads 152) */
#define MOSI_CMD_SIZE   16  /* ioctl arg: {uint32 magic, uint32 size, uint64 data_ptr} */
#define LED_FRAME_SIZE 152 /* the longest MOSI write: a deck bus's 152 bytes
                             * (the erp and mixer buses write 128; deck 2's
                             * LED colors sit in erp 0x40-0x7F) */
#define INTERVAL_US     1176   /* ~850 Hz */

/* Layout of the 16-byte MOSI ioctl argument */
struct mosi_cmd {
    u32 magic;    /* 0x01000000 */
    u32 size;     /* byte count of data at data_ptr (≤ LED_FRAME_SIZE) */
    u64 data_ptr; /* userspace VA of the LED frame */
} __packed;

/* Ioctl codes */
#define IOCTL_TIMER_STATUS_READ   0x80027003u
#define IOCTL_TIMER_STATUS_WRITE  0x40027003u
#define IOCTL_BITS_PER_WORD_READ  0x80017001u
#define IOCTL_BITS_PER_WORD_WRITE 0x40017001u
#define IOCTL_MOSI_TRANSFER       0x40107000u
#define IOCTL_RX_BYTES_READ       0x80047004u
#define IOCTL_RX_BYTES_WRITE      0x40047004u
#define IOCTL_INTERVAL_READ       0x80047002u
#define IOCTL_INTERVAL_WRITE      0x40047002u

/* ------------------------------------------------------------------ */
/* Global state                                                         */
/* ------------------------------------------------------------------ */

static struct class  *subucom_class;
static struct cdev    subucom_cdev[NDEVS];
static struct device *subucom_dev[NDEVS];

/* CRC-16/X-25: poly 0x8408 (reflected 0x1021), init 0xFFFF, final XOR 0xFFFF.
 * Applied over the first 62 bytes of the MISO frame; result stored LE in b62-b63. */
static u16 crc16_x25(const u8 *data, int n)
{
    u16 crc = 0xFFFF;
    int i, b;
    for (i = 0; i < n; i++) {
        crc ^= data[i];
        for (b = 0; b < 8; b++)
            crc = (crc & 1) ? (crc >> 1) ^ 0x8408u : crc >> 1;
    }
    return crc ^ 0xFFFF;
}

/* MISO state: spi_read() sleeps interval_us then delivers the frame.
 * No external hrtimer needed - usleep_range() throttles in the caller's
 * context, avoiding the hrtimer→wake_up→wait_event race on QEMU/HVF. */
static DEFINE_SPINLOCK(miso_lock);
/* Per-bus control overlay. When has_inject[bus] is set, spi_read serves
 * inject_pending[bus] instead of the idle template for that bus (and re-stamps
 * the per-bus CRC), so injecting deck jog state (spi5) never corrupts the erp/
 * mixer/other-deck buses. Host supplies control bytes only; CRC is computed here.
 * Deck jog map (spi2/spi5): touch=[9], pos=u16[10:12] LE, vel=u16[12:14] LE. */
static u8 inject_pending[N_SPI][MISO_SIZE];
static int has_inject[N_SPI];

/* MOSI: EP122 writes LED frames via MOSI_TRANSFER ioctl, on every bus, each
 * about 16 times a second. Each bus keeps its own latest payload (dereferenced
 * from the ioctl cmd), so a write on one bus never hides another bus's: with a
 * single slot shared by all four, most mixer frames were overwritten before the
 * host read them. Index = spi_dev_name[] bus (0=spi1 erp, 1=spi2 deck, 2=spi3
 * mixer, 3=spi5 deck); the host gets the index with each frame. */
static DEFINE_SPINLOCK(mosi_lock);
static u8 mosi_frame[N_SPI][LED_FRAME_SIZE];
/* The declared cmd.size of the write that produced each frame. A frame is
 * zeroed and then filled up to min(cmd.size, LED_FRAME_SIZE) (see
 * IOCTL_MOSI_TRANSFER below), so a short write looks like a frame whose tail is
 * zero. Forwarding the size lets the host tell a whole frame (128) from a
 * partial update. */
static u32 mosi_size[N_SPI];
static unsigned int mosi_fresh;  /* bit n = bus n has an unread frame */
static unsigned int mosi_next;   /* the bus ctrl_read looks at first */
static wait_queue_head_t mosi_wq;

/* Ioctl-configurable parameters */
static u16 timer_status;
static u8  bits_per_word = 8;
static u32 rx_bytes      = MISO_SIZE;
static u32 interval_us   = INTERVAL_US;

/* ------------------------------------------------------------------ */
/* Testmode boot injection                                            */
/*                                                                    */
/* inject_testmode=1 holds BTN_TEMPO_RANGE and BTN_CALL_PREV (byte 6 */
/* 0x04, byte 8 0x08 on the ErP bus) from load, so subucom_read sees  */
/* the service-mode combo during boot. The combo is ORed into every   */
/* ErP frame that spi_read delivers, idle or host-injected, so host   */
/* writes do not cancel it. The app's first MOSI_TRANSFER releases    */
/* it: pre-setting.sh runs subucom_read, which sends no MOSI, before  */
/* the app starts.                                                    */
/* ------------------------------------------------------------------ */

static int inject_testmode;
module_param(inject_testmode, int, 0444);
MODULE_PARM_DESC(inject_testmode, "Hold the testmode buttons from loading until the first LED frame (0 off, 1 on)");

/* RE-sweep helper: override per-bus CRC prefix length via cmdline
 * subucom_virt.spi_crc_len=28,LEN,96,LEN . Buses: [spi1,spi2,spi3,spi5]. */
static int n_crc = N_SPI;
module_param_array(spi_crc_len, int, &n_crc, 0644);
MODULE_PARM_DESC(spi_crc_len, "per-bus CRC prefix length {spi1,spi2,spi3,spi5}");

static int testmode_held;   /* 1 while the combo is ORed into every ErP frame */

/* Stop ORing the combo into the ErP frames. */
static void testmode_release(void)
{
    unsigned long flags;
    int was_held;

    spin_lock_irqsave(&miso_lock, flags);
    was_held = testmode_held;
    testmode_held = 0;
    spin_unlock_irqrestore(&miso_lock, flags);
    if (was_held)
        pr_info("%s: released the testmode combo on the first LED frame\n", DRV_NAME);
}

/* uaccess helpers (vanilla 6.6 kernel only). */
static inline int xput_bytes(void __user *dst, const u8 *src, size_t n)
{
    return copy_to_user(dst, src, n) ? -EFAULT : 0;
}

static inline int xget_bytes(u8 *dst, const void __user *src, size_t n)
{
    return copy_from_user(dst, src, n) ? -EFAULT : 0;
}

/* ------------------------------------------------------------------ */
/* spi1.0 file operations                                             */
/* ------------------------------------------------------------------ */

static int spi_open(struct inode *inode, struct file *filp)
{
    /* Remember which SPI bus (0..N_SPI-1) this fd is, for per-bus CRC length. */
    filp->private_data = (void *)(long)MINOR(inode->i_rdev);
    return 0;
}

static int spi_release(struct inode *inode, struct file *filp)
{
    return 0;
}

static ssize_t spi_read(struct file *filp, char __user *buf,
                         size_t count, loff_t *ppos)
{
    unsigned long flags;
    u8 frame[MISO_SIZE];
    size_t n;

    /* Throttle at ~850 Hz in the caller's context.
     * schedule_timeout_interruptible uses TASK_INTERRUPTIBLE - no get_current()
     * assumptions, and correctly returns early if a signal is pending. */
    if (schedule_timeout_interruptible(usecs_to_jiffies(interval_us) + 1))
        return -ERESTARTSYS;

    /* Serve this bus's frame: the host-injected overlay if present (control
     * state layered on the idle template), else the real captured idle frame
     * (spi2/spi5 = deck_ucom, spi1/spi3 = erp/mixer; see subucom_frames.h).
     * Either way the per-bus CRC is re-stamped below so EP147 accepts it. */
    {
        int bus = (int)(long)filp->private_data;
        int len;
        u16 crc;

        if (bus < 0 || bus >= N_SPI)
            bus = 0;

        spin_lock_irqsave(&miso_lock, flags);
        if (has_inject[bus])
            memcpy(frame, inject_pending[bus], MISO_SIZE);
        else
            memcpy(frame, az_bus_frame[bus], MISO_SIZE);
        if (bus == 0 && testmode_held) {
            frame[6] |= 0x04;   /* BTN_TEMPO_RANGE */
            frame[8] |= 0x08;   /* BTN_CALL_PREV */
        }
        spin_unlock_irqrestore(&miso_lock, flags);

        /* Re-stamp the per-bus CRC so overlaid control bytes stay valid.
         * No-op on an untouched idle frame (verified against captures). */
        len = spi_crc_len[bus];
        if (len >= 0 && len + 1 < MISO_SIZE) {
            crc = crc16_x25(frame, len);
            frame[len]     = crc & 0xff;
            frame[len + 1] = crc >> 8;
        }
    }

    n = min(count, (size_t)MISO_SIZE);
    if (xput_bytes(buf, frame, n))
        return -EFAULT;

    return (ssize_t)n;
}

static long spi_ioctl(struct file *filp, unsigned int cmd, unsigned long arg)
{
    switch (cmd) {

    case IOCTL_TIMER_STATUS_READ:
        return xput_bytes((void __user *)arg, (u8 *)&timer_status, sizeof(timer_status));

    case IOCTL_TIMER_STATUS_WRITE:
        return xget_bytes((u8 *)&timer_status, (const void __user *)arg, sizeof(timer_status));

    case IOCTL_BITS_PER_WORD_READ:
        return xput_bytes((void __user *)arg, &bits_per_word, sizeof(bits_per_word));

    case IOCTL_BITS_PER_WORD_WRITE:
        return xget_bytes(&bits_per_word, (const void __user *)arg, sizeof(bits_per_word));

    case IOCTL_MOSI_TRANSFER: {
        /* EP122 sends a 16-byte cmd: {magic=0x01000000, size, data_ptr}.
         * data_ptr is a userspace VA pointing to the actual LED frame.
         * We dereference it here to capture the real 64-byte payload. */
        struct mosi_cmd cmd;
        u8 led[LED_FRAME_SIZE];
        u32 copy_size;
        unsigned long flags;
        unsigned int bus;

        if (xget_bytes((u8 *)&cmd, (const void __user *)arg, MOSI_CMD_SIZE))
            return -EFAULT;

        copy_size = min_t(u32, cmd.size, LED_FRAME_SIZE);
        if (copy_size == 0)
            return 0;

        memset(led, 0, LED_FRAME_SIZE);
        if (xget_bytes(led, (const void __user *)(uintptr_t)cmd.data_ptr, copy_size))
            return -EFAULT;

        bus = (unsigned int)(long)filp->private_data;
        if (bus >= N_SPI)
            return -EINVAL;
        spin_lock_irqsave(&mosi_lock, flags);
        memcpy(mosi_frame[bus], led, LED_FRAME_SIZE);
        mosi_size[bus] = cmd.size;
        mosi_fresh |= 1u << bus;
        spin_unlock_irqrestore(&mosi_lock, flags);
        wake_up_interruptible(&mosi_wq);
        testmode_release();
        return 0;
    }

    case IOCTL_RX_BYTES_READ:
        return xput_bytes((void __user *)arg, (u8 *)&rx_bytes, sizeof(rx_bytes));

    case IOCTL_RX_BYTES_WRITE:
        return xget_bytes((u8 *)&rx_bytes, (const void __user *)arg, sizeof(rx_bytes));

    case IOCTL_INTERVAL_READ:
        return xput_bytes((void __user *)arg, (u8 *)&interval_us, sizeof(interval_us));

    case IOCTL_INTERVAL_WRITE:
        return xget_bytes((u8 *)&interval_us, (const void __user *)arg, sizeof(interval_us));

    default:
        return -ENOTTY;
    }
}

/* spi_read() always delivers a frame after a brief sleep (~1.2 ms).
 * From epoll's perspective this fd is always readable - EP122 calls
 * read() itself to throttle at 850 Hz.  Returning POLLIN unconditionally
 * lets epoll_ctl() succeed so EP122's event loop can watch this fd. */
static unsigned int spi_poll(struct file *filp, poll_table *wait)
{
    return POLLIN | POLLRDNORM;
}

static const struct file_operations spi_fops = {
    .owner          = THIS_MODULE,
    .open           = spi_open,
    .release        = spi_release,
    .read           = spi_read,
    .poll           = spi_poll,
    .unlocked_ioctl = spi_ioctl,
};

/* ------------------------------------------------------------------ */
/* ctrl file operations (/dev/subucom_ctrl)                           */
/* ------------------------------------------------------------------ */

static int ctrl_open(struct inode *inode, struct file *filp)
{
    return 0;
}

static int ctrl_release(struct inode *inode, struct file *filp)
{
    return 0;
}

/* Bus-tagged control protocol (byte 0 = bus id 0..N_SPI-1):
 *   write(ctrl, [bus] + MISO_SIZE bytes)  → overlay that bus (sticky). Host
 *                                            supplies control bytes only; the
 *                                            CRC is (re)computed in spi_read.
 *   write(ctrl, [bus])            (1 byte) → clear that bus, revert to idle.
 *   write(ctrl, 0 bytes)                   → clear ALL buses. */
static ssize_t ctrl_write(struct file *filp, const char __user *buf,
                           size_t count, loff_t *ppos)
{
    unsigned long flags;
    u8 bus;

    if (count == 0) {
        int i;
        spin_lock_irqsave(&miso_lock, flags);
        for (i = 0; i < N_SPI; i++)
            has_inject[i] = 0;
        spin_unlock_irqrestore(&miso_lock, flags);
        return 0;
    }

    if (xget_bytes(&bus, buf, 1))
        return -EFAULT;
    if (bus >= N_SPI)
        return -EINVAL;

    if (count == 1) {
        spin_lock_irqsave(&miso_lock, flags);
        has_inject[bus] = 0;
        spin_unlock_irqrestore(&miso_lock, flags);
        return 1;
    }

    if (count < 1 + MISO_SIZE)
        return -EINVAL;

    {
        u8 tmp[MISO_SIZE];
        if (xget_bytes(tmp, buf + 1, MISO_SIZE))
            return -EFAULT;
        spin_lock_irqsave(&miso_lock, flags);
        memcpy(inject_pending[bus], tmp, MISO_SIZE);
        has_inject[bus] = 1;
        spin_unlock_irqrestore(&miso_lock, flags);
    }

    return 1 + MISO_SIZE;
}

/* read(ctrl) → the latest LED frame of the next bus that has an unread one, in
 * turn, so a busy bus cannot starve the others; blocks if none */
/* [0]=source bus id, [1..1+LED_FRAME_SIZE)=payload, [1+LED_FRAME_SIZE..+4)=
 * the real cmd.size (LE u32) that produced this frame (see mosi_size above).
 * The size field is appended after the frame. */
#define LED_WIRE_SIZE (1 + LED_FRAME_SIZE + 4)

static ssize_t ctrl_read(struct file *filp, char __user *buf,
                          size_t count, loff_t *ppos)
{
    unsigned long flags;
    u8 tmp[LED_WIRE_SIZE];
    int ret;
    u32 size_le;
    unsigned int i, bus;

    if (count < LED_WIRE_SIZE)
        return -EINVAL;

again:
    ret = wait_event_interruptible(mosi_wq, mosi_fresh);
    if (ret)
        return ret;

    spin_lock_irqsave(&mosi_lock, flags);
    for (i = 0; i < N_SPI; i++) {
        bus = (mosi_next + i) % N_SPI;
        if (mosi_fresh & (1u << bus))
            break;
    }
    if (i == N_SPI) {  /* another reader took it between the wake-up and here */
        spin_unlock_irqrestore(&mosi_lock, flags);
        goto again;
    }
    tmp[0] = (u8)bus;
    memcpy(tmp + 1, mosi_frame[bus], LED_FRAME_SIZE);
    size_le = cpu_to_le32(mosi_size[bus]);
    memcpy(tmp + 1 + LED_FRAME_SIZE, &size_le, 4);
    mosi_fresh &= ~(1u << bus);
    mosi_next = (bus + 1) % N_SPI;
    spin_unlock_irqrestore(&mosi_lock, flags);

    if (xput_bytes(buf, tmp, LED_WIRE_SIZE))
        return -EFAULT;

    return LED_WIRE_SIZE;
}

static const struct file_operations ctrl_fops = {
    .owner   = THIS_MODULE,
    .open    = ctrl_open,
    .release = ctrl_release,
    .read    = ctrl_read,
    .write   = ctrl_write,
};

/* ------------------------------------------------------------------ */
/* Module init / exit                                                 */
/* ------------------------------------------------------------------ */

static int __init subucom_virt_init(void)
{
    dev_t devno;
    int ret, i, n_added = 0, ctrl_added = 0;

    ret = alloc_chrdev_region(&devno, 0, NDEVS, DRV_NAME);
    if (ret < 0) {
        pr_err("%s: alloc_chrdev_region failed: %d\n", DRV_NAME, ret);
        return ret;
    }
    subucom_major = MAJOR(devno);

#if LINUX_VERSION_CODE >= KERNEL_VERSION(6, 4, 0)
    subucom_class = class_create(CLASS_NAME);
#else
    subucom_class = class_create(THIS_MODULE, CLASS_NAME);
#endif
    if (IS_ERR(subucom_class)) {
        ret = PTR_ERR(subucom_class);
        pr_err("%s: class_create failed: %d\n", DRV_NAME, ret);
        unregister_chrdev_region(devno, NDEVS);
        return ret;
    }

    /* Create the N_SPI spi buses: /dev/subucom_spi{1,2,3,5}.0 (minors 0..N_SPI-1). */
    for (i = 0; i < N_SPI; i++) {
        cdev_init(&subucom_cdev[i], &spi_fops);
        subucom_cdev[i].owner = THIS_MODULE;
        ret = cdev_add(&subucom_cdev[i], MKDEV(subucom_major, i), 1);
        if (ret) goto err_cleanup;
        n_added = i + 1;
        subucom_dev[i] = device_create(subucom_class, NULL,
                                       MKDEV(subucom_major, i),
                                       NULL, spi_dev_name[i]);
        if (IS_ERR(subucom_dev[i])) {
            ret = PTR_ERR(subucom_dev[i]);
            subucom_dev[i] = NULL;
            goto err_cleanup;
        }
    }

    /* ctrl device (minor N_SPI) */
    cdev_init(&subucom_cdev[MINOR_CTRL], &ctrl_fops);
    subucom_cdev[MINOR_CTRL].owner = THIS_MODULE;
    ret = cdev_add(&subucom_cdev[MINOR_CTRL], MKDEV(subucom_major, MINOR_CTRL), 1);
    if (ret) goto err_cleanup;
    ctrl_added = 1;
    subucom_dev[MINOR_CTRL] = device_create(subucom_class, NULL,
                                   MKDEV(subucom_major, MINOR_CTRL),
                                   NULL, CTRL_DEV_NAME);
    if (IS_ERR(subucom_dev[MINOR_CTRL])) {
        ret = PTR_ERR(subucom_dev[MINOR_CTRL]);
        subucom_dev[MINOR_CTRL] = NULL;
        goto err_cleanup;
    }

    init_waitqueue_head(&mosi_wq);

    /* Testmode boot injection: hold the service-mode combo. spi_read ORs it
     * into the ErP frame and re-stamps the CRC. */
    if (inject_testmode) {
        unsigned long flags;
        spin_lock_irqsave(&miso_lock, flags);
        testmode_held = 1;
        spin_unlock_irqrestore(&miso_lock, flags);
        pr_info("%s: holding the testmode combo until the first LED frame\n",
                DRV_NAME);
    }

    pr_info("%s: %d spi buses (spi1/2/3/5.0, 152B frames) + /dev/%s ready, major %d\n",
            DRV_NAME, N_SPI, CTRL_DEV_NAME, subucom_major);
    return 0;

err_cleanup:
    if (ctrl_added) {
        if (subucom_dev[MINOR_CTRL])
            device_destroy(subucom_class, MKDEV(subucom_major, MINOR_CTRL));
        cdev_del(&subucom_cdev[MINOR_CTRL]);
    }
    for (i = 0; i < n_added; i++) {
        if (subucom_dev[i])
            device_destroy(subucom_class, MKDEV(subucom_major, i));
        cdev_del(&subucom_cdev[i]);
    }
    class_destroy(subucom_class);
    unregister_chrdev_region(devno, NDEVS);
    return ret;
}

static void __exit subucom_virt_exit(void)
{
    int i;
    device_destroy(subucom_class, MKDEV(subucom_major, MINOR_CTRL));
    cdev_del(&subucom_cdev[MINOR_CTRL]);
    for (i = 0; i < N_SPI; i++) {
        device_destroy(subucom_class, MKDEV(subucom_major, i));
        cdev_del(&subucom_cdev[i]);
    }
    class_destroy(subucom_class);
    unregister_chrdev_region(MKDEV(subucom_major, 0), NDEVS);
    pr_info("%s: unloaded\n", DRV_NAME);
}

module_init(subucom_virt_init);
module_exit(subucom_virt_exit);

MODULE_LICENSE("GPL");
MODULE_AUTHOR("cdj3k-emu");
MODULE_DESCRIPTION("Virtual subucom_spi device for CDJ-3000 QEMU emulation");
MODULE_ALIAS("subucom_virt");
