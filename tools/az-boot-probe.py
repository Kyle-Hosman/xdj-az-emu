#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""az-boot-probe.py - time an emulator boot from outside: serial-log milestones
plus a QMP screendump every N seconds, on the AUTOMATION QMP port (4446; 4445
is AZ-EMU's). Most useful under TCG (Windows), where boot time is the
question; works on any host.

    az-boot-probe.py <serial.log> <out-dir> [--every 20] [--limit 900] [--qmp 4446]

Prints one line per milestone, as seconds since the probe started (start it
right after QEMU). Stops at --limit, or once EP147 has run and two screendumps
in a row are the same size and not blank.
"""
import argparse, json, os, re, socket, time

MILESTONES = [
    ("kernel",      re.compile(r"Booting Linux")),
    ("init",        re.compile(r"Run /init|Freeing unused kernel memory")),
    ("systemd",     re.compile(r"systemd\[1\]: |Welcome to")),
    ("mnt mounted", re.compile(r"EXT4-fs \(vda\S*\): mounted|mounted filesystem", re.I)),
    ("login",       re.compile(r"login:")),
    ("X failed",    re.compile(r"x11-only.service: Failed")),
    # EP147's own banner, not any line naming EP147 (its unit's config warnings
    # name it long before it runs); the banner can arrive garbled on the serial
    # console, so its first log line after init is the fallback
    ("EP147",       re.compile(r"Application Started \(Rev:|success:createSharedMemory")),
    ("EP147 no X",  re.compile(r"Failed to connect to the X Server")),
    ("EP147 exit",  re.compile(r"EP147.service: Main process exited")),
]


def qmp(port, cmd, args=None):
    s = socket.create_connection(("127.0.0.1", port), timeout=30)
    f = s.makefile("rw", encoding="utf-8", newline="\n")
    f.readline()                                           # greeting
    for c, a in (("qmp_capabilities", None), (cmd, args)):
        f.write(json.dumps({"execute": c, **({"arguments": a} if a else {})}) + "\n"); f.flush()
        while True:
            r = json.loads(f.readline())
            if "return" in r or "error" in r:
                break
    s.close()
    return r


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("serial"); ap.add_argument("out")
    ap.add_argument("--every", type=float, default=20); ap.add_argument("--limit", type=float, default=900)
    ap.add_argument("--qmp", type=int, default=4446)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    t0, seen, last_shot, sizes = time.time(), {}, -1e9, []
    while True:
        t = time.time() - t0
        try:
            text = open(a.serial, encoding="utf-8", errors="replace").read()
        except OSError:
            text = ""
        for name, rx in MILESTONES:
            if name not in seen:
                m = rx.search(text)
                if m:
                    seen[name] = t
                    line = text[text.rfind("\n", 0, m.start()) + 1:text.find("\n", m.end())][:120]
                    print(f"{t:7.1f}s  {name:12s} {line.strip()}", flush=True)
        if t - last_shot >= a.every:
            last_shot = t
            png = os.path.abspath(os.path.join(a.out, f"t{int(t):04d}.png"))
            try:
                r = qmp(a.qmp, "screendump", {"filename": png, "format": "png"})
                n = os.path.getsize(png) if "return" in r and os.path.exists(png) else 0
                sizes.append(n)
                print(f"{t:7.1f}s  screendump   {os.path.basename(png)} {n} B", flush=True)
            except OSError as e:
                print(f"{t:7.1f}s  screendump   failed: {e}", flush=True)
        if t > a.limit:
            print(f"{t:7.1f}s  LIMIT reached", flush=True)
            break
        if "EP147 exit" in seen and t - seen["EP147 exit"] > 5:
            print(f"{t:7.1f}s  EP147 exited: stopping", flush=True)
            break
        if "EP147" in seen and len(sizes) >= 2 and sizes[-1] == sizes[-2] and sizes[-1] > 20000:
            print(f"{t:7.1f}s  screen stable after EP147", flush=True)
            break
        time.sleep(1)


if __name__ == "__main__":
    main()
