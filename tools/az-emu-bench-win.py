#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""az-emu-bench-win.py - one measured boot of the AZ guest on Windows (TCG), per setting.

    python tools/az-emu-bench-win.py <label> [--cpu max] [--tcg tb-size=512] [--sample 60]

Boots tools/az-boot-win.ps1 HEADLESS (no window on the desktop), then:
  ssh_s     seconds from QEMU start until the guest answers ssh
  ep147_s   ... until EP147 runs
  up_s      ... until az-mods-fetch.py reports the current mods release running and the
            AZ-MODS core writes /tmp/azmod-rates (the counters its FPS overlay draws)
  paint/s   azmod's paint counter = the FPS overlay's number, sampled every 5 s for
            --sample seconds once up (median, min, max); also dispRefresh/s
  load      the guest's 1-minute load average at the end
and quits QEMU over QMP 4446. Appends one JSON line to %TEMP%\\az-emu-win\\bench.jsonl.
SSH: Windows' OpenSSH with this install's key (tools/azemu.py)."""
import argparse, json, os, re, socket, statistics, subprocess, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import azemu  # noqa: E402

REPO = azemu.REPO
SOCK = azemu.SOCK_DIR


def ssh(cmd, timeout=20):
    return azemu.ssh(cmd, timeout=timeout)


def qmp_quit():
    try:
        s = socket.create_connection(("127.0.0.1", 4446), timeout=5)
        s.recv(4096)
        s.sendall(b'{"execute":"qmp_capabilities"}\n'); s.recv(4096)
        s.sendall(b'{"execute":"quit"}\n'); s.close()
    except OSError:
        pass
    for _ in range(30):
        if subprocess.run(["tasklist", "/FI", "IMAGENAME eq qemu-system-aarch64.exe"],
                          capture_output=True, text=True).stdout.count("qemu-system") == 0:
            return True
        time.sleep(1)
    return False


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("label")
    ap.add_argument("--cpu", default="max")
    ap.add_argument("--tcg", default="")
    ap.add_argument("--sample", type=int, default=60)
    ap.add_argument("--timeout", type=int, default=600)
    a = ap.parse_args()
    t0 = time.time()
    r = subprocess.run(["powershell", "-NoProfile", "-File", os.path.join(REPO, "tools", "az-boot-win.ps1"),
                        "-Display", "none", "-Cpu", a.cpu, "-Tcg", a.tcg, "-NoMods"], capture_output=True, text=True)
    print(r.stdout.strip() or r.stderr.strip(), flush=True)
    if r.returncode:
        sys.exit(r.returncode)
    # "up" is measured in three steps: ssh, EP147, then the current mods release
    # running (az-mods-fetch.py installs it if needed, and the core writes its rates).
    up = ep = ssh_s = None
    try:
        while time.time() - t0 < a.timeout:
            rc, out = ssh("pidof EP147", timeout=15)
            if rc != 255 and ssh_s is None:
                ssh_s = time.time() - t0
                print(f"{a.label}: guest ssh at {ssh_s:.0f} s", flush=True)
            if rc == 0 and out.strip():
                ep = time.time() - t0
                print(f"{a.label}: EP147 running at {ep:.0f} s", flush=True)
                break
            time.sleep(2)
        if ep is not None:
            r = subprocess.run([sys.executable, os.path.join(REPO, "tools", "az-mods-fetch.py")],
                               capture_output=True, text=True)
            print(r.stdout.strip() or r.stderr.strip(), flush=True)
            while r.returncode == 0 and time.time() - t0 < a.timeout:
                if ssh("test -s /tmp/azmod-rates")[0] == 0:
                    up = time.time() - t0
                    break
                time.sleep(2)
        if up is None:
            print(f"{a.label}: not up after {a.timeout} s", flush=True)
            res = {"label": a.label, "cpu": a.cpu, "tcg": a.tcg, "ssh_s": ssh_s and round(ssh_s),
                   "ep147_s": ep and round(ep), "up_s": None}
        else:
            print(f"{a.label}: up on current mods at {up:.0f} s; sampling {a.sample} s", flush=True)
            time.sleep(10)                                    # let the UI settle
            paint, disp = [], []
            for _ in range(max(1, a.sample // 5)):
                rc, out = ssh("cat /tmp/azmod-rates")
                m = re.search(r"paint=([\d.]+)/s over", out or "")
                d = re.search(r"dispRefresh=(\d+)/s", out or "")
                if m:
                    paint.append(float(m.group(1)))
                if d:
                    disp.append(int(d.group(1)))
                time.sleep(5)
            _, load = ssh("cut -d' ' -f1 /proc/loadavg")
            res = {"label": a.label, "cpu": a.cpu, "tcg": a.tcg, "ssh_s": round(ssh_s), "ep147_s": round(ep), "up_s": round(up),
                   "paint_med": statistics.median(paint) if paint else None,
                   "paint_min": min(paint) if paint else None, "paint_max": max(paint) if paint else None,
                   "disp_med": statistics.median(disp) if disp else None,
                   "guest_load1": load.strip(), "samples": len(paint)}
            print(json.dumps(res), flush=True)
    finally:
        print("qemu stopped" if qmp_quit() else "WARNING: qemu still running", flush=True)
    with open(os.path.join(SOCK, "bench.jsonl"), "a") as f:
        f.write(json.dumps(res) + "\n")


if __name__ == "__main__":
    main()
