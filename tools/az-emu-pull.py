#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""az-emu-pull: copy the stock XDJ-AZ root filesystem off YOUR rooted deck for the emulator.

Usage:
    python3 tools/az-emu-pull.py --host DECK-IP --key path/to/private-key [--out DIR]
                                 [--method auto|p1|p2] [--user root]
    python3 tools/az-emu-pull.py --scan-only FILE.cpio.gz   (run the no-key scan on an existing image)
    python3 tools/az-emu-pull.py --self-test                (check the scan and the P2 command offline)

The key is the SSH key the AZ-MODS app made when you turned on "SSH access".

Output (DIR defaults to the repository's build/, or $AZ_EMU_BUILD):
    DIR/initramfs-work-pulled/initramfs.cpio.gz   <- what ./build.sh consumes
    DIR/initramfs-work-pulled/manifest.json       <- what was read, from where, md5s, scan result
The pulled system stays on your computer: build/ is ignored by git, and no tool here
uploads it.

THE DECK IS NEVER WRITTEN. Every remote command is a read: `systemctl show`, `md5sum`,
`cat /proc/...`, `ls`, and `dd if=<boot partition>` piped straight to this machine over ssh.
Nothing is mounted, created, loaded or restarted. The script reads NRestarts and
ActiveEnterTimestamp before and after and fails if they moved.

Firmware gate: XDJ-AZ 2.00 / 2.01 only, recognized by the deck program's own revision string
(/home/root/pdj/app.rev), the same way cdj3k-emu reads APP.REV from the user's firmware. "2.01"
is the AZ-MODS release marker (etc/az-mods-release) on the stock 2.00 rootfs and carries the
same revision. No hashes of vendor files are kept in this repository: the only comparisons are
made at run time, between the copy and your own deck.

Methods:
  P1 (default): carve the initramfs gzip member out of the `IMAGE` file on the boot banks
      boota (/dev/mmcblk1p5) and bootb (/dev/mmcblk1p6). The FAT32 is parsed HERE, only the
      IMAGE file's clusters are read off the deck. The bank whose cpio holds an EP147 whose
      md5 equals the live /home/root/pdj/EP147 is chosen; if both match and differ elsewhere,
      live md5s of the differing files break the tie.
      AZ-MODS loader: a deck rooted with the AZ-MODS release carries the stock 2.00 cpio with
      our 8 loader entries APPENDED (LOADER_PATHS, our own files). They are KEPT: the
      emulator boots the deck's own loader, which loads the mods from the (emulated or real)
      USB stick exactly as on the deck. The scan allows exactly those paths; the same names
      anywhere else are still refused.
      Checks: the copied EP147 must equal the live one (md5 computed now, on both sides), the
      copied app.rev must equal the live one, and the no-key scan must find nothing of ours or
      of the device's secrets. The output gzip is re-compressed here (build.sh only gunzips it).
  P2 (fallback): `tar` of the live root filesystem only (`find -xdev`, so no other mount is
      entered), excluding runtime dirs, settings, /mnt, aes256.key and every file our mods
      install. Converted to a newc cpio here. A live root is NOT pristine
      (the mods overlay is copied over / at boot), so P2 output needs --allow-nonstock.

If the scan finds anything, nothing is written and the exit status is 2.

Never read: /dev/mmcblk1p4 (update partition, holds aes256.key), /home/root/settings,
/mnt, the az-usbload stage, MODS, the overlay.
"""
import argparse, gzip, hashlib, io, json, os, re, shlex, struct, subprocess, sys, tarfile, time, zlib

SUPPORTED_APP_REV = {  # contents of /home/root/pdj/app.rev -> firmware
    "05c40500f": "2.00/2.01",
}
# Entries the AZ-MODS release (.UPD delta, marker etc/az-mods-release = "2.01") APPENDS to the
# stock 2.00 cpio; it modifies no stock entry. They are the deck's USB mod loader, so the copy
# keeps them (the emulator runs the loader as the deck does), and scan() allows exactly these
# paths, nowhere else.
LOADER_PATHS = {
    "etc/az-mods-release",
    "etc/systemd/system/EP147.service.d",
    "etc/systemd/system/EP147.service.d/10-azmod.conf",
    "etc/systemd/system/az-usbload.service",
    "etc/systemd/system/multi-user.target.wants/az-usbload.service",
    "home/root/scripts/usb-renotify.sh",
    "usr/sbin/az-usbload",
    "usr/sbin/az-usbpick",
}
BANKS = {"boota": "/dev/mmcblk1p5", "bootb": "/dev/mmcblk1p6"}
FORBIDDEN_DEVS = ("/dev/mmcblk1p4", "/dev/mmcblk1p7", "/dev/mmcblk1p8")  # update, setting, reserve

# Paths that must never be in an emulator image. Applies to cpio entry names (no leading /).
FORBIDDEN_NAME_RES = [
    (r"(^|/)aes256\.key$", "aes256.key"),
    (r"(^|/)cabinet\.img$", "cabinet.img"),
    (r"(^|/)wifi\.json$", "wifi.json"),
    (r"^home/root/settings/.", "settings (p7) content"),
    (r"^mnt/.", "/mnt content"),
    (r"^media/.", "/media content"),
    (r"(^|/)(azmod|azstem|azaudio|ep122_shim)[^/]*\.so$", "our mod .so"),
    (r"(^|/)az-usbload(/|$)", "az-usbload stage/loader"),
    (r"(^|/)(az-usbpick|az-mods-release|usb-renotify\.sh|10-azmod\.conf|20-aztx[^/]*|30-aztx[^/]*)$",
     "AZ-MODS loader/mod file"),
    (r"(^|/)MODS(/|$)", "MODS folder"),
    (r"(^|/)azmod(/|$)", "azmod dir/overlay"),
    (r"(^|/)(id_rsa|id_ecdsa|id_ed25519|authorized_keys)$", "ssh user key material"),
    (r"(^|/)dropbear_[a-z0-9]+_host_key$", "dropbear host key"),
    (r"(^|/)(serverPrivateKey\.pem|key\.dat|iv\.dat)$", "cabinet PKI file"),
]
PEM_RE = re.compile(rb"-----BEGIN ([A-Z0-9 ]*)PRIVATE KEY-----\r?\n[A-Za-z0-9+/=\r\n:,-]{64}")
LUKS_MAGIC = b"LUKS\xba\xbe"


def log(*a):
    print(*a, file=sys.stderr, flush=True)


# ---------------------------------------------------------------- ssh (read-only commands)
# Windows: Windows' own OpenSSH even from Git Bash (whose PATH puts Git's ssh first).
_WIN_SSH = os.path.join(os.environ.get("SystemRoot", r"C:\Windows"), "System32", "OpenSSH", "ssh.exe")
SSH_EXE = _WIN_SSH if os.name == "nt" and os.path.isfile(_WIN_SSH) else "ssh"

class Deck:
    def __init__(self, host, key, user):
        self.base = [SSH_EXE, "-i", key, "-o", "IdentitiesOnly=yes", "-o", "ConnectTimeout=30",
                     "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=accept-new", f"{user}@{host}"]
        self.commands = []

    def run(self, cmd, binary=False):
        for d in FORBIDDEN_DEVS:
            if d in cmd:
                raise SystemExit(f"REFUSED: command would touch {d}: {cmd}")
        self.commands.append(cmd)
        p = subprocess.run(self.base + [cmd], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        if p.returncode != 0:
            raise RuntimeError(f"remote command failed ({p.returncode}): {cmd}\n{p.stderr.decode(errors='replace')}")
        return p.stdout if binary else p.stdout.decode(errors="replace")

    def state(self):
        out = self.run("systemctl show EP147.service -p NRestarts -p ActiveEnterTimestamp")
        return dict(l.split("=", 1) for l in out.strip().splitlines() if "=" in l)

    def read_dev(self, dev, offset, length):
        """Read [offset, offset+length) of a block device with dd. bs = largest power of two
        (<=1 MiB) dividing both offset and length, so skip/count are exact."""
        bs = 65536
        start = offset - offset % bs
        count = (offset + length - start + bs - 1) // bs
        data = self.run(f"nice -n 19 dd if={dev} bs={bs} skip={start // bs} count={count} 2>/dev/null",
                        binary=True)
        data = data[offset - start: offset - start + length]
        if len(data) != length:
            raise RuntimeError(f"short read from {dev}: {len(data)} of {length}")
        return data


# ---------------------------------------------------------------- FAT32 (parsed locally)
def fat32_read_file(deck, dev, want="IMAGE"):
    bs = deck.read_dev(dev, 0, 512)
    if bs[510:512] != b"\x55\xaa":
        raise RuntimeError(f"{dev}: no boot-sector signature")
    bps, spc, rsvd, nfats = struct.unpack_from("<HBHB", bs, 11)
    fatsz = struct.unpack_from("<I", bs, 36)[0]
    root_clus = struct.unpack_from("<I", bs, 44)[0]
    if struct.unpack_from("<H", bs, 22)[0] != 0 or bs[82:87] != b"FAT32":
        raise RuntimeError(f"{dev}: not FAT32 (fs type field {bs[82:90]!r})")
    clus = bps * spc
    fat_off = rsvd * bps
    fat_len = fatsz * bps
    # round the FAT read up to 4 KiB so dd's bs stays large
    fat = deck.read_dev(dev, fat_off, fat_len)
    data_off = (rsvd + nfats * fatsz) * bps

    def chain(c):
        out = []
        while 2 <= c < 0x0FFFFFF8:
            out.append(c)
            c = struct.unpack_from("<I", fat, c * 4)[0] & 0x0FFFFFFF
            if len(out) > len(fat) // 4:
                raise RuntimeError("FAT loop")
        return out

    def read_chain(cl, size=None):
        runs, start, prev = [], cl[0], cl[0]
        for c in cl[1:]:
            if c != prev + 1:
                runs.append((start, prev)); start = c
            prev = c
        runs.append((start, prev))
        buf = bytearray()
        for a, b in runs:
            buf += deck.read_dev(dev, data_off + (a - 2) * clus, (b - a + 1) * clus)
        return bytes(buf[:size] if size is not None else buf), runs

    root, _ = read_chain(chain(root_clus))
    names = []
    for i in range(0, len(root), 32):
        e = root[i:i + 32]
        if e[0] == 0:
            break
        if e[0] == 0xE5 or e[11] == 0x0F or e[11] & 0x08:
            continue
        nm = e[0:8].decode("latin1").rstrip() + ("." + e[8:11].decode("latin1").rstrip() if e[8:11].strip() else "")
        size = struct.unpack_from("<I", e, 28)[0]
        first = (struct.unpack_from("<H", e, 20)[0] << 16) | struct.unpack_from("<H", e, 26)[0]
        names.append((nm, size, first, bool(e[11] & 0x10)))
    hit = [n for n in names if n[0] == want and not n[3]]
    if not hit:
        raise RuntimeError(f"{dev}: no {want} in root dir; entries: {names}")
    _, size, first, _ = hit[0]
    data, runs = read_chain(chain(first), size)
    return data, {"dev": dev, "root_dir": [(n, s) for n, s, _, _ in names], "file": want, "size": size,
                  "cluster_bytes": clus, "runs": len(runs)}


def find_initramfs(image):
    """Find the gzip member in a kernel IMAGE that decompresses to a newc cpio holding EP147.
    Returns (offset, member_bytes). Scans every gzip header candidate; offset is reported, never assumed."""
    pos = 0
    while True:
        pos = image.find(b"\x1f\x8b\x08", pos)
        if pos < 0:
            raise RuntimeError("no gzip member containing a cpio with home/root/pdj/EP147 found in IMAGE")
        d = zlib.decompressobj(31)
        try:
            head = d.decompress(image[pos:pos + 65536], 4096)
        except zlib.error:
            pos += 1; continue
        if head[:6] != b"070701":
            pos += 1; continue
        d = zlib.decompressobj(31)
        try:
            raw = d.decompress(image[pos:])
        except zlib.error:
            pos += 1; continue
        if not d.eof:
            pos += 1; continue
        member = image[pos:len(image) - len(d.unused_data)]
        if b"home/root/pdj/EP147\x00" in raw:
            return pos, member, raw
        pos += 1


# ---------------------------------------------------------------- cpio newc
def norm(name):
    while name.startswith("./"):
        name = name[2:]
    name = name.lstrip("/")
    return name or "."


def cpio_entries(raw):
    off, out = 0, []
    while True:
        if raw[off:off + 6] != b"070701":
            raise RuntimeError(f"bad cpio magic at {off}")
        f = [int(raw[off + 6 + 8 * i: off + 14 + 8 * i], 16) for i in range(13)]
        mode, fsize, nsize = f[1], f[6], f[11]
        name = raw[off + 110: off + 110 + nsize - 1].decode("utf-8", "surrogateescape")
        doff = (off + 110 + nsize + 3) & ~3
        if name == "TRAILER!!!":
            return out
        out.append((norm(name), mode, raw[doff:doff + fsize]))
        off = (doff + fsize + 3) & ~3


def cpio_write(entries):
    """entries: (name, mode, data, mtime). Writes newc with fresh inode numbers; links become symlink data."""
    buf = io.BytesIO()
    for ino, (name, mode, data, mtime) in enumerate(list(entries) + [("TRAILER!!!", 0, b"", 0)], 1):
        nb = name.encode("utf-8", "surrogateescape") + b"\0"
        hdr = "070701" + "".join("%08X" % v for v in (ino, mode, 0, 0, 1, int(mtime), len(data), 0, 0, 0, 0, len(nb), 0))
        buf.write(hdr.encode() + nb)
        buf.write(b"\0" * ((4 - buf.tell() % 4) % 4))
        buf.write(data)
        buf.write(b"\0" * ((4 - buf.tell() % 4) % 4))
    return buf.getvalue()


def ep147_of(entries):
    for n, m, d in entries:
        if n == "home/root/pdj/EP147":
            return d
    return None


# ---------------------------------------------------------------- no-key scan
def scan(entries):
    hits = []
    for name, mode, data in entries:
        if (mode & 0o170000) == 0o040000:
            continue  # empty mountpoint dirs (mnt/, media/...) are stock; their CONTENTS are checked
        if name not in LOADER_PATHS:  # the deck's own loader, in its own place (see LOADER_PATHS)
            for rx, why in FORBIDDEN_NAME_RES:
                if re.search(rx, name):
                    hits.append((name, "name: " + why))
        if (mode & 0o170000) != 0o100000:
            continue
        if data[:6] == LUKS_MAGIC:
            hits.append((name, "LUKS header at offset 0"))
        elif LUKS_MAGIC + b"\x00\x01" in data:
            hits.append((name, "LUKS1 magic inside file"))
        m = PEM_RE.search(data)
        if m:
            hits.append((name, f"PEM private key body ({m.group(1).decode().strip() or 'generic'})"))
    return hits


CONTROLS = [  # each must be hit by scan(); proves the scan can produce the OTHER answer
    ("usr/local/atc/aes256.key", 0o100600, b"\x00" * 32),
    ("home/root/settings/cabinet.img", 0o100600, b"\x00" * 16),
    ("home/root/settings/wifi.json", 0o100600, b"{}"),
    ("tmp/control-luks.img", 0o100600, LUKS_MAGIC + b"\x00\x01" + b"\x00" * 64),
    # built at run time so this file itself never contains a key header
    ("tmp/control.pem", 0o100600, b"-----BEGIN RSA " + b"PRIVATE KEY-----\n" + b"A" * 64 + b"\n-----END RSA PRIVATE KEY-----\n"),
    ("tmp/innocent-pem-literal", 0o100644, b"-----BEGIN %s PRIVATE KEY-----\x00"),  # must NOT hit
    ("home/root/azmod.so", 0o100755, b"\x7fELF"),
    # the loader's names outside the loader's own paths are still refused
    ("usr/bin/az-usbload", 0o100755, b"#!/bin/sh\n"),
    ("home/root/settings/az-usbload/az-preload.env", 0o100644, b"LD_PRELOAD=\n"),
    ("etc/systemd/system/EP147.service.d/20-aztx.conf", 0o100644, b"[Service]\n"),
    # the deck's own loader in its own place must NOT hit (else a 2.01 copy is refused)
    ("usr/sbin/az-usbload", 0o100755, b"#!/bin/sh\n"),
    ("etc/systemd/system/EP147.service.d/10-azmod.conf", 0o100644, b"[Service]\n"),
]
MUST_NOT_HIT = {"tmp/innocent-pem-literal", "usr/sbin/az-usbload",
                "etc/systemd/system/EP147.service.d/10-azmod.conf"}


def scan_with_controls(entries):
    ctl = scan(CONTROLS)
    ctl_hit = {n for n, _ in ctl}
    missing = [n for n, _, _ in CONTROLS if n not in ctl_hit and n not in MUST_NOT_HIT]
    false_pos = sorted(MUST_NOT_HIT & ctl_hit)
    if missing or false_pos:
        raise SystemExit(f"SCAN CONTROL FAILED: missed {missing}, false positives {false_pos}")
    real = scan(entries)
    return {"controls": [f"{n}: {w}" for n, w in ctl], "hits": [f"{n}: {w}" for n, w in real]}


# ---------------------------------------------------------------- stock verification
def cpio_records(raw):
    off, out = 0, []
    while True:
        f = [int(raw[off + 6 + 8 * i: off + 14 + 8 * i], 16) for i in range(13)]
        name = norm(raw[off + 110: off + 110 + f[11] - 1].decode("utf-8", "surrogateescape"))
        end = (((off + 110 + f[11] + 3) & ~3) + f[6] + 3) & ~3
        out.append((name, off, end))
        if name == "TRAILER!!!":
            return out
        off = end


def app_rev_of(entries):
    for n, m, d in entries:
        if n == "home/root/pdj/app.rev":
            return d.decode("ascii", "replace").strip()
    return None


# ---------------------------------------------------------------- P1 / P2
def pull_p1(deck, live_md5):
    banks = {}
    for label, dev in BANKS.items():
        link = deck.run(f"readlink -f /dev/disk/by-partlabel/{label}").strip()
        if link != dev:
            raise RuntimeError(f"partlabel {label} -> {link}, expected {dev}; refusing to guess")
        t = time.time()
        image, info = fat32_read_file(deck, dev)
        off, member, raw = find_initramfs(image)
        ents = cpio_entries(raw)
        ep = ep147_of(ents)
        info.update(image_md5=hashlib.md5(image).hexdigest(), initramfs_offset=hex(off),
                    gz_size=len(member), gz_md5=hashlib.md5(member).hexdigest(),
                    cpio_size=len(raw), cpio_md5=hashlib.md5(raw).hexdigest(),
                    ep147_md5=hashlib.md5(ep).hexdigest() if ep else None, seconds=round(time.time() - t, 1))
        log(f"  {label}: IMAGE {info['size']} B, initramfs gzip at {info['initramfs_offset']} "
            f"({len(member)} B), EP147 {info['ep147_md5']}  [{info['seconds']}s]")
        info.update(app_rev=app_rev_of(ents))
        banks[label] = (info, member, raw, ents)
        if info["ep147_md5"] == live_md5:
            break  # this bank holds the live EP147: the other bank cannot change the answer
    match = [l for l, (i, *_) in banks.items() if i["ep147_md5"] == live_md5]
    if len(banks) == 1:
        evidence = {l: banks[l][0] for l in banks}
        evidence["chosen"] = match[0]
        evidence["note"] = "first bank matched the live EP147; other bank not read"
        info, member, raw, ents = banks[match[0]]
        return member, raw, ents, evidence
    evidence = {l: banks[l][0] for l in banks}
    if not match:
        raise RuntimeError("no boot bank holds the live EP147")
    if len(match) == 2 and banks["boota"][0]["cpio_md5"] != banks["bootb"][0]["cpio_md5"]:
        a = {n: hashlib.md5(d).hexdigest() for n, m, d in banks["boota"][3] if (m & 0o170000) == 0o100000}
        b = {n: hashlib.md5(d).hexdigest() for n, m, d in banks["bootb"][3] if (m & 0o170000) == 0o100000}
        diff = sorted(n for n in set(a) | set(b) if a.get(n) != b.get(n) and not n.startswith("home/root/settings"))[:40]
        live = {}
        if diff:
            out = deck.run("md5sum " + " ".join(shlex.quote("/" + n) for n in diff) + " 2>/dev/null; true")
            for l in out.splitlines():
                h, n = l.split(None, 1); live[n.lstrip("/")] = h
        score = {l: sum(live.get(n) == (a if l == "boota" else b).get(n) for n in diff) for l in match}
        evidence["tiebreak"] = {"differing_files": diff, "live_md5": live, "score": score}
        match = [max(score, key=score.get)]
    chosen = match[0]
    evidence["chosen"] = chosen
    evidence["both_banks_identical_cpio"] = banks["boota"][0]["cpio_md5"] == banks["bootb"][0]["cpio_md5"]
    info, member, raw, ents = banks[chosen]
    return member, raw, ents, evidence


# P2 reads only the root filesystem itself: `find -xdev` never descends into another mount
# (the settings partition at /home/root/settings, the reserve partition at /mnt, any update
# partition mount), and the pruned paths are never listed, so tar never opens them.
# BusyBox tar has no --one-file-system; find -xdev piped to `tar -T -` is the equivalent.
P2_PRUNE_PATHS = ["./proc", "./sys", "./dev", "./run", "./tmp", "./mnt", "./media", "./var/volatile",
                  "./home/root/settings", "./home/root/pdj/core*", "./home/root/*.so",
                  "./var/log", "./etc/dropbear", "./root/.ssh",
                  "./home/root/.ssh", "./usr/local/atc/aes256.key"]
P2_PRUNE_NAMES = ["aes256.key", "cabinet.img"]
# EP147.service.d: keep the loader's drop-in, prune any other (a developer's own, such as
# 20-aztx.conf, which the scan refuses)
P2_PRUNE_EXTRA = [r"\( -path './etc/systemd/system/EP147.service.d/*' ! -name 10-azmod.conf \)"]


def p2_command():
    """The one remote command P2 runs: list the root filesystem without crossing mounts or
    entering a pruned path, then archive exactly that list (no recursion)."""
    tests = [f"-path {shlex.quote(p)}" for p in P2_PRUNE_PATHS] + \
            [f"-name {shlex.quote(n)}" for n in P2_PRUNE_NAMES] + P2_PRUNE_EXTRA
    return (r"cd / && nice -n 19 find . -xdev \( " + " -o ".join(tests) + r" \) -prune -o -print"
            " | nice -n 19 tar -c -f - --no-recursion -T - 2>/dev/null")


def pull_p2(deck):
    data = deck.run(p2_command(), binary=True)
    ents, out = [], []
    with tarfile.open(fileobj=io.BytesIO(data)) as tf:
        for ti in tf:
            name = norm(ti.name)
            if ti.isdir(): mode, d = 0o040000 | ti.mode, b""
            elif ti.issym(): mode, d = 0o120000 | 0o777, ti.linkname.encode()
            elif ti.isfile(): mode, d = 0o100000 | ti.mode, tf.extractfile(ti).read()
            elif ti.islnk(): mode, d = 0o100000 | ti.mode, None  # resolved below
            else: continue  # device nodes etc.: the initramfs init creates /dev
            out.append([name, mode, d, ti.mtime, norm(ti.linkname) if ti.islnk() else None])
    byname = {e[0]: e for e in out}
    for e in out:
        if e[2] is None:
            e[2] = byname[e[4]][2]
    raw = cpio_write([(n, m, d, t) for n, m, d, t, _ in out])
    ents = [(n, m, d) for n, m, d, _, _ in out]
    member = gzip.compress(raw, 9, mtime=0)
    return member, raw, ents, {"method": "P2", "not_stock": "live root: overlay-copied files may be present",
                               "tar_bytes": len(data)}


def write_atomic(path, data):
    """Write to PATH.part, then rename over PATH, so a failed or interrupted run never leaves
    a partial file under the real name. The .part file is removed on any error."""
    tmp = path + ".part"
    try:
        with open(tmp, "wb") as f:
            f.write(data)
        os.replace(tmp, path)
    except BaseException:
        try:
            os.remove(tmp)
        except OSError:
            pass
        raise


def self_test():
    """Offline checks of the parts that keep forbidden data out of an image."""
    import tempfile
    scan_with_controls([])  # raises if a control is missed or the literal is a false positive
    cmd = p2_command()
    for need in ("find . -xdev", "-prune -o -print", "--no-recursion -T -", "-path ./home/root/settings",
                 "-path ./mnt", "-path ./usr/local/atc/aes256.key", "-name aes256.key",
                 "! -name 10-azmod.conf"):
        assert need in cmd, f"P2 command lacks {need!r}: {cmd}"
    assert not any(d in cmd for d in FORBIDDEN_DEVS), cmd
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "out.bin")
        write_atomic(p, b"ok")
        assert open(p, "rb").read() == b"ok" and not os.path.exists(p + ".part")

        try:
            write_atomic(os.path.join(d, "fail.bin"), "not bytes")  # write() raises TypeError
            raise AssertionError("write_atomic accepted a str")
        except TypeError:
            pass
        assert not os.path.exists(os.path.join(d, "fail.bin")), "failed write left the real file"
        assert not os.path.exists(os.path.join(d, "fail.bin.part")), "failed write left a .part file"
    log("self-test: PASS")
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--host"); ap.add_argument("--key"); ap.add_argument("--user", default="root")
    ap.add_argument("--out", default=os.environ.get("AZ_EMU_BUILD") or
                    os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "build"))
    ap.add_argument("--method", choices=["auto", "p1", "p2"], default="auto")
    ap.add_argument("--scan-only", metavar="CPIO_GZ")
    ap.add_argument("--allow-nonstock", action="store_true",
                    help="write the image even if it is not a boot-bank copy of the system (P2 always needs this)")
    ap.add_argument("--self-test", action="store_true",
                    help="check the scan controls, the P2 command and the atomic write; touches no deck")
    a = ap.parse_args()

    if a.self_test:
        return self_test()
    if a.scan_only:
        raw = gzip.decompress(open(a.scan_only, "rb").read())
        r = scan_with_controls(cpio_entries(raw))
        print(json.dumps(r, indent=1)); return 1 if r["hits"] else 0
    if not (a.host and a.key):
        ap.error("--host and --key are required")

    t0 = time.time()
    deck = Deck(a.host, os.path.expanduser(a.key), a.user)
    before = deck.state()
    log(f"deck: {before}")
    live = deck.run("md5sum /home/root/pdj/EP147").split()[0]
    live_rev = deck.run("cat /home/root/pdj/app.rev").strip()
    fw = SUPPORTED_APP_REV.get(live_rev)
    if not fw:
        raise SystemExit(f"UNSUPPORTED FIRMWARE: deck program revision {live_rev!r} is not 2.00/2.01. Nothing was copied.")
    log(f"firmware: {fw} (app.rev {live_rev})")

    method, evidence = None, None
    if a.method in ("auto", "p1"):
        try:
            member, raw, ents, evidence = pull_p1(deck, live); method = "P1"
        except Exception as e:
            if a.method == "p1": raise
            log(f"P1 (boot-bank carve) failed: {e}\nFALLING BACK to P2 (tar of the live root). "
                "The result is NOT a pristine stock image: review manifest.json before using it.")
    if method is None:
        member, raw, ents, evidence = pull_p2(deck); method = "P2"

    carved = {"gz_md5": hashlib.md5(member).hexdigest(), "cpio_md5": hashlib.md5(raw).hexdigest()}
    loader = sorted(n for n, _, _ in ents if n in LOADER_PATHS)
    if "usr/sbin/az-usbload" in loader:
        log(f"AZ-MODS loader: kept ({len(loader)} entries): the emulator loads mods from the USB stick through it")
    else:
        log("AZ-MODS loader: none (stock 2.00). The emulator can still run, but it will not load mods "
            "the way a deck does: pull from a deck with AZ-MODS installed (firmware 2.01).")
    if method == "P2" and not a.allow_nonstock:
        raise SystemExit("P2 copies the LIVE root, which is not a pristine system. Nothing written. "
                         "Re-run with --allow-nonstock to keep it anyway.")
    ep = ep147_of(ents)
    ep_md5 = hashlib.md5(ep).hexdigest() if ep else None
    if ep_md5 != live:
        raise SystemExit(f"VERIFY FAILED: EP147 in the copy {ep_md5} != live {live}")
    rev = app_rev_of(ents)
    if rev != live_rev:
        raise SystemExit(f"VERIFY FAILED: app.rev in the copy {rev!r} != live {live_rev!r}")
    sc = scan_with_controls(ents)

    after = deck.state()
    if after != before:
        raise SystemExit(f"DECK STATE CHANGED: before {before} after {after}")

    if sc["hits"]:
        for h in sc["hits"]:
            log("  SCAN HIT: " + h)
        log(f"REFUSED: the scan found {len(sc['hits'])} item(s) that must never be in an emulator "
            "image (listed above). Nothing was written.")
        part = os.path.join(a.out, "initramfs-work-pulled", "initramfs.cpio.gz.part")
        if os.path.exists(part):
            os.remove(part)
        return 2

    od = os.path.join(a.out, "initramfs-work-pulled"); os.makedirs(od, exist_ok=True)
    man = {"method": method, "firmware": fw, "app_rev": rev, "carved": carved,
           "loader_entries": loader, "ep147_md5": ep_md5,
           "initramfs_gz_md5": hashlib.md5(member).hexdigest(), "initramfs_gz_size": len(member),
           "cpio_md5": hashlib.md5(raw).hexdigest(), "cpio_size": len(raw), "cpio_entries": len(ents),
           "evidence": evidence, "scan": sc, "deck_before": before, "deck_after": after,
           "remote_commands": deck.commands, "seconds": round(time.time() - t0, 1)}
    write_atomic(os.path.join(od, "initramfs.cpio.gz"), member)
    write_atomic(os.path.join(od, "manifest.json"), json.dumps(man, indent=1).encode())
    log(f"wrote {od}/initramfs.cpio.gz ({len(member)} B, md5 {man['initramfs_gz_md5']}) via {method} "
        f"in {man['seconds']}s; scan hits: 0")
    log("next: ./build.sh")
    return 0


if __name__ == "__main__":
    sys.exit(main())
