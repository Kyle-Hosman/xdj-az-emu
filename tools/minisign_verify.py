# SPDX-License-Identifier: GPL-3.0-only
"""Verify minisign signatures with no dependency (the same check the AZ-MODS app does).

The mods channel ships binaries the deck program will load. A sha256 pin in the
manifest proves the files are intact; only the signature proves the MANIFEST came
from the AZ-MODS release key. This verifies minisign's format (jedisct1/minisign):

    public key:  base64(b"Ed" + key_id(8) + pub(32))
    signature:   "untrusted comment: ...\\n" + base64(b"ED"|b"Ed" + key_id(8) + sig(64))
                 "trusted comment: <text>\\n" + base64(global_sig(64))
                 global_sig = ed25519 over (sig(64) || trusted-comment-text)
    "hashed" mode (the default of minisign >= 0.9; marked by "hashed" at the end of
    the tab-separated trusted comment) signs BLAKE2b-512 of the file.

Uses the `cryptography` package when installed, else the pure-Python Ed25519 below
(RFC 8032; about a second per verify). `python3 tools/minisign_verify.py` runs a
self-check against the RFC 8032 test vector.
"""
import base64
import binascii
import hashlib

try:
    from cryptography.exceptions import InvalidSignature
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
    _HAVE_CRYPTO = True
except ImportError:
    _HAVE_CRYPTO = False

_Q = 2 ** 255 - 19
_L = 2 ** 252 + 27742317777372353535851937790883648493
_D = (-121665 * pow(121666, _Q - 2, _Q)) % _Q
_I = pow(2, (_Q - 1) // 4, _Q)
_BY = (4 * pow(5, _Q - 2, _Q)) % _Q


def _xrecover(y):
    xx = ((y * y - 1) * pow(_D * y * y + 1, _Q - 2, _Q)) % _Q
    x = pow(xx, (_Q + 3) // 8, _Q)
    if (x * x - xx) % _Q != 0:
        x = (x * _I) % _Q
    if x % 2 != 0:
        x = _Q - x
    return x


_B = (_xrecover(_BY), _BY)


def _add(p, q):
    x1, y1 = p
    x2, y2 = q
    x3 = (x1 * y2 + x2 * y1) * pow(1 + _D * x1 * x2 * y1 * y2, _Q - 2, _Q)
    y3 = (y1 * y2 + x1 * x2) * pow(1 - _D * x1 * x2 * y1 * y2, _Q - 2, _Q)
    return (x3 % _Q, y3 % _Q)


def _mul(p, e):
    q = (0, 1)
    while e > 0:
        if e & 1:
            q = _add(q, p)
        p = _add(p, p)
        e >>= 1
    return q


def _encode(p):
    x, y = p
    b = bytearray(y.to_bytes(32, "little"))
    b[31] = (b[31] & 0x7F) | ((x & 1) << 7)
    return bytes(b)


def _decode(s):
    y = int.from_bytes(s, "little") & ((1 << 255) - 1)
    x = _xrecover(y)
    if x & 1 != (s[31] >> 7) & 1:
        x = _Q - x
    if (-x * x + y * y - 1 - _D * x * x * y * y) % _Q != 0:
        raise ValueError("point not on curve")
    return (x, y)


def _pure_verify(sig, data, pub):
    if len(sig) != 64 or len(pub) != 32:
        return False
    try:
        r = _decode(sig[:32])
        a = _decode(pub)
    except ValueError:
        return False
    s = int.from_bytes(sig[32:], "little")
    if s >= _L:
        return False
    k = int.from_bytes(hashlib.sha512(sig[:32] + pub + data).digest(), "little") % _L
    return _mul(_B, s) == _add(r, _mul(a, k))


def _ed_verify(sig, data, pub):
    if _HAVE_CRYPTO:
        try:
            Ed25519PublicKey.from_public_bytes(pub).verify(sig, data)
            return True
        except (InvalidSignature, ValueError):
            return False
    return _pure_verify(sig, data, pub)


def parse_pubkey(text):
    lines = [ln for ln in text.strip().splitlines() if ln.strip()]
    if not lines:
        raise ValueError("empty public key")
    b64 = lines[1] if len(lines) >= 2 else lines[0]
    blob = base64.b64decode(b64.strip(), validate=True)
    if len(blob) != 42 or blob[:2] != b"Ed":
        raise ValueError("not an Ed25519 minisign public key")
    return blob[2:10], blob[10:]


def verify(data, sig_text, pubkey_text):
    """True only if `data` carries a valid signature from `pubkey_text`. Any parse
    failure is False: refusal is the normal outcome of a tampered download."""
    try:
        kid, pub = parse_pubkey(pubkey_text)
    except (ValueError, binascii.Error):
        return False
    lines = [ln for ln in sig_text.strip().splitlines() if ln.strip()]
    if len(lines) < 2:
        return False
    try:
        blob = base64.b64decode(lines[1].strip(), validate=True)
    except (binascii.Error, ValueError):
        return False
    if len(blob) != 74 or blob[:2].lower() != b"ed" or blob[2:10] != kid:
        return False
    sig64 = blob[10:]
    if len(lines) > 2 and (not lines[2].startswith("trusted comment: ") or len(lines) < 4):
        return False
    hashed = len(lines) > 2 and lines[2].split()[-1:] == ["hashed"]
    vdata = hashlib.blake2b(data, digest_size=64).digest() if hashed else data
    if not _ed_verify(sig64, vdata, pub):
        return False
    if len(lines) > 2:
        comment = lines[2][len("trusted comment: "):]
        try:
            gsig = base64.b64decode(lines[3].strip(), validate=True)
        except (binascii.Error, ValueError):
            return False
        return _ed_verify(gsig, sig64 + comment.encode(), pub)
    return True


def _self_check():
    pub = binascii.unhexlify("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
    sig = binascii.unhexlify(
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155"
        "5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b")
    assert _pure_verify(sig, b"", pub), "pure Ed25519 rejects the RFC 8032 vector"
    assert not _pure_verify(sig, b"x", pub), "pure Ed25519 accepts a tampered message"
    assert _ed_verify(sig, b"", pub) and not _ed_verify(sig, b"x", pub)
    print(f"minisign self-check ok ({'cryptography' if _HAVE_CRYPTO else 'pure python'})")


if __name__ == "__main__":
    _self_check()
