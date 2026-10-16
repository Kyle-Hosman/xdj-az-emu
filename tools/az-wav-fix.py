#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""az-wav-fix.py - make an emulator audio recording readable.

QEMU's wav backend writes the RIFF and data lengths as 0 and does not always fill
them in on exit, so some players refuse the file or play nothing. This sets both
lengths from the file's size. It is safe on a recording that is still growing: the
result covers everything written so far.

    python3 tools/az-wav-fix.py recording.wav [more.wav ...]
"""
import os
import struct
import sys

HEADER = 44


def fix(path):
    size = os.path.getsize(path)
    with open(path, "r+b") as f:
        head = f.read(HEADER)
        if len(head) < HEADER or head[0:4] != b"RIFF" or head[8:12] != b"WAVE" or head[36:40] != b"data":
            print(f"{path}: not a QEMU WAV recording, left alone", file=sys.stderr)
            return False
        channels, rate = struct.unpack("<HI", head[22:28])
        block = struct.unpack("<H", head[32:34])[0]
        data = (size - HEADER) // block * block
        f.seek(4)
        f.write(struct.pack("<I", HEADER - 8 + data))
        f.seek(40)
        f.write(struct.pack("<I", data))
    print(f"{path}: {data / (rate * block):.1f} s, {channels} channels, {rate} Hz")
    return True


def main():
    if len(sys.argv) < 2:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    return 0 if all([fix(p) for p in sys.argv[1:]]) else 1


if __name__ == "__main__":
    sys.exit(main())
