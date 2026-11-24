// Yerux native backend for x86-64 (Linux and Windows).
// Hand-written machine-code assembler + runtime + executable writers: ELF64 (Linux, raw syscalls,
// no libc) and PE32+ (Windows, kernel32 imports only). Only the few OS-boundary operations
// (write, exit, map/unmap memory) differ per `Os`; everything else is shared.
// Virtual registers are assigned machine registers by the linear-scan allocator in regalloc.rs
// (own ABI, documented there); what does not fit is spilled to the frame.
use crate::ir::*;
use crate::regalloc::{self, Alloc, Loc};
use std::collections::HashMap;

const RAX: u8 = 0;
const RCX: u8 = 1;
const RDX: u8 = 2;
const RBX: u8 = 3;
const RSP: u8 = 4;
const RBP: u8 = 5;
const RSI: u8 = 6;
const RDI: u8 = 7;
const R8: u8 = 8;
const R9: u8 = 9;
const R10: u8 = 10;
const SYSV_ARG_REGS: [u8; 6] = [RDI, RSI, RDX, RCX, R8, R9];
const R12: u8 = 12;
const R13: u8 = 13;
const R14: u8 = 14;
const R15: u8 = 15;
/// internal ABI: the first NREG_ARGS call arguments travel in these registers (scratch, never allocated)
const ARG_REGS: [u8; 4] = [RDI, RSI, RDX, RCX];

const CC_O: u8 = 0x0;
const CC_E: u8 = 0x4;
const CC_NE: u8 = 0x5;
const CC_L: u8 = 0xC;
const CC_GE: u8 = 0xD;
const CC_LE: u8 = 0xE;
const CC_G: u8 = 0xF;
const CC_NS: u8 = 0x9;
const CC_S: u8 = 0x8;
const CC_A: u8 = 0x7;
const CC_AE: u8 = 0x3;
const CC_B: u8 = 0x2;
const CC_BE: u8 = 0x6;
const CC_P: u8 = 0xA;
const CC_NP: u8 = 0xB;

/// Target operating system of the native x86-64 backend.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Os {
    Linux,
    Windows,
}

// Windows imports (kernel32.dll), in IAT order.
const IMPORTS: [&str; 9] = [
    "GetStdHandle",
    "WriteFile",
    "ExitProcess",
    "VirtualAlloc",
    "VirtualFree",
    "SetConsoleOutputCP",
    "ReadFile",
    "GetLastError",
    "GetCurrentProcessId",
];
const IMP_GETSTDHANDLE: usize = 0;
const IMP_WRITEFILE: usize = 1;
const IMP_EXITPROCESS: usize = 2;
const IMP_VIRTUALALLOC: usize = 3;
const IMP_VIRTUALFREE: usize = 4;
const IMP_SETCONSOLEOUTPUTCP: usize = 5;
const IMP_READFILE: usize = 6;
const IMP_GETLASTERROR: usize = 7;
const IMP_GETCURRENTPROCESSID: usize = 8;

pub fn validate_windows(ir: &IrProgram) -> Result<(), String> {
    for f in &ir.funcs {
        let constants = crate::ir::syscall_constants(f);
        for inst in f.blocks.iter().flat_map(|b| &b.insts) {
            let unsupported = match inst {
                Inst::Thread(..) | Inst::Atomic(..) => "thread runtime intrinsics".into(),
                Inst::Global(_, 0) => "process arguments and environment".into(),
                Inst::Syscall(_, args) => match constants[args[0]] {
                    Some(0 | 1 | 39 | 60 | 231) => continue,
                    Some(n) => format!("raw system call {n}"),
                    None => "raw system call with a dynamic number".into(),
                },
                _ => continue,
            };
            return Err(format!("{unsupported} (used by function '{}')", f.name));
        }
    }
    Ok(())
}

// ───────────────────────── Assembler ─────────────────────────

/// Emit DWARF source, frame and variable information for the native Linux backend (`--dwarf`, `--debug`).
pub static DWARF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// One function's entry in `.pdata`/`.xdata`: code range (offsets from the start of .text) and, for functions with
/// a frame, the frame size and the offset (from `begin`) where the prologue ends.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct UnwindRec {
    pub(crate) begin: usize,
    pub(crate) end: usize,
    pub(crate) frame: Option<(i32, usize)>,
}

struct Asm {
    code: Vec<u8>,
    /// (slot, register, code length right after it) of the last `mov [rbp+slot], reg`: an immediate reload is dropped
    last_store: (i32, u8, usize),
    /// code length at the last label bound (a jump may land there, so nothing before it can be assumed)
    last_bind: usize,
    /// heap-state base (Linux: absolute) and the places that need its position-independent form (Windows)
    gbase: i64,
    /// Windows unwind data: set by `gen_func` (frame size, end of the prologue) and collected per function
    func_unwind: Option<(i32, usize)>,
    unwind: Vec<UnwindRec>,
    gfix: Vec<(usize, i64)>,
    win_data_rel: usize,
    /// (vreg, condition code, code length) of the last comparison materialised with setcc: the flags still hold it
    cmp_rec: (usize, u8, usize),
    labels: Vec<Option<usize>>,
    fix: Vec<(usize, usize)>,  // (position of rel32, label)
    dfix: Vec<(usize, usize)>, // (position of rel32, data blob id)
    pool: Vec<u64>,            // 8-byte float constants, data ids pool_base + k
    pool_base: usize,
    ifix: Vec<(usize, usize)>, // (position of rel32, import index): call [rip+IAT slot] (Windows)
    xfix: Vec<(usize, usize)>, // Linux imported call displacement
    os: Os,
    /// the panic report is built in (see `emit_report`)
    report: bool,
    /// panic site table: (offset of the return address after a call, location id), see `emit_report`
    sites: Vec<(usize, u32)>,
    /// DWARF (see `dwarf.rs`): line rows (code offset, location id) and the frame-state rows of the current function
    dwarf: bool,
    lines: Vec<(usize, u32)>,
    cfi: Vec<(usize, u8)>,
    // Windows runtime stubs (labels are only bound when os == Windows)
    l_oswrite: usize,
    l_osexit: usize,
    l_winsyscall: usize,
}

impl Asm {
    fn new(os: Os) -> Asm {
        Asm {
            last_store: (0, 0, usize::MAX),
            last_bind: usize::MAX,
            gbase: 0,
            func_unwind: None,
            unwind: vec![],
            gfix: vec![],
            win_data_rel: 0,
            cmp_rec: (usize::MAX, 0, usize::MAX),
            code: vec![],
            labels: vec![],
            fix: vec![],
            dfix: vec![],
            pool: vec![],
            pool_base: 0,
            ifix: vec![],
            xfix: vec![],
            os,
            report: false,
            sites: vec![],
            dwarf: DWARF.load(std::sync::atomic::Ordering::Relaxed),
            lines: vec![],
            cfi: vec![],
            l_oswrite: 0,
            l_osexit: 0,
            l_winsyscall: 0,
        }
    }
    /// call qword [rip+disp32] through the import address table slot `imp`
    fn call_iat(&mut self, imp: usize) {
        self.raw(&[0xFF, 0x15]);
        let p = self.code.len();
        self.ifix.push((p, imp));
        self.imm32(0);
    }
    fn call_elf_got(&mut self, imp: usize) {
        self.raw(&[0xFF, 0x15]);
        let p = self.code.len();
        self.xfix.push((p, imp));
        self.imm32(0);
    }
    fn raw(&mut self, b: &[u8]) {
        self.code.extend_from_slice(b);
    }
    fn imm32(&mut self, v: i32) {
        self.code.extend_from_slice(&v.to_le_bytes());
    }
    fn label(&mut self) -> usize {
        self.labels.push(None);
        self.labels.len() - 1
    }
    fn bind(&mut self, l: usize) {
        self.labels[l] = Some(self.code.len());
        self.last_bind = self.code.len();
    }
    /// `n` bytes of multi-byte NOPs (Intel recommended forms), n <= 15
    fn nops(&mut self, mut n: usize) {
        const N: [&[u8]; 9] = [
            &[0x90],
            &[0x66, 0x90],
            &[0x0F, 0x1F, 0x00],
            &[0x0F, 0x1F, 0x40, 0x00],
            &[0x0F, 0x1F, 0x44, 0x00, 0x00],
            &[0x66, 0x0F, 0x1F, 0x44, 0x00, 0x00],
            &[0x0F, 0x1F, 0x80, 0x00, 0x00, 0x00, 0x00],
            &[0x0F, 0x1F, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00],
            &[0x66, 0x0F, 0x1F, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00],
        ];
        while n > 0 {
            let k = n.min(9);
            self.raw(N[k - 1]);
            n -= k;
        }
    }
    /// pad with NOPs to a 16-byte boundary when that takes at most `max` bytes (code starts 16-aligned in every image)
    fn align_small(&mut self, max: usize) {
        let need = (16 - self.code.len() % 16) % 16;
        if need > 0 && need <= max {
            self.nops(need);
        }
    }
    fn rel(&mut self, l: usize) {
        let p = self.code.len();
        self.fix.push((p, l));
        self.imm32(0);
    }
    fn jmp(&mut self, l: usize) {
        self.raw(&[0xE9]);
        self.rel(l);
    }
    fn jcc(&mut self, cc: u8, l: usize) {
        self.raw(&[0x0F, 0x80 + cc]);
        self.rel(l);
    }
    fn call(&mut self, l: usize) {
        self.raw(&[0xE8]);
        self.rel(l);
    }
    fn ret(&mut self) {
        self.raw(&[0xC3]);
    }
    fn syscall(&mut self) {
        self.raw(&[0x0F, 0x05]);
    }
    // ---- general encoders (all 16 registers, rsp/r12 via SIB, disp8/disp32) ----
    fn rex(&mut self, w: bool, reg: u8, rm: u8) {
        let b = 0x40 | (w as u8) << 3 | (reg >> 3) << 2 | (rm >> 3);
        if b != 0x40 {
            self.code.push(b);
        }
    }
    fn modrm_reg(&mut self, reg: u8, rm: u8) {
        self.code.push(0xC0 | (reg & 7) << 3 | (rm & 7));
    }
    fn modrm_mem(&mut self, reg: u8, base: u8, disp: i32) {
        let (r, b) = (reg & 7, base & 7);
        let mode = if disp == 0 && b != 5 {
            0x00
        } else if i8::try_from(disp).is_ok() {
            0x40
        } else {
            0x80
        };
        self.code.push(mode | r << 3 | b);
        if b == 4 {
            self.code.push(0x24); // SIB: base only (rsp / r12)
        }
        match mode {
            0x40 => self.code.push(disp as i8 as u8),
            0x80 => self.imm32(disp),
            _ => {}
        }
    }
    /// ModRM + SIB for [base + index*scale + disp]; `index` must not be rsp
    fn modrm_sib(&mut self, reg: u8, base: u8, index: u8, scale: u8, disp: i32) {
        let ss = match scale {
            1 => 0u8,
            2 => 1,
            4 => 2,
            _ => 3,
        };
        let (r, b) = (reg & 7, base & 7);
        let mode = if disp == 0 && b != 5 {
            0x00
        } else if i8::try_from(disp).is_ok() {
            0x40
        } else {
            0x80
        };
        self.code.push(mode | r << 3 | 4);
        self.code.push(ss << 6 | (index & 7) << 3 | b);
        match mode {
            0x40 => self.code.push(disp as i8 as u8),
            0x80 => self.imm32(disp),
            _ => {}
        }
    }
    /// REX.W op /r with a memory operand [base + index*scale + disp]
    fn rm_sib(&mut self, op: u8, reg: u8, base: u8, index: u8, scale: u8, disp: i32) {
        let b = 0x48 | (reg >> 3) << 2 | (index >> 3) << 1 | (base >> 3);
        self.code.push(b);
        self.code.push(op);
        self.modrm_sib(reg, base, index, scale, disp);
    }
    /// movzx r32, byte [base + index] / mov byte [base + index], r8 (always REX, so sil/dil are addressable)
    fn byte_sib(&mut self, load: bool, reg: u8, base: u8, index: u8, disp: i32) {
        self.code
            .push(0x40 | (reg >> 3) << 2 | (index >> 3) << 1 | (base >> 3));
        if load {
            self.raw(&[0x0F, 0xB6]);
        } else {
            self.code.push(0x88);
        }
        self.modrm_sib(reg, base, index, 1, disp);
    }
    /// REX.W op /r with a register r/m operand
    fn rr(&mut self, op: u8, reg: u8, rm: u8) {
        self.rex(true, reg, rm);
        self.code.push(op);
        self.modrm_reg(reg, rm);
    }
    /// REX.W op /r with a memory operand [base+disp]
    fn rm(&mut self, op: u8, reg: u8, base: u8, disp: i32) {
        self.rex(true, reg, base);
        self.code.push(op);
        self.modrm_mem(reg, base, disp);
    }
    /// REX.W (0x83 | 0x81) /ext imm on a register: add 0, or 1, and 4, sub 5, xor 6, cmp 7
    fn alu_imm(&mut self, ext: u8, r: u8, v: i32) {
        self.rex(true, 0, r);
        if let Ok(b) = i8::try_from(v) {
            self.code.push(0x83);
            self.modrm_reg(ext, r);
            self.code.push(b as u8);
        } else {
            self.code.push(0x81);
            self.modrm_reg(ext, r);
            self.imm32(v);
        }
    }
    fn mov_imm(&mut self, r: u8, v: i64) {
        if v >= 0 && v <= u32::MAX as i64 {
            self.rex(false, 0, r);
            self.code.push(0xB8 + (r & 7)); // mov r32, imm32 (zero-extends)
            self.imm32(v as u32 as i32);
        } else if let Ok(x) = i32::try_from(v) {
            self.rex(true, 0, r);
            self.code.push(0xC7); // mov r64, imm32 (sign-extended)
            self.modrm_reg(0, r);
            self.imm32(x);
        } else {
            self.rex(true, 0, r);
            self.code.push(0xB8 + (r & 7)); // movabs r64, imm64
            self.code.extend_from_slice(&v.to_le_bytes());
        }
    }
    /// mov r64, [rbp+disp]
    fn load(&mut self, r: u8, slot: i32) {
        if self.last_store == (slot, r, self.code.len()) && self.last_bind != self.code.len() {
            return; // `r` still holds the value that was just stored
        }
        self.rm(0x8B, r, RBP, slot);
    }
    /// mov [rbp+disp], r64
    fn store(&mut self, slot: i32, r: u8) {
        self.rm(0x89, r, RBP, slot);
        self.last_store = (slot, r, self.code.len());
    }
    /// mov r64, [base+disp]
    fn ld(&mut self, r: u8, base: u8, disp: i32) {
        if base == RBP && self.last_store == (disp, r, self.code.len()) && self.last_bind != self.code.len() {
            return; // `r` still holds the value that was just stored to this frame slot
        }
        self.rm(0x8B, r, base, disp);
    }
    /// mov [base+disp], r64
    fn st(&mut self, base: u8, disp: i32, r: u8) {
        self.rm(0x89, r, base, disp);
        if base == RBP {
            self.last_store = (disp, r, self.code.len());
        }
    }
    /// lea r64, [base+disp]
    fn lea(&mut self, r: u8, base: u8, disp: i32) {
        self.rm(0x8D, r, base, disp);
    }
    /// mov r32, [base+disp] (zero-extends)
    fn ld32(&mut self, r: u8, base: u8, disp: i32) {
        self.rex(false, r, base);
        self.code.push(0x8B);
        self.modrm_mem(r, base, disp);
    }
    /// mov [base+disp], r32
    fn st32(&mut self, base: u8, disp: i32, r: u8) {
        self.rex(false, r, base);
        self.code.push(0x89);
        self.modrm_mem(r, base, disp);
    }
    fn push(&mut self, r: u8) {
        self.rex(false, 0, r);
        self.code.push(0x50 + (r & 7));
    }
    fn pop(&mut self, r: u8) {
        self.rex(false, 0, r);
        self.code.push(0x58 + (r & 7));
    }
    /// push qword [rbp+disp]
    fn push_slot(&mut self, disp: i32) {
        self.code.push(0xFF);
        self.modrm_mem(6, RBP, disp);
    }
    /// movzx r32, word [base+disp]
    fn movzx16_mem(&mut self, r: u8, base: u8, disp: i32) {
        self.rex(false, r, base);
        self.raw(&[0x0F, 0xB7]);
        self.modrm_mem(r, base, disp);
    }
    /// mov word [base+disp], r16
    fn mov16_mem(&mut self, base: u8, disp: i32, r: u8) {
        self.code.push(0x66);
        self.rex(false, r, base);
        self.code.push(0x89);
        self.modrm_mem(r, base, disp);
    }
    /// movzx r32, byte [base+disp]
    fn movzx8_mem(&mut self, r: u8, base: u8, disp: i32) {
        self.rex(false, r, base);
        self.raw(&[0x0F, 0xB6]);
        self.modrm_mem(r, base, disp);
    }
    /// mov byte [base+disp], r8 (always with a REX prefix so that sil/dil/bpl/spl are addressable)
    fn mov8_mem(&mut self, base: u8, disp: i32, r: u8) {
        self.code.push(0x40 | (r >> 3) << 2 | (base >> 3));
        self.code.push(0x88);
        self.modrm_mem(r, base, disp);
    }
    fn push_imm(&mut self, v: i32) {
        self.code.push(0x68);
        self.imm32(v);
    }
    fn shl(&mut self, r: u8, n: u8) {
        self.raw(&[0x48, 0xC1, 0xE0 + r, n]);
    }
    fn add_imm(&mut self, r: u8, v: i32) {
        self.alu_imm(0, r, v);
    }
    fn sub_imm(&mut self, r: u8, v: i32) {
        self.alu_imm(5, r, v);
    }
    fn and_imm(&mut self, r: u8, v: i32) {
        self.alu_imm(4, r, v);
    }
    fn cmp_imm(&mut self, r: u8, v: i32) {
        if v == 0 {
            self.test_rr(r); // same flags as cmp r, 0, shorter
        } else {
            self.alu_imm(7, r, v);
        }
    }
    fn inc(&mut self, r: u8) {
        self.raw(&[0x48 | (r >> 3), 0xFF, 0xC0 + (r & 7)]);
    }
    /// lock inc qword [base+disp32]
    fn atomic_inc_mem(&mut self, base: u8, disp: i32) {
        self.raw(&[0xF0, 0x48, 0xFF, 0x80 | base]);
        self.imm32(disp);
    }
    /// mov r8/r9/r10, imm32 (needed for the 4th-6th syscall arguments)
    fn mov_imm_ext(&mut self, n: u8, v: i32) {
        self.raw(&[0x49, 0xC7, 0xC0 + (n - 8)]);
        self.imm32(v);
    }
    fn mov_rr(&mut self, dst: u8, src: u8) {
        if dst != src {
            self.alu(0x89, dst, src);
        }
    }
    /// address of the heap state + `off`: an absolute immediate on Linux (fixed base), `lea r, [rip+disp]` into
    /// .data on Windows, so the image can be relocated (ASLR) without a relocation entry for it
    fn gaddr(&mut self, r: u8, off: i64) {
        if self.os == Os::Windows {
            self.raw(&[0x48 | (r >> 3) << 2, 0x8D, 0x05 | ((r & 7) << 3)]);
            let p = self.code.len();
            self.gfix.push((p, off));
            self.imm32(0);
        } else {
            let g = self.gbase + off;
            self.mov_imm(r, g);
        }
    }
    fn lea_data(&mut self, r: u8, id: usize) {
        self.raw(&[0x48 | (r >> 3) << 2, 0x8D, 0x05 | ((r & 7) << 3)]); // lea r64, [rip+disp32]
        let p = self.code.len();
        self.dfix.push((p, id));
        self.imm32(0);
    }
    /// lea r64, [rip+label] (the address of a function)
    fn lea_label(&mut self, r: u8, l: usize) {
        self.raw(&[0x48 | (r >> 3) << 2, 0x8D, 0x05 | ((r & 7) << 3)]);
        self.rel(l);
    }
    /// call r64
    fn call_r(&mut self, r: u8) {
        if r >= 8 {
            self.raw(&[0x41]);
        }
        self.raw(&[0xFF, 0xD0 | (r & 7)]);
    }
    /// reg-reg ALU op: opcode 0x01 add, 0x29 sub, 0x39 cmp (dst op= src)
    fn alu(&mut self, op: u8, dst: u8, src: u8) {
        self.rr(op, src, dst);
    }
    /// REX.W F7 /ext (not 2, neg 3, idiv 7)
    fn f7(&mut self, ext: u8, r: u8) {
        self.rex(true, 0, r);
        self.code.push(0xF7);
        self.modrm_reg(ext, r);
    }
    /// REX.W C1 /ext ib: shl 4, shr 5, sar 7
    fn shift_imm(&mut self, ext: u8, r: u8, n: u8) {
        self.rex(true, 0, r);
        self.code.push(0xC1);
        self.modrm_reg(ext, r);
        self.code.push(n);
    }
    fn imul(&mut self, dst: u8, src: u8) {
        self.rex(true, dst, src);
        self.raw(&[0x0F, 0xAF]);
        self.modrm_reg(dst, src);
    }
    fn imul_imm(&mut self, dst: u8, src: u8, v: i32) {
        self.rr(0x69, dst, src);
        self.imm32(v);
    }
    /// setcc al; movzx r32, al
    fn setcc_to(&mut self, cc: u8, r: u8) {
        self.raw(&[0x0F, 0x90 + cc, 0xC0]);
        self.rex(false, r, 0);
        self.raw(&[0x0F, 0xB6]);
        self.modrm_reg(r, RAX);
    }
    // ---- SSE2 scalar double / single (only xmm0 and xmm1 are ever used, as scratch) ----
    /// movq xmm, r64 (66 REX.W 0F 6E)
    fn movq_xr(&mut self, x: u8, r: u8) {
        self.raw(&[0x66]);
        self.rex(true, x, r);
        self.raw(&[0x0F, 0x6E]);
        self.modrm_reg(x, r);
    }
    /// movq r64, xmm (66 REX.W 0F 7E)
    fn movq_rx(&mut self, r: u8, x: u8) {
        self.raw(&[0x66]);
        self.rex(true, x, r);
        self.raw(&[0x0F, 0x7E]);
        self.modrm_reg(x, r);
    }
    /// movd xmm, r32 (66 0F 6E): the low 32 bits of the register
    fn movd_xr(&mut self, x: u8, r: u8) {
        self.raw(&[0x66]);
        self.rex(false, x, r);
        self.raw(&[0x0F, 0x6E]);
        self.modrm_reg(x, r);
    }
    /// movd r32, xmm (66 0F 7E): zero-extends into the 64-bit register
    fn movd_rx(&mut self, r: u8, x: u8) {
        self.raw(&[0x66]);
        self.rex(false, x, r);
        self.raw(&[0x0F, 0x7E]);
        self.modrm_reg(x, r);
    }
    /// scalar SSE op between xmm registers: `prefix 0F op /r` (dst = reg field, src = rm field)
    fn sse_rr(&mut self, prefix: u8, op: u8, dst: u8, src: u8) {
        if prefix != 0 {
            self.raw(&[prefix]);
        }
        self.rex(false, dst, src);
        self.raw(&[0x0F, op]);
        self.modrm_reg(dst, src);
    }
    /// scalar SSE op with a memory operand [base+disp]
    fn sse_rm(&mut self, prefix: u8, op: u8, x: u8, base: u8, disp: i32) {
        if prefix != 0 {
            self.raw(&[prefix]);
        }
        self.rex(false, x, base);
        self.raw(&[0x0F, op]);
        self.modrm_mem(x, base, disp);
    }
    /// scalar SSE op with a memory operand [base+index*scale+disp]
    fn sse_sib(&mut self, prefix: u8, op: u8, x: u8, base: u8, index: u8, scale: u8, disp: i32) {
        if prefix != 0 {
            self.raw(&[prefix]);
        }
        let b = 0x40 | (x >> 3) << 2 | (index >> 3) << 1 | (base >> 3);
        if b != 0x40 {
            self.code.push(b);
        }
        self.raw(&[0x0F, op]);
        self.modrm_sib(x, base, index, scale, disp);
    }
    /// scalar SSE op with a rip-relative operand (8-byte constant of the float pool)
    fn sse_pool(&mut self, prefix: u8, op: u8, x: u8, bits: u64) {
        let id = self.pool_id(bits);
        if prefix != 0 {
            self.raw(&[prefix]);
        }
        self.rex(false, x, 0);
        self.raw(&[0x0F, op, 0x05 | ((x & 7) << 3)]);
        let p = self.code.len();
        self.dfix.push((p, id));
        self.imm32(0);
    }
    fn pool_id(&mut self, bits: u64) -> usize {
        let k = match self.pool.iter().position(|b| *b == bits) {
            Some(k) => k,
            None => {
                self.pool.push(bits);
                self.pool.len() - 1
            }
        };
        self.pool_base + k
    }
    fn movaps(&mut self, dst: u8, src: u8) {
        if dst != src {
            self.sse_rr(0, 0x28, dst, src);
        }
    }
    fn xorps(&mut self, x: u8) {
        self.sse_rr(0, 0x57, x, x);
    }
    /// cvtsi2sd / cvtsi2ss xmm, r64 (F2/F3 REX.W 0F 2A)
    fn cvtsi2fp(&mut self, prefix: u8, x: u8, r: u8) {
        self.raw(&[prefix]);
        self.rex(true, x, r);
        self.raw(&[0x0F, 0x2A]);
        self.modrm_reg(x, r);
    }
    /// cvttsd2si r64, xmm (F2 REX.W 0F 2C)
    fn cvttsd2si(&mut self, r: u8, x: u8) {
        self.raw(&[0xF2]);
        self.rex(true, r, x);
        self.raw(&[0x0F, 0x2C]);
        self.modrm_reg(r, x);
    }
    /// btc r64, imm8
    fn btc_imm(&mut self, r: u8, n: u8) {
        self.rex(true, 0, r);
        self.raw(&[0x0F, 0xBA]);
        self.modrm_reg(7, r);
        self.code.push(n);
    }
    /// bsr dst, src (index of the highest set bit; src != 0)
    fn bsr(&mut self, dst: u8, src: u8) {
        self.rex(true, dst, src);
        self.raw(&[0x0F, 0xBD]);
        self.modrm_reg(dst, src);
    }
    /// bts dst, bitreg (sets bit `bitreg` of dst)
    fn bts(&mut self, dst: u8, bit: u8) {
        self.rex(true, bit, dst);
        self.raw(&[0x0F, 0xAB]);
        self.modrm_reg(bit, dst);
    }
    /// `size` = smallest power of two >= n (at least 16), `cls` = its log2. Clobbers `size`, `cls` and flags.
    fn size_class(&mut self, n: u8, size: u8, cls: u8) {
        let done = self.label();
        self.mov_imm(cls, 4);
        self.mov_imm(size, 16);
        self.cmp_imm(n, 16);
        self.jcc(CC_BE, done);
        self.lea(size, n, -1);
        self.bsr(cls, size);
        self.inc(cls);
        self.alu(0x31, size, size);
        self.bts(size, cls);
        self.bind(done);
    }
    /// cmovcc dst, src (64-bit)
    fn cmov_rr(&mut self, cc: u8, dst: u8, src: u8) {
        self.rex(true, dst, src);
        self.raw(&[0x0F, 0x40 + cc]);
        self.modrm_reg(dst, src);
    }
    fn test_rr(&mut self, r: u8) {
        self.alu(0x85, r, r);
    }
    fn test_rax(&mut self) {
        self.raw(&[0x48, 0x85, 0xC0]);
    }
    fn test_rcx(&mut self) {
        self.raw(&[0x48, 0x85, 0xC9]);
    }
    /// OS-level write of rsi/rdx (buffer, length) to `fd`.
    fn sys_write(&mut self, fd: i64) {
        self.mov_imm(RDI, fd);
        self.write_rdi();
    }
    /// OS-level write: rdi = fd, rsi = buffer, rdx = length.
    fn write_rdi(&mut self) {
        match self.os {
            Os::Linux => {
                self.mov_imm(RAX, 1);
                self.syscall();
            }
            Os::Windows => {
                let l = self.l_oswrite;
                self.call(l);
            }
        }
    }
    /// Terminate the process with `code`; does not return.
    fn exit(&mut self, code: i64) {
        self.mov_imm(RDI, code);
        match self.os {
            Os::Linux => {
                self.mov_imm(RAX, 231); // exit_group
                self.syscall();
            }
            Os::Windows => {
                let l = self.l_osexit;
                self.jmp(l);
            }
        }
    }
    /// Windows API stub prologue: aligns rsp to 16 whatever the caller did, reserves 32 bytes of
    /// shadow space + `locals` bytes (multiple of 16). Preserves rbx/rsi/rdi (non-volatile in Win64).
    fn win_prologue(&mut self, locals: u8) {
        self.raw(&[0x55]); // push rbp
        self.raw(&[0x48, 0x89, 0xE5]); // mov rbp, rsp
        self.raw(&[0x48, 0x83, 0xE4, 0xF0]); // and rsp, -16
        self.raw(&[0x48, 0x83, 0xEC, 32 + locals]); // sub rsp, 32 + locals
    }
    fn win_epilogue(&mut self) {
        self.raw(&[0x48, 0x89, 0xEC]); // mov rsp, rbp
        self.raw(&[0x5D, 0xC3]); // pop rbp; ret
    }

    /// `data_start`: offset of the data blobs relative to the start of the code (may be larger than
    /// the code size when the data lives in a later section); `iat`: same, for each import slot.
    fn finalize(&mut self, data_start: usize, data_offs: &[usize], iat: &[usize], got: &[usize]) {
        for (p, i) in self.ifix.clone() {
            let rel = (iat[i] as i64 - (p as i64 + 4)) as i32;
            self.code[p..p + 4].copy_from_slice(&rel.to_le_bytes());
        }
        for (p, i) in self.xfix.clone() {
            let rel = (got[i] as i64 - (p as i64 + 4)) as i32;
            self.code[p..p + 4].copy_from_slice(&rel.to_le_bytes());
        }
        for (p, l) in self.fix.clone() {
            let target = self.labels[l].expect("unbound label") as i64;
            let rel = (target - (p as i64 + 4)) as i32;
            self.code[p..p + 4].copy_from_slice(&rel.to_le_bytes());
        }
        for (p, off) in self.gfix.clone() {
            let target = self.win_data_rel as i64 + off;
            let rel = (target - (p as i64 + 4)) as i32;
            self.code[p..p + 4].copy_from_slice(&rel.to_le_bytes());
        }
        for (p, id) in self.dfix.clone() {
            let target = (data_start + data_offs[id]) as i64;
            let rel = (target - (p as i64 + 4)) as i32;
            self.code[p..p + 4].copy_from_slice(&rel.to_le_bytes());
        }
    }
}

// ───────────────────────── Data blobs ─────────────────────────

const D_PANIC: usize = 0; // "panic: "
const D_NL: usize = 1; // "\n"
const D_TRUE: usize = 2;
const D_FALSE: usize = 3;
const D_MSG0: usize = 4; // panic messages
const D_DBG0: usize = D_MSG0 + PANIC_MSGS.len(); // "allocs: ", "frees: ", "live: "
const D_RNG0: usize = D_DBG0 + 3; // ": ", " not in ", ".." (the message of a failed range conversion)
const D_TEXT0: usize = D_DBG0 + 6; // user text literals start here

const PANIC_MSGS: [&str; 17] = [
    "integer overflow in addition",
    "integer overflow in subtraction",
    "integer overflow in multiplication",
    "integer overflow in division",
    "integer overflow in negation",
    "division by zero",
    "modulo by zero",
    "index out of range",
    "out of memory",
    "double free (debug)",
    "pop from an empty list",
    "assertion failed",
    "integer conversion out of range",
    "float is NaN or out of range for the integer type",
    "could not start thread",
    "stack overflow",
    "second panic during worker cleanup",
];
const P_ADD: usize = 0;
const P_SUB: usize = 1;
const P_MUL: usize = 2;
const P_DIV: usize = 3;
const P_NEG: usize = 4;
const P_DIV0: usize = 5;
const P_MOD0: usize = 6;
const P_INDEX: usize = 7;
const P_OOM: usize = 8;
const P_DOUBLE_FREE: usize = 9; // only reachable in --native-debug-alloc builds
const P_POP: usize = 10;
const P_ASSERT: usize = 11;
const P_CONV: usize = 12;
const P_F2I: usize = 13;
const P_THREAD_START: usize = 14;
const P_DOUBLE_PANIC: usize = 16;
const P_STACK: usize = 15; // raised by the SIGSEGV handler when the fault address is at the stack pointer
/// panic stub of each `NOp::Chk` message code (MSG_ADD .. MSG_F2I)
const CHK_PANIC: [usize; 7] = [P_ADD, P_SUB, P_MUL, P_DIV, P_NEG, P_CONV, P_F2I];
const FREED: i64 = 1 << 62; // debug builds tag the header of a block sitting on a free list

// Runtime heap state: a second, writable PT_LOAD segment (zero-filled, no bytes in the file).
pub(crate) const G: i64 = 0x600000;
const G_BUMP: i32 = 0; // next free byte of the current arena
const G_END: i32 = 8; // end of the current arena
const G_NALLOC: i32 = 16; // debug counters
const G_NFREE: i32 = 24;
const G_FREE: i32 = 32; // free-list heads, one per size class 2^k (k = 4..=16)
const FUTEX_WAIT_PRIVATE: i64 = 128;
const FUTEX_WAKE_PRIVATE: i64 = 129;
const G_HEAP_LOCK: i32 = G_FREE + 17 * 8; // after the last free-list head
const G_MAIN_TLS: i64 = 176; // self pointer, recovery flag, unwinding flag, cleanup head
const G_PRINT_LOCK: i32 = 208;
const G_USER: i64 = 256; // user cells (`__global(i)`): 8 * layout::GLOBAL_CELLS bytes
pub(crate) const G_SIZE: u64 = 4096;
// The runtime mutex preserves RDI and RSI on acquisition and RAX on release.
fn acquire_runtime_lock(a: &mut Asm, _g: i64, offset: i32) {
    let retry = a.label();
    let held = a.label();
    a.gaddr(RBX, 0);
    a.mov_imm(RCX, 1);
    a.mov_imm(RAX, 0);
    a.raw(&[0xF0, 0x48, 0x0F, 0xB1, 0x8B]); // lock cmpxchg [rbx+disp32], rcx
    a.imm32(offset);
    a.jcc(CC_E, held);
    a.bind(retry);
    a.mov_imm(RAX, 2);
    a.rm(0x87, RAX, RBX, offset); // acquire, with sleepers possibly present
    a.test_rax();
    a.jcc(CC_E, held);
    a.push(RDI);
    a.push(RSI);
    a.lea(RDI, RBX, offset);
    a.mov_imm(RDX, 2);
    emit_futex(a, FUTEX_WAIT_PRIVATE);
    a.pop(RSI);
    a.pop(RDI);
    a.jmp(retry);
    a.bind(held);
}

fn release_runtime_lock(a: &mut Asm, _g: i64, offset: i32) {
    let released = a.label();
    a.gaddr(RBX, 0);
    a.mov_imm(RCX, 0);
    a.rm(0x87, RCX, RBX, offset);
    a.cmp_imm(RCX, 2);
    a.jcc(CC_NE, released);
    a.push(RAX);
    a.push(RDI);
    a.push(RSI);
    a.lea(RDI, RBX, offset);
    a.mov_imm(RDX, 1);
    emit_futex(a, FUTEX_WAKE_PRIVATE);
    a.pop(RSI);
    a.pop(RDI);
    a.pop(RAX);
    a.bind(released);
}

fn acquire_print_lock(a: &mut Asm, g: i64) {
    a.push(RAX);
    a.push(RBX);
    a.push(RDX);
    acquire_runtime_lock(a, g, G_PRINT_LOCK);
    a.pop(RDX);
    a.pop(RBX);
    a.pop(RAX);
}

fn emit_futex(a: &mut Asm, operation: i64) {
    a.mov_imm(RSI, operation);
    a.mov_imm(R10, 0); // no timeout
    a.mov_imm(RAX, 202); // Linux x86-64 futex
    a.syscall();
}
const ARENA: i32 = 1 << 20;
const SMALL_MAX: i32 = 65536;

/// signed magic number and shift for dividing by `d` (|d| >= 2), Hacker's Delight fig. 10-1
fn magic(d: i64) -> (i64, u32) {
    let two63: u64 = 1 << 63;
    let ad = d.unsigned_abs();
    let t = two63 + ((d as u64) >> 63);
    let anc = t - 1 - t % ad;
    let mut p = 63u32;
    let (mut q1, mut q2) = (two63 / anc, two63 / ad);
    let (mut r1, mut r2) = (two63 - q1 * anc, two63 - q2 * ad);
    loop {
        p += 1;
        q1 = q1.wrapping_mul(2);
        r1 = r1.wrapping_mul(2);
        if r1 >= anc {
            q1 = q1.wrapping_add(1);
            r1 = r1.wrapping_sub(anc);
        }
        q2 = q2.wrapping_mul(2);
        r2 = r2.wrapping_mul(2);
        if r2 >= ad {
            q2 = q2.wrapping_add(1);
            r2 = r2.wrapping_sub(ad);
        }
        let delta = ad - r2;
        if !(q1 < delta || (q1 == delta && r1 == 0)) {
            break;
        }
    }
    let m = q2.wrapping_add(1) as i64;
    (if d < 0 { m.wrapping_neg() } else { m }, p - 64)
}

struct Rt {
    vtable_blob: usize,
    funcs: Vec<usize>,
    panic: Vec<usize>,
    /// entries of the same stubs for a panic raised by compiled code: entered with `call`, they record where
    /// (see `emit_report`); empty on Windows
    panic_at: Vec<usize>,
    /// per function id: a library function (its call sites are recorded in the panic site table)
    lib: Vec<bool>,
    /// rdi = value, rsi = lo, rdx = hi: "panic: integer conversion out of range: V not in LO..HI"
    range_panic: usize,
    print_int: usize,
    print_bool: usize,
    print_text: usize,
    print_bytes: usize,
    panic_bytes: usize,
    push: usize,
    alloc: usize,
    free: usize,
    realloc: usize,
    memcpy: usize,
    thread_start: usize,
    thread_wait: usize,
}

fn swap_cc(op: Op) -> Op {
    match op {
        Op::Lt => Op::Gt,
        Op::Le => Op::Ge,
        Op::Gt => Op::Lt,
        Op::Ge => Op::Le,
        o => o,
    }
}

fn cc_of(op: Op) -> u8 {
    match op {
        Op::Eq => CC_E,
        Op::Ne => CC_NE,
        Op::Lt => CC_L,
        Op::Le => CC_LE,
        Op::Gt => CC_G,
        _ => CC_GE,
    }
}

/// Per-function code generator: IR + register assignment -> machine code.
struct G<'a> {
    a: &'a mut Asm,
    rt: &'a Rt,
    al: Alloc,
    spill_base: i32,     // rbp displacement of spill slot 0 is -(spill_base + 8)
    slot_off: Vec<i32>,  // rbp displacement of every aggregate slot
    cur: (usize, usize), // block / instruction being generated
    /// out-of-line failure paths of `Chk(Iv)`: (label, register holding the value, lo, hi, location, frame fix)
    stubs: Vec<(usize, u8, i64, i64, u32, bool)>,
    /// location of every instruction (see `ir::strip_locs`)
    locs: Vec<Vec<u32>>,
    /// out-of-line panic paths: (label, panic kind, location, frame fix), one per distinct triple
    pstubs: Vec<(usize, usize, u32, bool)>,
    pmap: HashMap<(usize, u32, bool), usize>,
    /// the running code has no frame record of its own (frameless function or shrink-wrapped early block)
    no_frame: bool,
    linux: bool,
}

impl<'a> G<'a> {
    /// The label a failed check jumps to. On Linux every failure goes through a cold stub of the function that
    /// `call`s the panic routine, so the return address identifies the site and the routine can print its
    /// `.yx` position (and walk the frames). A function without a frame record gets one in the stub.
    fn pl(&mut self, kind: usize) -> usize {
        if !self.linux {
            return self.rt.panic[kind];
        }
        let loc = self.locs[self.cur.0][self.cur.1];
        let key = (kind, loc, self.no_frame);
        if let Some(l) = self.pmap.get(&key) {
            return *l;
        }
        let l = self.a.label();
        self.pstubs.push((l, kind, loc, self.no_frame));
        self.pmap.insert(key, l);
        l
    }
    /// Record the return address of the call just emitted as a site of the current instruction's location.
    fn note_site(&mut self) {
        if self.linux {
            let loc = self.locs[self.cur.0][self.cur.1];
            if loc != 0 {
                let at = self.a.code.len();
                self.a.sites.push((at, loc));
            }
        }
    }

    fn spill_disp(&self, s: usize) -> i32 {
        -(self.spill_base + 8 * (s as i32 + 1))
    }
    fn loc(&self, v: V) -> Loc {
        self.al.loc[v]
    }
    fn in_reg(&self, v: V, r: u8) -> bool {
        self.loc(v) == Loc::Reg(r)
    }
    /// the value as a sign-extended 32-bit immediate, when it is a rematerialised constant
    fn imm(&self, v: V) -> Option<i32> {
        match self.loc(v) {
            Loc::Const(n) => i32::try_from(n).ok(),
            _ => None,
        }
    }
    /// displacement `e + idx*scale` when `idx` is a constant and the sum fits in 32 bits
    fn imm_disp(&self, idx: V, scale: u8, e: i32) -> Option<i32> {
        let c = self.imm(idx)?;
        i32::try_from(c as i64 * scale as i64 + e as i64).ok()
    }
    /// put the value of `v` into register `dst`
    fn mov_to(&mut self, dst: u8, v: V) {
        match self.loc(v) {
            Loc::Reg(r) => self.a.mov_rr(dst, r),
            Loc::Xmm(x) => self.a.movq_rx(dst, x),
            Loc::Spill(s) => {
                let d = self.spill_disp(s);
                self.a.load(dst, d)
            }
            Loc::Const(n) => self.a.mov_imm(dst, n),
            Loc::Slot(k) => {
                let d = self.slot_off[k];
                self.a.lea(dst, RBP, d)
            }
            Loc::Text(t) => self.a.lea_data(dst, D_TEXT0 + t),
            Loc::None => {}
        }
    }
    /// a register holding the value of `v` (read-only!); `scratch` is used when it is not in one
    fn reg(&mut self, v: V, scratch: u8) -> u8 {
        if let Loc::Reg(r) = self.loc(v) {
            return r;
        }
        self.mov_to(scratch, v);
        scratch
    }
    /// base register and extra displacement to address memory through the pointer `v`
    fn addr(&mut self, v: V, scratch: u8) -> (u8, i32) {
        if let Loc::Slot(k) = self.loc(v) {
            return (RBP, self.slot_off[k]);
        }
        (self.reg(v, scratch), 0)
    }
    /// register in which to compute the result for `d`
    fn dreg(&self, d: V, scratch: u8) -> u8 {
        match self.loc(d) {
            Loc::Reg(r) => r,
            _ => scratch,
        }
    }
    /// the result of `d` is in `r` (== dreg(d)): write it back when `d` is spilled
    fn fin(&mut self, d: V, r: u8) {
        match self.loc(d) {
            Loc::Spill(s) => {
                let disp = self.spill_disp(s);
                self.a.store(disp, r);
            }
            Loc::Xmm(x) => self.a.movq_xr(x, r),
            _ => {}
        }
    }
    // ── floating point: values in xmm registers (Loc::Xmm), spill slots, GPRs or constants ──
    /// load the float `v` into the xmm register `x`
    fn fmov_to(&mut self, x: u8, v: V) {
        match self.loc(v) {
            Loc::Xmm(y) => self.a.movaps(x, y),
            Loc::Reg(r) => self.a.movq_xr(x, r),
            Loc::Spill(s) => {
                let d = self.spill_disp(s);
                self.a.sse_rm(0xF2, 0x10, x, RBP, d);
            }
            Loc::Const(n) => self.fconst(x, n as u64),
            _ => {}
        }
    }
    fn fconst(&mut self, x: u8, bits: u64) {
        if bits == 0 {
            self.a.xorps(x);
        } else {
            self.a.sse_pool(0xF2, 0x10, x, bits);
        }
    }
    /// an xmm register holding `v` (read-only); `scr` is used when it is not in one
    fn fx(&mut self, v: V, scr: u8) -> u8 {
        if let Loc::Xmm(x) = self.loc(v) {
            return x;
        }
        self.fmov_to(scr, v);
        scr
    }
    /// `prefix 0F op dst, v` where the source operand may be a register or memory
    fn fop(&mut self, prefix: u8, op: u8, dst: u8, v: V) {
        match self.loc(v) {
            Loc::Xmm(y) => self.a.sse_rr(prefix, op, dst, y),
            Loc::Spill(s) => {
                let d = self.spill_disp(s);
                self.a.sse_rm(prefix, op, dst, RBP, d);
            }
            Loc::Const(n) if n != 0 => self.a.sse_pool(prefix, op, dst, n as u64),
            _ => {
                let y = self.fx(v, 1);
                self.a.sse_rr(prefix, op, dst, y);
            }
        }
    }
    /// register in which to compute a float result for `d`
    fn fdreg(&self, d: V) -> u8 {
        match self.loc(d) {
            Loc::Xmm(x) => x,
            _ => 0,
        }
    }
    /// the float result is in xmm `x` (== fdreg(d)): move it to its location
    fn ffin(&mut self, d: V, x: u8) {
        match self.loc(d) {
            Loc::Xmm(y) => self.a.movaps(y, x),
            Loc::Reg(r) => self.a.movq_rx(r, x),
            Loc::Spill(s) => {
                let disp = self.spill_disp(s);
                self.a.sse_rm(0xF2, 0x11, x, RBP, disp);
            }
            _ => {}
        }
    }
    /// flags for `a < b` / `a <= b` (condition code that holds when true; false when unordered)
    fn fcmp_flags(&mut self, op: NOp, a: V, b: V) -> u8 {
        let xb = self.fx(b, 0);
        self.fop(0x66, 0x2E, xb, a); // ucomisd xb, a: flags of b ? a
        if op == NOp::FLt {
            CC_A
        } else {
            CC_AE
        }
    }
    fn is_remat(&self, d: V) -> bool {
        matches!(self.loc(d), Loc::Const(_) | Loc::Slot(_) | Loc::Text(_))
    }
    fn push_v(&mut self, v: V) {
        match self.loc(v) {
            Loc::Reg(r) => self.a.push(r),
            Loc::Spill(s) => {
                let d = self.spill_disp(s);
                self.a.push_slot(d)
            }
            Loc::Const(n) if i32::try_from(n).is_ok() => self.a.push_imm(n as i32),
            _ => {
                self.mov_to(RAX, v);
                self.a.push(RAX)
            }
        }
    }

    /// `cmp x, y`; returns the condition code that holds when `x op y`
    fn cmp_flags(&mut self, op: Op, x: V, y: V) -> u8 {
        let (mut op, mut x, mut y) = (op, x, y);
        if self.imm(x).is_some() && self.imm(y).is_none() {
            std::mem::swap(&mut x, &mut y);
            op = swap_cc(op);
        }
        let xr = self.reg(x, RAX);
        if let Some(i) = self.imm(y) {
            self.a.cmp_imm(xr, i);
        } else {
            let yr = self.reg(y, RCX);
            self.a.alu(0x39, xr, yr);
        }
        cc_of(op)
    }

    fn arith(&mut self, d: V, op: Op, x: V, y: V) {
        let chk = !op.is_unchecked(); // proven non-overflowing by the range analysis: no `jo`
        let op = op.checked();
        let (mut x, mut y) = (x, y);
        let comm = matches!(op, Op::Add | Op::Mul);
        if comm && self.imm(x).is_some() && self.imm(y).is_none() {
            std::mem::swap(&mut x, &mut y);
        }
        let dr = self.dreg(d, RAX);
        if comm && self.in_reg(y, dr) && !self.in_reg(x, dr) {
            std::mem::swap(&mut x, &mut y);
        }
        let kind = match op {
            Op::Add => P_ADD,
            Op::Sub => P_SUB,
            _ => P_MUL,
        };
        let work = if !comm && self.in_reg(y, dr) && !self.in_reg(x, dr) {
            RAX
        } else {
            dr
        };
        // unchecked add / multiply by a small constant: lea / shl (no flags needed)
        if !chk && matches!(op, Op::Add | Op::Sub | Op::Mul) {
            if let Some(c) = self.imm(y) {
                match op {
                    Op::Add | Op::Sub => {
                        let c = if op == Op::Sub { -(c as i64) } else { c as i64 };
                        if let Ok(c) = i32::try_from(c) {
                            let xr = self.reg(x, RCX);
                            if xr == dr {
                                self.a.add_imm(dr, c);
                            } else {
                                self.a.lea(dr, xr, c);
                            }
                            self.fin(d, dr);
                            return;
                        }
                    }
                    _ => {
                        if c >= 2 && (c as u32).is_power_of_two() {
                            let xr = self.reg(x, RCX);
                            self.a.mov_rr(dr, xr);
                            self.a.shift_imm(4, dr, c.trailing_zeros() as u8);
                            self.fin(d, dr);
                            return;
                        }
                        if matches!(c, 3 | 5 | 9) {
                            let xr = self.reg(x, RCX);
                            self.a.rm_sib(0x8D, dr, xr, xr, (c - 1) as u8, 0);
                            self.fin(d, dr);
                            return;
                        }
                    }
                }
            } else if op == Op::Add {
                let (xr, yr) = (self.reg(x, RCX), self.reg(y, RDX));
                if xr == dr {
                    self.a.alu(0x01, dr, yr);
                } else if yr == dr {
                    self.a.alu(0x01, dr, xr);
                } else {
                    self.a.rm_sib(0x8D, dr, xr, yr, 1, 0);
                }
                self.fin(d, dr);
                return;
            }
        }
        // negation: 0 - y
        if op == Op::Sub && self.imm(x) == Some(0) && self.imm(y).is_none() {
            let yr = self.reg(y, RCX);
            self.a.mov_rr(dr, yr);
            self.a.f7(3, dr);
            if chk {
                let l = self.pl(kind);
                self.a.jcc(CC_O, l);
            }
            self.fin(d, dr);
            return;
        }
        if op == Op::Mul && self.imm(y).is_some() {
            let xr = self.reg(x, RCX);
            self.a.imul_imm(work, xr, self.imm(y).unwrap());
        } else {
            self.mov_to(work, x);
            if let Some(i) = self.imm(y) {
                match op {
                    Op::Add => self.a.add_imm(work, i),
                    Op::Sub => self.a.sub_imm(work, i),
                    _ => {
                        self.a.mov_imm(RCX, i as i64);
                        self.a.imul(work, RCX);
                    }
                }
            } else {
                let yr = self.reg(y, RCX);
                match op {
                    Op::Add => self.a.alu(0x01, work, yr),
                    Op::Sub => self.a.alu(0x29, work, yr),
                    _ => self.a.imul(work, yr),
                }
            }
        }
        if chk {
            let l = self.pl(kind);
            self.a.jcc(CC_O, l);
        }
        self.a.mov_rr(dr, work);
        self.fin(d, dr);
    }

    fn divmod(&mut self, d: V, op: Op, x: V, y: V) {
        let unchecked = op.is_unchecked(); // divisor proven non-zero and not (MIN / -1)
        let op = op.checked();
        let is_div = op == Op::Div;
        if is_div {
            if let Loc::Const(c) = self.loc(y) {
                if c >= 2 && (c as u64).is_power_of_two() {
                    let shift = c.trailing_zeros() as u8;
                    self.mov_to(RAX, x);
                    self.a.mov_rr(RCX, RAX);
                    if shift == 1 {
                        self.a.shift_imm(5, RCX, 63); // shr: one for a negative numerator
                    } else {
                        self.a.shift_imm(7, RCX, 63); // sar: all ones or all zeros
                        self.a.shift_imm(5, RCX, 64 - shift); // shr: reduce to c - 1 or zero
                    }
                    // Adding the bias makes the arithmetic shift truncate toward zero.
                    self.a.alu(0x01, RAX, RCX);
                    self.a.shift_imm(7, RAX, shift);
                    let dr = self.dreg(d, RAX);
                    self.a.mov_rr(dr, RAX);
                    self.fin(d, dr);
                    return;
                }
            }
        }
        // A signed remainder by a positive power of two can use a mask while
        // preserving Rust/C truncation-toward-zero semantics for negative x.
        // This also handles constants outside the sign-extended i32 immediate
        // range (for example 2^31), which otherwise fell through to `idiv`.
        if !is_div {
            if let Loc::Const(c) = self.loc(y) {
                if c > 0 && (c as u64).is_power_of_two() {
                    let mask = c - 1;
                    self.mov_to(RAX, x);
                    self.a.mov_rr(RCX, RAX);
                    self.a.shift_imm(7, RCX, 63); // sign mask: -1 for x < 0
                    self.a.mov_imm(RDX, mask);
                    self.a.alu(0x21, RCX, RDX); // bias = sign & mask
                    self.a.alu(0x01, RAX, RCX); // x + bias
                    self.a.alu(0x21, RAX, RDX); // low bits
                    self.a.alu(0x29, RAX, RCX); // remove bias
                    let dr = self.dreg(d, RAX);
                    self.a.mov_rr(dr, RAX);
                    self.fin(d, dr);
                    return;
                }
            }
        }
        let magic_c = self.imm(y).filter(|c| c.unsigned_abs() >= 2);
        let mut yr = RCX;
        if magic_c.is_none() {
            self.mov_to(RAX, x);
            yr = if let Some(i) = self.imm(y) {
                self.a.mov_imm(RCX, i as i64);
                RCX
            } else {
                self.reg(y, RCX)
            };
        }
        let mut res = RAX;
        let safe = matches!(self.imm(y), Some(i) if i != 0 && i != -1);
        if let Some(c) = magic_c {
            // division by a constant: multiply by the "magic" reciprocal (Hacker's Delight, signed)
            let (m, sh) = magic(c as i64);
            self.mov_to(RCX, x); // rcx = n
            self.a.mov_imm(RAX, m);
            self.a.f7(5, RCX); // imul rcx: rdx:rax = m * n
            if c > 0 && m < 0 {
                self.a.alu(0x01, RDX, RCX);
            } else if c < 0 && m > 0 {
                self.a.alu(0x29, RDX, RCX);
            }
            if sh > 0 {
                self.a.shift_imm(7, RDX, sh as u8); // sar
            }
            self.a.mov_rr(RAX, RDX);
            self.a.shift_imm(5, RAX, 63); // shr: sign bit
            self.a.alu(0x01, RDX, RAX); // rdx = n / c
            if is_div {
                self.a.mov_rr(RAX, RDX);
            } else {
                self.a.imul_imm(RDX, RDX, c);
                self.a.alu(0x29, RCX, RDX); // n - (n/c)*c
                res = RCX;
            }
        } else if safe || unchecked {
            self.a.raw(&[0x48, 0x99]); // cqo
            self.a.f7(7, yr); // idiv
            if !is_div {
                self.a.mov_rr(RAX, RDX);
            }
        } else {
            let (l_do, l_done) = (self.a.label(), self.a.label());
            self.a.test_rr(yr);
            let l0 = self.pl(if is_div { P_DIV0 } else { P_MOD0 });
            self.a.jcc(CC_E, l0);
            self.a.cmp_imm(yr, -1);
            self.a.jcc(CC_NE, l_do);
            if is_div {
                self.a.f7(3, RAX); // neg rax (x / -1)
                let l = self.pl(P_DIV);
                self.a.jcc(CC_O, l);
            } else {
                self.a.raw(&[0x31, 0xC0]); // xor eax, eax (x % -1 == 0)
            }
            self.a.jmp(l_done);
            self.a.bind(l_do);
            self.a.raw(&[0x48, 0x99]); // cqo
            self.a.f7(7, yr); // idiv
            if !is_div {
                self.a.mov_rr(RAX, RDX);
            }
            self.a.bind(l_done);
        }
        let dr = self.dreg(d, RAX);
        self.a.mov_rr(dr, res);
        self.fin(d, dr);
    }

    /// Number operations (`Inst::N`). Floats are bit patterns in ordinary registers; each operation moves its
    /// operands through xmm0/xmm1.
    fn numop(&mut self, op: NOp, d: V, a: V, b: V) {
        match op {
            NOp::FAdd | NOp::FSub | NOp::FMul | NOp::FDiv => {
                let code = match op {
                    NOp::FAdd => 0x58,
                    NOp::FSub => 0x5C,
                    NOp::FMul => 0x59,
                    _ => 0x5E,
                };
                let (mut a, mut b) = (a, b);
                let comm = matches!(op, NOp::FAdd | NOp::FMul);
                let dx = self.fdreg(d);
                let in_dx = |g: &Self, v: V| g.loc(v) == Loc::Xmm(dx) && dx != 0;
                if comm && in_dx(self, b) && !in_dx(self, a) {
                    std::mem::swap(&mut a, &mut b);
                }
                let work = if in_dx(self, b) && !in_dx(self, a) {
                    0
                } else {
                    dx
                };
                self.fmov_to(work, a);
                self.fop(0xF2, code, work, b);
                if let Loc::Xmm(y) = self.loc(d) {
                    self.a.movaps(y, work);
                } else {
                    self.ffin(d, work);
                }
            }
            NOp::FSqrt => {
                let dx = self.fdreg(d);
                self.fop(0xF2, 0x51, dx, a);
                self.ffin(d, dx);
            }
            NOp::FLt | NOp::FLe => {
                let cc = self.fcmp_flags(op, a, b);
                let dr = self.dreg(d, RAX);
                self.a.setcc_to(cc, dr);
                self.fin(d, dr);
            }
            NOp::FEq | NOp::FNe => {
                let xa = self.fx(a, 0);
                self.fop(0x66, 0x2E, xa, b); // ucomisd xa, b
                let dr = self.dreg(d, RAX);
                if op == NOp::FEq {
                    self.a.raw(&[0x0F, 0x90 + CC_E, 0xC0]); // sete al
                    self.a.raw(&[0x0F, 0x90 + CC_NP, 0xC1]); // setnp cl
                    self.a.raw(&[0x20, 0xC8]); // and al, cl
                } else {
                    self.a.raw(&[0x0F, 0x90 + CC_NE, 0xC0]); // setne al
                    self.a.raw(&[0x0F, 0x90 + CC_P, 0xC1]); // setp cl
                    self.a.raw(&[0x08, 0xC8]); // or al, cl
                }
                self.a.rex(false, dr, 0);
                self.a.raw(&[0x0F, 0xB6]);
                self.a.modrm_reg(dr, RAX); // movzx dr, al
                self.fin(d, dr);
            }
            NOp::FNeg => {
                if let Loc::Xmm(dx) = self.loc(d) {
                    self.a.sse_pool(0xF2, 0x10, 1, 0x8000_0000_0000_0000); // movsd xmm1, [sign mask]
                    self.fmov_to(dx, a);
                    self.a.sse_rr(0x66, 0x57, dx, 1); // xorpd dx, xmm1
                } else {
                    let dr = self.dreg(d, RAX);
                    self.mov_to(dr, a);
                    self.a.btc_imm(dr, 63);
                    self.fin(d, dr);
                }
            }
            NOp::I2F | NOp::I2S => {
                let ar = self.reg(a, RAX);
                if op == NOp::I2F {
                    let dx = self.fdreg(d);
                    self.a.xorps(dx);
                    self.a.cvtsi2fp(0xF2, dx, ar);
                    self.ffin(d, dx);
                } else {
                    self.a.cvtsi2fp(0xF3, 0, ar);
                    let dr = self.dreg(d, RAX);
                    self.a.movd_rx(dr, 0);
                    self.fin(d, dr);
                }
            }
            NOp::S2F => {
                let ar = self.reg(a, RAX);
                self.a.movd_xr(0, ar);
                let dx = self.fdreg(d);
                self.a.sse_rr(0xF3, 0x5A, dx, 0); // cvtss2sd
                self.ffin(d, dx);
            }
            NOp::F2S => {
                let xa = self.fx(a, 0);
                self.a.sse_rr(0xF2, 0x5A, 0, xa); // cvtsd2ss xmm0, xa
                let dr = self.dreg(d, RAX);
                self.a.movd_rx(dr, 0);
                self.fin(d, dr);
            }
            NOp::F2I => {
                let xa = self.fx(a, 0);
                self.a.cvttsd2si(RAX, xa);
                // 0x8000000000000000 means NaN / out of range, unless the input really is -2^63
                let ok = self.a.label();
                self.a.cmp_imm(RAX, 1);
                self.a.jcc(0x1, ok); // jno: rax != i64::MIN
                self.a.movq_rx(RCX, xa);
                self.a.mov_imm(RDX, 0xC3E0_0000_0000_0000u64 as i64);
                self.a.alu(0x39, RCX, RDX);
                let l = self.pl(P_F2I);
                self.a.jcc(CC_NE, l);
                self.a.bind(ok);
                let dr = self.dreg(d, RAX);
                self.a.mov_rr(dr, RAX);
                self.fin(d, dr);
            }
            NOp::Chk(rng, msg) => {
                let (lo, hi) = rng.bounds();
                let ar = self.reg(a, RAX);
                let l = if let (Rng::Iv(..), MSG_CONV) = (rng, msg) {
                    // the message names the value and the bounds: a cold stub per check loads them
                    let l = self.a.label();
                    let loc = self.locs[self.cur.0][self.cur.1];
                    self.stubs.push((l, ar, lo, hi, loc, self.no_frame));
                    l
                } else {
                    self.pl(CHK_PANIC[msg as usize])
                };
                if rng == Rng::NonNeg {
                    self.a.test_rr(ar);
                    self.a.jcc(CC_S, l);
                } else {
                    // lo <= a <= hi  <=>  (a - lo) <= (hi - lo) as unsigned
                    self.a.mov_rr(RCX, ar);
                    if lo != 0 {
                        self.a.mov_imm(RDX, lo);
                        self.a.alu(0x29, RCX, RDX); // sub rcx, rdx
                    }
                    self.a.mov_imm(RDX, hi.wrapping_sub(lo));
                    self.a.alu(0x39, RCX, RDX);
                    self.a.jcc(CC_A, l);
                }
                let dr = self.dreg(d, RAX);
                self.mov_to(dr, a);
                self.fin(d, dr);
            }
            NOp::Ext(rng) => {
                let dr = self.dreg(d, RAX);
                self.mov_to(dr, a);
                let sh = (64 - rng.bits()) as u8;
                self.a.shift_imm(4, dr, sh); // shl
                self.a.shift_imm(if rng.signed() { 7 } else { 5 }, dr, sh); // sar / shr
                self.fin(d, dr);
            }
            NOp::WAdd => self.arith(d, Op::AddU, a, b),
            NOp::WSub => self.arith(d, Op::SubU, a, b),
            NOp::WMul => self.arith(d, Op::MulU, a, b),
            NOp::UAdd | NOp::USub => {
                // unsigned carry / borrow
                let dr = self.dreg(d, RAX);
                let work = if self.in_reg(b, dr) && !self.in_reg(a, dr) {
                    RAX
                } else {
                    dr
                };
                self.mov_to(work, a);
                let br = self.reg(b, RCX);
                self.a
                    .alu(if op == NOp::UAdd { 0x01 } else { 0x29 }, work, br);
                let l = self.pl(if op == NOp::UAdd { P_ADD } else { P_SUB });
                self.a.jcc(CC_B, l);
                self.a.mov_rr(dr, work);
                self.fin(d, dr);
            }
            NOp::UMul => {
                self.mov_to(RAX, a);
                let br = self.reg(b, RCX);
                self.a.f7(4, br); // mul: rdx:rax = rax * b
                let l = self.pl(P_MUL);
                self.a.jcc(CC_O, l);
                let dr = self.dreg(d, RAX);
                self.a.mov_rr(dr, RAX);
                self.fin(d, dr);
            }
            NOp::UDiv | NOp::UMod => {
                self.mov_to(RAX, a);
                let br = self.reg(b, RCX);
                self.a.test_rr(br);
                let l = self.pl(if op == NOp::UDiv { P_DIV0 } else { P_MOD0 });
                self.a.jcc(CC_E, l);
                self.a.raw(&[0x31, 0xD2]); // xor edx, edx
                self.a.f7(6, br); // div
                let dr = self.dreg(d, RAX);
                self.a.mov_rr(dr, if op == NOp::UDiv { RAX } else { RDX });
                self.fin(d, dr);
            }
            NOp::ULt | NOp::ULe => {
                let ar = self.reg(a, RAX);
                let br = self.reg(b, RCX);
                self.a.alu(0x39, ar, br);
                let dr = self.dreg(d, RAX);
                self.a
                    .setcc_to(if op == NOp::ULt { CC_B } else { CC_BE }, dr);
                self.fin(d, dr);
            }
        }
    }

    /// an instruction that calls into the runtime or a function, inside a cold block: the live caller-saved
    /// registers are saved around it (the allocator did not count it as a call)
    fn inst(&mut self, ins: &Inst, fused: bool) {
        let saved = self.al.saves.get(&self.cur).cloned();
        if let Some((gs, xs)) = &saved {
            for r in gs {
                self.a.push(*r);
            }
            for x in xs {
                self.a.sub_imm(RSP, 8);
                self.a.sse_rm(0xF2, 0x11, *x, RSP, 0);
            }
        }
        self.inst_inner(ins, fused);
        if let Some((gs, xs)) = &saved {
            for x in xs.iter().rev() {
                self.a.sse_rm(0xF2, 0x10, *x, RSP, 0);
                self.a.add_imm(RSP, 8);
            }
            for r in gs.iter().rev() {
                self.a.pop(*r);
            }
        }
    }

    fn inst_inner(&mut self, ins: &Inst, fused: bool) {
        match ins {
            Inst::N(op, d, a, b) => self.numop(*op, *d, *a, *b),
            Inst::Loc(_) => {} // stripped before code generation
            Inst::Const(d, n) => {
                if let Loc::Xmm(x) = self.loc(*d) {
                    self.fconst(x, *n as u64);
                } else if !self.is_remat(*d) {
                    let dr = self.dreg(*d, RAX);
                    self.a.mov_imm(dr, *n);
                    self.fin(*d, dr);
                }
            }
            Inst::Text(d, id) => {
                if !self.is_remat(*d) {
                    let dr = self.dreg(*d, RAX);
                    self.a.lea_data(dr, D_TEXT0 + id);
                    self.fin(*d, dr);
                }
            }
            Inst::Slot(d, k) => {
                if !self.is_remat(*d) {
                    let dr = self.dreg(*d, RAX);
                    let off = self.slot_off[*k];
                    self.a.lea(dr, RBP, off);
                    self.fin(*d, dr);
                }
            }
            Inst::Mov(d, s)
                if matches!(self.loc(*d), Loc::Xmm(_)) || matches!(self.loc(*s), Loc::Xmm(_)) =>
            {
                match (self.loc(*d), self.loc(*s)) {
                    (Loc::Xmm(x), _) => self.fmov_to(x, *s),
                    (_, Loc::Xmm(x)) => self.ffin(*d, x),
                    _ => unreachable!(),
                }
            }
            Inst::Mov(d, s) => {
                let dr = self.dreg(*d, RAX);
                self.mov_to(dr, *s);
                self.fin(*d, dr);
            }
            Inst::Neg(d, s) => {
                let dr = self.dreg(*d, RAX);
                self.mov_to(dr, *s);
                self.a.f7(3, dr);
                let l = self.pl(P_NEG);
                self.a.jcc(CC_O, l);
                self.fin(*d, dr);
            }
            Inst::Not(d, s) => {
                let dr = self.dreg(*d, RAX);
                self.mov_to(dr, *s);
                self.a.alu_imm(6, dr, 1); // xor dr, 1
                self.fin(*d, dr);
            }
            Inst::Bin(d, op, x, y) => match op {
                Op::Add | Op::Sub | Op::Mul | Op::AddU | Op::SubU | Op::MulU => {
                    self.arith(*d, *op, *x, *y)
                }
                Op::Div | Op::Mod | Op::DivU | Op::ModU => self.divmod(*d, *op, *x, *y),
                _ => {
                    let cc = self.cmp_flags(*op, *x, *y);
                    if !fused {
                        let dr = self.dreg(*d, RAX);
                        self.a.setcc_to(cc, dr);
                        self.fin(*d, dr);
                        self.a.cmp_rec = (*d, cc, self.a.code.len());
                    }
                }
            },
            Inst::VTableAddr(d, id) => {
                let dr = self.dreg(*d, RAX);
                self.a.lea_data(dr, self.rt.vtable_blob + id);
                self.fin(*d, dr);
            }
            Inst::FnAddr(d, fid) => {
                let dr = self.dreg(*d, RAX);
                let l = self.rt.funcs[*fid];
                self.a.lea_label(dr, l);
                self.fin(*d, dr);
            }
            Inst::ExternCall(dst, id, args, float_args, ret_float) => {
                self.extern_call(*dst, *id, args, float_args, *ret_float)
            }
            Inst::Call(dst, _, args) | Inst::CallInd(dst, _, args) => {
                let nreg = args.len().min(ARG_REGS.len());
                for arg in args[nreg..].iter().rev() {
                    self.push_v(*arg);
                }
                // allocatable registers never include rdi/rsi/rdx/rcx, so these moves cannot interfere
                for (k, arg) in args[..nreg].iter().enumerate() {
                    self.mov_to(ARG_REGS[k], *arg);
                }
                match ins {
                    Inst::Call(_, fid, _) => {
                        let l = self.rt.funcs[*fid];
                        self.a.call(l);
                        if self.rt.lib[*fid] {
                            self.note_site();
                        }
                    }
                    Inst::CallInd(_, code, _) => {
                        // rax is neither an argument register nor allocatable
                        self.mov_to(RAX, *code);
                        self.a.call_r(RAX);
                    }
                    _ => unreachable!(),
                }
                if args.len() > nreg {
                    self.a.add_imm(RSP, 8 * (args.len() - nreg) as i32);
                }
                if let Some(d) = dst {
                    let dr = self.dreg(*d, RAX);
                    self.a.mov_rr(dr, RAX);
                    self.fin(*d, dr);
                }
            }
            Inst::Load(d, ad, off) | Inst::LoadH(d, ad, off)
                if matches!(self.loc(*d), Loc::Xmm(_)) =>
            {
                let (b, e) = self.addr(*ad, RAX);
                let x = self.fdreg(*d);
                self.a.sse_rm(0xF2, 0x10, x, b, off + e);
            }
            Inst::Load(d, ad, off) | Inst::LoadH(d, ad, off) => {
                let (b, e) = self.addr(*ad, RAX);
                let dr = self.dreg(*d, RAX);
                self.a.ld(dr, b, off + e);
                self.fin(*d, dr);
            }
            Inst::Store(ad, off, v) if matches!(self.loc(*v), Loc::Xmm(_)) => {
                let (b, e) = self.addr(*ad, RAX);
                let x = self.fx(*v, 0);
                self.a.sse_rm(0xF2, 0x11, x, b, off + e);
            }
            Inst::Store(ad, off, v) => {
                let (b, e) = self.addr(*ad, RAX);
                let vr = self.reg(*v, RCX);
                self.a.st(b, off + e, vr);
            }
            Inst::LoadX(d, b, i, 0) => {
                // scale 0 = packed byte element (List<Bool>)
                let (br, e) = self.addr(*b, RAX);
                let dr = self.dreg(*d, RAX);
                if let Some(c) = self.imm_disp(*i, 1, e) {
                    self.a.movzx8_mem(dr, br, c);
                } else {
                    let ir = self.reg(*i, RCX);
                    self.a.byte_sib(true, dr, br, ir, e);
                }
                self.fin(*d, dr);
            }
            Inst::StoreX(b, i, 0, v) => {
                let (br, e) = self.addr(*b, RAX);
                let vr = self.reg(*v, RDX);
                if let Some(c) = self.imm_disp(*i, 1, e) {
                    self.a.mov8_mem(br, c, vr);
                } else {
                    let ir = self.reg(*i, RCX);
                    self.a.byte_sib(false, vr, br, ir, e);
                }
            }
            Inst::LoadX(d, b, i, k) if *k != 0 && matches!(self.loc(*d), Loc::Xmm(_)) => {
                let (br, e) = self.addr(*b, RAX);
                let x = self.fdreg(*d);
                if let Some(c) = self.imm_disp(*i, *k, e) {
                    self.a.sse_rm(0xF2, 0x10, x, br, c);
                } else {
                    let ir = self.reg(*i, RCX);
                    self.a.sse_sib(0xF2, 0x10, x, br, ir, *k, e);
                }
            }
            Inst::StoreX(b, i, k, v) if *k != 0 && matches!(self.loc(*v), Loc::Xmm(_)) => {
                let (br, e) = self.addr(*b, RAX);
                let x = self.fx(*v, 0);
                if let Some(c) = self.imm_disp(*i, *k, e) {
                    self.a.sse_rm(0xF2, 0x11, x, br, c);
                } else {
                    let ir = self.reg(*i, RCX);
                    self.a.sse_sib(0xF2, 0x11, x, br, ir, *k, e);
                }
            }
            Inst::LoadX(d, b, i, k) => {
                let (br, e) = self.addr(*b, RAX);
                let dr = self.dreg(*d, RAX);
                if let Some(c) = self.imm_disp(*i, *k, e) {
                    self.a.ld(dr, br, c);
                } else {
                    let ir = self.reg(*i, RCX);
                    self.a.rm_sib(0x8B, dr, br, ir, *k, e);
                }
                self.fin(*d, dr);
            }
            Inst::StoreX(b, i, k, v) => {
                let (br, e) = self.addr(*b, RAX);
                let vr = self.reg(*v, RDX);
                if let Some(c) = self.imm_disp(*i, *k, e) {
                    self.a.st(br, c, vr);
                } else {
                    let ir = self.reg(*i, RCX);
                    self.a.rm_sib(0x89, vr, br, ir, *k, e);
                }
            }
            Inst::Sel(d, c, a, b) => {
                // d = c ? a : b without a branch: rax = b; test c; cmovne rax, a
                // when the comparison that made `c` was the last thing emitted, its flags are still live (movs
                // do not touch them): cmp + cmovcc, no setcc/test round trip
                let rec = self.a.cmp_rec;
                let flags = (rec.0 == *c && rec.2 == self.a.code.len() && self.a.last_bind != rec.2).then_some(rec.1);
                self.mov_to(RAX, *b);
                let ar = self.reg(*a, RDX);
                match flags {
                    Some(cc) => self.a.cmov_rr(cc, RAX, ar),
                    None => {
                        let cr = self.reg(*c, RCX);
                        self.a.test_rr(cr);
                        self.a.cmov_rr(CC_NE, RAX, ar);
                    }
                }
                let dr = self.dreg(*d, RAX);
                if dr != RAX {
                    self.a.mov_rr(dr, RAX);
                }
                self.fin(*d, dr);
            }
            Inst::LeaX(d, b, i, k) => {
                let (br, e) = self.addr(*b, RAX);
                let dr = self.dreg(*d, RAX);
                if let Some(c) = self.imm_disp(*i, *k, e) {
                    self.a.lea(dr, br, c);
                } else {
                    let ir = self.reg(*i, RCX);
                    self.a.rm_sib(0x8D, dr, br, ir, *k, e);
                }
                self.fin(*d, dr);
            }
            Inst::Lea(d, ad, off) => {
                let (b, e) = self.addr(*ad, RAX);
                let dr = self.dreg(*d, RAX);
                self.a.lea(dr, b, off + e);
                self.fin(*d, dr);
            }
            Inst::Copy(dst, src, n) => {
                let (db, de) = self.addr(*dst, RSI);
                let (sb, se) = self.addr(*src, RDI);
                for i in 0..*n as i32 {
                    self.a.ld(RAX, sb, se + 8 * i);
                    self.a.st(db, de + 8 * i, RAX);
                }
            }
            Inst::ListPush(l, v) => {
                self.mov_to(RDI, *l);
                self.mov_to(RSI, *v);
                let t = self.rt.push;
                self.a.call(t);
            }
            Inst::ListGet(d, l, i) => {
                self.mov_to(RDI, *l);
                self.mov_to(RSI, *i);
                // Keep the checked list load in the caller. These accesses sit in
                // tight loops (notably sorting and map probing); a runtime call
                // per element prevents the native backend from matching C.
                self.a.ld(RAX, RDI, 8);
                self.a.alu(0x39, RSI, RAX);
                let l = self.pl(P_INDEX);
                self.a.jcc(CC_AE, l);
                self.a.ld(RDX, RDI, 0);
                self.a.shl(RSI, 3);
                self.a.alu(0x01, RDX, RSI);
                self.a.ld(RAX, RDX, 0);
                let dr = self.dreg(*d, RAX);
                self.a.mov_rr(dr, RAX);
                self.fin(*d, dr);
            }
            Inst::ListFree(l) => {
                let (b, e) = self.addr(*l, RDI);
                self.a.ld(RDI, b, e);
                let t = self.rt.free;
                self.a.call(t);
            }
            Inst::Alloc(d, n) => {
                self.mov_to(RDI, *n);
                let t = self.rt.alloc;
                self.a.call(t);
                let dr = self.dreg(*d, RAX);
                self.a.mov_rr(dr, RAX);
                self.fin(*d, dr);
            }
            Inst::Thread(op, d, args) => {
                use crate::ir::ThreadOp;
                if let Some(arg) = args.first() {
                    self.mov_to(RDI, *arg);
                }
                match op {
                    ThreadOp::CleanupPush => {
                        self.mov_to(RSI, args[1]);
                        self.mov_to(RDX, args[2]);
                        emit_tls_load(self.a, RAX, THREAD_CLEANUP_HEAD);
                        self.a.st(RDI, 8, RAX);
                        self.a.st(RDI, 16, RSI);
                        self.a.st(RDI, 24, RDX);
                        emit_tls_store(self.a, RDI, THREAD_CLEANUP_HEAD);
                    }
                    ThreadOp::CleanupPop => {
                        self.a.ld(RAX, RDI, 8);
                        emit_tls_store(self.a, RAX, THREAD_CLEANUP_HEAD);
                    }
                    ThreadOp::DropEnter => {
                        emit_tls_load(self.a, RAX, TLS_RECOVERY);
                        self.a.mov_imm(RCX, RECOVERY_IN_DESTRUCTOR);
                        emit_tls_store(self.a, RCX, TLS_RECOVERY);
                    }
                    ThreadOp::DropLeave => emit_tls_store(self.a, RDI, TLS_RECOVERY),
                    ThreadOp::Unlock => {
                        let normal = self.a.label();
                        let exchange = self.a.label();
                        let done = self.a.label();
                        emit_tls_load(self.a, RAX, THREAD_UNWINDING);
                        self.a.test_rax();
                        self.a.jcc(CC_E, normal);
                        self.a.mov_imm(RAX, 3);
                        self.a.mov_imm(RDX, i32::MAX as i64);
                        self.a.jmp(exchange);
                        self.a.bind(normal);
                        self.a.mov_imm(RDX, 1);
                        self.a.bind(exchange);
                        self.a.rm(0x87, RAX, RDI, 0); // exchange the mutex state
                        self.a.cmp_imm(RAX, 2);
                        self.a.jcc(CC_NE, done);
                        emit_futex(self.a, FUTEX_WAKE_PRIVATE);
                        self.a.bind(done);
                    }
                    ThreadOp::Start | ThreadOp::StartRecover => {
                        self.mov_to(RSI, args[1]);
                        self.a
                            .mov_imm(RDX, matches!(op, ThreadOp::StartRecover) as i64);
                        self.a.call(self.rt.thread_start);
                    }
                    ThreadOp::Wait => self.a.call(self.rt.thread_wait),
                    ThreadOp::Park | ThreadOp::Wake => {
                        self.mov_to(RDX, args[1]);
                        let operation = match op {
                            ThreadOp::Park => FUTEX_WAIT_PRIVATE,
                            _ => FUTEX_WAKE_PRIVATE,
                        };
                        emit_futex(self.a, operation);
                    }
                }
                let dr = self.dreg(*d, RAX);
                self.a.mov_rr(dr, RAX);
                self.fin(*d, dr);
            }
            Inst::Atomic(op, d, args) => {
                use crate::ir::AtomicOp;
                self.mov_to(RDI, args[0]);
                match op {
                    AtomicOp::Load => self.a.ld(RAX, RDI, 0),
                    AtomicOp::Store => {
                        self.mov_to(RAX, args[1]);
                        self.a.rm(0x87, RAX, RDI, 0); // xchg provides sequential consistency
                    }
                    AtomicOp::CompareExchange => {
                        self.mov_to(RAX, args[1]); // expected value; instruction returns the observed value
                        self.mov_to(RSI, args[2]);
                        self.a.raw(&[0xF0, 0x48, 0x0F, 0xB1, 0x37]); // lock cmpxchg [rdi], rsi
                    }
                    AtomicOp::FetchAdd => {
                        self.mov_to(RAX, args[1]);
                        self.a.raw(&[0xF0, 0x48, 0x0F, 0xC1, 0x07]); // lock xadd [rdi], rax
                    }
                }
                let dr = self.dreg(*d, RAX);
                self.a.mov_rr(dr, RAX);
                self.fin(*d, dr);
            }
            Inst::Syscall(d, args) => {
                // gather the seven values on the stack first: a value may live in one of the argument registers
                for v in args {
                    self.mov_to(RAX, *v);
                    self.a.push(RAX);
                }
                // Linux x86-64 kernel ABI: rax = number, rdi rsi rdx r10 r8 r9
                for r in [R9, R8, R10, RDX, RSI, RDI, RAX] {
                    self.a.pop(r);
                }
                match self.a.os {
                    Os::Linux => self.a.syscall(), // clobbers rcx and r11
                    Os::Windows => self.a.call(self.a.l_winsyscall),
                }
                let dr = self.dreg(*d, RAX);
                self.a.mov_rr(dr, RAX);
                self.fin(*d, dr);
            }
            Inst::Global(d, n) => {
                let dr = self.dreg(*d, RAX);
                self.a.gaddr(dr, G_USER + 8 * *n as i64);
                self.fin(*d, dr);
            }
            Inst::Free(p) => {
                self.mov_to(RDI, *p);
                let t = self.rt.free;
                self.a.call(t);
            }
            Inst::MemCpy(dst, src, n) => {
                self.mov_to(RDI, *dst);
                self.mov_to(RSI, *src);
                self.mov_to(RCX, *n);
                let t = self.rt.memcpy; // preserves every allocatable register
                self.a.call(t);
            }
            Inst::Realloc(d, old, n, used) => {
                self.mov_to(RDI, *old);
                self.mov_to(RSI, *n);
                self.mov_to(RDX, *used);
                let t = self.rt.realloc;
                self.a.call(t);
                let dr = self.dreg(*d, RAX);
                self.a.mov_rr(dr, RAX);
                self.fin(*d, dr);
            }
            Inst::Load8(d, ad, off) => {
                let (b, e) = self.addr(*ad, RAX);
                let dr = self.dreg(*d, RAX);
                self.a.movzx8_mem(dr, b, off + e);
                self.fin(*d, dr);
            }
            Inst::Store8(ad, off, v) => {
                let (b, e) = self.addr(*ad, RAX);
                let vr = self.reg(*v, RCX);
                self.a.mov8_mem(b, off + e, vr);
            }
            Inst::PrintBytes(p, n) => {
                self.mov_to(RSI, *p);
                self.mov_to(RDX, *n);
                let t = self.rt.print_bytes;
                self.a.call(t);
            }
            Inst::Panic(p, n) => {
                self.mov_to(RSI, *p);
                self.mov_to(RDX, *n);
                if self.linux && self.no_frame {
                    self.a.raw(&[0x55]); // push rbp
                    self.a.mov_rr(RBP, RSP); // a frame record, for the panic routine's walk
                }
                let t = self.rt.panic_bytes;
                self.a.call(t);
                self.note_site();
            }
            Inst::Check(i, n) => {
                let ir = self.reg(*i, RAX);
                if let Some(k) = self.imm(*n) {
                    self.a.cmp_imm(ir, k);
                } else {
                    let nr = self.reg(*n, RCX);
                    self.a.alu(0x39, ir, nr); // unsigned: negative indices fail too
                }
                let l = self.pl(P_INDEX);
                self.a.jcc(CC_AE, l);
            }
            Inst::CheckNonEmpty(n) => {
                let r = self.reg(*n, RAX);
                self.a.test_rr(r);
                let l = self.pl(P_POP);
                self.a.jcc(CC_E, l);
            }
            Inst::Assert(n) => {
                let r = self.reg(*n, RAX);
                self.a.test_rr(r);
                let l = self.pl(P_ASSERT);
                self.a.jcc(CC_E, l);
            }
            Inst::PrintInt(v) => {
                self.mov_to(RAX, *v);
                self.a.mov_imm(RBX, 1); // fd; rbx is caller-saved, nothing live is in it here
                let t = self.rt.print_int;
                self.a.call(t);
            }
            Inst::PrintBool(v) => {
                self.mov_to(RAX, *v);
                let t = self.rt.print_bool;
                self.a.call(t);
            }
            Inst::PrintText(v) => {
                self.mov_to(RAX, *v);
                let t = self.rt.print_text;
                self.a.call(t);
            }
        }
    }

    fn extern_call(
        &mut self,
        dst: Option<V>,
        id: usize,
        args: &[V],
        floats: &[bool],
        ret_float: bool,
    ) {
        // Every function that calls extern code has a frame (`push rbp` + a 16-byte multiple), so rsp is
        // 16-byte aligned between instructions; reserve the stack arguments rounded up to 16 so it is still
        // aligned immediately before CALL, with the first stack argument at [rsp].
        // SysV allocates integer and SSE argument registers independently. An
        // argument goes to memory only after its own register class is full.
        let nstack = args
            .iter()
            .zip(floats)
            .scan((0usize, 0usize), |(gi, xi), (_, is_float)| {
                let stack = if *is_float {
                    let s = *xi >= 8;
                    *xi += 1;
                    s
                } else {
                    let s = *gi >= SYSV_ARG_REGS.len();
                    *gi += 1;
                    s
                };
                Some(stack)
            })
            .filter(|stack| *stack)
            .count();
        let reserve = (nstack * 8 + 15) & !15;
        if reserve != 0 {
            self.a.sub_imm(RSP, reserve as i32);
        }
        let mut gi = 0usize;
        let mut xi = 0usize;
        let mut si = 0usize;
        // Stack arguments occupy consecutive eight-byte slots in source order.
        for (arg, is_float) in args.iter().zip(floats) {
            if *is_float {
                if xi >= 8 {
                    self.fmov_to(0, *arg);
                    self.a.movq_rx(RAX, 0);
                    self.a.st(RSP, (si * 8) as i32, RAX);
                    si += 1;
                }
                xi += 1;
            } else {
                if gi >= SYSV_ARG_REGS.len() {
                    self.mov_to(RAX, *arg);
                    self.a.st(RSP, (si * 8) as i32, RAX);
                    si += 1;
                }
                gi += 1;
            }
        }
        let mut gi = 0usize;
        let mut xi = 0usize;
        for (arg, is_float) in args.iter().zip(floats) {
            if *is_float {
                if xi < 8 {
                    self.fmov_to(xi as u8, *arg);
                }
                xi += 1;
            } else {
                if gi >= SYSV_ARG_REGS.len() {
                    gi += 1;
                    continue;
                }
                self.mov_to(SYSV_ARG_REGS[gi], *arg);
                gi += 1;
            }
        }
        // SysV variadic calls read the number of vector registers used from AL (an upper bound is fine);
        // it is harmless for non-variadic callees, so every extern call sets it. RAX is not an argument register.
        let nxmm = floats.iter().filter(|f| **f).count().min(8);
        self.a.mov_imm(RAX, nxmm as i64);
        self.a.call_elf_got(id);
        if reserve != 0 {
            self.a.add_imm(RSP, reserve as i32);
        }
        if let Some(d) = dst {
            if ret_float {
                self.ffin(d, 0);
            } else {
                let dr = self.dreg(d, RAX);
                self.a.mov_rr(dr, RAX);
                self.fin(d, dr);
            }
        }
    }
}

/// If-conversion: `if c { d = a }` / `if c { d = a } else { d = b }` where every arm is one `Mov`/`Const` into a
/// vreg becomes `Sel` (cmov) in the branching block, so the branch (and its misprediction) disappears. Block numbers
/// do not change; the emptied arms are left unreachable.
fn ifconv(f: &IrFunc) -> Option<IrFunc> {
    let n = f.blocks.len();
    let mut preds = vec![0usize; n];
    for b in &f.blocks {
        for s in crate::opt::succs(&b.term) {
            preds[s] += 1;
        }
    }
    let mut g = IrFunc {
        name: f.name.clone(),
        lib: f.lib,
        debug_bindings: f.debug_bindings,
        nparams: f.nparams,
        vranges: f.vranges.clone(),
        nvregs: f.nvregs,
        slots: f.slots.clone(),
        blocks: f.blocks.iter().map(|b| Block { insts: b.insts.clone(), term: b.term.clone() }).collect(),
    };
    let mut changed = false;
    for bi in 0..n {
        let (c, t, e) = match g.blocks[bi].term {
            Term::Br(c, t, e) => (c, t, e),
            _ => continue,
        };
        // an arm: (block, its single assignment); a branch straight to the join is an empty arm
        let arm = |g: &IrFunc, x: usize| -> Option<(Vec<Inst>, usize)> {
            if x == bi || preds[x] != 1 {
                return None;
            }
            let blk = &g.blocks[x];
            let insts: Vec<Inst> = blk.insts.iter().filter(|i| !matches!(i, Inst::Loc(_))).cloned().collect();
            match blk.term {
                Term::Jmp(j) if insts.len() == 1 && matches!(insts[0], Inst::Mov(d, s) if d != s) || matches!(insts.first(), Some(Inst::Const(..)) if insts.len() == 1) => Some((insts, j)),
                _ => None,
            }
        };
        let (ta, ea) = (arm(&g, t), arm(&g, e));
        let join = match (&ta, &ea) {
            (Some((_, j1)), Some((_, j2))) if j1 == j2 => *j1,
            (Some((_, j)), None) if *j == e && e != bi => *j,
            (None, Some((_, j))) if *j == t && t != bi => *j,
            _ => continue,
        };
        if join == bi {
            continue;
        }
        let mut sels: Vec<Inst> = vec![];
        let mut dsts: Vec<V> = vec![];
        let mut next = g.nvregs;
        let mut prep: Vec<Inst> = vec![];
        let src_of = |i: &Inst, prep: &mut Vec<Inst>, next: &mut usize| -> (V, V) {
            match i {
                Inst::Mov(d, s) => (*d, *s),
                Inst::Const(d, k) => {
                    let tmp = *next;
                    *next += 1;
                    prep.push(Inst::Const(tmp, *k));
                    (*d, tmp)
                }
                _ => unreachable!(),
            }
        };
        let tm = ta.as_ref().map(|(i, _)| src_of(&i[0], &mut prep, &mut next));
        let em = ea.as_ref().map(|(i, _)| src_of(&i[0], &mut prep, &mut next));
        match (tm, em) {
            (Some((d1, s1)), Some((d2, s2))) if d1 == d2 => {
                sels.push(Inst::Sel(d1, c, s1, s2));
                dsts.push(d1);
            }
            (a, b) => {
                if let Some((d, s)) = a {
                    sels.push(Inst::Sel(d, c, s, d));
                    dsts.push(d);
                }
                if let Some((d, s)) = b {
                    sels.push(Inst::Sel(d, c, d, s));
                    dsts.push(d);
                }
            }
        }
        if dsts.contains(&c) {
            continue;
        }
        g.nvregs = next;
        g.vranges.resize(next, None);
        let blk = &mut g.blocks[bi];
        for (k, p) in prep.into_iter().enumerate() {
            blk.insts.insert(k, p);
        }
        blk.insts.extend(sels);
        blk.term = Term::Jmp(join);
        for x in [t, e] {
            if x != join {
                g.blocks[x].insts.clear();
            }
        }
        changed = true;
    }
    changed.then_some(g)
}

fn gen_func(
    a: &mut Asm,
    rt: &Rt,
    f: &IrFunc,
    cold_fn: &[bool],
    debug_scopes: &[usize],
) -> (Vec<i32>, Vec<(usize, usize)>) {
    let converted = if std::env::var_os("YERUX_NO_IFCONV").is_some() || f.debug_bindings { None } else { ifconv(f) };
    let f = converted.as_ref().unwrap_or(f);
    let (laid, cold) = if std::env::var_os("YERUX_NO_COLD").is_some() {
        (None, vec![false; f.blocks.len()])
    } else {
        let (l, c) = crate::layout::cold_to_end(f, cold_fn);
        (Some(l), c)
    };
    let f = laid.as_ref().unwrap_or(f);
    let chained = if std::env::var_os("YERUX_NO_CHAIN").is_some() || f.blocks.len() > 4000 {
        None
    } else {
        Some(crate::layout::chain_layout(f, &cold))
    };
    let (f, cold) = match &chained {
        Some((g, c)) => (g, c.clone()),
        None => (f, cold),
    };
    let rotated = if std::env::var_os("YERUX_NO_ROTATE").is_some() {
        None
    } else {
        Some(crate::layout::rotate_loops(f))
    };
    let f = rotated.as_ref().unwrap_or(f);
    // the location markers leave the instruction stream here: allocation and emission see the same function as
    // without them, and the locations travel beside it
    let (stripped, locs) = strip_locs(f);
    let f = &stripped;
    let al = regalloc::allocate(f, &cold);
    if std::env::var_os("YERUX_RA_STATS").is_some() {
        let regs = al.loc.iter().filter(|l| matches!(l, Loc::Reg(_))).count();
        let remat = al
            .loc
            .iter()
            .filter(|l| matches!(l, Loc::Const(_) | Loc::Slot(_) | Loc::Text(_)))
            .count();
        eprintln!("regalloc {}: {} vregs, {} in registers, {} spilled, {} rematerialised, callee-saved used {:?}", f.name, f.nvregs, regs, al.nspill, remat, al.callee_used);
    }
    // frame: [callee-saved save area][spill slots][aggregate slots] below rbp
    let ncs = al.callee_used.len();
    let spill_base = 8 * ncs as i32;
    let mut words = ncs + al.nspill;
    let mut slot_off = vec![];
    for w in &f.slots {
        words += w;
        slot_off.push(-8 * words as i32);
    }
    let frame = ((words * 8 + 15) & !15) as i32;
    // a function that needs no frame at all (nothing spilled, no slots, no callee-saved registers, all
    // parameters in registers) skips `push rbp; mov rbp, rsp` and returns with a plain `ret`
    // (an extern call needs the frame too: it relies on the `push rbp` to make rsp 16-byte aligned)
    let has_extern = f
        .blocks
        .iter()
        .any(|b| b.insts.iter().any(|i| matches!(i, Inst::ExternCall(..))));
    // (on Linux a function that calls has a frame record too: the panic routine walks the rbp chain)
    let has_call = a.report
        && f.blocks
            .iter()
            .any(|b| b.insts.iter().any(|i| matches!(i, Inst::Call(..) | Inst::CallInd(..))));
    let frameless = frame == 0 && f.nparams <= ARG_REGS.len() && !has_extern && !has_call;
    let callee: Vec<u8> = al.callee_used.clone();
    let nb = f.blocks.len();
    let early = if frameless || a.os == Os::Windows || f.debug_bindings || std::env::var_os("YERUX_NO_SHRINKWRAP").is_some() {
        vec![false; nb]
    } else {
        early_blocks(f, &al)
    };
    let linux = a.report;
    let mut g = G {
        a,
        rt,
        al,
        spill_base,
        slot_off,
        cur: (0, 0),
        stubs: vec![],
        locs,
        pstubs: vec![],
        pmap: HashMap::new(),
        no_frame: frameless,
        linux,
    };
    // frame setup, callee-saved saves and parameter moves (the "prologue"); when `early` blocks exist it is
    // emitted out of line, after the early-exit region
    let prologue = |g: &mut G, tb: usize| {
        if !frameless {
            if g.a.dwarf {
                let at = g.a.code.len();
                g.a.cfi.push((at, crate::dwarf::CFA_ENTRY));
                g.a.cfi.push((at + 1, crate::dwarf::CFA_PUSHED));
                g.a.cfi.push((at + 4, crate::dwarf::CFA_FRAME));
            }
            g.a.raw(&[0x55]); // push rbp
            g.a.raw(&[0x48, 0x89, 0xE5]); // mov rbp, rsp
            if frame > 0 {
                g.a.sub_imm(RSP, frame);
            }
            g.a.func_unwind = Some((frame, g.a.code.len()));
        }
        for (k, r) in callee.iter().enumerate() {
            g.a.store(-8 * (k as i32 + 1), *r);
        }
        for i in 0..f.nparams {
            if !g.al.param_live[tb][i] {
                continue; // dead on entry to `tb`: its register may hold another value by now
            }
            // arg i < 4 arrives in ARG_REGS[i]; the others at [rbp+16+8*(i-4)]
            match (g.loc(i), i < ARG_REGS.len()) {
                (Loc::Reg(r), true) => g.a.mov_rr(r, ARG_REGS[i]),
                (Loc::Reg(r), false) => g.a.load(r, 16 + 8 * (i - ARG_REGS.len()) as i32),
                (Loc::Xmm(x), true) => g.a.movq_xr(x, ARG_REGS[i]),
                (Loc::Xmm(x), false) => {
                    g.a.sse_rm(0xF2, 0x10, x, RBP, 16 + 8 * (i - ARG_REGS.len()) as i32)
                }
                (Loc::Spill(s), true) => {
                    let d = g.spill_disp(s);
                    g.a.store(d, ARG_REGS[i]);
                }
                (Loc::Spill(s), false) => {
                    g.a.load(RAX, 16 + 8 * (i - ARG_REGS.len()) as i32);
                    let d = g.spill_disp(s);
                    g.a.store(d, RAX);
                }
                _ => {}
            }
        }
    };
    let any_early = early.iter().any(|e| *e);
    if g.a.dwarf {
        let at = g.a.code.len();
        g.a.cfi.push((at, crate::dwarf::CFA_ENTRY));
    }
    if !any_early {
        prologue(&mut g, 0);
    }
    let bl: Vec<usize> = f.blocks.iter().map(|_| g.a.label()).collect();
    // trampolines: edge from the early region to the first block that needs the full prologue
    let mut tramp: Vec<Option<usize>> = vec![None; nb];
    if any_early {
        for (bi, b) in f.blocks.iter().enumerate() {
            if early[bi] {
                for t in succs_of(&b.term) {
                    if !early[t] && tramp[t].is_none() {
                        tramp[t] = Some(g.a.label());
                    }
                }
            }
        }
    }
    // Forwarding blocks: after register allocation a block whose moves all became no-ops (`v7 = v4` with both in
    // the same register) and that just jumps on is skipped: edges to it go straight to its target, and it is not
    // emitted at all when nothing else refers to it. This turns `jcc L; jmp M; L:` into `jncc M`.
    let fwd: Vec<Option<usize>> = (0..nb)
        .map(|bi| {
            if bi == 0 || early[bi] || std::env::var_os("YERUX_NO_THREAD").is_some() {
                return None;
            }
            let Term::Jmp(t) = f.blocks[bi].term else {
                return None;
            };
            let noop = f.blocks[bi].insts.iter().all(|i| match i {
                Inst::Mov(d, s) => {
                    d == s
                        || (g.al.loc[*d] == g.al.loc[*s]
                            && matches!(g.al.loc[*d], Loc::Reg(_) | Loc::Xmm(_)))
                }
                _ => false,
            });
            if noop && !early[t] {
                Some(t)
            } else {
                None
            }
        })
        .collect();
    let thread = |mut t: usize| {
        for _ in 0..nb {
            match fwd[t] {
                Some(n) if n != t => t = n,
                _ => break,
            }
        }
        t
    };
    let mut skipped = vec![false; nb];
    if fwd.iter().any(|x| x.is_some()) {
        let mut refs = vec![0usize; nb];
        for (bi, b) in f.blocks.iter().enumerate() {
            if fwd[bi].is_none() {
                for t in succs_of(&b.term) {
                    refs[if early[bi] { t } else { thread(t) }] += 1;
                }
            }
        }
        for bi in 0..nb {
            // a forwarding block on a cycle of forwarders keeps a real jump
            if fwd[bi].is_some() && refs[bi] == 0 && fwd[thread(bi)].is_none() {
                skipped[bi] = true;
            }
        }
    }
    // blocks entered from exactly one place: when that is the fall-through from the block before, nothing can
    // jump to the label, so a reload of a slot stored just before it can be dropped (`Asm::load`)
    let mut npred = vec![0usize; nb];
    for (bi, b) in f.blocks.iter().enumerate() {
        if fwd[bi].is_none() {
            for t in succs_of(&b.term) {
                npred[if early[bi] { t } else { thread(t) }] += 1;
            }
        }
    }
    let no_early = !early.iter().any(|&e| e);
    // innermost loop headers (targets of back edges between hot blocks): aligned to 16 bytes when that costs few NOPs
    let align_max: usize = std::env::var("YERUX_ALIGN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(9);
    let mut hot_header = vec![false; nb];
    if align_max > 0 {
        let mut last = vec![0usize; nb]; // furthest back-edge source per header
        for (j, b) in f.blocks.iter().enumerate() {
            if skipped[j] || cold[j] || early[j] {
                continue;
            }
            for t in succs_of(&b.term) {
                let t = thread(t);
                if t <= j && !cold[t] && !early[t] && !skipped[t] {
                    hot_header[t] = true;
                    last[t] = last[t].max(j);
                }
            }
        }
        for h in 0..nb {
            if hot_header[h] && (h + 1..=last[h]).any(|k| hot_header[k]) {
                hot_header[h] = false; // not innermost
            }
        }
    }
    let mut last_loc = 0u32;
    let mut scopes = vec![];
    let mut record_scope = |offset, loc| {
        if f.debug_bindings {
            let scope = debug_scopes.get(loc as usize).copied().unwrap_or(0);
            if !scopes.last().is_some_and(|&(_, last)| last == scope) {
                scopes.push((offset, scope));
            }
        }
    };
    for (bi, b) in f.blocks.iter().enumerate() {
        if skipped[bi] {
            continue;
        }
        if hot_header[bi] {
            g.a.align_small(align_max);
        }
        let before_bind = g.a.last_bind;
        g.a.bind(bl[bi]);
        if no_early && bi > 0 && npred[bi] == 1 && fwd[bi].is_none() {
            g.a.last_bind = before_bind;
        }
        if g.a.dwarf {
            let at = g.a.code.len();
            g.a.cfi.push((at, if frameless || early[bi] { crate::dwarf::CFA_ENTRY } else { crate::dwarf::CFA_FRAME }));
        }
        g.no_frame = frameless || early[bi];
        // early blocks run before the prologue: parameters are still in their argument registers
        let saved: Vec<Loc> = if early[bi] {
            (0..f.nparams).map(|p| g.al.loc[p]).collect()
        } else {
            vec![]
        };
        if early[bi] {
            for p in 0..f.nparams {
                g.al.loc[p] = Loc::Reg(ARG_REGS[p]);
            }
        }
        // label of an edge, and whether it can fall through
        let nxt = (bi + 1..nb).find(|&j| !skipped[j]).unwrap_or(usize::MAX);
        let th = |t: usize| if early[bi] { t } else { thread(t) };
        let lab = |t: usize| {
            let t = th(t);
            if early[bi] && !early[t] {
                tramp[t].unwrap()
            } else {
                bl[t]
            }
        };
        let next = |t: usize| th(t) == nxt && early[bi] == early.get(nxt).copied().unwrap_or(false);
        let fused = g.al.fused[bi];
        let mut cc = CC_NE;
        for (ii, ins) in b.insts.iter().enumerate() {
            g.cur = (bi, ii);
            if g.a.dwarf {
                let l = g.locs[bi].get(ii).copied().unwrap_or(0);
                if l != 0 && l != last_loc {
                    let at = g.a.code.len();
                    g.a.lines.push((at, l));
                    last_loc = l;
                }
                record_scope(g.a.code.len(), l);
            }
            if fused && ii + 1 == b.insts.len() {
                match ins {
                    Inst::Bin(_, op, x, y) => cc = g.cmp_flags(*op, *x, *y),
                    Inst::N(op, _, x, y) => cc = g.fcmp_flags(*op, *x, *y),
                    _ => {}
                }
            } else {
                g.inst(ins, false);
            }
        }
        record_scope(g.a.code.len(), g.locs[bi][b.insts.len()]);
        match &b.term {
            Term::Jmp(t) => {
                if !next(*t) {
                    g.a.jmp(lab(*t));
                }
            }
            Term::Br(c, t, e) => {
                if !fused {
                    let cr = g.reg(*c, RAX);
                    g.a.test_rr(cr);
                    cc = CC_NE;
                }
                if next(*t) {
                    g.a.jcc(cc ^ 1, lab(*e));
                } else {
                    g.a.jcc(cc, lab(*t));
                    if !next(*e) {
                        g.a.jmp(lab(*e));
                    }
                }
            }
            Term::Ret(v) => {
                if let Some(v) = v {
                    g.mov_to(RAX, *v);
                }
                if early[bi] {
                    g.a.raw(&[0xC3]); // ret: nothing was saved or allocated yet
                } else {
                    for (k, r) in callee.iter().enumerate() {
                        g.a.load(*r, -8 * (k as i32 + 1));
                    }
                    if frameless {
                        g.a.raw(&[0xC3]); // ret
                    } else {
                        if g.a.dwarf {
                            let at = g.a.code.len();
                            g.a.cfi.push((at + 1, crate::dwarf::CFA_ENTRY)); // after `leave` the return address is on top
                            record_scope(at + 1, 0);
                        }
                        g.a.raw(&[0xC9, 0xC3]); // leave; ret
                    }
                }
            }
            Term::Unreachable => g.a.raw(&[0x0F, 0x0B]), // ud2
        }
        if early[bi] {
            for (p, l) in saved.into_iter().enumerate() {
                g.al.loc[p] = l;
            }
        }
    }
    record_scope(g.a.code.len(), 0); // the cold panic stubs have no source scope
    for t in 0..nb {
        if let Some(l) = tramp[t] {
            g.a.bind(l);
            prologue(&mut g, t);
            g.a.jmp(bl[t]);
        }
    }
    if g.a.dwarf {
        let at = g.a.code.len();
        g.a.cfi.push((at, if frameless { crate::dwarf::CFA_ENTRY } else { crate::dwarf::CFA_FRAME }));
    }
    for (l, r, lo, hi, loc, fix) in std::mem::take(&mut g.stubs) {
        g.a.bind(l);
        g.a.mov_rr(RDI, r);
        g.a.mov_imm(RSI, lo);
        g.a.mov_imm(RDX, hi);
        let t = g.rt.range_panic;
        if linux {
            if fix {
                g.a.raw(&[0x55]); // push rbp
                g.a.mov_rr(RBP, RSP);
            }
            g.a.call(t);
            if loc != 0 {
                let at = g.a.code.len();
                g.a.sites.push((at, loc));
            }
        } else {
            g.a.jmp(t);
        }
    }
    for (l, kind, loc, fix) in std::mem::take(&mut g.pstubs) {
        g.a.bind(l);
        if fix {
            g.a.raw(&[0x55]); // push rbp
            g.a.mov_rr(RBP, RSP); // the frame record of the function that has none
        }
        let t = g.rt.panic_at[kind];
        g.a.call(t);
        if loc != 0 {
            let at = g.a.code.len();
            g.a.sites.push((at, loc));
        }
    }
    (g.slot_off, scopes)
}

fn succs_of(t: &Term) -> Vec<usize> {
    match t {
        Term::Jmp(x) => vec![*x],
        Term::Br(_, a, b) => vec![*a, *b],
        _ => vec![],
    }
}

/// Shrink-wrapping for the common "check, return early" shape: the leading blocks that are cheap (compare /
/// add / sub / mov / constants on registers only), have no other way in, and need no callee-saved register,
/// spill or frame run BEFORE the prologue (parameters read straight from the argument registers) and return
/// with a plain `ret`. Every edge leaving that region goes through a trampoline holding the prologue.
fn early_blocks(f: &IrFunc, al: &Alloc) -> Vec<bool> {
    let nb = f.blocks.len();
    if f.nparams > ARG_REGS.len()
        || (al.callee_used.is_empty() && al.nspill == 0 && f.slots.is_empty())
    {
        return vec![false; nb];
    }
    // a vreg that an early block may read or write: a parameter (it sits in its argument register), a constant
    // that fits an immediate, or a caller-saved register
    let ok = |v: V| -> bool {
        if v < f.nparams {
            return true;
        }
        match al.loc[v] {
            Loc::Reg(r) => regalloc::CALLEE_SAVED >> r & 1 == 0,
            Loc::Const(n) => i32::try_from(n).is_ok(),
            Loc::None => true,
            _ => false,
        }
    };
    let pure = |bi: usize| -> bool {
        let b = &f.blocks[bi];
        for i in &b.insts {
            let good = match i {
                Inst::Const(d, _) => ok(*d),
                Inst::Mov(d, s) => ok(*d) && ok(*s),
                Inst::Bin(d, op, x, y) => {
                    matches!(
                        op,
                        Op::Add
                            | Op::Sub
                            | Op::AddU
                            | Op::SubU
                            | Op::Lt
                            | Op::Le
                            | Op::Gt
                            | Op::Ge
                            | Op::Eq
                            | Op::Ne
                    ) && ok(*d)
                        && ok(*x)
                        && ok(*y)
                }
                _ => false,
            };
            if !good {
                return false;
            }
        }
        match &b.term {
            Term::Jmp(_) | Term::Unreachable => true,
            Term::Br(c, ..) => ok(*c),
            Term::Ret(v) => v.is_none_or(&ok),
        }
    };
    let mut preds: Vec<Vec<usize>> = vec![vec![]; nb];
    for (bi, b) in f.blocks.iter().enumerate() {
        for t in succs_of(&b.term) {
            preds[t].push(bi);
        }
    }
    let mut early = vec![false; nb];
    if !preds[0].is_empty() || !pure(0) {
        return early;
    }
    early[0] = true;
    loop {
        let mut changed = false;
        for b in 1..nb {
            if !early[b] && !preds[b].is_empty() && preds[b].iter().all(|p| early[*p]) && pure(b) {
                early[b] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // The prologue moves incoming parameters into their allocated locations. An early block
    // runs before those moves, so values it computes cannot be carried into a block that
    // runs after the prologue: a parameter move may overwrite the register holding them.
    // Keep shrink-wrapping only when no value defined in the early region is used in the
    // regular region. Parameters themselves are initialized by the prologue as usual.
    let mut early_defs = vec![false; f.nvregs];
    for (bi, b) in f.blocks.iter().enumerate() {
        if early[bi] {
            for i in &b.insts {
                if let Some(d) = regalloc::inst_def(i) {
                    if d >= f.nparams {
                        early_defs[d] = true;
                    }
                }
            }
        }
    }
    for (bi, b) in f.blocks.iter().enumerate() {
        if !early[bi] {
            let uses = b
                .insts
                .iter()
                .flat_map(regalloc::inst_uses)
                .chain(match &b.term {
                    Term::Br(c, ..) => vec![*c],
                    Term::Ret(Some(v)) => vec![*v],
                    _ => vec![],
                });
            if uses.into_iter().any(|v| early_defs[v]) {
                return vec![false; nb];
            }
        }
    }
    // a function that is early everywhere has nothing to wrap
    if early.iter().all(|e| *e) {
        return vec![false; nb];
    }
    early
}

// ───────────────────────── Compiler ─────────────────────────

pub fn compile(ir: &IrProgram, debug_alloc: bool) -> Vec<u8> {
    compile_os(ir, debug_alloc, Os::Linux)
}

/// Where the Windows sections end up, relative to the start of .text (for rip-relative fixups).
#[derive(Default, Clone, Copy)]
struct WinLayout {
    rdata_rel: usize, // .rdata (data blobs) minus .text
    iat_rel: usize,   // IAT minus .text
    data_rel: usize,  // .data (heap state) minus .text
}

pub(crate) struct Built {
    pub(crate) code: Vec<u8>,
    pub(crate) blob: Vec<u8>,
    pub(crate) entry: usize,
    pub(crate) imports: Vec<(usize, usize)>,
    pub(crate) dbg: crate::dwarf::DebugInfo,
    pub(crate) unwind: Vec<UnwindRec>,
}

pub fn compile_os(ir: &IrProgram, debug_alloc: bool, os: Os) -> Vec<u8> {
    match os {
        Os::Linux => {
            let b = build(ir, debug_alloc, os, G, WinLayout::default());
            if !ir.externs.is_empty() {
                return crate::elfdyn::write(&b, &ir.links, &ir.externs);
            }
            let code_len = b.code.len();
            let mut all = b.code;
            all.extend_from_slice(&b.blob);
            let mut elf = elf64(&all, b.entry);
            if !b.dbg.funcs.is_empty() {
                crate::dwarf::append(&mut elf, &b.dbg, ir, BASE + HDR, HDR, code_len);
            }
            elf
        }
        Os::Windows => {
            // The image is position independent (ASLR): every reference to the heap state is `lea [rip+disp]`,
            // so no relocation entry is needed. The distance to .data depends on the layout, but the code size
            // does not: build once to learn the sizes, lay the sections out, build again for real.
            let probe = build(
                ir,
                debug_alloc,
                os,
                PE_BASE as i64 + 0x100000,
                WinLayout::default(),
            );
            let (_, xdata_len) = unwind_tables(&probe.unwind, 0, 0);
            let lay = pe_layout(probe.code.len(), probe.blob.len(), probe.unwind.len(), xdata_len);
            let b = build(
                ir,
                debug_alloc,
                os,
                (PE_BASE + lay.data_rva) as i64,
                WinLayout {
                    rdata_rel: (lay.rdata_rva - lay.text_rva) as usize,
                    iat_rel: (lay.iat_rva() - lay.text_rva) as usize,
                    data_rel: (lay.data_rva - lay.text_rva) as usize,
                },
            );
            assert_eq!(
                b.code.len(),
                probe.code.len(),
                "code size must not depend on the data address"
            );
            pe64(&b.code, &b.blob, b.entry, &lay, &b.unwind)
        }
    }
}

// One mapping per task: control page, inaccessible guard page, then a downward-growing stack.
const THREAD_TLS: i32 = 32;
const TLS_RECOVERY: i32 = 8;
const THREAD_RECOVER: i32 = THREAD_TLS + TLS_RECOVERY;
const RECOVERY_IN_DESTRUCTOR: i64 = 2;
const THREAD_UNWINDING: i32 = 16;
const THREAD_CLEANUP_HEAD: i32 = 24;
const TLS_ALLOC_FREE: i32 = 1040; // one cached block for each small allocation size class
const THREAD_STACK_END: i32 = 2 * 4096 + 2 * 1024 * 1024;

fn emit_tls_load(a: &mut Asm, reg: u8, offset: i32) {
    debug_assert!(reg < 8);
    a.raw(&[0x64, 0x48, 0x8b, (reg << 3) | 4, 0x25]); // mov reg, fs:[disp32]
    a.imm32(offset);
}

fn emit_tls_store(a: &mut Asm, reg: u8, offset: i32) {
    debug_assert!(reg < 8);
    a.raw(&[0x64, 0x48, 0x89, (reg << 3) | 4, 0x25]); // mov fs:[disp32], reg
    a.imm32(offset);
}

fn emit_threads(
    a: &mut Asm,
    start: usize,
    join: usize,
    mmap: usize,
    failure: usize,
    recover: usize,
) {
    const PAGE: i32 = 4096;
    const MAPPING: i32 = THREAD_STACK_END;
    const TID: i32 = 0;
    const CODE: i32 = 8;
    const ENV: i32 = 16;
    const RESULT: i32 = 24;
    const CLONE_FLAGS: i64 = 0x003d0f00; // VM, FS, FILES, SIGHAND, THREAD, SYSVSEM, SETTLS, PARENT_SETTID, CHILD_CLEARTID

    // start(code, env) -> control page. The child branches away without returning on the parent's stack.
    let parent = a.label();
    let child_exit = a.label();
    let child_cleanup = a.label();
    a.bind(start);
    a.push(RDI);
    a.push(RSI);
    a.push(RDX);
    a.mov_imm(RSI, MAPPING as i64);
    a.call(mmap);
    a.pop(RDX);
    a.pop(RSI);
    a.pop(RDI);
    a.mov_rr(RBX, RAX);
    a.st(RBX, CODE, RDI);
    a.st(RBX, ENV, RSI);
    a.st(RBX, THREAD_RECOVER, RDX);
    a.lea(RDI, RBX, PAGE);
    a.mov_imm(RSI, PAGE as i64);
    a.mov_imm(RDX, 0); // PROT_NONE
    a.mov_imm(RAX, 10); // mprotect
    a.syscall();
    a.test_rax();
    a.jcc(CC_S, failure);

    a.mov_imm(RDI, CLONE_FLAGS);
    a.lea(RSI, RBX, MAPPING);
    a.lea(RDX, RBX, TID); // parent_tid
    a.mov_rr(R10, RDX); // child_tid
    a.lea(R8, RBX, THREAD_TLS);
    a.st(R8, 0, R8); // TLS base self-pointer
    a.mov_imm(RAX, 56); // clone
    a.syscall();
    a.test_rax();
    a.jcc(CC_S, failure);
    a.jcc(CC_NE, parent);

    a.mov_rr(R12, RBX); // generated functions preserve r12
    a.mov_imm(RBP, 0);
    a.ld(RAX, R12, CODE);
    a.ld(RDI, R12, ENV);
    a.call_r(RAX);
    a.st(R12, RESULT, RAX);
    a.jmp(child_cleanup);

    a.bind(recover);
    // The panic probe passes the control page in rax. Drop live locals before discarding their frames.
    a.mov_rr(R12, RAX);
    a.mov_imm(RAX, 0);
    a.st(R12, THREAD_RECOVER, RAX); // a second panic during cleanup is fatal
    a.mov_imm(RAX, 1);
    emit_tls_store(a, RAX, THREAD_UNWINDING);
    a.and_imm(RSP, -16);
    let cleanup = a.label();
    let cleaned = a.label();
    a.bind(cleanup);
    emit_tls_load(a, RCX, THREAD_CLEANUP_HEAD);
    a.test_rr(RCX);
    a.jcc(CC_E, cleaned);
    a.ld(RAX, RCX, 8);
    emit_tls_store(a, RAX, THREAD_CLEANUP_HEAD);
    a.ld(RAX, RCX, 0);
    a.test_rr(RAX);
    a.jcc(CC_E, cleanup);
    a.mov_imm(RAX, 0);
    a.st(RCX, 0, RAX);
    a.ld(RDI, RCX, 16);
    a.ld(RAX, RCX, 24);
    a.call_r(RAX);
    a.jmp(cleanup);
    a.bind(cleaned);
    a.lea(RSP, R12, MAPPING);
    a.mov_imm(RBP, 0);
    a.bind(child_cleanup);
    a.mov_imm(RAX, 0);
    a.st(R12, THREAD_RECOVER, RAX);
    a.ld(RDI, R12, ENV);
    a.test_rr(RDI);
    a.jcc(CC_E, child_exit);
    a.ld(RAX, RDI, 0); // environment drop glue (second argument 0: no worklist, captured closures drop recursively)
    a.mov_imm(RSI, 0);
    a.call_r(RAX);
    a.bind(child_exit);
    // Return cached small blocks to the shared lists before discarding this
    // worker's TLS mapping. One global lock covers the whole cache flush.
    acquire_runtime_lock(a, G, G_HEAP_LOCK);
    emit_tls_load(a, RDX, 0);
    a.add_imm(RDX, TLS_ALLOC_FREE);
    for class in 0..17 {
        let empty = a.label();
        a.ld(RAX, RDX, class * 8);
        a.test_rax();
        a.jcc(CC_E, empty);
        a.mov_imm(RBX, 0);
        a.st(RDX, class * 8, RBX);
        a.mov_imm(RBX, G + G_FREE as i64 + class as i64 * 8);
        a.ld(RCX, RBX, 0);
        a.st(RAX, 0, RCX);
        a.st(RBX, 0, RAX);
        a.bind(empty);
    }
    release_runtime_lock(a, G, G_HEAP_LOCK);
    a.mov_imm(RDI, 0);
    a.mov_imm(RAX, 60); // exit this thread; the kernel clears TID and wakes the joiner
    a.syscall();

    a.bind(parent);
    a.mov_rr(RAX, RBX);
    a.ret();

    // join(control page) -> result. The kernel's clear-child-tid wake uses the shared futex key.
    let check = a.label();
    let complete = a.label();
    a.bind(join);
    a.mov_rr(RBX, RDI);
    a.bind(check);
    a.ld32(RDX, RBX, TID);
    a.test_rr(RDX);
    a.jcc(CC_E, complete);
    a.mov_imm(RSI, 0); // FUTEX_WAIT
    a.mov_imm(R10, 0); // no timeout
    a.mov_imm(RAX, 202); // futex
    a.syscall();
    a.jmp(check); // value changed, interrupted, or woken; always recheck TID
    a.bind(complete);
    a.ld(R8, RBX, RESULT);
    a.mov_imm(RSI, MAPPING as i64);
    a.mov_imm(RAX, 11); // munmap the entire task mapping
    a.syscall();
    a.mov_rr(RAX, R8);
    a.ret();
}

/// Redirect a recoverable worker panic before it writes to stderr.
fn emit_worker_panic_probe(a: &mut Asm, recover: usize, double_panic: usize) {
    emit_tls_load(a, RAX, THREAD_UNWINDING);
    a.test_rax();
    a.jcc(CC_NE, double_panic);

    emit_tls_load(a, RAX, 0);
    a.lea(RAX, RAX, -THREAD_TLS);
    a.ld(R8, RAX, THREAD_RECOVER);
    // Only 1 denotes a recoverable worker. A user destructor may run while a
    // replacement or an aggregate drop is in progress, so panic there is fatal.
    a.cmp_imm(R8, 1);
    a.jcc(CC_E, recover);
}

fn build(ir: &IrProgram, debug_alloc: bool, os: Os, g: i64, win: WinLayout) -> Built {
    let mut a = Asm::new(os);
    a.gbase = g;
    a.win_data_rel = win.data_rel;
    // panic reports (position, backtrace) exist when some user code has a checked operation or calls the library
    a.report = ir
        .funcs
        .iter()
        .any(|f| f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::Loc(_)))));
    let report = a.report;
    let has_workers = os == Os::Linux
        && ir.funcs.iter().any(|f| {
            f.blocks.iter().any(|b| {
                b.insts.iter().any(|i| {
                    matches!(
                        i,
                        Inst::Thread(
                            crate::ir::ThreadOp::Start
                                | crate::ir::ThreadOp::StartRecover
                                | crate::ir::ThreadOp::Wait
                                | crate::ir::ThreadOp::Unlock
                                | crate::ir::ThreadOp::CleanupPush
                                | crate::ir::ThreadOp::CleanupPop
                                | crate::ir::ThreadOp::DropEnter,
                            ..
                        )
                    )
                })
            })
        });

    let mut data: Vec<Vec<u8>> = vec![
        b"panic: ".to_vec(),
        b"\n".to_vec(),
        b"true\n".to_vec(),
        b"false\n".to_vec(),
    ];
    for m in PANIC_MSGS {
        data.push(m.as_bytes().to_vec());
    }
    for m in ["allocs: ", "frees: ", "live: "] {
        data.push(m.as_bytes().to_vec());
    }
    for m in [": ", " not in ", ".."] {
        data.push(m.as_bytes().to_vec());
    }
    for t in &ir.texts {
        // text blob = [len: u64][bytes]
        let mut b = (t.len() as u64).to_le_bytes().to_vec();
        b.extend_from_slice(t);
        b.push(0); // C-compatible view for extern calls
        data.push(b);
    }

    // panic report (Linux): strings of the stack backtrace, and the site / function / location tables, which are
    // only known after the code is generated (the blob is filled in below)
    let d_bt_hdr = data.len();
    data.push(b"stack backtrace:\n".to_vec());
    let d_sp2 = data.len();
    data.push(b"  ".to_vec());
    let d_sp1 = data.len();
    data.push(b" ".to_vec());
    let d_at = data.len();
    data.push(b"  at ".to_vec());
    let d_colon = data.len();
    data.push(b":".to_vec());
    let dbg_id = data.len();
    data.push(vec![]);

    a.pool_base = data.len() + ir.vtables.len();
    let l_start = a.label();
    let l_print_int = a.label();
    let l_print_bool = a.label();
    let l_print_text = a.label();
    let l_print_bytes = a.label();
    let l_panic_bytes = a.label();
    let l_range_panic = a.label();
    let l_write_int2 = a.label();
    let l_write2 = a.label();
    let l_panic: Vec<usize> = (0..PANIC_MSGS.len()).map(|_| a.label()).collect();
    let l_panic_at: Vec<usize> = (0..PANIC_MSGS.len()).map(|_| a.label()).collect();
    let l_report = a.label();
    let (l_segv, l_restorer) = (a.label(), a.label());
    let (l_mmap, l_alloc, l_free, l_push, l_get) =
        (a.label(), a.label(), a.label(), a.label(), a.label());
    let (l_realloc, l_memcpy) = (a.label(), a.label());
    let (l_thread_start, l_thread_wait, l_thread_recover) = (a.label(), a.label(), a.label());
    let l_funcs: Vec<usize> = ir.funcs.iter().map(|_| a.label()).collect();
    let (l_oswrite, l_osexit, l_osalloc, l_osfree) = (a.label(), a.label(), a.label(), a.label());
    a.l_oswrite = l_oswrite;
    a.l_osexit = l_osexit;
    let (l_osread, l_winsyscall) = (a.label(), a.label());
    a.l_winsyscall = l_winsyscall;

    // _start: main(); exit(0)
    a.bind(l_start);
    if os == Os::Windows {
        // the entry point is entered like a call: rsp = 8 mod 16. Realign, then make console output UTF-8
        // (Yerux text is UTF-8; the default console code page would garble non-ASCII).
        a.raw(&[0x48, 0x83, 0xE4, 0xF0]); // and rsp, -16
        a.raw(&[0x48, 0x83, 0xEC, 0x20]); // sub rsp, 32 (shadow space)
        a.raw(&[0xB9]); // mov ecx, 65001
        a.imm32(65001);
        a.call_iat(IMP_SETCONSOLEOUTPUTCP);
        a.mov_imm(RBP, 0); // the panic reporter walks Yerux frames up to this sentinel
    }
    if os == Os::Linux {
        // Stack overflow guard: a SIGSEGV handler on an alternate stack turns a fault at the stack pointer
        // (recursion past the 8 MiB main stack) into `panic: stack overflow` + exit 101 instead of a bare SIGSEGV.
        a.mov_imm(RSI, 65536);
        a.call(l_mmap);
        a.alu_imm(5, RSP, 64); // sub rsp, 64: stack_t at +0, struct sigaction at +24
        a.st(RSP, 0, RAX); // ss_sp
        a.mov_imm(RCX, 0);
        a.st(RSP, 8, RCX); // ss_flags
        a.mov_imm(RCX, 65536);
        a.st(RSP, 16, RCX); // ss_size
        a.mov_rr(RDI, RSP);
        a.mov_imm(RSI, 0);
        a.mov_imm(RAX, 131); // sigaltstack
        a.syscall();
        a.lea_label(RCX, l_segv);
        a.st(RSP, 24, RCX); // sa_handler
        a.mov_imm(RCX, 0x0c00_0004); // SA_SIGINFO | SA_ONSTACK | SA_RESTORER (x86-64 requires a restorer)
        a.st(RSP, 32, RCX);
        a.lea_label(RCX, l_restorer);
        a.st(RSP, 40, RCX); // sa_restorer
        a.mov_imm(RCX, 0);
        a.st(RSP, 48, RCX); // sa_mask
        a.mov_imm(RDI, 11); // SIGSEGV
        a.lea(RSI, RSP, 24);
        a.mov_imm(RDX, 0);
        a.mov_imm_ext(10, 8); // sigsetsize
        a.mov_imm(RAX, 13); // rt_sigaction
        a.syscall();
        a.alu_imm(0, RSP, 64); // add rsp, 64
    }
    if os == Os::Linux
        && (report
            || ir.funcs.iter().any(|f| {
                f.blocks
                    .iter()
                    .any(|b| b.insts.iter().any(|i| matches!(i, Inst::Global(..))))
            }))
    {
        // global cell 0: the initial stack pointer (argc, argv.., 0, envp.., 0), read by std.io
        a.gaddr(RBX, 0);
        a.st(RBX, G_USER as i32, RSP);
    }
    if has_workers {
        a.gaddr(RSI, G_MAIN_TLS as i64);
        a.st(RSI, 0, RSI);
        a.mov_imm(RDI, 0x1002); // ARCH_SET_FS
        a.mov_imm(RAX, 158); // arch_prctl
        a.syscall();
        a.test_rax();
        a.jcc(CC_S, l_panic[P_THREAD_START]);
    }
    a.call(l_funcs[ir.main]);
    if debug_alloc {
        // report allocations, frees and the difference on stderr
        for (i, name_len) in [8i64, 7, 6].iter().enumerate() {
            a.lea_data(RSI, D_DBG0 + i);
            a.mov_imm(RDX, *name_len);
            a.call(l_write2);
            a.gaddr(RBX, 0);
            if i == 0 {
                a.ld(RAX, RBX, G_NALLOC);
            } else if i == 1 {
                a.ld(RAX, RBX, G_NFREE);
            } else {
                a.ld(RAX, RBX, G_NALLOC);
                a.ld(RCX, RBX, G_NFREE);
                a.alu(0x29, RAX, RCX);
            }
            a.mov_imm(RBX, 2);
            a.call(l_print_int);
        }
    }
    a.exit(0);

    // write(2, rsi, rdx)
    a.bind(l_write2);
    a.sys_write(2);
    a.ret();

    // panic stubs: "panic: <msg>\n" to stderr, exit(101)
    for (i, l) in l_panic.iter().enumerate() {
        if report {
            // entered by `call` from a check of compiled code: r12 = where the return address is, r13 = rbp
            let over = a.label();
            a.bind(l_panic_at[i]);
            a.mov_rr(R12, RSP);
            a.mov_rr(R13, RBP);
            a.jmp(over);
            a.bind(*l);
            a.mov_imm(R12, 0); // raised by the runtime: no position
            a.bind(over);
        } else {
            a.bind(*l);
        }
        if has_workers
            && !matches!(
                i,
                P_OOM | P_DOUBLE_FREE | P_THREAD_START | P_STACK | P_DOUBLE_PANIC
            )
        {
            emit_worker_panic_probe(&mut a, l_thread_recover, l_panic[P_DOUBLE_PANIC]);
        }
        a.lea_data(RSI, D_PANIC);
        a.mov_imm(RDX, 7);
        a.call(l_write2);
        a.lea_data(RSI, D_MSG0 + i);
        a.mov_imm(RDX, PANIC_MSGS[i].len() as i64);
        a.call(l_write2);
        a.lea_data(RSI, D_NL);
        a.mov_imm(RDX, 1);
        a.call(l_write2);
        if report {
            a.call(l_report);
        }
        a.exit(101);
    }


    // report: called by the panic routines after the message. r12 = where the return address of the
    // panic site is on the stack (0: the panic came from the runtime), r13 = rbp. Prints "  at file:line:col" (the
    // first return address on the stack, the site and then the rbp chain, that is in the site table: library code
    // has none, so its panics are reported at the user call that led to them), then, on Linux with YERUX_BACKTRACE set,
    // the Yerux function of every frame. The tables are the `dbg` blob (u64 words), see below.
    if report {
        let (done, l_find, l_scan, l_hit, l_next, l_noloc, l_ok) =
            (a.label(), a.label(), a.label(), a.label(), a.label(), a.label(), a.label());
        a.bind(l_report);
        a.test_rr(R12);
        a.jcc(CC_E, done);
        a.lea_label(R14, l_start);
        a.lea_data(R15, dbg_id);
        a.ld(RBX, R12, 0); // return address under examination
        a.mov_rr(R8, R13); // frame record to walk next
        a.mov_imm(R9, 64); // frames to look at
        a.bind(l_find);
        a.mov_rr(RAX, RBX);
        a.alu(0x29, RAX, R14); // code offset
        a.ld32(RCX, R15, 0); // nsites
        a.ld32(RDI, R15, 8);
        a.alu(0x01, RDI, R15); // &sites[0]
        a.bind(l_scan);
        a.test_rr(RCX);
        a.jcc(CC_E, l_next);
        a.ld32(RDX, RDI, 0);
        a.alu(0x39, RDX, RAX);
        a.jcc(CC_E, l_hit);
        a.add_imm(RDI, 8);
        a.sub_imm(RCX, 1);
        a.jmp(l_scan);
        a.bind(l_next);
        a.test_rr(R8);
        a.jcc(CC_E, l_noloc);
        a.sub_imm(R9, 1);
        a.jcc(CC_E, l_noloc);
        a.ld(RBX, R8, 8);
        a.ld(RAX, R8, 0);
        a.alu(0x39, RAX, R8);
        a.jcc(CC_A, l_ok); // the next record must be higher up the stack
        a.mov_imm(RAX, 0);
        a.bind(l_ok);
        a.mov_rr(R8, RAX);
        a.jmp(l_find);
        a.bind(l_hit);
        // "  at <file>:<line>:<col>\n"; the location row is (file, line, col), 12 bytes
        a.ld32(RAX, RDI, 4); // location id
        a.mov_rr(RCX, RAX);
        a.shl(RAX, 1);
        a.alu(0x01, RAX, RCX);
        a.shl(RAX, 2); // id * 12
        a.ld32(RDI, R15, 12);
        a.alu(0x01, RDI, RAX);
        a.alu(0x01, RDI, R15); // &locs[id]
        a.ld32(RBX, RDI, 4); // line
        a.ld32(R8, RDI, 8); // column
        a.ld32(R9, RDI, 0); // file index
        a.lea_data(RSI, d_at);
        a.mov_imm(RDX, 5);
        a.call(l_write2);
        a.mov_rr(RAX, R9);
        a.shl(RAX, 3);
        a.ld32(RDI, R15, 16);
        a.alu(0x01, RDI, RAX);
        a.alu(0x01, RDI, R15); // &files[index]: (name offset, length)
        a.ld32(RSI, RDI, 0);
        a.ld32(RDX, RDI, 4);
        a.ld32(RAX, R15, 24);
        a.alu(0x01, RSI, RAX);
        a.alu(0x01, RSI, R15);
        a.call(l_write2);
        a.lea_data(RSI, d_colon);
        a.mov_imm(RDX, 1);
        a.call(l_write2);
        a.mov_rr(RAX, RBX);
        a.call(l_write_int2);
        a.lea_data(RSI, d_colon);
        a.mov_imm(RDX, 1);
        a.call(l_write2);
        a.mov_rr(RAX, R8);
        a.call(l_write_int2);
        a.lea_data(RSI, D_NL);
        a.mov_imm(RDX, 1);
        a.call(l_write2);
        a.bind(l_noloc);
        if os == Os::Linux {
            // YERUX_BACKTRACE in the environment (cell 0 holds the initial stack: argc, argv.., 0, envp.., 0)?
            let (l_env, l_envnext, l_bt) = (a.label(), a.label(), a.label());
            a.gaddr(RAX, G_USER as i64);
            a.ld(RAX, RAX, 0);
            a.test_rr(RAX);
            a.jcc(CC_E, done);
            a.ld(RCX, RAX, 0);
            a.shl(RCX, 3);
            a.alu(0x01, RAX, RCX);
            a.add_imm(RAX, 16); // &envp[0]
            a.bind(l_env);
            a.ld(RSI, RAX, 0);
            a.test_rr(RSI);
            a.jcc(CC_E, done);
            a.mov_imm(RDX, 0x41425F5855524559u64 as i64); // "YERUX_BA"
            a.ld(RCX, RSI, 0);
            a.alu(0x39, RCX, RDX);
            a.jcc(CC_NE, l_envnext);
            a.mov_imm(RDX, 0x3D45434152544B43u64 as i64); // "CKTRACE="
            a.ld(RCX, RSI, 8);
            a.alu(0x39, RCX, RDX);
            a.jcc(CC_NE, l_envnext);
            a.movzx8_mem(RCX, RSI, 16);
            a.test_rr(RCX);
            a.jcc(CC_E, l_envnext); // empty value
            a.cmp_imm(RCX, 0x30);
            a.jcc(CC_NE, l_bt);
            a.movzx8_mem(RCX, RSI, 17);
            a.test_rr(RCX);
            a.jcc(CC_NE, l_bt); // "0" alone switches it off
            a.bind(l_envnext);
            a.add_imm(RAX, 8);
            a.jmp(l_env);
            a.bind(l_bt);
            a.lea_data(RSI, d_bt_hdr);
            a.mov_imm(RDX, 17);
            a.call(l_write2);
            let (l_frame, l_fscan, l_fdone, l_fnext, l_fok, l_pad) =
                (a.label(), a.label(), a.label(), a.label(), a.label(), a.label());
            a.ld(RBX, R12, 0);
            a.mov_rr(R8, R13);
            a.mov_imm(R9, 64); // frames left
            a.mov_imm(R10, 0); // frames printed
            a.bind(l_frame);
            // the function containing the call (return address - 1): the last table entry that starts at or before it
            a.mov_rr(RAX, RBX);
            a.alu(0x29, RAX, R14);
            a.sub_imm(RAX, 1);
            a.ld32(RCX, R15, 4); // nfuncs
            a.ld32(RDI, R15, 20);
            a.alu(0x01, RDI, R15); // &funcs[0]: (start, name offset, name length)
            a.mov_imm(RDX, 0); // best entry
            a.bind(l_fscan);
            a.test_rr(RCX);
            a.jcc(CC_E, l_fdone);
            a.ld32(RSI, RDI, 0);
            a.alu(0x39, RSI, RAX);
            a.jcc(CC_A, l_fdone);
            a.mov_rr(RDX, RDI);
            a.add_imm(RDI, 12);
            a.sub_imm(RCX, 1);
            a.jmp(l_fscan);
            a.bind(l_fdone);
            a.test_rr(RDX);
            a.jcc(CC_E, l_fnext);
            a.ld32(RCX, RDX, 8);
            a.test_rr(RCX);
            a.jcc(CC_E, l_fnext); // a runtime helper: not shown
            a.mov_rr(RBP, RDX); // entry kept in rbp (the system call clobbers r11)
            a.lea_data(RSI, d_sp2);
            a.mov_imm(RDX, 2);
            a.call(l_write2);
            a.cmp_imm(R10, 10);
            a.jcc(CC_AE, l_pad);
            a.lea_data(RSI, d_sp1);
            a.mov_imm(RDX, 1);
            a.call(l_write2);
            a.bind(l_pad);
            a.mov_rr(RAX, R10);
            a.call(l_write_int2);
            a.lea_data(RSI, D_RNG0); // ": "
            a.mov_imm(RDX, 2);
            a.call(l_write2);
            a.ld32(RSI, RBP, 4);
            a.ld32(RCX, R15, 24);
            a.alu(0x01, RSI, RCX);
            a.alu(0x01, RSI, R15);
            a.ld32(RDX, RBP, 8);
            a.call(l_write2);
            a.lea_data(RSI, D_NL);
            a.mov_imm(RDX, 1);
            a.call(l_write2);
            a.add_imm(R10, 1);
            a.bind(l_fnext);
            a.test_rr(R8);
            a.jcc(CC_E, done);
            a.sub_imm(R9, 1);
            a.jcc(CC_E, done);
            a.ld(RBX, R8, 8);
            a.ld(RAX, R8, 0);
            a.alu(0x39, RAX, R8);
            a.jcc(CC_A, l_fok);
            a.mov_imm(RAX, 0);
            a.bind(l_fok);
            a.mov_rr(R8, RAX);
            a.jmp(l_frame);
        }
        a.bind(done);
        a.ret();
    }

    // write_int2: rax = value. Writes its decimal digits (no newline) to stderr.
    a.bind(l_write_int2);
    a.raw(&[0x48, 0x83, 0xEC, 0x20]); // sub rsp, 32
    a.raw(&[0x48, 0x89, 0xC7]); // mov rdi, rax  (remember the sign)
    a.test_rax();
    let l_wpos = a.label();
    a.jcc(CC_NS, l_wpos);
    a.raw(&[0x48, 0xF7, 0xD8]); // neg rax  (INT64_MIN stays 2^63, fine as unsigned)
    a.bind(l_wpos);
    a.raw(&[0x48, 0x8D, 0x74, 0x24, 0x20]); // lea rsi, [rsp+32]
    a.raw(&[0xB9, 0x0A, 0x00, 0x00, 0x00]); // mov ecx, 10
    let l_wdig = a.label();
    a.bind(l_wdig);
    a.raw(&[0x31, 0xD2]); // xor edx, edx
    a.raw(&[0x48, 0xF7, 0xF1]); // div rcx (unsigned)
    a.raw(&[0x80, 0xC2, 0x30]); // add dl, '0'
    a.raw(&[0x48, 0xFF, 0xCE]); // dec rsi
    a.raw(&[0x88, 0x16]); // mov [rsi], dl
    a.test_rax();
    a.jcc(CC_NE, l_wdig);
    a.raw(&[0x48, 0x85, 0xFF]); // test rdi, rdi
    let l_wnosign = a.label();
    a.jcc(CC_NS, l_wnosign);
    a.raw(&[0x48, 0xFF, 0xCE]); // dec rsi
    a.raw(&[0xC6, 0x06, 0x2D]); // mov byte [rsi], '-'
    a.bind(l_wnosign);
    a.raw(&[0x48, 0x8D, 0x54, 0x24, 0x20]); // lea rdx, [rsp+32]
    a.raw(&[0x48, 0x29, 0xF2]); // sub rdx, rsi
    a.sys_write(2);
    a.raw(&[0x48, 0x83, 0xC4, 0x20]); // add rsp, 32
    a.ret();

    // range_panic: rdi = value, rsi = lo, rdx = hi. "panic: integer conversion out of range: V not in LO..HI", exit(101).
    a.bind(l_range_panic);
    if report {
        a.mov_rr(R12, RSP); // entered by `call` from a cold stub of compiled code
        a.mov_rr(R13, RBP);
    }
    if has_workers {
        emit_worker_panic_probe(&mut a, l_thread_recover, l_panic[P_DOUBLE_PANIC]);
        // clobbers only rax and r8
    }
    a.push(RDX);
    a.push(RSI);
    a.push(RDI);
    a.lea_data(RSI, D_PANIC);
    a.mov_imm(RDX, 7);
    a.call(l_write2);
    a.lea_data(RSI, D_MSG0 + P_CONV);
    a.mov_imm(RDX, PANIC_MSGS[P_CONV].len() as i64);
    a.call(l_write2);
    for (k, len) in [2i64, 8, 2].into_iter().enumerate() {
        a.lea_data(RSI, D_RNG0 + k);
        a.mov_imm(RDX, len);
        a.call(l_write2);
        a.ld(RAX, RSP, 8 * k as i32);
        a.call(l_write_int2);
    }
    a.lea_data(RSI, D_NL);
    a.mov_imm(RDX, 1);
    a.call(l_write2);
    if report {
        a.call(l_report);
    }
    a.exit(101);

    // SIGSEGV handler (Linux): rsi = siginfo, rdx = ucontext. A fault address within [rsp - 4 KiB, rsp + 16 MiB)
    // of the faulting stack pointer is a stack overflow. Anything else gets the default action back and the
    // faulting instruction is re-executed by returning through rt_sigreturn.
    if os == Os::Linux {
        let other = a.label();
        a.bind(l_segv);
        a.ld(RAX, RSI, 16); // si_addr
        a.ld(RCX, RDX, 160); // uc_mcontext.gregs[REG_RSP]
        a.alu(0x29, RAX, RCX); // rax = si_addr - rsp
        a.alu_imm(0, RAX, 4096);
        a.mov_imm(RCX, 16 << 20);
        a.alu(0x39, RAX, RCX); // cmp rax, rcx
        a.jcc(CC_AE, other);
        a.jmp(l_panic[P_STACK]);
        a.bind(other);
        a.mov_imm(RCX, 0);
        for _ in 0..4 {
            a.push(RCX);
        }
        a.mov_imm(RDI, 11);
        a.mov_rr(RSI, RSP);
        a.mov_imm(RDX, 0);
        a.mov_imm_ext(10, 8);
        a.mov_imm(RAX, 13); // rt_sigaction(SIGSEGV, SIG_DFL)
        a.syscall();
        a.alu_imm(0, RSP, 32);
        a.ret(); // to the restorer
        a.bind(l_restorer);
        a.mov_imm(RAX, 15); // rt_sigreturn
        a.syscall();
    }

    // print_int: value in rax, fd in rbx. Prints decimal + newline.
    a.bind(l_print_int);
    if has_workers {
        acquire_print_lock(&mut a, g);
    }
    a.raw(&[0x48, 0x83, 0xEC, 0x20]); // sub rsp, 32
    a.raw(&[0x48, 0x89, 0xC7]); // mov rdi, rax  (remember the sign)
    a.test_rax();
    let l_pos = a.label();
    a.jcc(CC_NS, l_pos);
    a.raw(&[0x48, 0xF7, 0xD8]); // neg rax  (INT64_MIN stays 2^63, fine as unsigned)
    a.bind(l_pos);
    a.raw(&[0x48, 0x8D, 0x74, 0x24, 0x1F]); // lea rsi, [rsp+31]
    a.raw(&[0xC6, 0x06, 0x0A]); // mov byte [rsi], '\n'
    a.raw(&[0xB9, 0x0A, 0x00, 0x00, 0x00]); // mov ecx, 10
    let l_dig = a.label();
    a.bind(l_dig);
    a.raw(&[0x31, 0xD2]); // xor edx, edx
    a.raw(&[0x48, 0xF7, 0xF1]); // div rcx (unsigned)
    a.raw(&[0x80, 0xC2, 0x30]); // add dl, '0'
    a.raw(&[0x48, 0xFF, 0xCE]); // dec rsi
    a.raw(&[0x88, 0x16]); // mov [rsi], dl
    a.test_rax();
    a.jcc(CC_NE, l_dig);
    a.raw(&[0x48, 0x85, 0xFF]); // test rdi, rdi
    let l_nosign = a.label();
    a.jcc(CC_NS, l_nosign);
    a.raw(&[0x48, 0xFF, 0xCE]); // dec rsi
    a.raw(&[0xC6, 0x06, 0x2D]); // mov byte [rsi], '-'
    a.bind(l_nosign);
    a.raw(&[0x48, 0x8D, 0x54, 0x24, 0x20]); // lea rdx, [rsp+32]
    a.raw(&[0x48, 0x29, 0xF2]); // sub rdx, rsi
    a.mov_rr(RDI, RBX); // fd (rbx) is chosen by the caller
    a.write_rdi();
    a.raw(&[0x48, 0x83, 0xC4, 0x20]); // add rsp, 32
    if has_workers {
        release_runtime_lock(&mut a, g, G_PRINT_LOCK);
    }
    a.ret();

    // print_bool: value in rax
    a.bind(l_print_bool);
    if has_workers {
        acquire_print_lock(&mut a, g);
    }
    let (l_false, l_wr) = (a.label(), a.label());
    a.test_rax();
    a.jcc(CC_E, l_false);
    a.lea_data(RSI, D_TRUE);
    a.mov_imm(RDX, 5);
    a.jmp(l_wr);
    a.bind(l_false);
    a.lea_data(RSI, D_FALSE);
    a.mov_imm(RDX, 6);
    a.bind(l_wr);
    a.sys_write(1);
    if has_workers {
        release_runtime_lock(&mut a, g, G_PRINT_LOCK);
    }
    a.ret();

    // print_text: rax = pointer to [len][bytes]
    a.bind(l_print_text);
    if has_workers {
        acquire_print_lock(&mut a, g);
    }
    a.raw(&[0x48, 0x8B, 0x10]); // mov rdx, [rax]
    a.raw(&[0x48, 0x8D, 0x70, 0x08]); // lea rsi, [rax+8]
    a.sys_write(1);
    a.lea_data(RSI, D_NL);
    a.mov_imm(RDX, 1);
    a.sys_write(1);
    if has_workers {
        release_runtime_lock(&mut a, g, G_PRINT_LOCK);
    }
    a.ret();

    // print_bytes: rsi = pointer, rdx = length. Writes the bytes (if any) and a newline.
    {
        let l_nl = a.label();
        a.bind(l_print_bytes);
        if has_workers {
            acquire_print_lock(&mut a, g);
        }
        a.raw(&[0x48, 0x85, 0xD2]); // test rdx, rdx
        a.jcc(CC_E, l_nl);
        a.sys_write(1);
        a.bind(l_nl);
        a.lea_data(RSI, D_NL);
        a.mov_imm(RDX, 1);
        a.sys_write(1);
        if has_workers {
            release_runtime_lock(&mut a, g, G_PRINT_LOCK);
        }
        a.ret();
    }

    // panic_bytes: rsi = pointer, rdx = length. "panic: " + the bytes + newline on stderr, exit(101).
    {
        let l_nl = a.label();
        a.bind(l_panic_bytes);
        if report {
            a.mov_rr(R12, RSP); // entered by `call` (`panic(..)` in compiled code)
            a.mov_rr(R13, RBP);
        }
        if has_workers {
            emit_worker_panic_probe(&mut a, l_thread_recover, l_panic[P_DOUBLE_PANIC]);
        }
        a.raw(&[0x56, 0x52]); // push rsi; push rdx
        a.lea_data(RSI, D_PANIC);
        a.mov_imm(RDX, 7);
        a.call(l_write2);
        a.raw(&[0x5A, 0x5E]); // pop rdx; pop rsi
        a.raw(&[0x48, 0x85, 0xD2]); // test rdx, rdx
        a.jcc(CC_E, l_nl);
        a.call(l_write2);
        a.bind(l_nl);
        a.lea_data(RSI, D_NL);
        a.mov_imm(RDX, 1);
        a.call(l_write2);
        if report {
            a.call(l_report);
        }
        a.exit(101);
    }

    // ── heap runtime ──
    // rt_mmap: rsi = length -> rax = fresh zeroed pages (panics when the kernel refuses)
    a.bind(l_mmap);
    match os {
        Os::Linux => {
            a.mov_imm(RDI, 0);
            a.mov_imm(RDX, 3); // PROT_READ | PROT_WRITE
            a.mov_imm_ext(10, 0x22); // MAP_PRIVATE | MAP_ANONYMOUS
            a.mov_imm_ext(8, -1);
            a.mov_imm_ext(9, 0);
            a.mov_imm(RAX, 9);
            a.syscall();
            a.test_rax();
            a.jcc(CC_S, l_panic[P_OOM]);
        }
        Os::Windows => {
            a.call(l_osalloc);
            a.test_rax();
            a.jcc(CC_E, l_panic[P_OOM]); // VirtualAlloc returns NULL on failure
        }
    }
    a.ret();

    // rt_alloc: rdi = bytes -> rax = payload pointer. Block = [size header][payload].
    // Small blocks (<= 64 KiB) are power-of-two size classes served from per-class free lists,
    // then bumped out of 1 MiB arenas. Larger blocks get their own mmap (page rounded).
    {
        let (l_large, l_bump, l_newarena, l_release, l_done, l_shared) =
            (a.label(), a.label(), a.label(), a.label(), a.label(), a.label());
        a.bind(l_alloc);
        a.add_imm(RDI, 8);
        a.cmp_imm(RDI, SMALL_MAX);
        a.jcc(CC_A, l_large);
        if has_workers {
            a.size_class(RDI, RSI, RCX);
            emit_tls_load(&mut a, RDX, 0);
            a.add_imm(RDX, TLS_ALLOC_FREE);
            a.rm_sib(0x8B, RAX, RDX, RCX, 3, 0); // thread-local free-list head
            a.test_rax();
            a.jcc(CC_E, l_shared);
            a.ld(RBX, RAX, 0); // pop cached block
            a.rm_sib(0x89, RBX, RDX, RCX, 3, 0);
            if debug_alloc {
                a.ld(RBX, RAX, -8);
                a.mov_imm(RCX, !FREED);
                a.alu(0x21, RBX, RCX);
                a.st(RAX, -8, RBX);
            }
            a.jmp(l_done);
            a.bind(l_shared);
            acquire_runtime_lock(&mut a, g, G_HEAP_LOCK);
        }
        a.size_class(RDI, RSI, RCX); // rsi = block size (smallest 2^k >= rdi), rcx = k
        a.mov_rr(RDX, RCX);
        a.shl(RDX, 3);
        a.gaddr(RBX, G_FREE as i64);
        a.alu(0x01, RDX, RBX); // rdx = &free_list[class]
        a.ld(RAX, RDX, 0);
        a.test_rax();
        a.jcc(CC_E, l_bump);
        a.ld(RBX, RAX, 0); // pop: head = head.next
        a.st(RDX, 0, RBX);
        if debug_alloc {
            a.ld(RBX, RAX, -8); // clear the FREED tag
            a.mov_imm(RCX, !FREED);
            a.alu(0x21, RBX, RCX);
            a.st(RAX, -8, RBX);
        }
        a.jmp(l_release);
        a.bind(l_bump);
        a.gaddr(RBX, 0);
        a.ld(RAX, RBX, G_BUMP);
        a.mov_rr(RDI, RAX);
        a.alu(0x01, RDI, RSI); // rdi = bump + size
        a.ld(RCX, RBX, G_END);
        a.alu(0x39, RDI, RCX);
        a.jcc(CC_A, l_newarena);
        a.st(RBX, G_BUMP, RDI);
        a.st(RAX, 0, RSI); // header = block size
        a.add_imm(RAX, 8);
        a.jmp(l_release);
        a.bind(l_newarena);
        a.push(RSI);
        a.mov_imm(RSI, ARENA as i64);
        a.call(l_mmap);
        a.pop(RSI);
        a.gaddr(RBX, 0);
        a.st(RBX, G_BUMP, RAX);
        a.mov_rr(RCX, RAX);
        a.add_imm(RCX, ARENA);
        a.st(RBX, G_END, RCX);
        a.jmp(l_bump);
        a.bind(l_release);
        if has_workers {
            release_runtime_lock(&mut a, g, G_HEAP_LOCK);
        }
        a.jmp(l_done);
        a.bind(l_large);
        a.add_imm(RDI, 4095);
        a.and_imm(RDI, -4096);
        a.mov_rr(RSI, RDI);
        a.push(RSI);
        a.call(l_mmap);
        a.pop(RSI);
        a.st(RAX, 0, RSI); // header = mapping size
        a.add_imm(RAX, 8);
        a.bind(l_done);
        if debug_alloc {
            a.gaddr(RBX, 0);
            a.atomic_inc_mem(RBX, G_NALLOC);
        }
        a.ret();
    }

    // rt_free: rdi = payload pointer (0 is ignored).
    {
        let (l_ret, l_large, l_shared) = (a.label(), a.label(), a.label());
        a.bind(l_free);
        a.raw(&[0x48, 0x85, 0xFF]); // test rdi, rdi
        a.jcc(CC_E, l_ret);
        if debug_alloc {
            a.gaddr(RBX, 0);
            a.atomic_inc_mem(RBX, G_NFREE);
        }
        a.ld(RSI, RDI, -8); // block size
        if debug_alloc {
            a.mov_imm(RBX, FREED);
            a.alu(0x21, RBX, RSI); // and rbx, rsi
            a.jcc(CC_NE, l_panic[P_DOUBLE_FREE]);
            a.mov_imm(RBX, FREED);
            a.alu(0x09, RBX, RSI); // rbx = size | FREED
            a.st(RDI, -8, RBX);
        }
        a.cmp_imm(RSI, SMALL_MAX);
        a.jcc(CC_A, l_large);
        a.bsr(RCX, RSI); // the header holds the class size 2^k
        if has_workers {
            emit_tls_load(&mut a, RDX, 0);
            a.add_imm(RDX, TLS_ALLOC_FREE);
            a.rm_sib(0x8B, RBX, RDX, RCX, 3, 0);
            a.test_rr(RBX);
            a.jcc(CC_NE, l_shared);
            a.mov_imm(RAX, 0);
            a.st(RDI, 0, RAX);
            a.rm_sib(0x89, RDI, RDX, RCX, 3, 0); // keep one hot block per size class locally
            a.ret();
            a.bind(l_shared);
            acquire_runtime_lock(&mut a, g, G_HEAP_LOCK);
            a.bsr(RCX, RSI); // the lock helper clobbers rcx
        }
        a.mov_rr(RDX, RCX);
        a.shl(RDX, 3);
        a.gaddr(RBX, G_FREE as i64);
        a.alu(0x01, RDX, RBX); // rdx = &free_list[class]
        a.ld(RBX, RDX, 0);
        a.st(RDI, 0, RBX); // block.next = head
        a.st(RDX, 0, RDI); // head = block
        if has_workers {
            release_runtime_lock(&mut a, g, G_HEAP_LOCK);
        }
        a.ret();
        a.bind(l_large);
        a.sub_imm(RDI, 8);
        match os {
            Os::Linux => {
                a.mov_imm(RAX, 11); // munmap(rdi = block, rsi = size)
                a.syscall();
            }
            Os::Windows => a.call(l_osfree), // VirtualFree(rdi, 0, MEM_RELEASE)
        }
        a.bind(l_ret);
        a.ret();
    }

    // rt_push: rdi = &list, rsi = value. List = {ptr, len, cap}; capacity 7, then 2*cap+1 (blocks stay 2^k bytes),
    // grown through rt_realloc.
    {
        let (l_store, l_nz) = (a.label(), a.label());
        a.bind(l_push);
        a.push(RSI);
        a.ld(RAX, RDI, 8);
        a.ld(RCX, RDI, 16);
        a.alu(0x39, RAX, RCX);
        a.jcc(CC_NE, l_store);
        a.lea(RDX, RCX, 0);
        a.alu(0x01, RDX, RDX); // 2*cap
        a.inc(RDX); // 2*cap+1
        a.test_rcx();
        a.jcc(CC_NE, l_nz);
        a.mov_imm(RDX, 7);
        a.bind(l_nz);
        a.st(RDI, 16, RDX); // cap = new capacity
        a.push(RDI);
        a.mov_rr(RSI, RDX);
        a.shl(RSI, 3); // new payload bytes
        a.mov_rr(RDX, RAX);
        a.shl(RDX, 3); // bytes to keep (len * 8)
        a.ld(RDI, RDI, 0);
        a.call(l_realloc); // rax = new buffer (old one released)
        a.pop(RDI);
        a.st(RDI, 0, RAX);
        a.bind(l_store);
        a.pop(RSI);
        a.ld(RAX, RDI, 8);
        a.mov_rr(RCX, RAX);
        a.shl(RCX, 3);
        a.ld(RDX, RDI, 0);
        a.alu(0x01, RDX, RCX);
        a.st(RDX, 0, RSI);
        a.add_imm(RAX, 1);
        a.st(RDI, 8, RAX);
        a.ret();
    }

    // rt_get: rdi = &list, rsi = index -> rax. Unsigned compare also rejects negative indices.
    a.bind(l_get);
    a.ld(RAX, RDI, 8);
    a.alu(0x39, RSI, RAX);
    a.jcc(CC_AE, l_panic[P_INDEX]);
    a.ld(RDX, RDI, 0);
    a.shl(RSI, 3);
    a.alu(0x01, RDX, RSI);
    a.ld(RAX, RDX, 0);
    a.ret();

    // rt_memcpy: rdi = dst, rsi = src, rcx = n (regions do not overlap). Clobbers rax, rcx, rdx, rsi, rdi only.
    // Up to 3 bytes use a word and/or a byte. Up to 16 use overlapping head/tail moves.
    // Up to 256 use 8-byte words plus an overlapping tail. Larger copies use rep movsb.
    {
        let (l_lt8, l_lt4, l_lt2, l_big, l_words, l_rep, l_ret) = (
            a.label(),
            a.label(),
            a.label(),
            a.label(),
            a.label(),
            a.label(),
            a.label(),
        );
        a.bind(l_memcpy);
        a.cmp_imm(RCX, 16);
        a.jcc(CC_A, l_big);
        a.cmp_imm(RCX, 8);
        a.jcc(CC_B, l_lt8);
        a.ld(RAX, RSI, 0); // 8..=16: first and last word
        a.alu(0x01, RSI, RCX);
        a.ld(RDX, RSI, -8);
        a.st(RDI, 0, RAX);
        a.alu(0x01, RDI, RCX);
        a.st(RDI, -8, RDX);
        a.ret();
        a.bind(l_lt8);
        a.cmp_imm(RCX, 4);
        a.jcc(CC_B, l_lt4);
        a.ld32(RAX, RSI, 0); // 4..=7: first and last dword
        a.alu(0x01, RSI, RCX);
        a.ld32(RDX, RSI, -4);
        a.st32(RDI, 0, RAX);
        a.alu(0x01, RDI, RCX);
        a.st32(RDI, -4, RDX);
        a.ret();
        a.bind(l_lt4);
        a.cmp_imm(RCX, 2);
        a.jcc(CC_B, l_lt2);
        a.movzx16_mem(RAX, RSI, 0);
        a.mov16_mem(RDI, 0, RAX);
        a.jcc(CC_E, l_ret); // MOV preserves the comparison flags.
        a.movzx8_mem(RAX, RSI, 2);
        a.mov8_mem(RDI, 2, RAX);
        a.ret();
        a.bind(l_lt2);
        a.test_rcx();
        a.jcc(CC_E, l_ret);
        a.movzx8_mem(RAX, RSI, 0);
        a.mov8_mem(RDI, 0, RAX);
        a.bind(l_ret);
        a.ret();
        a.bind(l_big);
        a.cmp_imm(RCX, 256);
        a.jcc(CC_A, l_rep);
        a.bind(l_words); // while more than 8 bytes remain
        a.ld(RAX, RSI, 0);
        a.st(RDI, 0, RAX);
        a.add_imm(RSI, 8);
        a.add_imm(RDI, 8);
        a.sub_imm(RCX, 8);
        a.cmp_imm(RCX, 8);
        a.jcc(CC_A, l_words);
        a.mov_imm(RDX, 8); // 1..=8 left: copy the last 8 bytes of the range (n > 16, so they exist)
        a.alu(0x29, RDX, RCX);
        a.alu(0x29, RSI, RDX);
        a.alu(0x29, RDI, RDX);
        a.ld(RAX, RSI, 0);
        a.st(RDI, 0, RAX);
        a.ret();
        a.bind(l_rep);
        a.raw(&[0xF3, 0xA4]); // rep movsb
        a.ret();
    }

    // rt_realloc: rdi = old payload (0 = none), rsi = new payload bytes, rdx = bytes to keep -> rax = payload.
    // Fits in the block: returned as is. Large block (own mapping) on Linux: mremap (the kernel moves the pages,
    // nothing is copied). Otherwise alloc + copy + free.
    {
        let (l_same, l_alloc0, l_move, l_large, l_big_new) =
            (a.label(), a.label(), a.label(), a.label(), a.label());
        a.bind(l_realloc);
        a.test_rr(RDI);
        a.jcc(CC_E, l_alloc0);
        a.ld(RAX, RDI, -8); // rax = block size S
        if debug_alloc {
            a.mov_imm(RBX, FREED);
            a.alu(0x21, RBX, RAX);
            a.jcc(CC_NE, l_panic[P_DOUBLE_FREE]);
        }
        a.lea(RCX, RSI, 8); // rcx = needed block size N
        a.alu(0x39, RCX, RAX);
        a.jcc(CC_BE, l_same);
        a.cmp_imm(RAX, SMALL_MAX);
        a.jcc(CC_A, l_large);
        a.cmp_imm(RCX, SMALL_MAX);
        a.jcc(CC_A, l_move);
        // small -> small: always a move. (Extending the arena's last block in place would change its size class
        // behind the free lists' back: the smaller class is never refilled and the arena grows without bound.)
        a.jmp(l_move);
        a.bind(l_same);
        a.mov_rr(RAX, RDI);
        a.ret();
        a.bind(l_alloc0);
        a.mov_rr(RDI, RSI);
        a.jmp(l_alloc);
        a.bind(l_large);
        if os == Os::Linux {
            a.add_imm(RCX, 4095);
            a.and_imm(RCX, -4096); // new mapping length
            a.sub_imm(RDI, 8);
            a.mov_rr(RSI, RAX); // old length
            a.mov_rr(RDX, RCX); // new length
            a.mov_imm_ext(10, 1); // MREMAP_MAYMOVE
            a.mov_imm(RAX, 25); // mremap
            a.syscall();
            a.test_rax();
            a.jcc(CC_S, l_panic[P_OOM]);
            a.st(RAX, 0, RDX); // header = mapping size
            a.add_imm(RAX, 8);
            a.ret();
        }
        a.bind(l_big_new);
        a.bind(l_move);
        a.push(RDI);
        a.push(RDX);
        a.mov_rr(RDI, RSI);
        a.call(l_alloc);
        a.pop(RCX); // bytes to keep
        a.pop(RSI); // old payload
        a.push(RAX);
        a.push(RSI);
        a.mov_rr(RDI, RAX);
        a.call(l_memcpy);
        a.pop(RDI);
        a.call(l_free);
        a.pop(RAX);
        a.ret();
    }

    if has_workers {
        emit_threads(
            &mut a,
            l_thread_start,
            l_thread_wait,
            l_mmap,
            l_panic[P_THREAD_START],
            l_thread_recover,
        );
    }

    // user functions
    let rt = Rt {
        vtable_blob: data.len(),
        funcs: l_funcs.clone(),
        panic: l_panic.clone(),
        panic_at: l_panic_at.clone(),
        lib: ir.funcs.iter().map(|f| f.lib).collect(),
        range_panic: l_range_panic,
        print_int: l_print_int,
        print_bool: l_print_bool,
        print_text: l_print_text,
        print_bytes: l_print_bytes,
        panic_bytes: l_panic_bytes,
        push: l_push,
        alloc: l_alloc,
        free: l_free,
        realloc: l_realloc,
        memcpy: l_memcpy,
        thread_start: l_thread_start,
        thread_wait: l_thread_wait,
    };
    let cold_fn: Vec<bool> = ir.funcs.iter().map(|f| f.name == "__list_grow").collect();
    let mut dbg_info = crate::dwarf::DebugInfo::default();
    let debug_scopes = ir.debug.as_ref().map_or(&[][..], |debug| debug.scopes.as_slice());
    for (fi, f) in ir.funcs.iter().enumerate() {
        // functions start on 16 bytes (strbuild: -15% CPU time; padding is never executed)
        if std::env::var_os("YERUX_NO_FALIGN").is_none() {
            a.align_small(15);
        }
        a.bind(l_funcs[fi]);
        let start = a.code.len();
        let decl_loc = ir
            .debug
            .as_ref()
            .and_then(|debug| debug.functions.get(fi))
            .map_or(0, |bindings| bindings.decl_loc);
        if decl_loc != 0 {
            a.lines.push((start, decl_loc));
        }
        a.cfi.clear();
        a.func_unwind = None;
        let (slot_off, scopes) = gen_func(&mut a, &rt, f, &cold_fn, debug_scopes);
        if os == Os::Windows {
            let frame = a.func_unwind.take().map(|(fr, pe)| (fr, pe - start));
            let end = a.code.len();
            a.unwind.push(UnwindRec { begin: start, end, frame });
        }
        if a.dwarf {
            dbg_info.funcs.push(crate::dwarf::FuncInfo {
                name: f.name.clone(),
                start,
                end: a.code.len(),
                cfi: std::mem::take(&mut a.cfi),
                slot_off,
                scopes,
                decl_loc,
            });
        }
    }

    // the tables read by the panic report (see `l_report`), u32 words: a header, the panic site table (return
    // address offset, location id), the locations (file, line, column; indexed by location id), the source files
    // and the functions (start offset, name offset, name length; length 0 = not shown). Names share one blob.
    if report {
        let sites = std::mem::take(&mut a.sites);
        let mut names: Vec<u8> = vec![];
        let mut file_rows: Vec<(u32, u32)> = vec![];
        for name in &ir.files {
            file_rows.push((names.len() as u32, name.len() as u32));
            names.extend_from_slice(name.as_bytes());
        }
        let mut fn_rows: Vec<(u32, u32, u32)> = vec![];
        for (fi, f) in ir.funcs.iter().enumerate() {
            let start = a.labels[l_funcs[fi]].unwrap() as u32;
            let off = names.len() as u32;
            if !(f.name.starts_with("__") || f.name.starts_with('$')) {
                names.extend_from_slice(f.name.as_bytes());
            }
            fn_rows.push((start, off, names.len() as u32 - off));
        }
        let off_sites = 32u32;
        let off_locs = off_sites + 8 * sites.len() as u32;
        let off_files = off_locs + 12 * ir.locs.len() as u32;
        let off_funcs = off_files + 8 * file_rows.len() as u32;
        let off_names = off_funcs + 12 * fn_rows.len() as u32;
        let mut blob: Vec<u8> = vec![];
        let put = |blob: &mut Vec<u8>, w: u32| blob.extend_from_slice(&w.to_le_bytes());
        for w in [sites.len() as u32, fn_rows.len() as u32, off_sites, off_locs, off_files, off_funcs, off_names, 0] {
            put(&mut blob, w);
        }
        for (at, loc) in &sites {
            put(&mut blob, *at as u32);
            put(&mut blob, *loc);
        }
        for (f, l, c) in &ir.locs {
            put(&mut blob, *f);
            put(&mut blob, *l);
            put(&mut blob, *c);
        }
        for (o, n) in &file_rows {
            put(&mut blob, *o);
            put(&mut blob, *n);
        }
        for (st, o, n) in &fn_rows {
            put(&mut blob, *st);
            put(&mut blob, *o);
            put(&mut blob, *n);
        }
        blob.extend_from_slice(&names);
        data[dbg_id] = blob;
    }

    // ── Windows API stubs (Microsoft x64 convention: rcx, rdx, r8, r9, 32-byte shadow space,
    //    rsp 16-aligned at every call; the runtime above calls them with arbitrary alignment, so the
    //    stubs realign themselves) ──
    if os == Os::Windows {
        // Both console I/O stubs take fd/buffer/count in rdi/rsi/rdx.
        // Keep the Linux convention used by std.io: byte count or negative errno.
        for (read, label, import) in [(true, l_osread, IMP_READFILE), (false, l_oswrite, IMP_WRITEFILE)] {
            let (bad_fd, error, broken_pipe, eof, done) =
                (a.label(), a.label(), a.label(), a.label(), a.label());
            a.bind(label);
            if read {
                a.test_rr(RDI);
                a.jcc(CC_NE, bad_fd);
            } else {
                a.cmp_imm(RDI, 1);
                a.jcc(CC_B, bad_fd);
                a.cmp_imm(RDI, 2);
                a.jcc(CC_A, bad_fd);
            }
            a.win_prologue(48);
            a.st(RSP, 48, RDX); // save the original count
            // The panic reporter keeps the source column and file index across writes.
            a.st(RSP, 56, R8);
            a.st(RSP, 64, R9);
            a.mov_imm(RCX, -10);
            a.alu(0x29, RCX, RDI); // STD_INPUT_HANDLE (-10) through STD_ERROR_HANDLE (-12)
            a.call_iat(IMP_GETSTDHANDLE);
            a.mov_rr(RCX, RAX);
            a.ld(R8, RSP, 48);
            a.mov_imm(RAX, u32::MAX as i64);
            a.alu(0x39, R8, RAX);
            a.cmov_rr(CC_A, R8, RAX); // ReadFile and WriteFile take a DWORD count
            a.mov_rr(RDX, RSI);
            a.lea(R9, RSP, 40); // bytes transferred
            a.mov_imm(RAX, 0);
            a.st(RSP, 32, RAX); // no OVERLAPPED
            a.st(RSP, 40, RAX);
            a.call_iat(import);
            a.raw(&[0x85, 0xC0]); // test eax, eax (Win32 BOOL)
            a.jcc(CC_E, error);
            a.ld32(RAX, RSP, 40);
            a.jmp(done);
            a.bind(error);
            a.call_iat(IMP_GETLASTERROR);
            a.raw(&[0x89, 0xC0]); // zero-extend eax (DWORD)
            a.cmp_imm(RAX, 38); // ERROR_HANDLE_EOF
            a.jcc(CC_E, eof);
            for code in [109, 232, 233] { // BROKEN_PIPE, NO_DATA, PIPE_NOT_CONNECTED
                a.cmp_imm(RAX, code);
                a.jcc(CC_E, broken_pipe);
            }
            a.cmp_imm(RAX, 6); // ERROR_INVALID_HANDLE
            a.mov_imm(RCX, -9); // EBADF
            a.mov_imm(RDX, -5); // EIO
            a.cmov_rr(CC_E, RDX, RCX);
            a.cmp_imm(RAX, 5); // ERROR_ACCESS_DENIED
            a.mov_imm(RAX, -13); // EACCES
            a.cmov_rr(CC_NE, RAX, RDX);
            a.jmp(done);
            a.bind(broken_pipe);
            a.mov_imm(RAX, if read { 0 } else { -32 }); // EOF or EPIPE
            a.jmp(done);
            a.bind(eof);
            a.mov_imm(RAX, 0);
            a.bind(done);
            a.ld(RDX, RSP, 48);
            a.ld(R8, RSP, 56);
            a.ld(R9, RSP, 64);
            a.win_epilogue();
            a.bind(bad_fd);
            a.mov_imm(RAX, -9); // EBADF
            a.ret();
        }

        // Only calls accepted by validate_windows can reach this dispatch.
        a.bind(l_winsyscall);
        let pid = a.label();
        for (number, stub) in [(0, l_osread), (1, l_oswrite), (39, pid), (60, l_osexit), (231, l_osexit)] {
            a.cmp_imm(RAX, number);
            a.jcc(CC_E, stub);
        }
        a.raw(&[0x0F, 0x0B]); // ud2
        a.bind(pid);
        a.win_prologue(0);
        a.call_iat(IMP_GETCURRENTPROCESSID);
        a.raw(&[0x89, 0xC0]); // zero-extend eax (DWORD)
        a.win_epilogue();

        // os_exit: rdi = exit code
        a.bind(l_osexit);
        a.mov_rr(RCX, RDI);
        a.raw(&[0x48, 0x83, 0xE4, 0xF0]); // and rsp, -16
        a.raw(&[0x48, 0x83, 0xEC, 0x20]); // sub rsp, 32
        a.call_iat(IMP_EXITPROCESS);
        a.raw(&[0x0F, 0x0B]); // ud2 (not reached)

        // os_alloc: rsi = length -> rax = zeroed read/write pages, or 0
        a.bind(l_osalloc);
        a.win_prologue(0);
        a.raw(&[0x31, 0xC9]); // xor ecx, ecx            lpAddress = NULL
        a.mov_rr(RDX, RSI); //                             dwSize
        a.raw(&[0x41, 0xB8]); // mov r8d, MEM_COMMIT | MEM_RESERVE
        a.imm32(0x3000);
        a.raw(&[0x41, 0xB9]); // mov r9d, PAGE_READWRITE
        a.imm32(4);
        a.call_iat(IMP_VIRTUALALLOC);
        a.win_epilogue();

        // os_free: rdi = base address of a VirtualAlloc'd region
        a.bind(l_osfree);
        a.win_prologue(0);
        a.mov_rr(RCX, RDI);
        a.raw(&[0x31, 0xD2]); // xor edx, edx            dwSize = 0 (required with MEM_RELEASE)
        a.raw(&[0x41, 0xB8]); // mov r8d, MEM_RELEASE
        a.imm32(0x8000);
        a.call_iat(IMP_VIRTUALFREE);
        a.win_epilogue();
    }

    // lay out: code, padding to 16, then data blobs
    while !a.code.len().is_multiple_of(16) {
        a.code.push(0x90);
    }
    let mut offs = vec![];
    let mut blob: Vec<u8> = vec![];
    for d in &data {
        offs.push(blob.len());
        blob.extend_from_slice(d);
    }
    let data_start = match os {
        Os::Linux => a.code.len(),
        Os::Windows => win.rdata_rel,
    };
    for table in &ir.vtables {
        blob.resize((blob.len() + 7) & !7, 0);
        offs.push(blob.len());
        let address = (data_start + blob.len()) as i64;
        for &func in table {
            let code = a.labels[l_funcs[func]].unwrap() as i64;
            blob.extend_from_slice(&(code - address).to_le_bytes());
        }
    }
    for bits in a.pool.clone() {
        while !blob.len().is_multiple_of(8) {
            blob.push(0);
        }
        offs.push(blob.len());
        blob.extend_from_slice(&bits.to_le_bytes());
    }
    let imports: Vec<(usize, usize)> = a.xfix.clone();
    match os {
        Os::Linux => {
            // Dynamic ELF places the import GOT in the RW segment at G. The
            // assembler fixup is relative to the RX code base, so encode that
            // virtual-address difference here; the static path never imports.
            let got_base = (G - (0x400000 + ((64 + 56 * 5 + 15) & !15) as i64)) as usize;
            let got: Vec<usize> = (0..ir.externs.len())
                .map(|i| got_base + blob.len() + 64 + 8 * i)
                .collect();
            a.finalize(data_start, &offs, &[], &got);
        }
        Os::Windows => {
            let iat: Vec<usize> = (0..IMPORTS.len()).map(|i| win.iat_rel + 8 * i).collect();
            a.finalize(win.rdata_rel, &offs, &iat, &[]);
        }
    }
    let entry = a.labels[l_start].unwrap();
    dbg_info.lines = std::mem::take(&mut a.lines);
    Built {
        dbg: dbg_info,
        unwind: std::mem::take(&mut a.unwind),
        code: a.code,
        blob,
        entry,
        imports,
    }
}

// ───────────────────────── ELF64 writer ─────────────────────────
// One PT_LOAD segment (R+X) holding headers + code + read-only data.

const BASE: u64 = 0x400000;
const HDR: u64 = 64 + 56 * 2;

fn elf64(blob: &[u8], entry_off: usize) -> Vec<u8> {
    let total = HDR + blob.len() as u64;
    let mut o: Vec<u8> = vec![];
    o.extend_from_slice(&[0x7F, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // ident
    o.extend_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    o.extend_from_slice(&0x3Eu16.to_le_bytes()); // x86-64
    o.extend_from_slice(&1u32.to_le_bytes()); // version
    o.extend_from_slice(&(BASE + HDR + entry_off as u64).to_le_bytes()); // entry
    o.extend_from_slice(&64u64.to_le_bytes()); // phoff
    o.extend_from_slice(&0u64.to_le_bytes()); // shoff
    o.extend_from_slice(&0u32.to_le_bytes()); // flags
    o.extend_from_slice(&64u16.to_le_bytes()); // ehsize
    o.extend_from_slice(&56u16.to_le_bytes()); // phentsize
    o.extend_from_slice(&2u16.to_le_bytes()); // phnum
    o.extend_from_slice(&64u16.to_le_bytes()); // shentsize
    o.extend_from_slice(&0u16.to_le_bytes()); // shnum
    o.extend_from_slice(&0u16.to_le_bytes()); // shstrndx
                                              // program header
    o.extend_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    o.extend_from_slice(&5u32.to_le_bytes()); // R + X
    o.extend_from_slice(&0u64.to_le_bytes()); // offset
    o.extend_from_slice(&BASE.to_le_bytes()); // vaddr
    o.extend_from_slice(&BASE.to_le_bytes()); // paddr
    o.extend_from_slice(&total.to_le_bytes()); // filesz
    o.extend_from_slice(&total.to_le_bytes()); // memsz
    o.extend_from_slice(&0x1000u64.to_le_bytes()); // align
                                                   // writable zero-filled segment for the runtime state (heap free lists, counters)
    o.extend_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    o.extend_from_slice(&6u32.to_le_bytes()); // R + W
    o.extend_from_slice(&0u64.to_le_bytes()); // offset
    o.extend_from_slice(&(G as u64).to_le_bytes()); // vaddr
    o.extend_from_slice(&(G as u64).to_le_bytes()); // paddr
    o.extend_from_slice(&0u64.to_le_bytes()); // filesz
    o.extend_from_slice(&G_SIZE.to_le_bytes()); // memsz
    o.extend_from_slice(&0x1000u64.to_le_bytes()); // align
    o.extend_from_slice(blob);
    o
}

// ───────────────────────── PE32+ writer (Windows x86-64) ─────────────────────────
// No linker involved. Layout (file alignment 0x200, section alignment 0x1000):
//   headers  : DOS header + stub, "PE\0\0", COFF header, optional header (PE32+), 4 section headers
//   .text    : code                         (R+X)
//   .rdata   : data blobs (text literals…)  (R)
//   .data    : heap state, zero-filled      (R+W; raw part is one file-alignment unit of zeros)
//   .idata   : import directory, ILT, IAT, names for kernel32.dll (R+W; the loader writes the IAT)
// The image is position independent: DYNAMIC_BASE is set (ASLR) and the code holds no absolute address (the heap
// state, text literals, vtables and the IAT are all reached rip-relative), so the base relocation directory is empty.
// .pdata/.xdata carry the unwind information of every generated function.

const PE_BASE: u64 = 0x1_4000_0000;
const PE_SECT_ALIGN: u64 = 0x1000;
const PE_FILE_ALIGN: u64 = 0x200;
const PE_HDR_SIZE: u64 = 0x400;
const PE_STACK: u64 = 8 << 20; // reserve = commit: no guard-page growth, so large frames need no probes
const PE_DLL: &str = "KERNEL32.dll";

fn align_up(v: u64, a: u64) -> u64 {
    (v + a - 1) & !(a - 1)
}

#[derive(Clone, Copy)]
struct PeLayout {
    text_rva: u64,
    rdata_rva: u64,
    data_rva: u64,
    idata_rva: u64,
    text_size: u64,
    rdata_size: u64,
    idata_size: u64,
    pdata_rva: u64,
    pdata_size: u64,
    xdata_rva: u64,
    xdata_size: u64,
    reloc_rva: u64,
}

/// size of the (empty) base relocation directory: one block header for page 0. The image holds no absolute
/// address, so there is nothing to relocate; the directory only has to exist for the loader to rebase the image.
const PE_RELOC_SIZE: u64 = 8;

impl PeLayout {
    /// RVA of the import address table (inside .idata, after the descriptors and the ILT)
    fn iat_rva(&self) -> u64 {
        self.idata_rva + 40 + 8 * (IMPORTS.len() as u64 + 1)
    }
}

fn idata_len() -> u64 {
    let mut n = 40 + 2 * 8 * (IMPORTS.len() as u64 + 1);
    for name in IMPORTS {
        n += (2 + name.len() as u64 + 1 + 1) & !1;
    }
    n + PE_DLL.len() as u64 + 1
}

fn pe_layout(code_len: usize, blob_len: usize, nfuncs: usize, xdata_len: usize) -> PeLayout {
    let text_rva = PE_SECT_ALIGN;
    let text_size = code_len as u64;
    let rdata_rva = text_rva + align_up(text_size, PE_SECT_ALIGN);
    let rdata_size = blob_len as u64;
    let data_rva = rdata_rva + align_up(rdata_size.max(1), PE_SECT_ALIGN);
    let idata_rva = data_rva + PE_SECT_ALIGN; // .data is one page (G_SIZE)
    let pdata_rva = idata_rva + align_up(idata_len(), PE_SECT_ALIGN);
    let pdata_size = 12 * nfuncs as u64;
    let xdata_rva = pdata_rva + align_up(pdata_size.max(1), PE_SECT_ALIGN);
    let xdata_size = xdata_len as u64;
    let reloc_rva = xdata_rva + align_up(xdata_size.max(1), PE_SECT_ALIGN);
    PeLayout {
        pdata_rva,
        pdata_size,
        xdata_rva,
        xdata_size,
        reloc_rva,
        text_rva,
        rdata_rva,
        data_rva,
        idata_rva,
        text_size,
        rdata_size,
        idata_size: idata_len(),
    }
}

fn put16(o: &mut Vec<u8>, v: u16) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn put32(o: &mut Vec<u8>, v: u32) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn put64(o: &mut Vec<u8>, v: u64) {
    o.extend_from_slice(&v.to_le_bytes());
}

/// Import directory for kernel32.dll: descriptor + null descriptor, ILT, IAT, hint/name table, DLL name.
fn build_idata(l: &PeLayout) -> Vec<u8> {
    let n = IMPORTS.len() as u64;
    let ilt = l.idata_rva + 40;
    let iat = ilt + 8 * (n + 1);
    let mut names_at = iat + 8 * (n + 1);
    let mut hint_rvas = vec![];
    let mut names: Vec<u8> = vec![];
    for name in IMPORTS {
        hint_rvas.push(names_at);
        put16(&mut names, 0); // hint
        names.extend_from_slice(name.as_bytes());
        names.push(0);
        if names.len() % 2 == 1 {
            names.push(0);
        }
        names_at = l.idata_rva + 40 + 16 * (n + 1) + names.len() as u64;
    }
    let dll_rva = names_at;
    let mut o = vec![];
    put32(&mut o, ilt as u32); // OriginalFirstThunk
    put32(&mut o, 0); // TimeDateStamp
    put32(&mut o, 0); // ForwarderChain
    put32(&mut o, dll_rva as u32); // Name
    put32(&mut o, iat as u32); // FirstThunk
    o.extend_from_slice(&[0u8; 20]); // terminating descriptor
    for _ in 0..2 {
        // ILT and IAT start out identical: RVAs of the hint/name entries
        for r in &hint_rvas {
            put64(&mut o, *r);
        }
        put64(&mut o, 0);
    }
    o.extend_from_slice(&names);
    o.extend_from_slice(PE_DLL.as_bytes());
    o.push(0);
    assert_eq!(o.len() as u64, l.idata_size);
    o
}

/// `.pdata` (RUNTIME_FUNCTION: begin, end, unwind RVA, sorted by begin) and `.xdata` (UNWIND_INFO, shared between
/// functions with the same frame) for the functions in `recs`. With `text_rva == 0` only the sizes are meaningful.
///
/// A framed function is `push rbp; mov rbp, rsp; sub rsp, frame`: unwind codes ALLOC, SET_FPREG, PUSH_NONVOL(rbp).
/// A frameless one has no codes (rsp points at the return address throughout). The callee-saved registers the
/// prologue stores with `mov` are not described (UWOP_SAVE_NONVOL offsets are relative to a frame base the
/// frame size does not fit), so an unwinder recovers rip and rsp exactly but not r12-r15/rbx of the caller.
fn unwind_tables(recs: &[UnwindRec], text_rva: u64, xdata_rva: u64) -> (Vec<u8>, usize) {
    let mut xdata: Vec<u8> = vec![];
    let mut seen: Vec<(Option<(i32, usize)>, u32)> = vec![];
    let mut pdata: Vec<u8> = vec![];
    for r in recs {
        let at = match seen.iter().find(|(k, _)| *k == r.frame) {
            Some((_, off)) => *off,
            None => {
                let off = xdata.len() as u32;
                let mut codes: Vec<u8> = vec![];
                let mut prolog = 0u8;
                if let Some((frame, pe)) = r.frame {
                    prolog = pe as u8;
                    let f = frame as u32;
                    if frame > 0 {
                        if (8..=128).contains(&f) && f % 8 == 0 {
                            codes.extend_from_slice(&[pe as u8, (((f - 8) / 8) as u8) << 4 | 2]); // ALLOC_SMALL
                        } else if f % 8 == 0 && f / 8 <= 0xFFFF {
                            codes.extend_from_slice(&[pe as u8, 1]); // ALLOC_LARGE, size/8 in the next slot
                            codes.extend_from_slice(&((f / 8) as u16).to_le_bytes());
                        } else {
                            codes.extend_from_slice(&[pe as u8, 0x11]); // ALLOC_LARGE, 32-bit size
                            codes.extend_from_slice(&f.to_le_bytes());
                        }
                    }
                    codes.extend_from_slice(&[4, 3]); // SET_FPREG (rbp, offset 0) after `mov rbp, rsp`
                    codes.extend_from_slice(&[1, (5 << 4) | 0]); // PUSH_NONVOL rbp after `push rbp`
                }
                let slots = codes.len() / 2;
                xdata.push(1); // version 1, no flags
                xdata.push(prolog);
                xdata.push(slots as u8);
                xdata.push(if r.frame.is_some() { 5 } else { 0 }); // frame register rbp, offset 0
                xdata.extend_from_slice(&codes);
                if slots % 2 == 1 {
                    xdata.extend_from_slice(&[0, 0]); // the code array is padded to an even slot count
                }
                seen.push((r.frame, off));
                off
            }
        };
        put32(&mut pdata, (text_rva + r.begin as u64) as u32);
        put32(&mut pdata, (text_rva + r.end as u64) as u32);
        put32(&mut pdata, (xdata_rva + at as u64) as u32);
    }
    let n = xdata.len();
    let mut out = pdata;
    out.extend_from_slice(&xdata);
    (out, n)
}

fn pe64(code: &[u8], blob: &[u8], entry_off: usize, l: &PeLayout, unwind: &[UnwindRec]) -> Vec<u8> {
    let (tables, _) = unwind_tables(unwind, l.text_rva, l.xdata_rva);
    let (pdata, xdata) = tables.split_at(l.pdata_size as usize);
    let mut reloc: Vec<u8> = vec![];
    put32(&mut reloc, 0); // page RVA
    put32(&mut reloc, PE_RELOC_SIZE as u32); // block size: header only

    let idata = build_idata(l);
    let raw_text = align_up(code.len() as u64, PE_FILE_ALIGN);
    let raw_rdata = align_up(blob.len() as u64, PE_FILE_ALIGN);
    let raw_data = PE_FILE_ALIGN;
    let raw_idata = align_up(idata.len() as u64, PE_FILE_ALIGN);
    let ptr_text = PE_HDR_SIZE;
    let ptr_rdata = ptr_text + raw_text;
    let ptr_data = ptr_rdata + raw_rdata;
    let ptr_idata = ptr_data + raw_data;
    let raw_pdata = align_up(pdata.len() as u64, PE_FILE_ALIGN);
    let raw_xdata = align_up(xdata.len() as u64, PE_FILE_ALIGN);
    let raw_reloc = align_up(reloc.len() as u64, PE_FILE_ALIGN);
    let ptr_pdata = ptr_idata + raw_idata;
    let ptr_xdata = ptr_pdata + raw_pdata;
    let ptr_reloc = ptr_xdata + raw_xdata;
    let size_of_image = l.reloc_rva + align_up(PE_RELOC_SIZE, PE_SECT_ALIGN);

    let mut o: Vec<u8> = vec![];
    // DOS header (64 bytes) + stub; e_lfanew = 0x80
    o.extend_from_slice(b"MZ");
    for v in [0x90u16, 3, 0, 4, 0, 0xFFFF, 0, 0xB8, 0, 0, 0, 0x40] {
        put16(&mut o, v); // e_cblp, e_cp, e_crlc, e_cparhdr, e_minalloc, e_maxalloc, e_ss, e_sp, e_csum, e_ip, e_cs, e_lfarlc
    }
    o.resize(0x3C, 0);
    put32(&mut o, 0x80);
    // DOS stub: prints a message and exits when run under DOS
    o.extend_from_slice(&[
        0x0E, 0x1F, 0xBA, 0x0E, 0x00, 0xB4, 0x09, 0xCD, 0x21, 0xB8, 0x01, 0x4C, 0xCD,
        0x21, // push cs; pop ds; mov dx,0xE; mov ah,9; int 21h; mov ax,4C01h; int 21h
    ]);
    o.extend_from_slice(b"This program cannot be run in DOS mode.\r\r\n$");
    o.resize(0x80, 0);
    // PE signature + COFF file header
    o.extend_from_slice(b"PE\0\0");
    put16(&mut o, 0x8664); // Machine: AMD64
    put16(&mut o, 7); // NumberOfSections
    put32(&mut o, 0); // TimeDateStamp (0: reproducible output)
    put32(&mut o, 0); // PointerToSymbolTable
    put32(&mut o, 0); // NumberOfSymbols
    put16(&mut o, 240); // SizeOfOptionalHeader
    put16(&mut o, 0x0022); // EXECUTABLE_IMAGE | LARGE_ADDRESS_AWARE
                           // optional header (PE32+)
    put16(&mut o, 0x20B); // magic
    o.push(14); // linker version (informational)
    o.push(0);
    put32(&mut o, raw_text as u32); // SizeOfCode
    put32(&mut o, (raw_rdata + raw_data + raw_idata + raw_pdata + raw_xdata + raw_reloc) as u32); // SizeOfInitializedData
    put32(&mut o, 0); // SizeOfUninitializedData
    put32(&mut o, (l.text_rva + entry_off as u64) as u32); // AddressOfEntryPoint
    put32(&mut o, l.text_rva as u32); // BaseOfCode
    put64(&mut o, PE_BASE); // ImageBase
    put32(&mut o, PE_SECT_ALIGN as u32);
    put32(&mut o, PE_FILE_ALIGN as u32);
    put16(&mut o, 6); // OS version 6.0 (Vista)
    put16(&mut o, 0);
    put16(&mut o, 0); // image version
    put16(&mut o, 0);
    put16(&mut o, 6); // subsystem version 6.0
    put16(&mut o, 0);
    put32(&mut o, 0); // Win32VersionValue
    put32(&mut o, size_of_image as u32);
    put32(&mut o, PE_HDR_SIZE as u32); // SizeOfHeaders
    put32(&mut o, 0); // CheckSum (not verified for user-mode exes)
    put16(&mut o, 3); // Subsystem: WINDOWS_CUI (console)
    // DllCharacteristics: HIGH_ENTROPY_VA | DYNAMIC_BASE (ASLR) | NX_COMPAT | TERMINAL_SERVER_AWARE
    put16(&mut o, 0x0020 | 0x0040 | 0x0100 | 0x8000);
    put64(&mut o, PE_STACK); // SizeOfStackReserve
    put64(&mut o, PE_STACK); // SizeOfStackCommit
    put64(&mut o, 0x100000); // SizeOfHeapReserve
    put64(&mut o, 0x1000); // SizeOfHeapCommit
    put32(&mut o, 0); // LoaderFlags
    put32(&mut o, 16); // NumberOfRvaAndSizes
    for i in 0..16 {
        let (rva, size) = match i {
            1 => (l.idata_rva, l.idata_size),                    // import table
            3 => (l.pdata_rva, l.pdata_size),                    // exception table (.pdata)
            5 => (l.reloc_rva, PE_RELOC_SIZE),                   // base relocations (empty)
            12 => (l.iat_rva(), 8 * (IMPORTS.len() as u64 + 1)), // IAT
            _ => (0, 0),
        };
        put32(&mut o, rva as u32);
        put32(&mut o, size as u32);
    }
    // section headers
    let mut sect = |name: &[u8; 8], vsize: u64, rva: u64, rawsz: u64, rawptr: u64, flags: u32| {
        o.extend_from_slice(name);
        put32(&mut o, vsize as u32);
        put32(&mut o, rva as u32);
        put32(&mut o, rawsz as u32);
        put32(&mut o, rawptr as u32);
        put32(&mut o, 0); // relocations
        put32(&mut o, 0); // line numbers
        put16(&mut o, 0);
        put16(&mut o, 0);
        put32(&mut o, flags);
    };
    sect(
        b".text\0\0\0",
        l.text_size,
        l.text_rva,
        raw_text,
        ptr_text,
        0x6000_0020,
    ); // code | exec | read
    sect(
        b".rdata\0\0",
        l.rdata_size.max(1),
        l.rdata_rva,
        raw_rdata,
        ptr_rdata,
        0x4000_0040,
    ); // initialized | read
    sect(
        b".data\0\0\0",
        G_SIZE,
        l.data_rva,
        raw_data,
        ptr_data,
        0xC000_0040,
    ); // initialized | read | write
    sect(
        b".idata\0\0",
        l.idata_size,
        l.idata_rva,
        raw_idata,
        ptr_idata,
        0xC000_0040,
    );
    sect(b".pdata\0\0", l.pdata_size.max(1), l.pdata_rva, raw_pdata, ptr_pdata, 0x4000_0040); // initialized | read
    sect(b".xdata\0\0", l.xdata_size.max(1), l.xdata_rva, raw_xdata, ptr_xdata, 0x4000_0040);
    sect(b".reloc\0\0", PE_RELOC_SIZE, l.reloc_rva, raw_reloc, ptr_reloc, 0x4200_0040); // initialized | discardable | read
    assert!(o.len() as u64 <= PE_HDR_SIZE);
    o.resize(PE_HDR_SIZE as usize, 0);
    let put_sect = |o: &mut Vec<u8>, bytes: &[u8], raw: u64| {
        o.extend_from_slice(bytes);
        o.resize(o.len() + (raw as usize - bytes.len()), 0);
    };
    put_sect(&mut o, code, raw_text);
    put_sect(&mut o, blob, raw_rdata);
    put_sect(&mut o, &[], raw_data);
    put_sect(&mut o, &idata, raw_idata);
    put_sect(&mut o, pdata, raw_pdata);
    put_sect(&mut o, xdata, raw_xdata);
    put_sect(&mut o, &reloc, raw_reloc);
    o
}

#[cfg(test)]
mod tests {
    use super::magic;

    // emulate the emitted instruction sequence with i128 and compare with Rust's / and %
    fn div_by_magic(n: i64, d: i64) -> i64 {
        let (m, s) = magic(d);
        let mut hi = ((m as i128 * n as i128) >> 64) as i64;
        if d > 0 && m < 0 {
            hi = hi.wrapping_add(n);
        } else if d < 0 && m > 0 {
            hi = hi.wrapping_sub(n);
        }
        hi >>= s;
        hi + ((hi as u64) >> 63) as i64
    }

    #[test]
    fn magic_division_matches_idiv() {
        let mut x: u64 = 0x9E3779B97F4A7C15;
        let mut rnd = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let mut ds: Vec<i64> = (2..300)
            .chain([1000003, 65536, 2147483647, 2147483648])
            .collect();
        ds.extend((0..300).map(|_| (rnd() >> (rnd() % 40 + 22)) as i64 | 2));
        let ds: Vec<i64> = ds
            .iter()
            .flat_map(|&d| [d, -d])
            .filter(|d| d.unsigned_abs() >= 2 && *d >= i32::MIN as i64 && *d <= i32::MAX as i64)
            .collect();
        for d in ds {
            let mut ns = vec![
                0,
                1,
                -1,
                d,
                -d,
                d - 1,
                d + 1,
                i64::MAX,
                i64::MIN,
                i64::MIN + 1,
                i64::MAX - 1,
            ];
            ns.extend((0..400).map(|_| (rnd() as i64) >> (rnd() % 64)));
            for n in ns {
                assert_eq!(div_by_magic(n, d), n.wrapping_div(d), "{} / {}", n, d);
            }
        }
    }
}
