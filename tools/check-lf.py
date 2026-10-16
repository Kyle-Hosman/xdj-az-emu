#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""check-lf.py - fail loudly if any file that must have LF line endings has CRLF.

Shell scripts, patches, the Dockerfile and everything that ends up inside the guest
must be LF. .gitattributes enforces that on checkout; this catches a file edited or
copied in afterwards (for example by a Windows editor). Exit 1 lists every bad file.
"""
import os
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ALLOW_CRLF = (".ps1",)


def files():
    try:
        out = subprocess.run(["git", "-C", REPO, "ls-files", "-z", "--cached", "--others",
                              "--exclude-standard"], capture_output=True, check=True).stdout
        return [f for f in out.decode().split("\0") if f]
    except (OSError, subprocess.CalledProcessError):
        res = []
        for d, dirs, fs in os.walk(REPO):
            dirs[:] = [x for x in dirs if x not in (".git", "build", "target", "qemu")]
            res += [os.path.relpath(os.path.join(d, f), REPO) for f in fs]
        return res


def main():
    bad = []
    for f in files():
        p = os.path.join(REPO, f)
        if f.endswith(ALLOW_CRLF) or not os.path.isfile(p):
            continue
        with open(p, "rb") as fh:
            data = fh.read()
        if b"\0" in data[:8192]:
            continue                      # binary
        if b"\r\n" in data:
            bad.append(f)
    if bad:
        print("CRLF line endings found in files that must be LF:", file=sys.stderr)
        for f in bad:
            print("   ", f, file=sys.stderr)
        print("Fix: re-clone the repository (.gitattributes forces LF on a fresh checkout),\n"
              "or convert the files listed above back to LF in your editor.", file=sys.stderr)
        sys.exit(1)
    print("line endings OK")


if __name__ == "__main__":
    main()
