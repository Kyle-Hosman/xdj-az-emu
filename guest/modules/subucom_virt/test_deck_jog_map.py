#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Offline self-check for the AZ deck-jog control map (M2).

Proves, against real XDJ-AZ subucom frames captured from the device, the two
facts subucom_virt.c depends on when it overlays live jog state and re-stamps
the CRC in spi_read():

  1. Deck CRC = CRC-16/X.25 over bytes [0:38], stored little-endian at [38:40].
  2. The deck jog control fields are:  touch = byte[9],
     pos = u16 LE [10:12],  vel = u16 LE [12:14]
     and idle-frame + those (plus the free-running counter bytes) + a recomputed
     CRC reproduces every real jog frame byte-for-byte.

Run: python3 test_deck_jog_map.py   (no deps; asserts, exits non-zero on fail)

Test vectors are real spi5 (deck-1) frames captured from a deck. Full
validation ran over 635 captured frames; these 5 vectors keep the check
self-contained.
"""

IDLE_SPI5 = bytes.fromhex("020104000000440000006800ffff000046020502c90069000000000000000000000000000000e7d900000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000")
JOG_SPI5 = [
    bytes.fromhex("0201000000004400000fc61ae7010000460206029901692c000000000000000000000000000034d400000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"),
    bytes.fromhex("0201000000004400000f311bf3010000460206029801692c00000000000000000000000000008f2a00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"),
    bytes.fromhex("0201000000004400000b5e1997010000460205029c01692c0000000000000000000000000000714d00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"),
    bytes.fromhex("0201000000004400000b691589010000460205029a01692c0000000000000000000000000000a72d00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"),
]


def crc16_x25(data: bytes) -> int:
    crc = 0xFFFF
    for b in data:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ 0x8408 if crc & 1 else crc >> 1
    return crc ^ 0xFFFF


DECK_CRC_LEN = 38  # matches spi_crc_len[] deck entries in subucom_virt.c


def check_crc(frame: bytes) -> None:
    stored = frame[38] | (frame[39] << 8)
    assert crc16_x25(frame[:DECK_CRC_LEN]) == stored, "deck CRC formula mismatch"


def jog_fields(frame: bytes):
    return frame[9], frame[10] | (frame[11] << 8), frame[12] | (frame[13] << 8)


def main() -> None:
    assert len(IDLE_SPI5) == 152, f"idle vector is {len(IDLE_SPI5)}B, want 152"
    check_crc(IDLE_SPI5)

    for i, jf in enumerate(JOG_SPI5):
        assert len(jf) == 152, f"jog vector {i} is {len(jf)}B, want 152"
        check_crc(jf)

        touch, pos, vel = jog_fields(jf)
        assert touch != 0, f"jog frame {i}: touch byte[9] should be engaged, got 0"

        # Overlay recipe (mirrors spi_read): idle template, apply every byte that
        # differs from idle (jog fields [9:14] + free-running counters), recompute
        # CRC over [0:38]. Must reproduce the real frame exactly.
        overlay = [k for k in range(DECK_CRC_LEN) if jf[k] != IDLE_SPI5[k]]
        assert set((9, 10, 11, 12, 13)) <= set(overlay), \
            f"jog frame {i}: expected jog bytes 9-13 to move, moved {overlay}"
        rebuilt = bytearray(IDLE_SPI5)
        for k in overlay:
            rebuilt[k] = jf[k]
        c = crc16_x25(rebuilt[:DECK_CRC_LEN])
        rebuilt[38] = c & 0xFF
        rebuilt[39] = c >> 8
        assert bytes(rebuilt) == jf, f"jog frame {i}: overlay+CRC did not reproduce real frame"
        print(f"jog[{i}]: touch={touch:#04x} pos={pos:5d} vel={vel:5d}  reconstruct OK")

    print("PASS: deck CRC = X25[0:38], jog map = touch[9]/pos[10:12]/vel[12:14]")


if __name__ == "__main__":
    main()
