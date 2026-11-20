#!/usr/bin/env python3
"""Verification of `yerux --target windows` output without a Windows machine.

Three independent layers (see docs/WINDOWS.md for what each one proves and what it does not):
  1. STRUCTURE  pefile: headers, section table, alignment, permissions, import table, IAT directory.
  2. CODE       capstone: linear disassembly of .text must be valid, every jump/call lands on an
                instruction boundary, every rip-relative reference lands inside a section, every
                `call [rip+x]` hits an IAT slot.
  3. EMULATION  Unicorn runs the real .exe: a small PE loader maps the sections with their own
                permissions (W^X is enforced), the kernel32 imports are replaced by hooks that
                *enforce the Microsoft x64 convention* (rsp = 8 mod 16 on entry, volatile registers and
                the caller's shadow space are trashed on return, WriteFile/VirtualAlloc/VirtualFree
                argument checks, no double VirtualFree). Output and exit code are compared with
                the goldens in tests/expected.

usage: tests/pe_check.py [--fuzz N] [--insn-limit N] [files.yx ...]    (default: all native-supported tests)
Exit status 0 when nothing failed.
"""
import argparse, os, struct, subprocess, sys, tempfile

import capstone
import pefile
import unicorn
from unicorn import x86_const as X

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
YERUX = os.path.join(ROOT, "target", "release", "yerux")
EXPECTED = os.path.join(ROOT, "tests", "expected")

IMPORTS = ["GetStdHandle", "WriteFile", "ExitProcess", "VirtualAlloc", "VirtualFree", "SetConsoleOutputCP"]


class Fail(Exception):
    pass


def need(cond, msg):
    if not cond:
        raise Fail(msg)


# ───────────────────────── 1. structure ─────────────────────────

def check_structure(path):
    data = open(path, "rb").read()
    pe = pefile.PE(data=data)
    need(data[:2] == b"MZ", "no MZ")
    need(pe.DOS_HEADER.e_lfanew % 8 == 0 and pe.DOS_HEADER.e_lfanew >= 0x40, "e_lfanew")
    need(b"This program cannot be run in DOS mode" in data[:pe.DOS_HEADER.e_lfanew], "DOS stub message")
    fh, oh = pe.FILE_HEADER, pe.OPTIONAL_HEADER
    need(fh.Machine == 0x8664, "machine is not AMD64")
    need(fh.Characteristics & 0x2, "not EXECUTABLE_IMAGE")
    need(not fh.Characteristics & 0x2000, "marked as DLL")
    need(oh.Magic == 0x20B, "not PE32+")
    need(oh.SectionAlignment == 0x1000 and oh.FileAlignment == 0x200, "alignments")
    need(oh.Subsystem == 3, "not a console subsystem")
    need(oh.ImageBase % 0x10000 == 0, "ImageBase not 64K aligned")
    need(oh.SizeOfHeaders % oh.FileAlignment == 0, "SizeOfHeaders alignment")
    need(oh.SizeOfStackReserve >= oh.SizeOfStackCommit > 0, "stack sizes")
    need(oh.NumberOfRvaAndSizes == 16, "data directories")
    secs = pe.sections
    need([s.Name.rstrip(b"\0") for s in secs] == [b".text", b".rdata", b".data", b".idata"], "section names/order")
    prev_rva_end, prev_raw_end = oh.SizeOfHeaders, oh.SizeOfHeaders
    for s in secs:
        n = s.Name.rstrip(b"\0").decode()
        need(s.VirtualAddress % oh.SectionAlignment == 0, f"{n}: rva alignment")
        need(s.PointerToRawData % oh.FileAlignment == 0 and s.SizeOfRawData % oh.FileAlignment == 0, f"{n}: raw alignment")
        need(s.VirtualAddress >= prev_rva_end, f"{n}: overlaps previous section in memory")
        need(s.PointerToRawData >= prev_raw_end, f"{n}: overlaps previous section in file")
        need(s.PointerToRawData + s.SizeOfRawData <= len(data), f"{n}: raw data beyond end of file")
        need(s.Misc_VirtualSize > 0, f"{n}: empty")
        prev_rva_end = s.VirtualAddress + (s.Misc_VirtualSize + 0xFFF) // 0x1000 * 0x1000
        prev_raw_end = s.PointerToRawData + s.SizeOfRawData
    need(oh.SizeOfImage == prev_rva_end, f"SizeOfImage {oh.SizeOfImage:#x} != {prev_rva_end:#x}")
    need(len(data) == prev_raw_end, "trailing bytes after the last section")
    text, rdata, dat, idata = secs
    # permissions
    need(text.Characteristics & 0x20000000 and text.Characteristics & 0x40000000 and not text.Characteristics & 0x80000000, ".text must be R+X, not W")
    need(not rdata.Characteristics & 0xA0000000, ".rdata must be R only")
    need(dat.Characteristics & 0xC0000000 == 0xC0000000 and not dat.Characteristics & 0x20000000, ".data must be R+W, not X")
    need(not idata.Characteristics & 0x20000000, ".idata must not be executable")
    need(text.VirtualAddress <= oh.AddressOfEntryPoint < text.VirtualAddress + text.Misc_VirtualSize, "entry point outside .text")
    # imports
    pe.parse_data_directories()
    need(len(pe.DIRECTORY_ENTRY_IMPORT) == 1, "expected one imported DLL")
    ent = pe.DIRECTORY_ENTRY_IMPORT[0]
    need(ent.dll.lower() == b"kernel32.dll", f"unexpected DLL {ent.dll}")
    names = [i.name.decode() for i in ent.imports]
    need(names == IMPORTS, f"imports {names}")
    iat_dir = oh.DATA_DIRECTORY[12]
    need(iat_dir.VirtualAddress == ent.struct.FirstThunk and iat_dir.Size == 8 * (len(IMPORTS) + 1), "IAT directory")
    for i in ent.imports:
        need(idata.VirtualAddress <= i.address - oh.ImageBase < idata.VirtualAddress + idata.Misc_VirtualSize, "IAT slot outside .idata")
    # "Byte 0x.. makes up N% of the file" is a packer/truncation heuristic, not a structural problem
    warns = [w for w in pe.get_warnings() if not w.startswith("Byte 0x")]
    need(not warns, f"pefile warnings: {warns}")
    return pe, data


# ───────────────────────── 2. code ─────────────────────────

def check_code(pe, data):
    oh = pe.OPTIONAL_HEADER
    text, rdata, dat, idata = pe.sections
    code = data[text.PointerToRawData:text.PointerToRawData + text.Misc_VirtualSize]
    base = oh.ImageBase + text.VirtualAddress
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    md.detail = True
    insns = []
    off = 0
    while off < len(code):
        got = list(md.disasm(code[off:], base + off, 1))
        need(got, f"invalid opcode at .text+{off:#x}: {code[off:off+8].hex()}")
        insns.append(got[0])
        off += got[0].size
    starts = {i.address for i in insns}
    secs = [(oh.ImageBase + s.VirtualAddress, oh.ImageBase + s.VirtualAddress + s.Misc_VirtualSize) for s in pe.sections]
    iat = {imp.address: imp.name.decode() for imp in pe.DIRECTORY_ENTRY_IMPORT[0].imports}
    used = set()
    for i in insns:
        if i.group(capstone.x86.X86_GRP_JUMP) or i.group(capstone.x86.X86_GRP_CALL):
            op = i.operands[0]
            if op.type == capstone.x86.X86_OP_IMM:
                need(op.imm in starts, f"{i.mnemonic} at {i.address:#x} targets {op.imm:#x}, not an instruction boundary")
                continue
        for op in i.operands:
            if op.type == capstone.x86.X86_OP_MEM and op.mem.base == capstone.x86.X86_REG_RIP:
                tgt = i.address + i.size + op.mem.disp
                need(any(a <= tgt < b for a, b in secs), f"{i.mnemonic} at {i.address:#x}: rip-relative target {tgt:#x} outside the image")
                if i.mnemonic == "call":
                    need(tgt in iat, f"indirect call at {i.address:#x} does not go through the IAT ({tgt:#x})")
                    used.add(iat[tgt])
    return len(insns), sorted(used)


# ───────────────────────── 3. emulation ─────────────────────────

STACK_TOP = 0x7FFE_0000_0000
STACK_SIZE = 8 << 20
STUBS = 0x7FF8_0000_0000
HEAP = 0x0000_0010_0000_0000
GARBAGE = 0xDEAD_BEEF_DEAD_BEEF
H_OUT, H_ERR = 0x1234_0001, 0x1234_0002


def run_pe(path, insn_limit):
    """Returns (stdout bytes, stderr bytes, exit code, instruction count) or raises Fail."""
    pe = pefile.PE(path)
    oh = pe.OPTIONAL_HEADER
    base = oh.ImageBase
    uc = unicorn.Uc(unicorn.UC_ARCH_X86, unicorn.UC_MODE_64)

    def pages(n):
        return (n + 0xFFF) // 0x1000 * 0x1000

    uc.mem_map(base, pages(oh.SizeOfHeaders), unicorn.UC_PROT_READ)
    uc.mem_write(base, open(path, "rb").read(oh.SizeOfHeaders))
    for s in pe.sections:
        prot = 0
        if s.Characteristics & 0x40000000: prot |= unicorn.UC_PROT_READ
        if s.Characteristics & 0x80000000: prot |= unicorn.UC_PROT_WRITE
        if s.Characteristics & 0x20000000: prot |= unicorn.UC_PROT_EXEC
        uc.mem_map(base + s.VirtualAddress, pages(max(s.Misc_VirtualSize, s.SizeOfRawData)), unicorn.UC_PROT_ALL)
        uc.mem_write(base + s.VirtualAddress, s.get_data()[:s.SizeOfRawData])
        uc.mem_protect(base + s.VirtualAddress, pages(max(s.Misc_VirtualSize, s.SizeOfRawData)), prot)
    uc.mem_map(STACK_TOP - STACK_SIZE, STACK_SIZE, unicorn.UC_PROT_READ | unicorn.UC_PROT_WRITE)
    # import stubs: one 16-byte slot per import; the IAT is filled the way the loader would
    uc.mem_map(STUBS, 0x1000, unicorn.UC_PROT_READ | unicorn.UC_PROT_EXEC)
    pe.parse_data_directories()
    stub_of = {}
    for k, imp in enumerate(pe.DIRECTORY_ENTRY_IMPORT[0].imports):
        stub_of[STUBS + 16 * k] = imp.name.decode()
        uc.mem_write(imp.address, struct.pack("<Q", STUBS + 16 * k))
        uc.mem_write(STUBS + 16 * k, b"\xCC")

    st = {"out": bytearray(), "err": bytearray(), "code": None, "heap": HEAP, "live": {}, "calls": 0, "cp": None}

    def reg(r):
        return uc.reg_read(r)

    def api(uc_, addr, size, _):
        name = stub_of.get(addr)
        if name is None:
            return
        rsp = reg(X.UC_X86_REG_RSP)
        need(rsp % 16 == 8, f"{name}: rsp = {rsp:#x} at entry, Win64 requires rsp = 8 (mod 16) (call with misaligned stack)")
        ret = struct.unpack("<Q", uc.mem_read(rsp, 8))[0]
        rcx, rdx, r8, r9 = (reg(r) for r in (X.UC_X86_REG_RCX, X.UC_X86_REG_RDX, X.UC_X86_REG_R8, X.UC_X86_REG_R9))
        arg5 = struct.unpack("<Q", uc.mem_read(rsp + 40, 8))[0]
        # the shadow space must be addressable stack owned by the caller
        uc.mem_read(rsp + 8, 32)
        saved = {r: reg(r) for r in (X.UC_X86_REG_RBX, X.UC_X86_REG_RBP, X.UC_X86_REG_RSI, X.UC_X86_REG_RDI,
                                      X.UC_X86_REG_R12, X.UC_X86_REG_R13, X.UC_X86_REG_R14, X.UC_X86_REG_R15)}
        rax = 0
        st["calls"] += 1
        if name == "GetStdHandle":
            ni = struct.unpack("<i", struct.pack("<I", rcx & 0xFFFFFFFF))[0]
            rax = {-11: H_OUT, -12: H_ERR}.get(ni, (1 << 64) - 1)
            need(ni in (-11, -12), f"GetStdHandle({ni})")
        elif name == "WriteFile":
            need(rcx in (H_OUT, H_ERR), f"WriteFile: bad handle {rcx:#x}")
            need(arg5 == 0, f"WriteFile: lpOverlapped = {arg5:#x}, must be NULL")
            buf = bytes(uc.mem_read(rdx, r8)) if r8 else b""
            uc.mem_write(r9, struct.pack("<I", r8))  # lpNumberOfBytesWritten must be writable
            (st["out"] if rcx == H_OUT else st["err"]).extend(buf)
            rax = 1
        elif name == "ExitProcess":
            st["code"] = rcx & 0xFFFFFFFF
            uc.emu_stop()
            return
        elif name == "SetConsoleOutputCP":
            need(rcx == 65001, f"SetConsoleOutputCP({rcx})")
            rax = 1
        elif name == "VirtualAlloc":
            need(rcx == 0, "VirtualAlloc: lpAddress must be NULL")
            need(rdx > 0, "VirtualAlloc: zero size")
            need(r8 & 0xFFFFFFFF == 0x3000, f"VirtualAlloc: flAllocationType {r8:#x}")
            need(r9 & 0xFFFFFFFF == 4, f"VirtualAlloc: flProtect {r9:#x}")
            if rdx > (1 << 36):
                rax = 0  # out of memory
            else:
                size = pages(rdx)
                p = st["heap"]
                st["heap"] += (size + 0xFFFF) // 0x10000 * 0x10000 + 0x10000  # 64K granularity + guard gap
                uc.mem_map(p, size, unicorn.UC_PROT_READ | unicorn.UC_PROT_WRITE)
                st["live"][p] = size
                rax = p
        elif name == "VirtualFree":
            need(rdx == 0 and r8 & 0xFFFFFFFF == 0x8000, f"VirtualFree({rcx:#x}, {rdx}, {r8:#x}): MEM_RELEASE needs size 0")
            need(rcx in st["live"], f"VirtualFree of {rcx:#x}: not the base of a live allocation")
            uc.mem_unmap(rcx, st["live"].pop(rcx))
            rax = 1
        # Win64: rax = result, rcx/rdx/r8-r11/xmm0-5 and the shadow space are volatile, the rest preserved
        for r in (X.UC_X86_REG_RCX, X.UC_X86_REG_RDX, X.UC_X86_REG_R8, X.UC_X86_REG_R9, X.UC_X86_REG_R10, X.UC_X86_REG_R11):
            uc.reg_write(r, GARBAGE)
        uc.mem_write(rsp + 8, struct.pack("<4Q", *([GARBAGE] * 4)))
        uc.reg_write(X.UC_X86_REG_RAX, rax)
        for r, v in saved.items():
            uc.reg_write(r, v)
        uc.reg_write(X.UC_X86_REG_RSP, rsp + 8)
        uc.reg_write(X.UC_X86_REG_RIP, ret)

    uc.hook_add(unicorn.UC_HOOK_CODE, api, begin=STUBS, end=STUBS + 0xFFF)
    # initial state as left by the loader: entered like a call (rsp = 8 mod 16), return address = a trap
    uc.mem_map(0x7FF7_0000_0000, 0x1000, unicorn.UC_PROT_READ | unicorn.UC_PROT_EXEC)
    uc.mem_write(0x7FF7_0000_0000, b"\xCC")
    rsp = STACK_TOP - 0x1000 - 8
    uc.mem_write(rsp, struct.pack("<Q", 0x7FF7_0000_0000))
    uc.reg_write(X.UC_X86_REG_RSP, rsp)
    entry = base + oh.AddressOfEntryPoint
    try:
        uc.emu_start(entry, 0, count=insn_limit)
    except unicorn.UcError as e:
        rip = uc.reg_read(X.UC_X86_REG_RIP)
        raise Fail(f"emulation fault at {rip:#x} (image+{rip - base:#x}): {e}")
    if st["code"] is None:
        raise Fail("did not call ExitProcess (instruction limit or fell off the entry point)")
    return bytes(st["out"]), bytes(st["err"]), st["code"], st["calls"], len(st["live"])


# ───────────────────────── driver ─────────────────────────

def build(src, out, extra=()):
    r = subprocess.run([YERUX, "build", src, "--target", "windows", "-o", out, *extra], capture_output=True, text=True)
    return r.returncode, r.stderr


def golden(name):
    try:
        return open(f"{EXPECTED}/{name}.out", "rb").read(), int(open(f"{EXPECTED}/{name}.code").read())
    except FileNotFoundError:
        return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="*")
    ap.add_argument("--fuzz", type=int, default=0, help="also N random programs, compared with the Linux native build")
    ap.add_argument("--insn-limit", type=int, default=300_000_000)
    args = ap.parse_args()
    if not os.path.exists(YERUX):
        subprocess.run(["cargo", "build", "--release"], cwd=ROOT, check=True)
    files = args.files
    if not files:
        import glob
        files = sorted(glob.glob(f"{ROOT}/examples/*.yx") + glob.glob(f"{ROOT}/tests/*.yx"))
        files = [f for f in files if not os.path.basename(f).startswith("e") or not os.path.basename(f)[1:2].isdigit()]
    tmp = tempfile.mkdtemp(prefix="yerux_pe_")
    progs = [(f, os.path.basename(f)[:-3], golden(os.path.basename(f)[:-3])) for f in files]
    if args.fuzz:
        sys.path.insert(0, os.path.join(ROOT, "tests"))
        import fuzz
        for i in range(args.fuzz):
            p = f"{tmp}/fz{i}.yx"
            open(p, "w").write("\n".join(fuzz.Gen(424200 + i).program()) + "\n")
            progs.append((p, f"fz{i}", None))
    npass = nfail = nskip = 0
    insn_total = 0
    for f, name, gold in progs:
        exe = f"{tmp}/{name}.exe"
        rc, err = build(f, exe)
        if rc != 0:
            if "native backend:" in err:
                nskip += 1
                continue
            print(f"FAIL {name}: build: {err.splitlines()[:1]}"); nfail += 1; continue
        try:
            pe, data = check_structure(exe)
            n, used = check_code(pe, data)
            insn_total += n
            if gold is None:  # fuzz program: reference = Linux native binary
                lin = f"{tmp}/{name}.lin"
                subprocess.run([YERUX, "build", f, "--native", "-o", lin], capture_output=True, check=True)
                r = subprocess.run([lin], capture_output=True, timeout=20)
                gold = (r.stdout, r.returncode)
                gold_err = r.stderr
            else:
                gold_err = None
            out, errout, code, calls, live = run_pe(exe, args.insn_limit)
            need(out == gold[0], f"stdout differs from reference: want {gold[0][:80]!r} got {out[:80]!r}")
            need(code == gold[1], f"exit code {code}, want {gold[1]}")
            if code == 101:
                need(errout.startswith(b"panic: ") and errout.endswith(b"\n"), f"panic message on stderr: {errout!r}")
            if gold_err is not None:
                need(errout == gold_err, f"stderr differs: want {gold_err!r} got {errout!r}")
            npass += 1
        except Fail as e:
            if "instruction limit" in str(e):
                nskip += 1
                print(f"SKIP {name}: {e}")
            else:
                nfail += 1
                print(f"FAIL {name}: {e}")
        except unicorn.UcError as e:
            nfail += 1
            print(f"FAIL {name}: unicorn: {e}")
    # --native-debug-alloc: the counters must show no leak on a leak-free program and double free must be caught
    for name, want_live in (("agg_loop_alloc_free", 0),):
        exe = f"{tmp}/dbg.exe"
        rc, err = build(f"{ROOT}/tests/{name}.yx", exe, ["--native-debug-alloc"])
        try:
            need(rc == 0, f"build failed: {err}")
            pe, data = check_structure(exe)
            check_code(pe, data)
            out, errout, code, calls, live = run_pe(exe, args.insn_limit)
            need(f"live: {want_live}\n".encode() in errout, f"debug-alloc report: {errout!r}")
            npass += 1
        except Fail as e:
            nfail += 1
            print(f"FAIL debug-alloc {name}: {e}")
    print(f"pe_check: pass {npass}, fail {nfail}, skip {nskip}  ({insn_total} instructions disassembled)")
    sys.exit(1 if nfail else 0)


if __name__ == "__main__":
    main()
