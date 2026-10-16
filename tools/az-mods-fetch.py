#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""az-mods-fetch.py - install the PUBLISHED AZ-MODS set into the running emulator,
verified exactly the way the AZ-MODS desktop app verifies it.

    python3 tools/az-mods-fetch.py                 newest published set
    python3 tools/az-mods-fetch.py --release mods-v1.2.2
    python3 tools/az-mods-fetch.py --check         CURRENT or STALE, change nothing
    python3 tools/az-mods-fetch.py --download-only verify into the cache, no emulator needed
    python3 tools/az-mods-fetch.py --wait 300      first wait for a booting emulator

Steps:
  1. find the newest published mods-v* release on the public mods channel
     (GitHub Kyle-Hosman/az-mods-updates; drafts and prereleases are skipped)
  2. download manifest.json, mods-v<V>.tar.gz and their .minisig signatures
  3. REFUSE unless both signatures verify against the AZ-MODS release key below
     and every file's sha256 matches the signed manifest
  4. write the whole set to the emulated USB stick's MODS/, as the AZ-MODS app writes
     it for a deck (deck/ included, and MODS/mods.on), and turn the developer switch off
  5. have the deck's own USB loader take it from there, as at a power cycle: it mirrors
     MODS/ into the stage (/home/root/settings/az-usbload/), checks the set (from AZ-MODS
     1.2.3 on, only what its signed trust pins name loads) and writes the env EP147 reads;
     a deck already running mods is not restarted ("Reload to apply."), so this then
     reloads as the Mod Menu's Reload does, and confirms EP147 maps the set's azmod.so

With a real stick (the boot scripts pass AZ_EMU_STICK=real for AZ_USB_FILE=/dev/...)
it writes nothing: the emulator runs what that stick carries, as a deck would, and
this says which set that is. Firmware without the loader: the set's azmod.so and
azstem.so are copied into the stage, which EP147 then preloads directly.

When it cannot confirm that the installed set is the newest one (no network, the
channel unreachable, a set that fails its signature check), it says so in one line
starting "AZ-MODS:", names the set the emulator keeps running, and exits 3. The boot
scripts copy that line to the terminal, and AZ-EMU.app shows it in its title bar
(it reads mods-status.txt in the socket folder).

The stick is the truth, as on a deck: the stage on the settings disk
(build/az-mnt.img) only ever holds what the loader copied from the stick, and the
loader compares the two at every boot. The deck/ part of the set (the stick's loader,
its helpers and boot hooks) is what the deck's loader runs, so it runs here too.
Firmware without the loader (a stock 2.00 system): the set's .so files are copied into
the stage instead.
"""
import argparse
import hashlib
import io
import json
import os
import re
import sys
import tarfile
import time
import urllib.error
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import azemu  # noqa: E402
import minisign_verify  # noqa: E402

CHANNEL = os.environ.get("AZ_MODS_CHANNEL", "Kyle-Hosman/az-mods-updates")
# The AZ-MODS release key (minisign key id 5B07109A2F3C24FE). The same key the
# AZ-MODS app embeds and every published set is signed with.
PUBKEY = "RWT+JDwvmhAHW7mlYdLaNSuBFujiADXEbi9OYnvdJbjRTPYFCTgaio+D"
CACHE = os.path.join(azemu.BUILD, "mods")
STAGE = "/home/root/settings/az-usbload"
TAG = re.compile(r"^mods-v(\d+(?:\.\d+)+)$")
VERSION_FILE = f"{STAGE}/mods.version"      # "<version> <md5 of azmod.so>"
STATUS_FILE = os.path.join(azemu.SOCK_DIR, "mods-status.txt")
UNCONFIRMED = 3                             # exit status: could not check for updates


def status(line):
    """Record the outcome for AZ-EMU.app; a failure here never matters."""
    try:
        os.makedirs(azemu.SOCK_DIR, exist_ok=True)
        with open(STATUS_FILE, "w") as f:
            f.write(line + "\n")
    except OSError:
        pass


def installed_set():
    """What the emulator runs now, in words (a version or your own build); None = nothing."""
    try:
        rc, out = azemu.ssh(f"cat {VERSION_FILE} 2>/dev/null; echo; md5sum {STAGE}/azmod.so 2>/dev/null; true",
                            timeout=30)
    except SystemExit:   # no SSH key yet (./build.sh not run)
        rc, out = 1, ""
    if rc != 0:
        return "the installed set (the emulator is not reachable to say which)"
    lines = out.split("\n")
    recorded = lines[0].split()
    have = out.split("\n", 1)[1].split()[:1] if "\n" in out else []
    if not have:
        return None
    if len(recorded) == 2 and recorded[1] == have[0]:
        return f"installed set {recorded[0]}"
    return "an AZ-MODS build installed by hand (not a published set)"


def cannot_check(reason, wait):
    if wait:
        azemu.wait_for_guest(wait)
    running = installed_set()
    line = f"AZ-MODS: could not check for updates ({reason}); " + (
        f"running {running}, which may be out of date" if running
        else "no AZ-MODS set is installed, so the emulator runs without mods")
    print(line, flush=True)
    status("WARN " + line)
    sys.exit(UNCONFIRMED)


def get(url, accept="application/vnd.github+json"):
    req = urllib.request.Request(url, headers={"Accept": accept, "User-Agent": "xdj-az-emu"})
    with urllib.request.urlopen(req, timeout=120) as r:
        return r.read()


def newest(tag):
    if tag:
        return json.loads(get(f"https://api.github.com/repos/{CHANNEL}/releases/tags/{tag}"))
    rels = json.loads(get(f"https://api.github.com/repos/{CHANNEL}/releases?per_page=50"))
    rels = [r for r in rels if not r["draft"] and not r["prerelease"] and TAG.match(r["tag_name"])]
    if not rels:
        sys.exit(f"no published mods-v* release on {CHANNEL}")
    return max(rels, key=lambda r: tuple(int(x) for x in TAG.match(r["tag_name"]).group(1).split(".")))


def rel_path(e, f):
    d = f.get("dir") or e.get("dir")
    return f"{d}/{f['name']}" if d else f["name"]


def verify_set(d):
    """Every reason not to trust the set in `d`. Empty list = verified."""
    problems = []
    try:
        man = open(os.path.join(d, "manifest.json"), "rb").read()
        sig = open(os.path.join(d, "manifest.json.minisig")).read()
        m = json.loads(man)
    except (OSError, ValueError) as e:
        return [f"manifest unreadable: {e}"]
    if not minisign_verify.verify(man, sig, PUBKEY):
        problems.append("manifest.json signature does not verify against the AZ-MODS key")
    for e in m.get("entries", []):
        for f in e.get("files", []):
            p = os.path.join(d, rel_path(e, f))
            if not os.path.isfile(p):
                problems.append(f"missing: {rel_path(e, f)}")
            elif hashlib.sha256(open(p, "rb").read()).hexdigest() != f.get("sha256"):
                problems.append(f"sha256 mismatch: {rel_path(e, f)}")
    return problems


def fetch(tag):
    rel = newest(tag)
    tag = rel["tag_name"]
    ver = TAG.match(tag).group(1)
    d = os.path.join(CACHE, ver)
    if os.path.isfile(os.path.join(d, ".verified")) and not verify_set(d):
        print(f"{tag}: already downloaded and verified ({d})")
        return d, ver
    assets = {a["name"]: a["browser_download_url"] for a in rel["assets"]}
    tgz = f"mods-v{ver}.tar.gz"
    need = ["manifest.json", "manifest.json.minisig", tgz, tgz + ".minisig"]
    missing = [n for n in need if n not in assets]
    if missing:
        sys.exit(f"{tag}: release lacks {', '.join(missing)}")
    print(f"{tag}: downloading from {CHANNEL} ...")
    blobs = {n: get(assets[n], "application/octet-stream") for n in need}
    if not minisign_verify.verify(blobs["manifest.json"], blobs["manifest.json.minisig"].decode(), PUBKEY):
        sys.exit("REFUSED: manifest.json is not signed by the AZ-MODS key. Nothing installed.")
    if not minisign_verify.verify(blobs[tgz], blobs[tgz + ".minisig"].decode(), PUBKEY):
        sys.exit(f"REFUSED: {tgz} is not signed by the AZ-MODS key. Nothing installed.")
    os.makedirs(d, exist_ok=True)
    with tarfile.open(fileobj=io.BytesIO(blobs[tgz])) as t:
        for mem in t.getmembers():      # no absolute paths, no .., no links
            if mem.name.startswith(("/", "..")) or "/../" in mem.name or not (mem.isfile() or mem.isdir()):
                sys.exit(f"REFUSED: archive member {mem.name}")
        if hasattr(tarfile, "data_filter"):   # Python 3.12+ (and recent 3.9-3.11 updates)
            t.extractall(d, filter="data")
        else:
            t.extractall(d)
    for n in ("manifest.json", "manifest.json.minisig"):
        with open(os.path.join(d, n), "wb") as f:
            f.write(blobs[n])
    problems = verify_set(d)
    if problems:
        sys.exit("REFUSED: " + "; ".join(problems[:6]))
    open(os.path.join(d, ".verified"), "w").write(ver + "\n")
    m = json.load(open(os.path.join(d, "manifest.json")))
    n = sum(len(e["files"]) for e in m["entries"])
    print(f"{tag}: signatures OK, {n} files match the signed manifest "
          f"(built from {m.get('source', {}).get('repo', '?')} {m.get('source', {}).get('commit', '?')})")
    return d, ver


def plan(d):
    """{guest path: local path} for what the emulator needs from the set."""
    out = {f"{STAGE}/azmod.so": os.path.join(d, "azmod.so"),
           f"{STAGE}/azstem.so": os.path.join(d, "azstem.so")}
    pp = os.path.join(d, "azmod-pngpatch")
    if os.path.isdir(pp):
        for n in sorted(os.listdir(pp)):
            out[f"/home/root/settings/azmod-pngpatch/{n}"] = os.path.join(pp, n)
    return out


def md5(p):
    return hashlib.md5(open(p, "rb").read()).hexdigest()


UNSIGNED = azemu.UNSIGNED
NOT_IN_SET = {"manifest.json", "manifest.json.minisig", ".verified"}


def live():
    """{azmod, azstem}: md5 of what the running EP147 has mapped ("none" when not)."""
    return azemu.mapped()


def set_files(d):
    """{path under MODS/: local path} for every file of the verified set in `d`."""
    out = {}
    for root, dirs, names in os.walk(d):
        dirs.sort()
        for n in sorted(names):
            rel = os.path.relpath(os.path.join(root, n), d).replace(os.sep, "/")
            if rel not in NOT_IN_SET:
                out[rel] = os.path.join(root, n)
    return out


def install_on_stick(d, ver, a):
    """Firmware with the USB loader: the set goes on the stick and the loader applies it."""
    want = {"azmod": md5(os.path.join(d, "azmod.so")), "azstem": md5(os.path.join(d, "azstem.so"))}
    if os.environ.get("AZ_EMU_STICK") == "real":
        rc, out = azemu.ssh("M=$(az-emu-modsinstall stick) && cd \"$M/MODS\" && cat deck/version 2>/dev/null", timeout=30)
        run = live()
        what = ("the published set " + ver) if run == want else (
            "STOCK (no mods)" if run.get("azmod", "none") == "none" else
            f"the stick's own set (deck set {out.strip() or 'unknown'}, azmod {run.get('azmod')})")
        line = f"AZ-MODS: real USB stick, left as it is: the emulator runs {what}, as a deck would with it"
        print(line)
        status(("OK " if run == want else "WARN ") + line)
        sys.exit(0 if run == want else UNCONFIRMED)
    rc, mnt = azemu.ssh("az-emu-modsinstall stick", timeout=30)
    mnt = mnt.strip()
    if rc or not mnt:
        sys.exit("no USB stick in the emulator to put the set on (is one attached?)")
    files = set_files(d)
    have = azemu.guest_md5([f"{mnt}/MODS/{r}" for r in files])
    stale = [r for r, l in files.items() if have.get(f"{mnt}/MODS/{r}") != md5(l)]
    if not stale and live() == want:
        print(f"CURRENT: the emulator runs mods {ver} from its stick")
        if not a.check:
            record(ver, {f"{STAGE}/azmod.so": os.path.join(d, "azmod.so")})
        return
    if not stale:
        # the stick already carries the set: this boot's loader run applies it (do not
        # rewrite the stick or run the loader a second time under it)
        print(f"the stick carries mods {ver}; EP147 maps {live()}: waiting for this boot's loader run")
        if a.check:
            sys.exit(1)
        decision = azemu.loader_decision()
        print("loader:", decision)
        finish_on_stick(ver, d, want, azemu.apply_staged(decision))
        return
    print(f"STALE vs mods {ver}: {len(stale)} file(s) on the stick differ; EP147 maps {live()}")
    if a.check:
        sys.exit(1)
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as t:
        for rel, local in files.items():
            t.add(local, arcname=rel)
    rc, _ = azemu.ssh(f"cd '{mnt}/MODS' 2>/dev/null || mkdir -p '{mnt}/MODS'; cd '{mnt}/MODS' && "
                      f"rm -rf azmod.so azmod-core.so azstem.so deck azmod-pngpatch && tar -xf - 2>/dev/null; "
                      f"[ -e mods.off ] || [ -e disable ] || : > mods.on; "
                      f"rm -f {UNSIGNED}; sync", data=buf.getvalue(), timeout=300)
    if rc:
        sys.exit(f"writing mods {ver} to {mnt}/MODS failed")
    print(f"wrote mods {ver} to {mnt}/MODS ({len(files)} files); developer switch off "
          "(a loader from AZ-MODS 1.2.3 on loads only what a set's signed trust pins name)")
    if a.no_restart:
        print("--no-restart: the stick's loader stages it at the next boot")
        return
    decision = azemu.run_loader()
    print("loader:", decision)
    finish_on_stick(ver, d, want, azemu.apply_staged(decision))


def finish_on_stick(ver, d, want, how):
    t0 = time.time()
    while time.time() - t0 < 180:
        if live() == want:
            print(f"EP147 runs mods {ver} (azmod {want['azmod']}; {how})")
            record(ver, {f"{STAGE}/azmod.so": os.path.join(d, "azmod.so")})
            return
        time.sleep(3)
    sys.exit(f"EP147 maps {live()} after the loader ran, not mods {ver} ({want}); "
             f"the loader's log: tools/az-ssh cat {azemu.LOADER_LOG}")


def record(ver, files):
    """Note which published set is installed, so a later offline boot can name it."""
    azemu.ssh(f"echo '{ver} {md5(files[STAGE + '/azmod.so'])}' > {VERSION_FILE}.new && "
              f"mv -f {VERSION_FILE}.new {VERSION_FILE}", timeout=30)
    status(f"OK AZ-MODS {ver} (newest published set)")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--release", help="a mods-v* tag (default: newest published)")
    ap.add_argument("--check", action="store_true", help="report CURRENT/STALE, change nothing")
    ap.add_argument("--download-only", action="store_true", help="download and verify only")
    ap.add_argument("--no-restart", action="store_true")
    ap.add_argument("--wait", type=int, default=0, metavar="S",
                    help="first wait up to S seconds for the emulator to boot")
    a = ap.parse_args()

    try:
        d, ver = fetch(a.release)
    except urllib.error.HTTPError as e:
        cannot_check(f"the mods channel answered HTTP {e.code}", a.wait)
    except (urllib.error.URLError, OSError) as e:
        cannot_check(f"no connection to the mods channel: {getattr(e, 'reason', e)}", a.wait)
    except SystemExit as e:
        if not isinstance(e.code, str):
            raise
        cannot_check(e.code.replace("REFUSED: ", "refused the newest set: ").rstrip("."), a.wait)
    if a.download_only:
        return
    if a.wait and not azemu.wait_for_guest(a.wait):
        sys.exit(f"emulator not up (SSH and EP147) after {a.wait} s")
    if azemu.ssh("true", timeout=30)[0] != 0:
        sys.exit("emulator unreachable on 127.0.0.1:2222. Is it running? (docs/troubleshooting.md)")
    if azemu.has_loader():
        install_on_stick(d, ver, a)
        return
    files = plan(d)
    have = azemu.guest_md5(list(files))
    stale = [g for g, l in files.items() if have.get(g) != md5(l)]
    if not stale:
        print(f"CURRENT: the emulator has mods {ver}")
        if not a.check:
            record(ver, files)
    else:
        print(f"STALE vs mods {ver}: {len(stale)} file(s) differ")
    if a.check or not stale:
        sys.exit(1 if stale else 0)

    for g in stale:
        if not azemu.push_file(files[g], g, "755" if g.endswith(".so") else "644"):
            sys.exit(f"copy to {g} failed")
    have = azemu.guest_md5(list(files))
    still = [g for g, l in files.items() if have.get(g) != md5(l)]
    if still:
        sys.exit(f"after copying, these differ on the emulator: {', '.join(still[:6])}")
    print(f"installed mods {ver}: {len(stale)} file(s) copied and checked")
    record(ver, files)
    if a.no_restart:
        print("--no-restart: the deck program runs the old mods until it restarts")
        return
    t = azemu.restart_ep147(check=f"grep -q {STAGE}/azmod.so /proc/$(pidof EP147)/maps")
    if t is None:
        sys.exit("EP147 did not come back with the new azmod.so within 180 s "
                 "(tools/az-ssh 'journalctl -u EP147 -n 50')")
    print(f"EP147 restarted on mods {ver} in {t:.0f} s")


if __name__ == "__main__":
    main()
