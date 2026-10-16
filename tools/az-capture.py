#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""az-capture.py - record exactly what the decks play, sample for sample.

The deck program's audio (each deck, before the mixer) is streamed out of the guest as it is rendered,
unpadded, so the recording is complete however fast or slow the host runs the
emulator (unlike AZ_AUDIO=file..., which runs on the host's clock and has gaps when
the guest falls behind real time, as it does under software emulation on Windows).
Nothing is stored inside the emulator: the WAV is written on this computer.

    python3 tools/az-capture.py start [--mix] [out.wav]
    python3 tools/az-capture.py stop
    python3 tools/az-capture.py status

The file goes to audio-out-captures/ in this repository: capture-<date_time>.wav, or
the name you give; an absolute path is used as given. It is 32-bit PCM at 44.1 kHz,
the decks' own samples, with 8 channels in this order: deck 1 left, deck 1 right,
deck 2 left, deck 2 right, deck 3 left, deck 3 right, deck 4 left, deck 4 right
(about 1.4 MB a second). With --mix, channels 9 and 10 are the emulator's
approximate mix (the same as the speakers or file output; see docs/guide.md).
"""
import json
import os
import struct
import subprocess
import sys
import time
from datetime import datetime

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import azemu  # noqa: E402

CAPTURES = os.path.join(azemu.REPO, "audio-out-captures")
FIFO = "/tmp/master_out.capture"
INFO = "/tmp/master_out.capture.txt"
MIX_FLAG = "/tmp/master_out.capture.mix"
STATE = os.path.join(azemu.SOCK_DIR, "capture.json")
RATE = 44100
PCM_GUID = bytes.fromhex("0100000000001000800000aa00389b71")


def wav_header(data_bytes, channels):
    """WAVE_FORMAT_EXTENSIBLE, 32-bit PCM, no speaker positions (the channels are
    decks, in the order in the docstring)."""
    block = 4 * channels
    data_bytes = min(data_bytes, 0xFFFFFFFF - 60)
    fmt = struct.pack("<HHIIHHHHI", 0xFFFE, channels, RATE, RATE * block, block, 32, 22, 32, 0) + PCM_GUID
    return (b"RIFF" + struct.pack("<I", 4 + 8 + len(fmt) + 8 + data_bytes) + b"WAVE" +
            b"fmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", data_bytes))


def guest_info():
    _, out = azemu.ssh(f"cat {INFO} 2>/dev/null")
    return dict(line.split("=", 1) for line in out.split() if "=" in line)


def read_state():
    try:
        with open(STATE) as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def pump(out_path, channels):
    """Runs detached: copies the guest's FIFO into out_path until the guest closes it."""
    block = 4 * channels
    with open(out_path, "wb") as f:
        head = len(wav_header(0, channels))
        f.write(wav_header(0, channels))
        p = subprocess.Popen(azemu._ssh_base() + [f"cat {FIFO}"], stdout=subprocess.PIPE)
        total = 0
        while True:
            chunk = p.stdout.read(65536)
            if not chunk:
                break
            f.write(chunk)
            total += len(chunk)
        p.wait()
        total -= total % block
        f.truncate(head + total)
        f.seek(0)
        f.write(wav_header(total, channels))
    st = read_state() or {}
    st.update(done=True, bytes=total)
    with open(STATE, "w") as f:
        json.dump(st, f)


def start(name, mix):
    st = read_state()
    if st and not st.get("done"):
        sys.exit(f"A capture is already running into {st['path']}. Stop it first.")
    _, out = azemu.ssh("grep -q master_out.so /proc/$(pidof EP147)/maps && echo ok")
    if "ok" not in out:
        sys.exit("The deck program is not running with master_out.so. Rebuild the image (./build.sh).")
    name = name or datetime.now().strftime("capture-%Y-%m-%d_%H-%M-%S.wav")
    path = name if os.path.isabs(name) else os.path.join(CAPTURES, name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    channels = 10 if mix else 8
    azemu.ssh(f"rm -f {FIFO} {INFO} {MIX_FLAG}; {'touch ' + MIX_FLAG + '; ' if mix else ''}mkfifo {FIFO}")
    os.makedirs(azemu.SOCK_DIR, exist_ok=True)
    with open(STATE, "w") as f:
        json.dump({"path": path, "done": False, "channels": channels}, f)
    kw = {"stdin": subprocess.DEVNULL, "stdout": subprocess.DEVNULL, "stderr": subprocess.DEVNULL}
    if azemu.WIN:
        kw["creationflags"] = 0x00000008 | 0x00000200  # DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    else:
        kw["start_new_session"] = True
    subprocess.Popen([sys.executable, os.path.abspath(__file__), "_pump", path, str(channels)], **kw)
    for _ in range(40):
        time.sleep(0.25)
        info = guest_info()
        if info.get("state") == "recording":
            if info.get("channels") != str(channels):
                azemu.ssh(f"rm -f {FIFO}")
                sys.exit("The deck program's master_out.so is too old for this capture. Rebuild the image (./build.sh).")
            print(f"Recording to {path}\nStop with: python3 tools/az-capture.py stop")
            return 0
    azemu.ssh(f"rm -f {FIFO}")
    sys.exit("The capture did not start (no status from master_out.so).")


def stop():
    st = read_state()
    if not st or st.get("done"):
        sys.exit("No capture is running.")
    azemu.ssh(f"rm -f {FIFO}")
    # The guest closes the stream once its writer notices; under software emulation
    # (Windows) that can take well over 20 s, so wait up to 2 minutes for the pump.
    for _ in range(480):
        time.sleep(0.25)
        st = read_state()
        if st and st.get("done"):
            break
    else:
        sys.exit(f"The capture did not finish; {st['path']} may be incomplete.")
    info = guest_info()
    ch = st.get("channels", 8)
    print(f"{st['path']}: {st['bytes'] / (RATE * 4 * ch):.1f} s, {ch} channels, 32-bit 44.1 kHz")
    lost = int(info.get("dropped", "0"))
    if lost:
        print(f"WARNING: {lost} frames were lost (the guest fell too far behind); the recording has gaps.")
    return 0


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    if cmd == "_pump":
        pump(sys.argv[2], int(sys.argv[3]))
        return 0
    if cmd == "start":
        args = sys.argv[2:]
        mix = "--mix" in args
        names = [a for a in args if a != "--mix"]
        return start(names[0] if names else None, mix)
    if cmd == "stop":
        return stop()
    if cmd == "status":
        st = read_state()
        print(st or "No capture.", guest_info() or "")
        return 0
    print(__doc__.strip(), file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
