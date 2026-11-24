#!/usr/bin/env python3
"""Structural check of a PE32+ file written by `yerux build --native --target windows` (no Windows needed).

  python3 tests/pe_check.py prog.exe

Checks: ASLR flags (DYNAMIC_BASE | HIGH_ENTROPY_VA | NX_COMPAT) and a base relocation directory; an exception
directory (.pdata) whose RUNTIME_FUNCTION entries are sorted, inside .text and non-overlapping, each pointing at a
well-formed UNWIND_INFO in .xdata (version 1, codes within the prologue, frame register rbp when there are codes);
and that .text holds no `movabs reg, <image address>` (the old fixed-base heap-state loads).
Exit 0 and one summary line, or exit 1 with the reason.
"""
import struct, sys

def fail(msg):
    print("pe_check: FAIL: " + msg)
    sys.exit(1)

d = open(sys.argv[1], "rb").read()
if d[:2] != b"MZ":
    fail("no MZ header")
pe = struct.unpack_from("<I", d, 0x3C)[0]
if d[pe:pe + 4] != b"PE\0\0":
    fail("no PE signature")
nsec = struct.unpack_from("<H", d, pe + 6)[0]
opt = pe + 24
if struct.unpack_from("<H", d, opt)[0] != 0x20B:
    fail("not PE32+")
image_base = struct.unpack_from("<Q", d, opt + 24)[0]
dllchar = struct.unpack_from("<H", d, opt + 70)[0]
for bit, name in ((0x40, "DYNAMIC_BASE"), (0x20, "HIGH_ENTROPY_VA"), (0x100, "NX_COMPAT")):
    if not dllchar & bit:
        fail(name + " not set (DllCharacteristics=%#x)" % dllchar)
coff_chars = struct.unpack_from("<H", d, pe + 22)[0]
if coff_chars & 1:
    fail("RELOCS_STRIPPED is set")
dd = opt + 112
def ddir(i):
    return struct.unpack_from("<II", d, dd + 8 * i)
secs = {}
off = opt + struct.unpack_from("<H", d, pe + 20)[0]
for i in range(nsec):
    s = d[off + 40 * i: off + 40 * i + 40]
    name = s[:8].rstrip(b"\0").decode()
    vsize, rva, rawsz, rawptr = struct.unpack_from("<IIII", s, 8)
    secs[name] = (rva, vsize, rawptr, rawsz)
for need in (".text", ".pdata", ".xdata", ".reloc"):
    if need not in secs:
        fail("missing section " + need)
def rva_off(rva):
    for (r, v, p, sz) in secs.values():
        if r <= rva < r + max(v, sz):
            return p + rva - r
    fail("RVA %#x is in no section" % rva)
rel_rva, rel_size = ddir(5)
if rel_rva != secs[".reloc"][0] or rel_size < 8:
    fail("base relocation directory missing")
blk = rva_off(rel_rva)
if struct.unpack_from("<I", d, blk + 4)[0] < 8:
    fail("bad relocation block")
pd_rva, pd_size = ddir(3)
if pd_rva != secs[".pdata"][0] or pd_size == 0 or pd_size % 12:
    fail("exception directory missing or misaligned")
t_rva, t_vsize, t_ptr, t_raw = secs[".text"]
x_rva, x_vsize, _, _ = secs[".xdata"]
n = pd_size // 12
last_end = 0
framed = 0
for i in range(n):
    b, e, u = struct.unpack_from("<III", d, rva_off(pd_rva) + 12 * i)
    if not (t_rva <= b < e <= t_rva + t_vsize):
        fail("entry %d outside .text: %#x-%#x" % (i, b, e))
    if b < last_end:
        fail("entry %d overlaps or is unsorted" % i)
    last_end = e
    if not (x_rva <= u < x_rva + x_vsize) or u % 4:
        fail("entry %d: unwind info %#x outside .xdata or unaligned" % (i, u))
    o = rva_off(u)
    ver_flags, prolog, ncodes, frame = d[o], d[o + 1], d[o + 2], d[o + 3]
    if ver_flags != 1:
        fail("entry %d: unwind version/flags %#x" % (i, ver_flags))
    if ncodes == 0:
        if prolog or frame:
            fail("entry %d: frameless info with a prologue or frame register" % i)
        continue
    framed += 1
    if frame & 0xF != 5:
        fail("entry %d: frame register is not rbp" % i)
    k, ops = 0, []
    while k < ncodes:
        off_in_prolog, ob = d[o + 4 + 2 * k], d[o + 5 + 2 * k]
        op, info = ob & 0xF, ob >> 4
        if off_in_prolog > prolog:
            fail("entry %d: code offset %d beyond the prologue (%d)" % (i, off_in_prolog, prolog))
        ops.append(op)
        k += {1: 3 if info else 2}.get(op, 1)
    if ops[-2:] != [3, 0]:
        fail("entry %d: expected SET_FPREG then PUSH_NONVOL last, got %s" % (i, ops))
# absolute heap-state loads of the old fixed-base scheme: movabs r, 0x14xxxxxxx
text = d[t_ptr:t_ptr + t_raw]
for k in range(len(text) - 10):
    if text[k] in (0x48, 0x49) and 0xB8 <= text[k + 1] <= 0xBF and struct.unpack_from("<Q", text, k + 2)[0] >> 32 == image_base >> 32:
        fail("absolute address (movabs %#x) in .text at +%#x" % (struct.unpack_from("<Q", text, k + 2)[0], k))
print("pe_check: ok: %d functions (%d framed), DYNAMIC_BASE+HIGH_ENTROPY_VA+NX_COMPAT, empty .reloc, %d bytes" % (n, framed, len(d)))
