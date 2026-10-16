#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""az-push-mod.py - load YOUR build into the running emulator in one command.

A Community Mod (the usual case: a folder with azmod.json, <id>.so, icon.png):

    python3 tools/az-push-mod.py path/to/my-mod/            the folder
    python3 tools/az-push-mod.py path/to/my-mod/azmod.json  the same
    python3 tools/az-push-mod.py my-mod/ --so out/my-mod.so a .so built elsewhere

  copies azmod.json, the .so and icon.png (if present) to
  /home/root/settings/az-usbload/community/<id>/, adds <id> to the `enabled` list,
  approves that build on the emulated deck (the core loads a community mod only when its
  own list holds the id with the SHA-256 of exactly that .so; pushing your build is that
  decision, so it does not ask "New mod found: allow?"), restarts the deck program and
  prints the mod's load result.

    python3 tools/az-push-mod.py my-mod/ --stick         also onto the emulated USB stick

  --stick also copies the mod, its assets/ included, to MODS/community/<id>/ on the USB
  stick the emulator has mounted. A mod's files live there on a deck (assets_dir() and
  data_dir() look for the stick that carries MODS/community/<id>/azmod.json). On firmware
  with the USB loader the copy on the stick is always made: the loader rebuilds the
  stage's community/ from the stick at every boot, as on a deck.

Your own build of the AZ-MODS core (contributors):

    python3 tools/az-push-mod.py --core out/azmod.so [out/azstem.so]

  puts them where a deck gets them, MODS/ on the emulated stick, and turns on the deck's
  developer switch (settings/azmods-unsigned.on): your build is not a signed set, and a
  loader that checks the set's signed trust pins (AZ-MODS 1.2.3 and later) loads it only
  with that switch. Then it runs the deck's own loader against the stick, as the next
  power cycle would (it stages the set and writes the env), and, when the loader leaves the
  set waiting with "Reload to apply.", reloads as the Mod Menu's Reload does. It prints
  the md5 of the azmod.so EP147 has mapped. This replaces the published set until you run
  tools/az-mods-fetch.py again (or boot without AZ_EMU_NOMODS=1).
  Firmware without the loader (a stock 2.00 system): the files go straight into the stage.

Every file is written as <name>.new and then renamed over the old one. Never copy
over a library the running program has loaded: changing a loaded file in place
changes its code under the program and crashes it.

  --no-restart    copy only (the next restart of EP147 picks it up)
"""
import argparse
import hashlib
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import azemu  # noqa: E402

STAGE = "/home/root/settings/az-usbload"
COMMUNITY = STAGE + "/community"
APPROVED = "/home/root/settings/azmod-community/approved"   # the deck's approved list: "<id> <sha256 of <id>.so>"
CM_STATE = "/home/root/settings/azmod-community"   # the core's per-mod state (cfg, crash suspension)


def check_elf_aarch64(path):
    with open(path, "rb") as f:
        h = f.read(20)
    if h[:4] != b"\x7fELF" or h[4] != 2 or int.from_bytes(h[18:20], "little") != 183:
        sys.exit(f"{path} is not a 64-bit ARM (aarch64) shared library. Build it with the SDK's "
                 "toolchain; a library built for your computer cannot load on the deck.")


def restart(check, label):
    t = azemu.restart_ep147(check=check)
    if t is None:
        sys.exit("EP147 did not come back within 180 s. Look at its log:\n"
                 "  tools/az-ssh 'journalctl -u EP147 -n 60 --no-pager'")
    print(f"EP147 restarted in {t:.0f} s with {label}")


def mounted_stick():
    """The emulated USB stick's mount point (/media/usb/...), or None."""
    rc, out = azemu.ssh(r"mount | sed -n 's|.* on \(/media/usb/[^ ]*\) type.*|\1|p' | head -1")
    return out.strip() or None


def push_to_stick(src, mid, files):
    """files: [(local, name in the mod folder, mode)] plus src/assets/** -> the stick."""
    mnt = mounted_stick()
    if not mnt:
        sys.exit("--stick: no USB stick is mounted in the emulator (is one attached?)")
    dst = f"{mnt}/MODS/community/{mid}"
    todo = [(local, f"{dst}/{name}", mode) for local, name, mode in files]
    adir = os.path.join(src, "assets")
    for root, dirs, names in os.walk(adir):
        dirs[:] = sorted(d for d in dirs if not d.startswith("."))
        for n in sorted(names):
            if n.startswith("."):
                continue
            rel = os.path.relpath(os.path.join(root, n), src).replace(os.sep, "/")
            todo.append((os.path.join(root, n), f"{dst}/{rel}", "644"))
    for local, remote, mode in todo:
        if not azemu.push_file(local, remote, mode):
            sys.exit(f"copy to {remote} failed")
    azemu.ssh(f"cd '{mnt}/MODS/community' && touch enabled && (grep -qxF '{mid}' enabled || echo '{mid}' >> enabled) && sync")
    n_assets = len(todo) - len(files)
    print(f"copied {mid} to the stick: {dst}/ ({n_assets} asset file{'s' if n_assets != 1 else ''})")


def push_community(src, so_override, restart_after, stick=False):
    if os.path.isfile(src) and src.endswith(".json"):
        src = os.path.dirname(os.path.abspath(src))
    mj = os.path.join(src, "azmod.json")
    if not os.path.isfile(mj):
        sys.exit(f"no azmod.json in {src}")
    try:
        mid = json.load(open(mj))["id"]
    except (ValueError, KeyError):
        sys.exit(f"{mj} has no \"id\"")
    so = so_override or next((p for p in (os.path.join(src, f"{mid}.so"),
                                          os.path.join(src, "out", f"{mid}.so")) if os.path.isfile(p)), None)
    if not so:
        sys.exit(f"cannot find {mid}.so in {src} or {src}/out (or pass --so)")
    check_elf_aarch64(so)
    # a core without Community Mods (1.2.2 and older) never loads it: say so before copying
    if azemu.ssh(f"test -f {STAGE}/azmod.so")[0] != 0:
        sys.exit("No AZ-MODS core is installed on the emulator. Install one first: "
                 "tools/az-mods-fetch.py, or your own build with --core path/to/azmod.so")
    if azemu.ssh(f"grep -q az_mod_v1_entry {STAGE}/azmod.so")[0] != 0:
        sys.exit(f"The AZ-MODS core on the emulator has no Community Mods, so it cannot load {mid}. "
                 "They need core 1.2.3 or newer: install a set that has them (tools/az-mods-fetch.py "
                 "once it is published, or a build with --core path/to/azmod.so path/to/azstem.so).")
    dst = f"{COMMUNITY}/{mid}"
    files = [(mj, f"{dst}/azmod.json", "644"), (so, f"{dst}/{mid}.so", "755")]
    icon = os.path.join(src, "icon.png")
    if os.path.isfile(icon):
        files.append((icon, f"{dst}/icon.png", "644"))
    for local, remote, mode in files:
        if not azemu.push_file(local, remote, mode):
            sys.exit(f"copy to {remote} failed (is the emulator running? tools/az-ssh true)")
    # enable it: one id per line in community/enabled
    azemu.ssh(f"cd {COMMUNITY} && touch enabled && (grep -qxF '{mid}' enabled || echo '{mid}' >> enabled) "
              f"&& rm -f '{dst}/{mid}.off'")
    # approve exactly this build (a later line for the id replaces an earlier one)
    with open(so, "rb") as f:
        digest = hashlib.sha256(f.read()).hexdigest()
    if azemu.ssh(f"mkdir -p {os.path.dirname(APPROVED)} && echo '{mid} {digest}' >> {APPROVED}")[0] != 0:
        sys.exit(f"could not approve {mid} on the emulator ({APPROVED})")
    print(f"copied {mid} to {dst}/, enabled and approved it")
    # a new build is a deliberate retry: lift a crash suspension, as Try again does on the deck
    if azemu.ssh(f"rm '{CM_STATE}/{mid}.crashed' 2>/dev/null")[0] == 0:
        print(f"{mid} was suspended after a crash; this build gets a fresh start")
    if stick or loader_image():
        push_to_stick(src, mid, [(local, os.path.basename(remote), mode) for local, remote, mode in files])
    if not restart_after:
        return
    # a restart you asked for, like the Mod Menu's Reload: not a start that died early, or two
    # pushes within 90 s would hold every community mod back (unless already held back)
    azemu.ssh(f"grep -q '^suspended=1' /tmp/azmod-community/loaded 2>/dev/null || rm -f '{CM_STATE}/.starting'")
    # the core rewrites its load list at every start; remove the last one first, or it
    # answers the check below before the new program has loaded anything
    azemu.ssh("rm -f /tmp/azmod-community/loaded")
    restart("test -s /tmp/azmod-community/loaded", f"{mid}")
    rc, out = azemu.ssh(f"grep -m1 '{mid}' /tmp/azmod-community/loaded; "
                        f"tail -5 /tmp/azmod-community/{mid}.log 2>/dev/null")
    print("load result:", out.strip() or f"(not in the core's load list: is {mid} in {COMMUNITY}/enabled?)")


def loader_image():
    return azemu.has_loader()


def push_core(paths, restart_after):
    loader = loader_image()
    want = {}
    for p in paths:
        name = os.path.basename(p)
        if name not in ("azmod.so", "azstem.so"):
            sys.exit(f"--core takes azmod.so and/or azstem.so, not {name}")
        check_elf_aarch64(p)
        want[name[:-3]] = hashlib.md5(open(p, "rb").read()).hexdigest()
        if loader:
            with open(p, "rb") as f:
                rc, out = azemu.ssh(f"az-emu-modsinstall {name}", data=f.read())
            if rc:
                sys.exit(f"copy of {name} to the stick failed (is a USB stick attached?)")
            print(f"copied {p} -> MODS/{name} on the stick ({want[name[:-3]]})")
        elif not azemu.push_file(p, f"{STAGE}/{name}", "755"):
            sys.exit(f"copy of {name} failed (is the emulator running? tools/az-ssh true)")
        else:
            print(f"copied {p} -> {STAGE}/{name}")
    if not loader:
        if restart_after:
            restart(f"grep -q {STAGE}/azmod.so /proc/$(pidof EP147)/maps", "your core build")
        return
    azemu.ssh(f"az-emu-modsinstall rm azmod-core.so; touch {azemu.UNSIGNED}; sync")
    print(f"developer switch ON ({azemu.UNSIGNED}): the deck's loader loads this unsigned "
          "build; tools/az-mods-fetch.py turns it off again")
    if not restart_after:
        print("--no-restart: the deck's loader stages it at the next boot; the Mod Menu's Reload "
              "(or that boot) loads it")
        return
    decision = azemu.run_loader()
    print("loader:", decision)
    how = azemu.apply_staged(decision)
    got = azemu.mapped()
    bad = {k: got.get(k) for k in want if got.get(k) != want[k]}
    if bad:
        sys.exit(f"EP147 maps {got}, not your build {want} ({how}); the loader's log: "
                 f"tools/az-ssh cat {azemu.LOADER_LOG}")
    print(f"EP147 runs your core build ({how}): " + ", ".join(f"{k} {v}" for k, v in want.items()))


def main():
    ap = argparse.ArgumentParser(description="Load your build into the running emulator.")
    ap.add_argument("path", nargs="*", help="a mod folder or azmod.json; with --core, the .so files")
    ap.add_argument("--so", help="the mod's .so, if it is not beside azmod.json or in out/")
    ap.add_argument("--core", action="store_true", help="push your own azmod.so/azstem.so build")
    ap.add_argument("--no-restart", action="store_true")
    ap.add_argument("--stick", action="store_true",
                    help="also copy the mod, assets/ included, to the emulated USB stick")
    a = ap.parse_args()
    if not a.path:
        ap.print_help()
        sys.exit(1)
    if a.core:
        push_core(a.path, not a.no_restart)
    else:
        push_community(a.path[0], a.so, not a.no_restart, a.stick)


if __name__ == "__main__":
    main()
