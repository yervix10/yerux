#!/usr/bin/env python3
"""Emulate `yerux --target aarch64` output and compare it with the goldens / the x86-64 backend.

usage: python3 tests/a64_emu.py [--network] [--debug-alloc] [-j N] [-k SUBSTR] [files.yx ...]
  default: every examples/*.yx and tests/*.yx that the native backend accepts (the e*.yx error programs are
  skipped). Per program the emulated aarch64 build must match tests/expected/<name>.out/.code (stdout, exit code)
  and the x86-64 native build's stderr; every code word must also decode in capstone.
  --network : run just the socket tests, using qemu-aarch64 for real Linux TCP.
  --debug-alloc : build with --native-debug-alloc and compare stderr (allocs/frees/live) with the x86-64 build.
Needs: `cargo build --release` first. The socket tests need qemu-aarch64 on PATH.
Other tests need `pip install unicorn capstone`."""
import argparse, os, shutil, struct, subprocess, sys, tempfile
from concurrent.futures import ProcessPoolExecutor

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
YERUX = os.path.join(ROOT, "target/release/yerux")
STACK = 0x7F000000
MMAP_BASE = 0x10000000
MAX_INSNS = 400_000_000
NETWORK = ("net_errors", "net_hostile_address", "net_read_timeout", "net_timeouts")
SHRINK = {"02_fast": ("100000000", "20000"), "own_deep_box_drop": ("return 1000000", "return 2000"), "own_deep_scc_drop": ("return 1000000", "return 2000"), "own_deep_closure_drop": ("return 1000000", "return 2000")}  # loops too long for emulation


def load_elf(elf):
    assert elf[:4] == b"\x7fELF" and struct.unpack_from("<H", elf, 18)[0] == 183, "not an EM_AARCH64 ELF"
    entry = struct.unpack_from("<Q", elf, 24)[0]
    phnum = struct.unpack_from("<H", elf, 56)[0]
    segs = []
    for i in range(phnum):
        t, fl, off, vaddr, _, filesz, memsz, _ = struct.unpack_from("<IIQQQQQQ", elf, 64 + 56 * i)
        if t == 1:
            segs.append((vaddr, elf[off:off + filesz], memsz, fl))
    return entry, segs


def disasm_check(elf, entry, segs):
    """Every word of the code region must decode; the data blobs follow the code (first segment)."""
    from capstone import Cs, CS_ARCH_ARM64, CS_MODE_ARM

    vaddr, seg, _, _ = segs[0]
    md = Cs(CS_ARCH_ARM64, CS_MODE_ARM)
    code_end = seg.index(b"panic: ") & ~15
    bad, n = [], 0
    for i in range(120 + 56, code_end, 4):
        w = seg[i:i + 4]
        if list(md.disasm(w, vaddr + i)):
            n += 1
        elif w != b"\0\0\0\0":
            bad.append((hex(vaddr + i), w.hex()))
    return n, bad


def emulate(elf, max_insns=MAX_INSNS):
    from unicorn import Uc, UC_ARCH_ARM64, UC_MODE_ARM, UC_HOOK_INTR, UcError
    from unicorn.arm64_const import (
        UC_ARM64_REG_PC, UC_ARM64_REG_SP, UC_ARM64_REG_X0, UC_ARM64_REG_X1,
        UC_ARM64_REG_X2, UC_ARM64_REG_X3, UC_ARM64_REG_X8,
    )

    entry, segs = load_elf(elf)
    uc = Uc(UC_ARCH_ARM64, UC_MODE_ARM)
    for vaddr, data, memsz, _ in segs:
        lo = vaddr & ~0xFFF
        size = ((vaddr + memsz + 0xFFF) & ~0xFFF) - lo
        uc.mem_map(lo, size)
        uc.mem_write(vaddr, data)
    uc.mem_map(STACK - 0x800000, 0x800000)  # 8 MiB, like the default Linux stack
    uc.reg_write(UC_ARM64_REG_SP, STACK - 0x100)
    out = {1: b"", 2: b""}
    st = {"exit": None, "next": MMAP_BASE, "maps": {}}

    def intr(uc, no, ud):
        x8 = uc.reg_read(UC_ARM64_REG_X8)
        x0 = uc.reg_read(UC_ARM64_REG_X0)
        x1 = uc.reg_read(UC_ARM64_REG_X1)
        if x8 == 64:  # write
            n = uc.reg_read(UC_ARM64_REG_X2)
            out[x0] += bytes(uc.mem_read(x1, n))
            uc.reg_write(UC_ARM64_REG_X0, n)
        elif x8 in (93, 94):  # exit, exit_group
            st["exit"] = x0 & 0xFF
            uc.emu_stop()
        elif x8 == 222:  # mmap(addr=0, len, prot, flags, -1, 0): anonymous, zero-filled
            assert x0 == 0 and uc.reg_read(UC_ARM64_REG_X3) == 0x22, "unexpected mmap arguments"
            size = (x1 + 0xFFF) & ~0xFFF
            addr = st["next"]
            st["next"] += size + 0x1000  # guard page between mappings
            uc.mem_map(addr, size)
            st["maps"][addr] = size
            uc.reg_write(UC_ARM64_REG_X0, addr)
        elif x8 == 215:  # munmap(addr, len): must be exactly a live mapping
            size = st["maps"].pop(x0, None)
            if size is None or size != ((x1 + 0xFFF) & ~0xFFF):
                raise RuntimeError(f"bad munmap {x0:#x} {x1:#x}")
            uc.mem_unmap(x0, size)
            uc.reg_write(UC_ARM64_REG_X0, 0)
        elif x8 == 216:  # mremap(old, old_len, new_len, MREMAP_MAYMOVE): always moves, contents kept
            new_len = uc.reg_read(UC_ARM64_REG_X2)
            assert uc.reg_read(UC_ARM64_REG_X3) == 1, "unexpected mremap flags"
            size = st["maps"].pop(x0, None)
            if size is None or size != ((x1 + 0xFFF) & ~0xFFF) or new_len < x1:
                raise RuntimeError(f"bad mremap {x0:#x} {x1:#x} {new_len:#x}")
            data = bytes(uc.mem_read(x0, size))
            uc.mem_unmap(x0, size)
            nsize = (new_len + 0xFFF) & ~0xFFF
            addr = st["next"]
            st["next"] += nsize + 0x1000
            uc.mem_map(addr, nsize)
            uc.mem_write(addr, data)
            st["maps"][addr] = nsize
            uc.reg_write(UC_ARM64_REG_X0, addr)
        else:
            raise RuntimeError(f"unexpected syscall {x8}")

    uc.hook_add(UC_HOOK_INTR, intr)
    try:
        uc.emu_start(entry, 0, count=max_insns)
    except (UcError, RuntimeError) as e:
        return None, out[1], out[2], f"emulation error: {e} pc={uc.reg_read(UC_ARM64_REG_PC):#x}"
    if st["exit"] is None:
        return None, out[1], out[2], "instruction limit reached"
    return st["exit"], out[1], out[2], None


def build(src, exe, extra):
    return subprocess.run([YERUX, "build", src, "--native", "-o", exe] + extra, capture_output=True)


def run_x64(src, tmp, extra):
    exe = os.path.join(tmp, "x64.bin")
    p = build(src, exe, extra)
    if p.returncode != 0:
        return None
    r = subprocess.run([exe], capture_output=True, stdin=subprocess.DEVNULL, timeout=120)
    return r.returncode, r.stdout, r.stderr


def a64_run(src, tmp, extra, check_decode=True, qemu=None):
    """Build `src` for aarch64 and emulate it. Returns (code, stdout, stderr, err, decode_report)."""
    path = os.path.join(tmp, "out.a64")
    p = build(src, path, ["--target", "aarch64"] + extra)
    if p.returncode != 0:
        return None, b"", p.stderr, "aarch64 build failed: " + p.stderr.decode(errors="replace")[:300], None
    if qemu:
        try:
            r = subprocess.run([qemu, path], capture_output=True, stdin=subprocess.DEVNULL, timeout=45)
        except subprocess.TimeoutExpired:
            return None, b"", b"", "QEMU timed out", None
        return r.returncode, r.stdout, r.stderr, None, None
    elf = open(path, "rb").read()
    rep = None
    if check_decode:
        entry, segs = load_elf(elf)
        rep = disasm_check(elf, entry, segs)
    ec, so, se, err = emulate(elf)
    return ec, so, se, err, rep


def check_one(args):
    f, debug = args
    name = os.path.basename(f)[:-3]
    network = name in NETWORK
    qemu = shutil.which("qemu-aarch64") if network else None
    if network and qemu is None:
        return f, "SKIP", "qemu-aarch64 unavailable"
    with tempfile.TemporaryDirectory() as tmp:
        src = os.path.join(ROOT, f)
        if name in SHRINK:
            small = os.path.join(tmp, name + ".yx")
            open(small, "w").write(open(src).read().replace(*SHRINK[name]))
            src = small
        extra = ["--native-debug-alloc"] if debug else []
        x = run_x64(src, tmp, extra)
        if x is None:
            if network:
                return f, "FAIL", "x86-64 build failed"
            return f, "SKIP", "native backend rejects it"
        ec, so, se, err, rep = a64_run(src, tmp, extra, qemu=qemu)
        xc, xo, xe = x
        if err and not network and "is not supported on the aarch64 target yet" in err:
            return f, "SKIP", "aarch64 target rejects it (documented limitation)"
        if err:
            return f, "FAIL", err
        detail = "QEMU"
        if rep:
            n, bad = rep
            if bad:
                return f, "FAIL", f"{len(bad)} undecodable words, first {bad[:3]}"
            detail = f"{n} insns decoded"
        if debug:
            gold_o, gold_c = xo, xc
        else:
            go, gc = os.path.join(ROOT, "tests/expected", name + ".out"), os.path.join(ROOT, "tests/expected", name + ".code")
            if name in SHRINK or not os.path.exists(go):
                gold_o, gold_c = xo, xc  # no usable golden: fall back to the x86-64 build
            else:
                gold_o, gold_c = open(go, "rb").read(), int(open(gc).read())
        why = []
        if ec != gold_c: why.append(f"exit {ec} != {gold_c}")
        if so != gold_o: why.append("stdout differs")
        if se != xe: why.append(f"stderr differs: a64={se[:80]!r} x64={xe[:80]!r}")
        if network and (xo, xc) != (gold_o, gold_c):
            why.append("x86-64 differs from golden")
        return f, ("FAIL" if why else "PASS"), "; ".join(why) or detail


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="*")
    ap.add_argument("--network", action="store_true", help="run socket tests under QEMU")
    ap.add_argument("--debug-alloc", action="store_true")
    ap.add_argument("-j", type=int, default=os.cpu_count() or 4)
    ap.add_argument("-k", default="")
    a = ap.parse_args()
    files = [f"tests/{name}.yx" for name in NETWORK] if a.network else a.files
    if not files:
        for d in ("examples", "tests"):
            for fn in sorted(os.listdir(os.path.join(ROOT, d))):
                if fn.endswith(".yx") and not (d == "examples" and fn[0] == "e" and fn[1].isdigit()):
                    files.append(f"{d}/{fn}")
    files = [f for f in files if a.k in f]
    with ProcessPoolExecutor(a.j) as ex:
        res = list(ex.map(check_one, [(f, a.debug_alloc) for f in files]))
    fails = 0
    for f, st, msg in res:
        if st != "SKIP" or a.network:
            print(f"{st:5} {f}: {msg}")
        fails += st == "FAIL"
    cnt = {s: sum(1 for r in res if r[1] == s) for s in ("PASS", "FAIL", "SKIP")}
    print(f"a64 emulation: {cnt['PASS']} pass, {cnt['FAIL']} fail, {cnt['SKIP']} skipped")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
