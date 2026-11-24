// Yerux native backend for AArch64 (ARM64) Linux.
// Hand-written A64 encoder + runtime (raw syscalls, no libc) + ELF64 writer (EM_AARCH64).
// Mirrors x64.rs: every virtual register lives in a stack slot; same panic messages,
// output format and exit codes. Syscalls (number in x8): write = 64, mmap = 222, munmap = 215, exit_group = 94.
//
// Register use: x0/x1/x2/x3 scratch (x0 = results, print args), x8 syscall number,
// x16 address scratch, x29 frame pointer, x30 link register.
// Calling convention (internal): all arguments on the stack, argument i at
// [x29 + 16 + 8*i] in the callee; result in x0.
use crate::ir::*;

mod threads;

const X0: u32 = 0;
const X1: u32 = 1;
const X2: u32 = 2;
const X3: u32 = 3;
const X4: u32 = 4;
const X5: u32 = 5;
const X6: u32 = 6;
const X7: u32 = 7;
const X8: u32 = 8;
const X9: u32 = 9;
const X10: u32 = 10;
const X11: u32 = 11;
const X12: u32 = 12;
const X13: u32 = 13;
const X15: u32 = 15;
const X16: u32 = 16;
const X17: u32 = 17;
const X19: u32 = 19; // current worker context
const X29: u32 = 29;
const X30: u32 = 30;
const XZR: u32 = 31; // zero register in most encodings
const SP: u32 = 31; // stack pointer in add/sub-immediate, ldr/str base, extended forms

// condition codes
const EQ: u32 = 0;
const NE: u32 = 1;
const HS: u32 = 2;
const LO: u32 = 3;
const LS: u32 = 9;
const MI: u32 = 4;
const HI: u32 = 8;
const VS: u32 = 6;
const PL: u32 = 5;
const GE: u32 = 10;
const LT: u32 = 11;
const GT: u32 = 12;
const LE: u32 = 13;

#[derive(Clone, Copy)]
enum Fix {
    B26, // b / bl: imm26 words
    B19, // b.cond / cbz / cbnz: imm19 words
}

// ───────────────────────── Encoder ─────────────────────────

pub struct Asm {
    pub code: Vec<u8>,
    labels: Vec<Option<usize>>,
    fix: Vec<(usize, Fix, usize)>, // (instruction position, kind, label)
    dfix: Vec<(usize, usize)>,     // (position of ADRP; ADD follows, data blob id)
    lfix: Vec<(usize, usize)>, // (position of ADRP; ADD follows, code label): address of a function
}

impl Asm {
    pub fn new() -> Asm {
        Asm {
            code: vec![],
            labels: vec![],
            fix: vec![],
            dfix: vec![],
            lfix: vec![],
        }
    }
    pub fn emit(&mut self, w: u32) {
        self.code.extend_from_slice(&w.to_le_bytes());
    }
    pub fn label(&mut self) -> usize {
        self.labels.push(None);
        self.labels.len() - 1
    }
    pub fn bind(&mut self, l: usize) {
        self.labels[l] = Some(self.code.len());
    }

    // --- moves / constants
    pub fn movz(&mut self, rd: u32, imm16: u32, hw: u32) {
        self.emit(0xD280_0000 | hw << 21 | imm16 << 5 | rd);
    }
    pub fn movk(&mut self, rd: u32, imm16: u32, hw: u32) {
        self.emit(0xF280_0000 | hw << 21 | imm16 << 5 | rd);
    }
    pub fn movn(&mut self, rd: u32, imm16: u32, hw: u32) {
        self.emit(0x9280_0000 | hw << 21 | imm16 << 5 | rd);
    }
    pub fn mov_imm(&mut self, rd: u32, v: i64) {
        let u = v as u64;
        let chunks = |x: u64| {
            [
                (x & 0xFFFF) as u32,
                ((x >> 16) & 0xFFFF) as u32,
                ((x >> 32) & 0xFFFF) as u32,
                ((x >> 48) & 0xFFFF) as u32,
            ]
        };
        let zeros = chunks(u).iter().filter(|c| **c == 0).count();
        let ones = chunks(u).iter().filter(|c| **c == 0xFFFF).count();
        if ones > zeros {
            let c = chunks(!u);
            let first = c.iter().position(|x| *x != 0).unwrap_or(0);
            self.movn(rd, c[first], first as u32);
            for (i, x) in c.iter().enumerate() {
                if i != first && *x != 0 {
                    self.movk(rd, (!*x) & 0xFFFF, i as u32); // restore original chunk
                }
            }
        } else {
            let c = chunks(u);
            let first = c.iter().position(|x| *x != 0).unwrap_or(0);
            self.movz(rd, c[first], first as u32);
            for (i, x) in c.iter().enumerate() {
                if i != first && *x != 0 {
                    self.movk(rd, *x, i as u32);
                }
            }
        }
    }
    /// mov xd, xm  (orr xd, xzr, xm); neither may be SP.
    pub fn mov(&mut self, rd: u32, rm: u32) {
        self.emit(0xAA00_03E0 | rm << 16 | rd);
    }
    /// mov xd, sp / mov sp, xn  (add #0)
    pub fn mov_sp(&mut self, rd: u32, rn: u32) {
        self.add_imm(rd, rn, 0);
    }

    // --- arithmetic
    pub fn add_imm(&mut self, rd: u32, rn: u32, imm12: u32) {
        assert!(imm12 < 4096);
        self.emit(0x9100_0000 | imm12 << 10 | rn << 5 | rd);
    }
    pub fn sub_imm(&mut self, rd: u32, rn: u32, imm12: u32) {
        assert!(imm12 < 4096);
        self.emit(0xD100_0000 | imm12 << 10 | rn << 5 | rd);
    }
    /// sub rd, rn, #imm (any 24-bit amount, as two instructions if needed)
    pub fn sub_sp_const(&mut self, rd: u32, rn: u32, imm: u32, add: bool) {
        assert!(imm < (1 << 24));
        let (hi, lo) = (imm >> 12, imm & 0xFFF);
        let base = if add { 0x9100_0000u32 } else { 0xD100_0000 };
        let mut src = rn;
        if hi != 0 {
            self.emit(base | 1 << 22 | hi << 10 | src << 5 | rd);
            src = rd;
        }
        if lo != 0 || hi == 0 {
            self.emit(base | lo << 10 | src << 5 | rd);
        }
    }
    pub fn add_reg(&mut self, rd: u32, rn: u32, rm: u32) {
        self.emit(0x8B00_0000 | rm << 16 | rn << 5 | rd);
    }
    /// add xd, xn, xm, uxtx  (valid with SP as xd/xn)
    pub fn add_ext(&mut self, rd: u32, rn: u32, rm: u32) {
        self.emit(0x8B20_6000 | rm << 16 | rn << 5 | rd);
    }
    pub fn adds(&mut self, rd: u32, rn: u32, rm: u32) {
        self.emit(0xAB00_0000 | rm << 16 | rn << 5 | rd);
    }
    pub fn subs(&mut self, rd: u32, rn: u32, rm: u32) {
        self.emit(0xEB00_0000 | rm << 16 | rn << 5 | rd);
    }
    pub fn sub_reg(&mut self, rd: u32, rn: u32, rm: u32) {
        self.emit(0xCB00_0000 | rm << 16 | rn << 5 | rd);
    }
    pub fn cmp(&mut self, rn: u32, rm: u32) {
        self.subs(XZR, rn, rm);
    }
    /// cmn xn, #1  (adds xzr, xn, #1): sets Z when xn == -1
    pub fn cmn_1(&mut self, rn: u32) {
        self.emit(0xB100_0400 | rn << 5 | XZR);
    }
    pub fn mul(&mut self, rd: u32, rn: u32, rm: u32) {
        self.emit(0x9B00_7C00 | rm << 16 | rn << 5 | rd);
    }
    pub fn smulh(&mut self, rd: u32, rn: u32, rm: u32) {
        self.emit(0x9B40_7C00 | rm << 16 | rn << 5 | rd);
    }
    pub fn sdiv(&mut self, rd: u32, rn: u32, rm: u32) {
        self.emit(0x9AC0_0C00 | rm << 16 | rn << 5 | rd);
    }
    pub fn udiv(&mut self, rd: u32, rn: u32, rm: u32) {
        self.emit(0x9AC0_0800 | rm << 16 | rn << 5 | rd);
    }
    /// msub rd, rn, rm, ra : rd = ra - rn*rm
    pub fn msub(&mut self, rd: u32, rn: u32, rm: u32, ra: u32) {
        self.emit(0x9B00_8000 | rm << 16 | ra << 10 | rn << 5 | rd);
    }
    /// cmp xn, xm, asr #63  (subs xzr, xn, xm, asr #63)
    pub fn cmp_asr63(&mut self, rn: u32, rm: u32) {
        self.emit(0xEB80_0000 | rm << 16 | 63 << 10 | rn << 5 | XZR);
    }
    pub fn eor1(&mut self, rd: u32, rn: u32) {
        self.emit(0xD240_0000 | rn << 5 | rd); // eor xd, xn, #1
    }
    /// cset xd, cond  (csinc xd, xzr, xzr, !cond)
    pub fn cset(&mut self, rd: u32, cond: u32) {
        self.emit(0x9A9F_07E0 | (cond ^ 1) << 12 | rd);
    }

    // --- memory. `mem` picks scaled-unsigned, unscaled-signed (ldur/stur) or an x16-computed address.
    fn mem(&mut self, scaled_op: u32, unscaled_op: u32, scale: u32, rt: u32, rn: u32, off: i32) {
        if off >= 0 && (off as u32).is_multiple_of(scale) && (off as u32 / scale) < 4096 {
            self.emit(scaled_op | (off as u32 / scale) << 10 | rn << 5 | rt);
        } else if (-256..256).contains(&off) {
            self.emit(unscaled_op | ((off as u32) & 0x1FF) << 12 | rn << 5 | rt);
        } else {
            self.mov_imm(X16, off as i64);
            self.add_ext(X16, rn, X16);
            self.emit(scaled_op | X16 << 5 | rt);
        }
    }
    pub fn ldr_i(&mut self, rt: u32, rn: u32, off: i32) {
        self.mem(0xF940_0000, 0xF840_0000, 8, rt, rn, off);
    }
    pub fn str_i(&mut self, rt: u32, rn: u32, off: i32) {
        self.mem(0xF900_0000, 0xF800_0000, 8, rt, rn, off);
    }
    pub fn ldrb_i(&mut self, rt: u32, rn: u32, off: i32) {
        self.mem(0x3940_0000, 0x3840_0000, 1, rt, rn, off);
    }
    pub fn strb_i(&mut self, rt: u32, rn: u32, off: i32) {
        self.mem(0x3900_0000, 0x3800_0000, 1, rt, rn, off);
    }
    pub fn ldr(&mut self, rt: u32, rn: u32, off: u32) {
        self.ldr_i(rt, rn, off as i32);
    }
    pub fn str(&mut self, rt: u32, rn: u32, off: u32) {
        self.str_i(rt, rn, off as i32);
    }
    pub fn strb(&mut self, rt: u32, rn: u32, off: u32) {
        self.strb_i(rt, rn, off as i32);
    }
    /// ldrb wt, [xn], #1  /  strb wt, [xn], #1  (post-index)
    pub fn ldrb_post1(&mut self, rt: u32, rn: u32) {
        self.emit(0x3840_1400 | rn << 5 | rt);
    }
    pub fn strb_post1(&mut self, rt: u32, rn: u32) {
        self.emit(0x3800_1400 | rn << 5 | rt);
    }
    /// clz xd, xn
    pub fn clz(&mut self, rd: u32, rn: u32) {
        self.emit(0xDAC0_1000 | rn << 5 | rd);
    }
    /// lsl xd, xn, xm (variable shift)
    pub fn lslv(&mut self, rd: u32, rn: u32, rm: u32) {
        self.emit(0x9AC0_2000 | rm << 16 | rn << 5 | rd);
    }
    /// ldr wt, [xn, #off] / str wt, [xn, #off] (32-bit, zero-extends)
    pub fn ldr32_i(&mut self, rt: u32, rn: u32, off: i32) {
        self.mem(0xB940_0000, 0xB840_0000, 4, rt, rn, off);
    }
    pub fn str32_i(&mut self, rt: u32, rn: u32, off: i32) {
        self.mem(0xB900_0000, 0xB800_0000, 4, rt, rn, off);
    }
    /// ldp xt, xt2, [xn]  /  stp xt, xt2, [xn]
    pub fn ldp0(&mut self, rt: u32, rt2: u32, rn: u32) {
        self.emit(0xA940_0000 | rt2 << 10 | rn << 5 | rt);
    }
    pub fn stp0(&mut self, rt: u32, rt2: u32, rn: u32) {
        self.emit(0xA900_0000 | rt2 << 10 | rn << 5 | rt);
    }
    /// ldp xt, xt2, [xn], #16  /  stp xt, xt2, [xn], #16 (post-index)
    pub fn ldp_post16_r(&mut self, rt: u32, rt2: u32, rn: u32) {
        self.emit(0xA8C1_0000 | rt2 << 10 | rn << 5 | rt);
    }
    pub fn stp_post16_r(&mut self, rt: u32, rt2: u32, rn: u32) {
        self.emit(0xA881_0000 | rt2 << 10 | rn << 5 | rt);
    }
    /// `size` = smallest power of two >= n (at least 16), `cls` = its log2. Clobbers size, cls, x16 and flags.
    pub fn size_class(&mut self, n: u32, size: u32, cls: u32) {
        let done = self.label();
        self.mov_imm(cls, 4);
        self.mov_imm(size, 16);
        self.mov_imm(X16, 16);
        self.cmp(n, X16);
        self.bcond(LS, done);
        self.sub_imm(size, n, 1);
        self.clz(cls, size);
        self.mov_imm(X16, 64);
        self.sub_reg(cls, X16, cls);
        self.mov_imm(X16, 1);
        self.lslv(size, X16, cls);
        self.bind(done);
    }
    pub fn lsl_imm(&mut self, rd: u32, rn: u32, s: u32) {
        self.emit(0xD340_0000 | ((64 - s) & 63) << 16 | (63 - s) << 10 | rn << 5 | rd);
    }
    pub fn and_reg(&mut self, rd: u32, rn: u32, rm: u32) {
        self.emit(0x8A00_0000 | rm << 16 | rn << 5 | rd);
    }
    pub fn orr_reg(&mut self, rd: u32, rn: u32, rm: u32) {
        self.emit(0xAA00_0000 | rm << 16 | rn << 5 | rd);
    }
    /// tst xn, xm  (ands xzr, xn, xm)
    pub fn tst(&mut self, rn: u32, rm: u32) {
        self.emit(0xEA00_0000 | rm << 16 | rn << 5 | XZR);
    }
    /// stp xt, xt2, [sp, #-16]!  (pre-index, used for the frame record)
    pub fn stp_pre16(&mut self, rt: u32, rt2: u32) {
        self.emit(0xA9BF_0000 | rt2 << 10 | SP << 5 | rt);
    }
    /// ldp xt, xt2, [sp], #16  (post-index)
    pub fn ldp_post16(&mut self, rt: u32, rt2: u32) {
        self.emit(0xA8C1_0000 | rt2 << 10 | SP << 5 | rt);
    }

    // --- floating point (d0..d31 are only used as scratch; values live in GPR slots as bit patterns)
    pub fn fmov_dx(&mut self, d: u32, x: u32) {
        self.emit(0x9E67_0000 | x << 5 | d);
    }
    pub fn fmov_xd(&mut self, x: u32, d: u32) {
        self.emit(0x9E66_0000 | d << 5 | x);
    }
    pub fn fmov_sw(&mut self, s: u32, w: u32) {
        self.emit(0x1E27_0000 | w << 5 | s);
    }
    pub fn fmov_ws(&mut self, w: u32, s: u32) {
        self.emit(0x1E26_0000 | s << 5 | w);
    }
    /// fadd/fsub/fmul/fdiv dd, dn, dm  (`k` = 0x2800 / 0x3800 / 0x0800 / 0x1800)
    pub fn farith(&mut self, k: u32, dd: u32, dn: u32, dm: u32) {
        self.emit(0x1E60_0000 | k | dm << 16 | dn << 5 | dd);
    }
    pub fn fsqrt(&mut self, dd: u32, dn: u32) {
        self.emit(0x1E61_C000 | dn << 5 | dd);
    }
    pub fn fcmp(&mut self, dn: u32, dm: u32) {
        self.emit(0x1E60_2000 | dm << 16 | dn << 5);
    }
    /// scvtf dd, xn
    pub fn scvtf_d(&mut self, dd: u32, xn: u32) {
        self.emit(0x9E62_0000 | xn << 5 | dd);
    }
    /// scvtf sd, xn
    pub fn scvtf_s(&mut self, sd: u32, xn: u32) {
        self.emit(0x9E22_0000 | xn << 5 | sd);
    }
    /// fcvtzs xd, dn  (saturating; NaN -> 0)
    pub fn fcvtzs(&mut self, xd: u32, dn: u32) {
        self.emit(0x9E78_0000 | dn << 5 | xd);
    }
    /// fcvt dd, sn
    pub fn fcvt_ds(&mut self, dd: u32, sn: u32) {
        self.emit(0x1E22_C000 | sn << 5 | dd);
    }
    /// fcvt sd, dn  (round to nearest even)
    pub fn fcvt_sd(&mut self, sd: u32, dn: u32) {
        self.emit(0x1E62_4000 | dn << 5 | sd);
    }
    pub fn umulh(&mut self, rd: u32, rn: u32, rm: u32) {
        self.emit(0x9BC0_7C00 | rm << 16 | rn << 5 | rd);
    }
    pub fn eor_reg(&mut self, rd: u32, rn: u32, rm: u32) {
        self.emit(0xCA00_0000 | rm << 16 | rn << 5 | rd);
    }
    pub fn lsr_imm(&mut self, rd: u32, rn: u32, s: u32) {
        self.emit(0xD340_FC00 | s << 16 | rn << 5 | rd);
    }
    pub fn asr_imm(&mut self, rd: u32, rn: u32, s: u32) {
        self.emit(0x9340_FC00 | s << 16 | rn << 5 | rd);
    }

    // --- control flow
    pub fn b(&mut self, l: usize) {
        self.fix.push((self.code.len(), Fix::B26, l));
        self.emit(0x1400_0000);
    }
    pub fn bl(&mut self, l: usize) {
        self.fix.push((self.code.len(), Fix::B26, l));
        self.emit(0x9400_0000);
    }
    pub fn bcond(&mut self, cond: u32, l: usize) {
        self.fix.push((self.code.len(), Fix::B19, l));
        self.emit(0x5400_0000 | cond);
    }
    pub fn cbz(&mut self, rt: u32, l: usize) {
        self.fix.push((self.code.len(), Fix::B19, l));
        self.emit(0xB400_0000 | rt);
    }
    pub fn cbnz(&mut self, rt: u32, l: usize) {
        self.fix.push((self.code.len(), Fix::B19, l));
        self.emit(0xB500_0000 | rt);
    }
    pub fn ret(&mut self) {
        self.emit(0xD65F_03C0);
    }
    pub fn svc(&mut self) {
        self.emit(0xD400_0001);
    }
    pub fn udf(&mut self) {
        self.emit(0x0000_0000);
    }
    /// adrp rd, data; add rd, rd, :lo12:data  (position-independent, any distance)
    /// rd = address of code label `l` (adrp + add, resolved in `finalize`)
    pub fn lea_label(&mut self, rd: u32, l: usize) {
        self.lfix.push((self.code.len(), l));
        self.emit(0x9000_0000 | rd);
        self.emit(0x9100_0000 | rd << 5 | rd);
    }
    /// blr xn
    pub fn blr(&mut self, rn: u32) {
        self.emit(0xD63F_0000 | rn << 5);
    }
    pub fn lea_data(&mut self, rd: u32, id: usize) {
        self.dfix.push((self.code.len(), id));
        self.emit(0x9000_0000 | rd);
        self.emit(0x9100_0000 | rd << 5 | rd);
    }
    fn sys_write(&mut self, fd: i64) {
        self.mov_imm(X0, fd);
        self.mov_imm(X8, 64);
        self.svc();
    }
    fn sys_exit(&mut self, code: i64) {
        self.mov_imm(X0, code);
        self.mov_imm(X8, 94);
        self.svc();
    }

    /// `base` = virtual address of code[0]; `data_start` = offset of the data blobs in code.
    fn finalize(&mut self, base: u64, data_start: usize, data_offs: &[usize]) {
        for (p, kind, l) in self.fix.clone() {
            let target = self.labels[l].expect("unbound label") as i64;
            let words = (target - p as i64) / 4;
            let mut w = u32::from_le_bytes(self.code[p..p + 4].try_into().unwrap());
            match kind {
                Fix::B26 => {
                    assert!(
                        (-(1 << 25)..(1 << 25)).contains(&words),
                        "branch out of range"
                    );
                    w |= (words as u32) & 0x03FF_FFFF;
                }
                Fix::B19 => {
                    assert!(
                        (-(1 << 18)..(1 << 18)).contains(&words),
                        "conditional branch out of range"
                    );
                    w |= ((words as u32) & 0x7FFFF) << 5;
                }
            }
            self.code[p..p + 4].copy_from_slice(&w.to_le_bytes());
        }
        let mut addr_fix: Vec<(usize, u64)> = self
            .dfix
            .iter()
            .map(|&(p, id)| (p, base + (data_start + data_offs[id]) as u64))
            .collect();
        for &(p, l) in &self.lfix {
            addr_fix.push((p, base + self.labels[l].expect("unbound label") as u64));
        }
        for (p, target) in addr_fix {
            let pc = base + p as u64;
            let pages = ((target >> 12) as i64) - ((pc >> 12) as i64);
            assert!(
                (-(1 << 20)..(1 << 20)).contains(&pages),
                "adrp out of range"
            );
            let imm = (pages as u32) & 0x1F_FFFF;
            let mut w = u32::from_le_bytes(self.code[p..p + 4].try_into().unwrap());
            w |= (imm & 3) << 29 | (imm >> 2) << 5;
            self.code[p..p + 4].copy_from_slice(&w.to_le_bytes());
            let lo = (target & 0xFFF) as u32;
            let mut w2 = u32::from_le_bytes(self.code[p + 4..p + 8].try_into().unwrap());
            w2 |= lo << 10;
            self.code[p + 4..p + 8].copy_from_slice(&w2.to_le_bytes());
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
    "second panic during worker cleanup",
    "extern C calls from AArch64 Yerux workers require libc TLS",
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
const P_DOUBLE_PANIC: usize = 15;
const P_WORKER_FFI: usize = 16;
const CHK_PANIC: [usize; 7] = [P_ADD, P_SUB, P_MUL, P_DIV, P_NEG, P_CONV, P_F2I];
const FREED: i64 = 1 << 62; // debug builds tag the header of a block sitting on a free list

// Runtime heap state: a second, writable PT_LOAD segment (zero-filled, no bytes in the file).
pub(crate) const GOT_REL: usize = 4096;
pub(crate) const G: i64 = 0x600000;
const G_BUMP: i32 = 0; // next free byte of the current arena
const G_END: i32 = 8; // end of the current arena
const G_NALLOC: i32 = 16; // debug counters
const G_NFREE: i32 = 24;
const G_USER: i64 = 256; // cells addressed by __global
const G_FREE: i32 = 32; // free-list heads, one per size class 2^k (k = 4..=16)
const ARENA: i64 = 1 << 20;
const SMALL_MAX: i64 = 65536;

/// Byte offset of vreg `v` from sp (slots live at the bottom of the frame).
fn slot(v: V) -> u32 {
    8 * v as u32
}

// ───────────────────────── Compiler ─────────────────────────

/// The source intrinsic uses x86-64 syscall numbers and argument layouts.
enum LinuxSyscall {
    Direct(i64),
    At(i64),
    UnlinkAt(i64),
    TwoPathsAt(i64),
    SymlinkAt,
    Poll,
    Dup2,
    Fork,
    OpenAt { has_dirfd: bool },
}

impl LinuxSyscall {
    fn from_x64(number: i64) -> Option<Self> {
        use LinuxSyscall::*;
        Some(match number {
            0 => Direct(63), // read
            1 => Direct(64), // write
            2 => OpenAt { has_dirfd: false },
            3 => Direct(57),      // close
            7 => Poll,
            8 => Direct(62),      // lseek
            9 => Direct(222),     // mmap
            10 => Direct(226),    // mprotect
            11 => Direct(215),    // munmap
            13 => Direct(134), // rt_sigaction
            21 => At(48),         // access -> faccessat
            25 => Direct(216),    // mremap
            33 => Dup2,
            35 => Direct(101),    // nanosleep
            39 => Direct(172),    // getpid
            41 => Direct(198),    // socket
            42 => Direct(203),    // connect
            43 => Direct(202),    // accept
            44 => Direct(206),    // sendto
            49 => Direct(200),    // bind
            50 => Direct(201),    // listen
            51 => Direct(204),    // getsockname
            52 => Direct(205),    // getpeername
            54 => Direct(208),    // setsockopt
            57 => Fork,
            59 => Direct(221),    // execve
            60 => Direct(93),     // exit
            61 => Direct(260),    // wait4
            62 => Direct(129),    // kill
            72 => Direct(25),     // fcntl
            74 => Direct(82),     // fsync
            77 => Direct(46),     // ftruncate
            79 => Direct(17),     // getcwd
            80 => Direct(49),     // chdir
            82 => TwoPathsAt(38), // rename -> renameat
            83 => At(34),         // mkdir -> mkdirat
            84 => UnlinkAt(512),  // rmdir -> unlinkat with AT_REMOVEDIR
            86 => TwoPathsAt(37), // link -> linkat
            87 => UnlinkAt(0),    // unlink -> unlinkat
            88 => SymlinkAt,
            89 => At(78),       // readlink -> readlinkat
            90 => At(53),       // chmod -> fchmodat
            107 => Direct(175), // geteuid
            217 => Direct(61),  // getdents64
            228 => Direct(113), // clock_gettime
            231 => Direct(94),  // exit_group
            257 => OpenAt { has_dirfd: true },
            263 => Direct(35),  // unlinkat
            271 => Direct(73),  // ppoll
            293 => Direct(59), // pipe2
            332 => Direct(291), // statx
            _ => return None,
        })
    }

    fn emit(self, a: &mut Asm) {
        const AT_FDCWD: i64 = -100;
        let number = match self {
            Self::Direct(number) => number,
            Self::Poll => {
                // ppoll takes a timespec instead of signed 32-bit milliseconds.
                let call = a.label();
                a.sub_imm(SP, SP, 16);
                a.lsl_imm(X2, X2, 32);
                a.asr_imm(X2, X2, 32);
                a.mov(X3, X2);
                a.mov_imm(X2, 0);
                a.cmp(X3, XZR);
                a.bcond(LT, call);
                a.mov_imm(X4, 1000);
                a.udiv(X5, X3, X4);
                a.msub(X3, X5, X4, X3);
                a.mov_imm(X4, 1_000_000);
                a.mul(X3, X3, X4);
                a.str_i(X5, SP, 0);
                a.str_i(X3, SP, 8);
                a.mov_sp(X2, SP);
                a.bind(call);
                a.mov_imm(X3, 0);
                a.mov_imm(X4, 0);
                a.mov_imm(X8, 73);
                a.svc();
                a.add_imm(SP, SP, 16);
                return;
            }
            Self::Dup2 => {
                // dup3 rejects equal descriptors; dup2 checks that fd is open.
                let distinct = a.label();
                let done = a.label();
                a.cmp(X0, X1);
                a.bcond(NE, distinct);
                a.mov(X6, X0);
                a.mov_imm(X1, 1); // F_GETFD
                a.mov_imm(X8, 25);
                a.svc();
                a.cmp(X0, XZR);
                a.bcond(LT, done);
                a.mov(X0, X6);
                a.b(done);
                a.bind(distinct);
                a.mov_imm(X2, 0);
                a.mov_imm(X8, 24);
                a.svc();
                a.bind(done);
                return;
            }
            Self::Fork => {
                a.mov_imm(X0, 17); // clone(SIGCHLD, NULL, NULL, NULL, NULL)
                for reg in [X1, X2, X3, X4] {
                    a.mov_imm(reg, 0);
                }
                220
            }
            Self::At(number) => {
                a.mov(X3, X2);
                a.mov(X2, X1);
                a.mov(X1, X0);
                a.mov_imm(X0, AT_FDCWD);
                number
            }
            Self::UnlinkAt(flags) => {
                a.mov(X1, X0);
                a.mov_imm(X0, AT_FDCWD);
                a.mov_imm(X2, flags);
                35
            }
            Self::TwoPathsAt(number) => {
                a.mov(X3, X1);
                a.mov(X1, X0);
                a.mov_imm(X0, AT_FDCWD);
                a.mov(X2, X0);
                a.mov_imm(X4, 0);
                number
            }
            Self::SymlinkAt => {
                a.mov(X2, X1);
                a.mov_imm(X1, AT_FDCWD);
                36
            }
            Self::OpenAt { has_dirfd } => {
                if !has_dirfd {
                    a.mov(X3, X2);
                    a.mov(X2, X1);
                    a.mov(X1, X0);
                    a.mov_imm(X0, AT_FDCWD);
                }
                Self::open_flags(a);
                56
            }
        };
        // Linux AArch64 kernel ABI: x8 = number, x0..x5 = arguments.
        a.mov_imm(X8, number);
        a.svc();
    }

    fn open_flags(a: &mut Asm) {
        // asm-generic/fcntl.h (x86-64) and arm64's asm/fcntl.h swap these pairs.
        const DIRECT_LARGEFILE: i64 = 0o140000;
        const DIRECTORY_NOFOLLOW: i64 = 0o600000;
        a.mov_imm(X6, DIRECT_LARGEFILE);
        a.and_reg(X6, X2, X6);
        a.lsl_imm(X6, X6, 2);
        a.mov_imm(X7, DIRECTORY_NOFOLLOW);
        a.and_reg(X7, X2, X7);
        a.lsr_imm(X7, X7, 2);
        a.orr_reg(X6, X6, X7);
        a.mov_imm(X7, !(DIRECT_LARGEFILE | DIRECTORY_NOFOLLOW));
        a.and_reg(X2, X2, X7);
        a.orr_reg(X2, X2, X6);
    }
}

pub fn validate_syscalls(ir: &IrProgram) -> Result<(), String> {
    for f in &ir.funcs {
        let constants = crate::ir::syscall_constants(f);
        for inst in f.blocks.iter().flat_map(|b| &b.insts) {
            if let Inst::Syscall(_, args) = inst {
                let number = constants[args[0]];
                if number.and_then(LinuxSyscall::from_x64).is_none() {
                    let number =
                        number.map_or_else(|| "dynamic number".to_string(), |n| n.to_string());
                    return Err(format!(
                        "raw system calls ({} in function '{}')",
                        number, f.name
                    ));
                }
            }
        }
    }
    Ok(())
}

pub fn compile(ir: &IrProgram, debug_alloc: bool) -> Vec<u8> {
    let mut a = Asm::new();
    let has_threads = ir.funcs.iter().flat_map(|f| &f.blocks)
        .flat_map(|b| &b.insts).any(|i| matches!(i, Inst::Thread(..)));
    let thread_runtime = has_threads.then(|| threads::Runtime::new(&mut a));

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
        let mut b = (t.len() as u64).to_le_bytes().to_vec();
        b.extend_from_slice(t);
        b.push(0); // Text passed to C points here as a NUL-terminated string.
        data.push(b);
    }

    let l_start = a.label();
    let l_print_int = a.label();
    let l_print_bool = a.label();
    let l_print_text = a.label();
    let l_write2 = a.label();
    let l_print_bytes = a.label();
    let l_panic_bytes = a.label();
    let l_range_panic = a.label();
    let l_write_int2 = a.label();
    let l_at = a.label();
    // `  at file:line:col` after a panic message (see `l_at`): only when the program has user source positions
    let report = ir.locs.len() > 1;
    let dbg0 = D_TEXT0 + ir.texts.len() + ir.vtables.len();
    let (d_at, d_colon, d_locs, d_files) = (dbg0, dbg0 + 1, dbg0 + 2, dbg0 + 3);
    let (l_mmap, l_alloc, l_free, l_push, l_get) =
        (a.label(), a.label(), a.label(), a.label(), a.label());
    let (l_realloc, l_memcpy) = (a.label(), a.label());
    let l_panic: Vec<usize> = (0..PANIC_MSGS.len()).map(|_| a.label()).collect();
    let l_funcs: Vec<usize> = ir.funcs.iter().map(|_| a.label()).collect();

    // _start: expose the initial Linux stack for std.io and std.env.
    a.bind(l_start);
    a.mov_imm(X0, G + G_USER);
    a.mov_sp(X1, SP);
    a.str_i(X1, X0, 0);
    a.mov_imm(X17, 0);
    if has_threads {
        a.mov_imm(X19, threads::MAIN_CONTEXT);
    }
    a.bl(l_funcs[ir.main]);
    if debug_alloc {
        // report allocations, frees and the difference on stderr
        for (i, name_len) in [8i64, 7, 6].iter().enumerate() {
            a.lea_data(X1, D_DBG0 + i);
            a.mov_imm(X2, *name_len);
            a.bl(l_write2);
            a.mov_imm(X4, G);
            if i == 0 {
                a.ldr_i(X0, X4, G_NALLOC);
            } else if i == 1 {
                a.ldr_i(X0, X4, G_NFREE);
            } else {
                a.ldr_i(X0, X4, G_NALLOC);
                a.ldr_i(X1, X4, G_NFREE);
                a.sub_reg(X0, X0, X1);
            }
            a.mov_imm(X6, 2);
            a.bl(l_print_int);
        }
    }
    a.sys_exit(0);

    // write2: x1 = buf, x2 = len -> write(2, buf, len)
    a.bind(l_write2);
    a.sys_write(2);
    a.ret();

    // panic stubs: "panic: <msg>\n" to stderr, exit(101)
    for (i, l) in l_panic.iter().enumerate() {
        a.bind(*l);
        if let Some(rt) = &thread_runtime {
            if !matches!(i, P_OOM | P_DOUBLE_FREE | P_THREAD_START | P_DOUBLE_PANIC) {
                rt.panic_probe(&mut a, l_panic[P_DOUBLE_PANIC]);
            }
        }
        a.lea_data(X1, D_PANIC);
        a.mov_imm(X2, 7);
        a.bl(l_write2);
        a.lea_data(X1, D_MSG0 + i);
        a.mov_imm(X2, PANIC_MSGS[i].len() as i64);
        a.bl(l_write2);
        a.lea_data(X1, D_NL);
        a.mov_imm(X2, 1);
        a.bl(l_write2);
        if report && i != P_DOUBLE_PANIC {
            a.bl(l_at);
        }
        a.sys_exit(101);
    }

    // write_int2: value in x0. Writes its decimal digits (no newline) to stderr. Leaf routine; keeps x9-x11.
    a.bind(l_write_int2);
    a.sub_imm(SP, SP, 32);
    a.mov(X3, X0);
    a.cmp(X0, XZR);
    let l_wpos = a.label();
    a.bcond(PL, l_wpos);
    a.sub_reg(X0, XZR, X0);
    a.bind(l_wpos);
    a.add_imm(X1, SP, 32);
    a.mov_imm(X2, 10);
    let l_wdig = a.label();
    a.bind(l_wdig);
    a.udiv(X4, X0, X2);
    a.msub(X5, X4, X2, X0);
    a.add_imm(X5, X5, b'0' as u32);
    a.sub_imm(X1, X1, 1);
    a.strb(X5, X1, 0);
    a.mov(X0, X4);
    a.cbnz(X0, l_wdig);
    a.cmp(X3, XZR);
    let l_wnosign = a.label();
    a.bcond(PL, l_wnosign);
    a.sub_imm(X1, X1, 1);
    a.mov_imm(X4, b'-' as i64);
    a.strb(X4, X1, 0);
    a.bind(l_wnosign);
    a.add_imm(X2, SP, 32);
    a.sub_reg(X2, X2, X1);
    a.mov_imm(X0, 2);
    a.mov_imm(X8, 64);
    a.svc();
    a.add_imm(SP, SP, 32);
    a.ret();

    // range_panic: x9 = value, x10 = lo, x11 = hi. "panic: integer conversion out of range: V not in LO..HI", exit(101).
    a.bind(l_range_panic);
    if let Some(rt) = &thread_runtime {
        rt.panic_probe(&mut a, l_panic[P_DOUBLE_PANIC]);
    }
    a.lea_data(X1, D_PANIC);
    a.mov_imm(X2, 7);
    a.bl(l_write2);
    a.lea_data(X1, D_MSG0 + P_CONV);
    a.mov_imm(X2, PANIC_MSGS[P_CONV].len() as i64);
    a.bl(l_write2);
    for (k, (len, src)) in [(2i64, X9), (8, X10), (2, X11)].into_iter().enumerate() {
        a.lea_data(X1, D_RNG0 + k);
        a.mov_imm(X2, len);
        a.bl(l_write2);
        a.mov(X0, src);
        a.bl(l_write_int2);
    }
    a.lea_data(X1, D_NL);
    a.mov_imm(X2, 1);
    a.bl(l_write2);
    if report {
        a.bl(l_at);
    }
    a.sys_exit(101);

    // at: x17 = location id (0: none). Writes "  at file:line:col\n" to stderr. Keeps x9-x11 (write_int2) and uses x12-x15.
    if report {
        let l_none = a.label();
        a.bind(l_at);
        a.cbz(X17, l_none);
        a.mov(X15, X30);
        a.lea_data(X10, d_locs);
        a.mov_imm(X11, 24);
        a.mul(X9, X17, X11);
        a.add_reg(X10, X10, X9); // x10 = &locs[id] = {file, line, col}
        a.lea_data(X1, d_at);
        a.mov_imm(X2, 5);
        a.bl(l_write2);
        a.ldr_i(X12, X10, 0);
        a.lsl_imm(X12, X12, 4);
        a.lea_data(X13, d_files);
        a.add_reg(X12, X13, X12); // x12 = &files[file] = {offset from the table, length}
        a.ldr_i(X1, X12, 0);
        a.add_reg(X1, X13, X1);
        a.ldr_i(X2, X12, 8);
        a.bl(l_write2);
        for k in [8i32, 16] {
            a.lea_data(X1, d_colon);
            a.mov_imm(X2, 1);
            a.bl(l_write2);
            a.ldr_i(X0, X10, k);
            a.bl(l_write_int2);
        }
        a.lea_data(X1, D_NL);
        a.mov_imm(X2, 1);
        a.bl(l_write2);
        a.mov(X30, X15);
        a.bind(l_none);
        a.ret();
    }

    // print_int: value in x0, fd in x6. Prints decimal + newline to stdout. Leaf routine.
    a.bind(l_print_int);
    if has_threads {
        threads::lock(&mut a, threads::PRINT_LOCK);
    }
    a.sub_imm(SP, SP, 32);
    a.mov(X3, X0); // remember the sign
    a.cmp(X0, XZR);
    let l_pos = a.label();
    a.bcond(PL, l_pos);
    a.sub_reg(X0, XZR, X0); // neg (INT64_MIN stays 2^63, fine as unsigned)
    a.bind(l_pos);
    a.add_imm(X1, SP, 31);
    a.mov_imm(X4, 10);
    a.strb(X4, X1, 0); // '\n'
    a.mov_imm(X2, 10);
    let l_dig = a.label();
    a.bind(l_dig);
    a.udiv(X4, X0, X2);
    a.msub(X5, X4, X2, X0); // x5 = x0 % 10
    a.add_imm(X5, X5, b'0' as u32);
    a.sub_imm(X1, X1, 1);
    a.strb(X5, X1, 0);
    a.mov(X0, X4);
    a.cbnz(X0, l_dig);
    a.cmp(X3, XZR);
    let l_nosign = a.label();
    a.bcond(PL, l_nosign);
    a.sub_imm(X1, X1, 1);
    a.mov_imm(X4, b'-' as i64);
    a.strb(X4, X1, 0);
    a.bind(l_nosign);
    a.add_imm(X2, SP, 32);
    a.sub_reg(X2, X2, X1); // length
    a.mov(X0, X6); // fd (x6) is chosen by the caller
    a.mov_imm(X8, 64);
    a.svc();
    a.add_imm(SP, SP, 32);
    if has_threads {
        threads::unlock(&mut a, threads::PRINT_LOCK);
    }
    a.ret();

    // print_bool: value in x0
    a.bind(l_print_bool);
    if has_threads {
        threads::lock(&mut a, threads::PRINT_LOCK);
    }
    let (l_false, l_wr) = (a.label(), a.label());
    a.cbz(X0, l_false);
    a.lea_data(X1, D_TRUE);
    a.mov_imm(X2, 5);
    a.b(l_wr);
    a.bind(l_false);
    a.lea_data(X1, D_FALSE);
    a.mov_imm(X2, 6);
    a.bind(l_wr);
    a.sys_write(1);
    if has_threads {
        threads::unlock(&mut a, threads::PRINT_LOCK);
    }
    a.ret();

    // print_text: x0 = pointer to [len][bytes]. Needs lr preserved (calls nothing, uses svc only).
    a.bind(l_print_text);
    if has_threads {
        threads::lock(&mut a, threads::PRINT_LOCK);
    }
    a.ldr(X2, X0, 0);
    a.add_imm(X1, X0, 8);
    a.sys_write(1);
    a.lea_data(X1, D_NL);
    a.mov_imm(X2, 1);
    a.sys_write(1);
    if has_threads {
        threads::unlock(&mut a, threads::PRINT_LOCK);
    }
    a.ret();

    // print_bytes: x1 = pointer, x2 = length. Writes the bytes (if any) and a newline.
    {
        let l_nl = a.label();
        a.bind(l_print_bytes);
        if has_threads {
            threads::lock(&mut a, threads::PRINT_LOCK);
        }
        a.cbz(X2, l_nl);
        a.sys_write(1);
        a.bind(l_nl);
        a.lea_data(X1, D_NL);
        a.mov_imm(X2, 1);
        a.sys_write(1);
        if has_threads {
            threads::unlock(&mut a, threads::PRINT_LOCK);
        }
        a.ret();
    }

    // panic_bytes: x1 = pointer, x2 = length. "panic: " + the bytes + newline on stderr, exit(101).
    {
        let l_nl = a.label();
        a.bind(l_panic_bytes);
        if let Some(rt) = &thread_runtime {
            rt.panic_probe(&mut a, l_panic[P_DOUBLE_PANIC]);
        }
        a.mov(X9, X1);
        a.mov(X10, X2);
        a.lea_data(X1, D_PANIC);
        a.mov_imm(X2, 7);
        a.bl(l_write2);
        a.mov(X1, X9);
        a.mov(X2, X10);
        a.cbz(X2, l_nl);
        a.bl(l_write2);
        a.bind(l_nl);
        a.lea_data(X1, D_NL);
        a.mov_imm(X2, 1);
        a.bl(l_write2);
        if report {
            a.bl(l_at);
        }
        a.sys_exit(101);
    }

    // ── heap runtime ──
    // rt_mmap: x1 = length -> x0 = fresh zeroed pages or a negative errno. Leaf; clobbers x0-x5, x8.
    a.bind(l_mmap);
    a.mov_imm(X0, 0);
    a.mov_imm(X2, 3); // PROT_READ | PROT_WRITE
    a.mov_imm(X3, 0x22); // MAP_PRIVATE | MAP_ANONYMOUS
    a.mov_imm(X4, -1);
    a.mov_imm(X5, 0);
    a.mov_imm(X8, 222);
    a.svc();
    a.ret();
    if let Some(rt) = &thread_runtime {
        rt.emit(&mut a, l_mmap, l_panic[P_THREAD_START]);
    }

    // rt_alloc: x0 = bytes -> x0 = payload pointer. Block = [size header][payload].
    // Small blocks (<= 64 KiB) are power-of-two size classes served from per-class free lists,
    // then bumped out of 1 MiB arenas. Larger blocks get their own mmap (page rounded).
    // Clobbers x0-x13 and x16. lr is kept in x7 (rt_mmap is a leaf that does not touch it).
    {
        let (l_large, l_found, l_bump, l_newarena, l_done) =
            (a.label(), a.label(), a.label(), a.label(), a.label());
        let l_oom = a.label();
        a.bind(l_alloc);
        if has_threads {
            threads::lock(&mut a, threads::HEAP_LOCK);
        }
        a.mov(X7, X30);
        a.add_imm(X0, X0, 8);
        a.mov_imm(X16, SMALL_MAX);
        a.cmp(X0, X16);
        a.bcond(HI, l_large);
        a.size_class(X0, X1, X2); // x1 = block size (smallest 2^k >= x0), x2 = k
        a.bind(l_found);
        a.lsl_imm(X4, X2, 3);
        a.mov_imm(X3, G + G_FREE as i64);
        a.add_reg(X3, X3, X4); // x3 = &free_list[class]
        a.ldr_i(X0, X3, 0);
        a.cbz(X0, l_bump);
        a.ldr_i(X4, X0, 0); // pop: head = head.next
        a.str_i(X4, X3, 0);
        if debug_alloc {
            a.ldr_i(X4, X0, -8); // clear the FREED tag
            a.mov_imm(X5, !FREED);
            a.and_reg(X4, X4, X5);
            a.str_i(X4, X0, -8);
        }
        a.b(l_done);
        a.bind(l_bump);
        a.mov_imm(X6, G);
        a.ldr_i(X0, X6, G_BUMP);
        a.add_reg(X4, X0, X1); // x4 = bump + size
        a.ldr_i(X5, X6, G_END);
        a.cmp(X4, X5);
        a.bcond(HI, l_newarena);
        a.str_i(X4, X6, G_BUMP);
        a.str_i(X1, X0, 0); // header = block size
        a.add_imm(X0, X0, 8);
        a.b(l_done);
        a.bind(l_newarena);
        a.mov(X9, X1);
        a.mov_imm(X1, ARENA);
        a.bl(l_mmap);
        a.cmp(X0, XZR);
        a.bcond(LT, l_oom);
        a.mov(X1, X9);
        a.mov_imm(X6, G);
        a.str_i(X0, X6, G_BUMP);
        a.mov_imm(X4, ARENA);
        a.add_reg(X4, X0, X4);
        a.str_i(X4, X6, G_END);
        a.b(l_bump);
        a.bind(l_large);
        a.add_imm(X0, X0, 4095);
        a.mov_imm(X16, -4096);
        a.and_reg(X0, X0, X16);
        a.mov(X1, X0);
        a.bl(l_mmap);
        a.cmp(X0, XZR);
        a.bcond(LT, l_oom);
        a.str_i(X1, X0, 0); // header = mapping size
        a.add_imm(X0, X0, 8);
        a.bind(l_done);
        if debug_alloc {
            a.mov_imm(X6, G);
            a.ldr_i(X4, X6, G_NALLOC);
            a.add_imm(X4, X4, 1);
            a.str_i(X4, X6, G_NALLOC);
        }
        if has_threads {
            threads::unlock(&mut a, threads::HEAP_LOCK);
        }
        a.mov(X30, X7);
        a.ret();
        a.bind(l_oom);
        if has_threads {
            threads::unlock(&mut a, threads::HEAP_LOCK);
        }
        a.b(l_panic[P_OOM]);
    }

    // rt_free: x0 = payload pointer (0 is ignored). Leaf; clobbers x0-x6, x8, x10-x13 and x16.
    {
        let (l_ret, l_large, l_found) = (a.label(), a.label(), a.label());
        a.bind(l_free);
        a.cbz(X0, l_ret);
        if has_threads {
            threads::lock(&mut a, threads::HEAP_LOCK);
        }
        if debug_alloc {
            a.mov_imm(X6, G);
            a.ldr_i(X4, X6, G_NFREE);
            a.add_imm(X4, X4, 1);
            a.str_i(X4, X6, G_NFREE);
        }
        a.ldr_i(X1, X0, -8); // block size
        if debug_alloc {
            a.mov_imm(X4, FREED);
            a.tst(X4, X1);
            let live = a.label();
            a.bcond(EQ, live);
            if has_threads {
                threads::unlock(&mut a, threads::HEAP_LOCK);
            }
            a.b(l_panic[P_DOUBLE_FREE]);
            a.bind(live);
        }
        a.mov_imm(X16, SMALL_MAX);
        a.cmp(X1, X16);
        a.bcond(HI, l_large);
        a.clz(X2, X1); // class = 63 - clz(size): the header holds the class size 2^k
        a.mov_imm(X3, 63);
        a.sub_reg(X2, X3, X2);
        a.bind(l_found);
        a.lsl_imm(X4, X2, 3);
        a.mov_imm(X5, G + G_FREE as i64);
        a.add_reg(X4, X4, X5); // x4 = &free_list[class]
        if debug_alloc {
            a.mov_imm(X5, FREED);
            a.orr_reg(X5, X5, X1);
            a.str_i(X5, X0, -8);
        }
        a.ldr_i(X5, X4, 0);
        a.str_i(X5, X0, 0); // block.next = head
        a.str_i(X0, X4, 0); // head = block
        if has_threads {
            threads::unlock(&mut a, threads::HEAP_LOCK);
        }
        a.ret();
        a.bind(l_large);
        if has_threads {
            threads::unlock(&mut a, threads::HEAP_LOCK);
        }
        a.sub_imm(X0, X0, 8);
        a.mov_imm(X8, 215); // munmap(x0 = block, x1 = size)
        a.svc();
        a.bind(l_ret);
        a.ret();
    }

    // rt_memcpy: x0 = dst, x1 = src, x2 = n (no overlap). Leaf; clobbers x0-x5, x16.
    {
        let (l_lt8, l_lt4, l_byte, l_big, l_words, l_ret) = (
            a.label(),
            a.label(),
            a.label(),
            a.label(),
            a.label(),
            a.label(),
        );
        a.bind(l_memcpy);
        a.mov_imm(X16, 16);
        a.cmp(X2, X16);
        a.bcond(HI, l_big);
        a.mov_imm(X16, 8);
        a.cmp(X2, X16);
        a.bcond(LO, l_lt8);
        a.ldr_i(X3, X1, 0); // 8..=16: first and last word
        a.add_reg(X1, X1, X2);
        a.ldr_i(X4, X1, -8);
        a.str_i(X3, X0, 0);
        a.add_reg(X0, X0, X2);
        a.str_i(X4, X0, -8);
        a.ret();
        a.bind(l_lt8);
        a.mov_imm(X16, 4);
        a.cmp(X2, X16);
        a.bcond(LO, l_lt4);
        a.ldr32_i(X3, X1, 0); // 4..=7: first and last dword
        a.add_reg(X1, X1, X2);
        a.ldr32_i(X4, X1, -4);
        a.str32_i(X3, X0, 0);
        a.add_reg(X0, X0, X2);
        a.str32_i(X4, X0, -4);
        a.ret();
        a.bind(l_lt4);
        a.cbz(X2, l_ret);
        a.bind(l_byte);
        a.ldrb_post1(X3, X1);
        a.strb_post1(X3, X0);
        a.sub_imm(X2, X2, 1);
        a.cbnz(X2, l_byte);
        a.bind(l_ret);
        a.ret();
        a.bind(l_big); // n > 16: 16-byte pairs while more than 16 remain, then the last 16 bytes of the range
        a.bind(l_words);
        a.ldp_post16_r(X3, X4, X1);
        a.stp_post16_r(X3, X4, X0);
        a.sub_imm(X2, X2, 16);
        a.mov_imm(X16, 16);
        a.cmp(X2, X16);
        a.bcond(HI, l_words);
        a.sub_reg(X5, X16, X2); // 16 - remaining
        a.sub_reg(X1, X1, X5);
        a.sub_reg(X0, X0, X5);
        a.ldp0(X3, X4, X1);
        a.stp0(X3, X4, X0);
        a.ret();
    }

    // rt_realloc: x0 = old payload (0 = none), x1 = new payload bytes, x2 = bytes to keep -> x0 = payload.
    // Same policy as x64: fits -> unchanged; large block -> mremap (syscall 216, MAYMOVE); otherwise
    // alloc + copy + free. Clobbers x0-x10, x16.
    {
        let (l_same, l_alloc0, l_move, l_large) = (a.label(), a.label(), a.label(), a.label());
        a.bind(l_realloc);
        a.cbz(X0, l_alloc0);
        a.ldr_i(X3, X0, -8); // x3 = block size S
        if debug_alloc {
            a.mov_imm(X4, FREED);
            a.tst(X4, X3);
            a.bcond(NE, l_panic[P_DOUBLE_FREE]);
        }
        a.add_imm(X4, X1, 8); // x4 = needed block size N
        a.cmp(X4, X3);
        a.bcond(LS, l_same);
        a.mov_imm(X16, SMALL_MAX);
        a.cmp(X3, X16);
        a.bcond(HI, l_large);
        a.cmp(X4, X16);
        a.bcond(HI, l_move);
        a.b(l_move); // small -> small: always a move (see x64.rs)
        a.bind(l_same);
        a.ret();
        a.bind(l_alloc0);
        a.mov(X0, X1);
        a.b(l_alloc);
        a.bind(l_large);
        a.add_imm(X4, X4, 4095);
        a.mov_imm(X16, -4096);
        a.and_reg(X4, X4, X16); // new mapping length
        a.sub_imm(X0, X0, 8);
        a.mov(X1, X3); // old length
        a.mov(X2, X4); // new length
        a.mov_imm(X3, 1); // MREMAP_MAYMOVE
        a.mov_imm(X8, 216);
        a.svc();
        a.cmp(X0, XZR);
        a.bcond(LT, l_panic[P_OOM]);
        a.str_i(X2, X0, 0); // header = mapping size
        a.add_imm(X0, X0, 8);
        a.ret();
        a.bind(l_move);
        a.stp_pre16(X29, X30);
        a.sub_imm(SP, SP, 32);
        a.str_i(X0, SP, 0); // old
        a.str_i(X2, SP, 8); // bytes to keep
        a.mov(X0, X1);
        a.bl(l_alloc);
        a.str_i(X0, SP, 16); // new
        a.ldr_i(X1, SP, 0);
        a.ldr_i(X2, SP, 8);
        a.bl(l_memcpy);
        a.ldr_i(X0, SP, 0);
        a.bl(l_free);
        a.ldr_i(X0, SP, 16);
        a.add_imm(SP, SP, 32);
        a.ldp_post16(X29, X30);
        a.ret();
    }

    // rt_push: x0 = &list, x1 = value. List = {ptr, len, cap}; capacity 7, then 2*cap+1, grown via rt_realloc.
    {
        let (l_store, l_nz) = (a.label(), a.label());
        a.bind(l_push);
        a.stp_pre16(X29, X30);
        a.sub_imm(SP, SP, 32);
        a.str_i(X1, SP, 0); // value
        a.str_i(X0, SP, 8); // list
        a.ldr_i(X2, X0, 8);
        a.ldr_i(X3, X0, 16);
        a.cmp(X2, X3);
        a.bcond(NE, l_store);
        a.lsl_imm(X4, X3, 1);
        a.add_imm(X4, X4, 1);
        a.cbnz(X3, l_nz);
        a.mov_imm(X4, 7);
        a.bind(l_nz);
        a.str_i(X4, X0, 16); // cap = new capacity
        a.lsl_imm(X1, X4, 3); // new payload bytes
        a.lsl_imm(X2, X2, 3); // bytes to keep
        a.ldr_i(X0, X0, 0); // old buffer
        a.bl(l_realloc);
        a.ldr_i(X1, SP, 8);
        a.str_i(X0, X1, 0);
        a.bind(l_store);
        a.ldr_i(X0, SP, 8);
        a.ldr_i(X1, SP, 0);
        a.ldr_i(X2, X0, 8);
        a.lsl_imm(X3, X2, 3);
        a.ldr_i(X4, X0, 0);
        a.add_reg(X4, X4, X3);
        a.str_i(X1, X4, 0);
        a.add_imm(X2, X2, 1);
        a.str_i(X2, X0, 8);
        a.add_imm(SP, SP, 32);
        a.ldp_post16(X29, X30);
        a.ret();
    }

    // rt_get: x0 = &list, x1 = index -> x0. Unsigned compare also rejects negative indices.
    a.bind(l_get);
    a.ldr_i(X2, X0, 8);
    a.cmp(X1, X2);
    a.bcond(HS, l_panic[P_INDEX]);
    a.ldr_i(X3, X0, 0);
    a.lsl_imm(X1, X1, 3);
    a.add_reg(X3, X3, X1);
    a.ldr_i(X0, X3, 0);
    a.ret();

    // user functions
    // failure paths with a position: `mov x17, loc; b <shared stub>` per (target, location), after each function
    let site = |a: &mut Asm, stubs: &mut Vec<(usize, usize, u32)>, target: usize, loc: u32| -> usize {
        if !report || loc == 0 {
            return target;
        }
        if let Some((l, _, _)) = stubs.iter().find(|(_, t, c)| *t == target && *c == loc) {
            return *l;
        }
        let l = a.label();
        stubs.push((l, target, loc));
        l
    };
    for (fi, f) in ir.funcs.iter().enumerate() {
        let constants = crate::ir::syscall_constants(f);
        let (sf, flocs) = strip_locs(f);
        let f = &sf;
        let mut stubs: Vec<(usize, usize, u32)> = vec![];
        a.bind(l_funcs[fi]);
        a.stp_pre16(X29, X30);
        a.mov_sp(X29, SP);
        // frame: [vreg slots][aggregate slots] above sp
        let mut slot_off = vec![];
        let mut tot = f.nvregs * 8;
        for w in &f.slots {
            slot_off.push(tot as u32);
            tot += w * 8;
        }
        let frame = ((tot + 15) & !15) as u32;
        if frame > 0 {
            a.sub_sp_const(SP, SP, frame, false);
        }
        for i in 0..f.nparams {
            a.ldr(X0, X29, 16 + 8 * i as u32); // arg i lives at [x29+16+8i]
            a.str(X0, SP, slot(i));
        }
        let bl: Vec<usize> = f.blocks.iter().map(|_| a.label()).collect();
        for (bi, b) in f.blocks.iter().enumerate() {
            a.bind(bl[bi]);
            for (ii, ins) in b.insts.iter().enumerate() {
                let cl = flocs[bi][ii];
                match ins {
                    Inst::Loc(_) => {}
                    Inst::Sel(..) => unreachable!("x86-64 if-conversion only"),
                    Inst::Thread(op, d, args) => {
                        for (reg, arg) in [X0, X1, X2].into_iter().zip(args) {
                            a.ldr(reg, SP, slot(*arg));
                        }
                        thread_runtime.as_ref().unwrap().operation(&mut a, *op);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::Atomic(op, d, args) => {
                        for (reg, arg) in [X0, X1, X2].into_iter().zip(args) {
                            a.ldr(reg, SP, slot(*arg));
                        }
                        threads::atomic(&mut a, *op);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::Global(d, n) => {
                        a.mov_imm(X0, G + G_USER + 8 * *n as i64);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::Syscall(d, args) => {
                        for (reg, arg) in [X0, X1, X2, X3, X4, X5].into_iter().zip(&args[1..]) {
                            a.ldr(reg, SP, slot(*arg));
                        }
                        constants[args[0]]
                            .and_then(LinuxSyscall::from_x64)
                            .expect("validated Linux syscall")
                            .emit(&mut a);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::Const(d, n) => {
                        a.mov_imm(X0, *n);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::Text(d, id) => {
                        a.lea_data(X0, D_TEXT0 + id);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::N(op, d, x, y) => {
                        a.ldr(X0, SP, slot(*x));
                        a.ldr(X1, SP, slot(*y));
                        match *op {
                            NOp::FAdd | NOp::FSub | NOp::FMul | NOp::FDiv => {
                                a.fmov_dx(0, X0);
                                a.fmov_dx(1, X1);
                                let k = match op {
                                    NOp::FAdd => 0x2800,
                                    NOp::FSub => 0x3800,
                                    NOp::FMul => 0x0800,
                                    _ => 0x1800,
                                };
                                a.farith(k, 0, 0, 1);
                                a.fmov_xd(X0, 0);
                            }
                            NOp::FSqrt => {
                                a.fmov_dx(0, X0);
                                a.fsqrt(0, 0);
                                a.fmov_xd(X0, 0);
                            }
                            NOp::FLt | NOp::FLe | NOp::FEq | NOp::FNe => {
                                // unordered sets NZCV = 0011: MI, LS and EQ are false, NE is true
                                a.fmov_dx(0, X0);
                                a.fmov_dx(1, X1);
                                a.fcmp(0, 1);
                                let cc = match op {
                                    NOp::FLt => MI,
                                    NOp::FLe => LS,
                                    NOp::FEq => EQ,
                                    _ => NE,
                                };
                                a.cset(X0, cc);
                            }
                            NOp::FNeg => {
                                a.mov_imm(X1, i64::MIN);
                                a.eor_reg(X0, X0, X1);
                            }
                            NOp::I2F => {
                                a.scvtf_d(0, X0);
                                a.fmov_xd(X0, 0);
                            }
                            NOp::I2S => {
                                a.scvtf_s(0, X0);
                                a.fmov_ws(X0, 0); // zero-extends into x0
                            }
                            NOp::S2F => {
                                a.fmov_sw(0, X0);
                                a.fcvt_ds(0, 0);
                                a.fmov_xd(X0, 0);
                            }
                            NOp::F2S => {
                                a.fmov_dx(0, X0);
                                a.fcvt_sd(0, 0);
                                a.fmov_ws(X0, 0);
                            }
                            NOp::F2I => {
                                // fcvtzs saturates and maps NaN to 0: reject NaN, i64::MAX (>= 2^63), and i64::MIN unless exactly -2^63
                                let ok = a.label();
                                a.fmov_dx(0, X0);
                                a.fcmp(0, 0);
                                { let t = site(&mut a, &mut stubs, l_panic[P_F2I], cl); a.bcond(VS, t); }
                                a.fcvtzs(X2, 0);
                                a.mov_imm(X1, i64::MAX);
                                a.cmp(X2, X1);
                                { let t = site(&mut a, &mut stubs, l_panic[P_F2I], cl); a.bcond(EQ, t); }
                                a.mov_imm(X1, i64::MIN);
                                a.cmp(X2, X1);
                                a.bcond(NE, ok);
                                a.mov_imm(X1, 0xC3E0_0000_0000_0000u64 as i64);
                                a.cmp(X0, X1);
                                { let t = site(&mut a, &mut stubs, l_panic[P_F2I], cl); a.bcond(NE, t); }
                                a.bind(ok);
                                a.mov(X0, X2);
                            }
                            NOp::Chk(rng, msg) => {
                                let (lo, hi) = rng.bounds();
                                let l = site(&mut a, &mut stubs, l_panic[CHK_PANIC[msg as usize]], cl);
                                if rng == Rng::NonNeg {
                                    a.cmp(X0, XZR);
                                    a.bcond(MI, l);
                                } else if let (Rng::Iv(..), MSG_CONV) = (rng, msg) {
                                    // the message names the value and the bounds
                                    let l_ok = a.label();
                                    a.mov_imm(X2, lo);
                                    a.sub_reg(X2, X0, X2);
                                    a.mov_imm(X3, hi.wrapping_sub(lo));
                                    a.cmp(X2, X3);
                                    a.bcond(LS, l_ok);
                                    a.mov(X9, X0);
                                    a.mov_imm(X10, lo);
                                    a.mov_imm(X11, hi);
                                    { let t = site(&mut a, &mut stubs, l_range_panic, cl); a.b(t); }
                                    a.bind(l_ok);
                                } else {
                                    a.mov_imm(X2, lo);
                                    a.sub_reg(X2, X0, X2);
                                    a.mov_imm(X3, hi.wrapping_sub(lo));
                                    a.cmp(X2, X3);
                                    a.bcond(HI, l);
                                }
                            }
                            NOp::Ext(rng) => {
                                let sh = 64 - rng.bits();
                                a.lsl_imm(X0, X0, sh);
                                if rng.signed() {
                                    a.asr_imm(X0, X0, sh);
                                } else {
                                    a.lsr_imm(X0, X0, sh);
                                }
                            }
                            NOp::WAdd => a.add_reg(X0, X0, X1),
                            NOp::WSub => a.sub_reg(X0, X0, X1),
                            NOp::WMul => a.mul(X0, X0, X1),
                            NOp::UAdd => {
                                a.adds(X0, X0, X1);
                                { let t = site(&mut a, &mut stubs, l_panic[P_ADD], cl); a.bcond(HS, t); }
                            }
                            NOp::USub => {
                                a.subs(X0, X0, X1);
                                { let t = site(&mut a, &mut stubs, l_panic[P_SUB], cl); a.bcond(LO, t); }
                            }
                            NOp::UMul => {
                                a.umulh(X2, X0, X1);
                                a.mul(X0, X0, X1);
                                { let t = site(&mut a, &mut stubs, l_panic[P_MUL], cl); a.cbnz(X2, t); }
                            }
                            NOp::UDiv | NOp::UMod => {
                                let is_div = *op == NOp::UDiv;
                                { let t = site(&mut a, &mut stubs, l_panic[if is_div { P_DIV0 } else { P_MOD0 }], cl); a.cbz(X1, t); }
                                a.udiv(X2, X0, X1);
                                if is_div {
                                    a.mov(X0, X2);
                                } else {
                                    a.msub(X0, X2, X1, X0);
                                }
                            }
                            NOp::ULt | NOp::ULe => {
                                a.cmp(X0, X1);
                                a.cset(X0, if *op == NOp::ULt { LO } else { LS });
                            }
                        }
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::Mov(d, s) => {
                        a.ldr(X0, SP, slot(*s));
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::Neg(d, s) => {
                        a.ldr(X0, SP, slot(*s));
                        a.subs(X0, XZR, X0);
                        { let t = site(&mut a, &mut stubs, l_panic[P_NEG], cl); a.bcond(VS, t); }
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::Not(d, s) => {
                        a.ldr(X0, SP, slot(*s));
                        a.eor1(X0, X0);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::Bin(d, op, x, y) => {
                        a.ldr(X0, SP, slot(*x));
                        a.ldr(X1, SP, slot(*y));
                        match op.checked() {
                            Op::Add => {
                                a.adds(X0, X0, X1);
                                { let t = site(&mut a, &mut stubs, l_panic[P_ADD], cl); a.bcond(VS, t); }
                            }
                            Op::Sub => {
                                a.subs(X0, X0, X1);
                                { let t = site(&mut a, &mut stubs, l_panic[P_SUB], cl); a.bcond(VS, t); }
                            }
                            Op::Mul => {
                                // overflow iff high 64 bits != sign-extension of the low 64 bits
                                a.mul(X2, X0, X1);
                                a.smulh(X3, X0, X1);
                                a.cmp_asr63(X3, X2);
                                { let t = site(&mut a, &mut stubs, l_panic[P_MUL], cl); a.bcond(NE, t); }
                                a.mov(X0, X2);
                            }
                            Op::Div | Op::Mod => {
                                let is_div = op.checked() == Op::Div;
                                let (l_do, l_done) = (a.label(), a.label());
                                { let t = site(&mut a, &mut stubs, l_panic[if is_div { P_DIV0 } else { P_MOD0 }], cl); a.cbz(X1, t); }
                                a.cmn_1(X1); // divisor == -1 ?
                                a.bcond(NE, l_do);
                                if is_div {
                                    a.subs(X0, XZR, X0); // x / -1 = -x
                                    { let t = site(&mut a, &mut stubs, l_panic[P_DIV], cl); a.bcond(VS, t); }
                                } else {
                                    a.mov_imm(X0, 0); // x % -1 == 0
                                }
                                a.b(l_done);
                                a.bind(l_do);
                                a.sdiv(X2, X0, X1);
                                if is_div {
                                    a.mov(X0, X2);
                                } else {
                                    a.msub(X0, X2, X1, X0);
                                }
                                a.bind(l_done);
                            }
                            cmp => {
                                a.cmp(X0, X1);
                                let cc = match cmp {
                                    Op::Eq => EQ,
                                    Op::Ne => NE,
                                    Op::Lt => LT,
                                    Op::Le => LE,
                                    Op::Gt => GT,
                                    _ => GE,
                                };
                                a.cset(X0, cc);
                            }
                        }
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::VTableAddr(d, id) => {
                        a.lea_data(X0, data.len() + id);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::FnAddr(d, fid) => {
                        a.lea_label(X0, l_funcs[*fid]);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::ExternCall(dst, id, args, floats, ret_float) => {
                        if has_threads {
                            a.mov_imm(X9, threads::MAIN_CONTEXT);
                            a.cmp(X19, X9);
                            a.bcond(NE, l_panic[P_WORKER_FFI]);
                        }
                        // Linux AAPCS64: eight registers in each argument bank.
                        // Arguments that overflow either bank spill in source order.
                        let (mut gp, mut fp, mut spilled) = (0, 0, 0);
                        let float_count = floats.iter().filter(|&&f| f).count();
                        let stack_count = float_count.saturating_sub(8)
                            + (args.len() - float_count).saturating_sub(8);
                        let size = ((8 * stack_count + 15) & !15) as u32;
                        if size != 0 {
                            a.sub_sp_const(SP, SP, size, false);
                        }
                        for (&arg, &float) in args.iter().zip(floats) {
                            let offset = slot(arg) + size;
                            if float && fp < 8 {
                                a.ldr(X9, SP, offset);
                                a.fmov_dx(fp, X9);
                                fp += 1;
                            } else if !float && gp < 8 {
                                a.ldr(gp, SP, offset);
                                gp += 1;
                            } else {
                                a.ldr(X9, SP, offset);
                                a.str(X9, SP, spilled);
                                spilled += 8;
                            }
                        }
                        a.mov_imm(X16, G + (GOT_REL + 8 * id) as i64);
                        a.ldr_i(X16, X16, 0);
                        a.blr(X16);
                        if size != 0 {
                            a.sub_sp_const(SP, SP, size, true);
                        }
                        if let Some(d) = dst {
                            if *ret_float {
                                a.fmov_xd(X0, 0);
                            }
                            a.str(X0, SP, slot(*d));
                        }
                    }
                    Inst::Call(dst, _, args) | Inst::CallInd(dst, _, args) => {
                        let sz = ((8 * args.len() + 15) & !15) as u32;
                        if sz > 0 {
                            a.sub_sp_const(SP, SP, sz, false);
                        }
                        for (i, arg) in args.iter().enumerate() {
                            a.ldr(X0, SP, slot(*arg) + sz); // slots are sp-relative: skip the arg area
                            a.str(X0, SP, 8 * i as u32);
                        }
                        match ins {
                            Inst::Call(_, fid, _) => {
                        // a failure inside a library function is reported at its caller's position
                        if report && cl != 0 && ir.funcs[*fid].lib {
                            a.mov_imm(X17, cl as i64);
                        }
                        a.bl(l_funcs[*fid])
                    }
                            Inst::CallInd(_, code, _) => {
                                a.ldr(X16, SP, slot(*code) + sz);
                                a.blr(X16);
                            }
                            _ => unreachable!(),
                        }
                        if sz > 0 {
                            a.sub_sp_const(SP, SP, sz, true);
                        }
                        if let Some(d) = dst {
                            a.str(X0, SP, slot(*d));
                        }
                    }
                    Inst::PrintInt(v) => {
                        a.ldr(X0, SP, slot(*v));
                        a.mov_imm(X6, 1);
                        a.bl(l_print_int);
                    }
                    Inst::PrintBool(v) => {
                        a.ldr(X0, SP, slot(*v));
                        a.bl(l_print_bool);
                    }
                    Inst::PrintText(v) => {
                        a.ldr(X0, SP, slot(*v));
                        a.bl(l_print_text);
                    }
                    Inst::Slot(d, k) => {
                        a.sub_sp_const(X0, SP, slot_off[*k], true);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::Load(d, ad, off) | Inst::LoadH(d, ad, off) => {
                        a.ldr(X1, SP, slot(*ad));
                        a.ldr_i(X0, X1, *off);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::Store(ad, off, v) => {
                        a.ldr(X1, SP, slot(*ad));
                        a.ldr(X0, SP, slot(*v));
                        a.str_i(X0, X1, *off);
                    }
                    Inst::LoadX(d, b, i, 0) => {
                        a.ldr(X1, SP, slot(*b));
                        a.ldr(X2, SP, slot(*i));
                        a.add_reg(X1, X1, X2);
                        a.ldrb_i(X0, X1, 0);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::StoreX(b, i, 0, v) => {
                        a.ldr(X1, SP, slot(*b));
                        a.ldr(X2, SP, slot(*i));
                        a.add_reg(X1, X1, X2);
                        a.ldr(X0, SP, slot(*v));
                        a.strb_i(X0, X1, 0);
                    }
                    Inst::LoadX(d, b, i, k) => {
                        a.ldr(X1, SP, slot(*b));
                        a.ldr(X2, SP, slot(*i));
                        a.lsl_imm(X2, X2, k.trailing_zeros());
                        a.add_reg(X1, X1, X2);
                        a.ldr_i(X0, X1, 0);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::StoreX(b, i, k, v) => {
                        a.ldr(X1, SP, slot(*b));
                        a.ldr(X2, SP, slot(*i));
                        a.lsl_imm(X2, X2, k.trailing_zeros());
                        a.add_reg(X1, X1, X2);
                        a.ldr(X0, SP, slot(*v));
                        a.str_i(X0, X1, 0);
                    }
                    Inst::LeaX(d, b, i, k) => {
                        a.ldr(X1, SP, slot(*b));
                        a.ldr(X2, SP, slot(*i));
                        a.lsl_imm(X2, X2, k.trailing_zeros());
                        a.add_reg(X0, X1, X2);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::Lea(d, ad, off) => {
                        a.ldr(X0, SP, slot(*ad));
                        a.mov_imm(X1, *off as i64);
                        a.add_reg(X0, X0, X1);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::Copy(dst, src, n) => {
                        a.ldr(X1, SP, slot(*dst));
                        a.ldr(X2, SP, slot(*src));
                        for i in 0..*n as i32 {
                            a.ldr_i(X0, X2, 8 * i);
                            a.str_i(X0, X1, 8 * i);
                        }
                    }
                    Inst::ListPush(l, v) => {
                        a.ldr(X0, SP, slot(*l));
                        a.ldr(X1, SP, slot(*v));
                        if report && cl != 0 { a.mov_imm(X17, cl as i64); }
                        a.bl(l_push);
                    }
                    Inst::ListGet(d, l, i) => {
                        a.ldr(X0, SP, slot(*l));
                        a.ldr(X1, SP, slot(*i));
                        if report && cl != 0 { a.mov_imm(X17, cl as i64); }
                        a.bl(l_get);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::ListFree(l) => {
                        a.ldr(X1, SP, slot(*l));
                        a.ldr_i(X0, X1, 0);
                        a.bl(l_free);
                    }
                    Inst::Alloc(d, n) => {
                        a.ldr(X0, SP, slot(*n));
                        a.bl(l_alloc);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::Free(p) => {
                        a.ldr(X0, SP, slot(*p));
                        a.bl(l_free);
                    }
                    Inst::MemCpy(dst, src, n) => {
                        a.ldr(X0, SP, slot(*dst));
                        a.ldr(X1, SP, slot(*src));
                        a.ldr(X2, SP, slot(*n));
                        a.bl(l_memcpy);
                    }
                    Inst::Realloc(d, old, n, used) => {
                        a.ldr(X0, SP, slot(*old));
                        a.ldr(X1, SP, slot(*n));
                        a.ldr(X2, SP, slot(*used));
                        a.bl(l_realloc);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::Load8(d, ad, off) => {
                        a.ldr(X1, SP, slot(*ad));
                        a.ldrb_i(X0, X1, *off);
                        a.str(X0, SP, slot(*d));
                    }
                    Inst::Store8(ad, off, v) => {
                        a.ldr(X1, SP, slot(*ad));
                        a.ldr(X0, SP, slot(*v));
                        a.strb_i(X0, X1, *off);
                    }
                    Inst::PrintBytes(p, n) => {
                        a.ldr(X1, SP, slot(*p));
                        a.ldr(X2, SP, slot(*n));
                        a.bl(l_print_bytes);
                    }
                    Inst::Panic(p, n) => {
                        a.ldr(X1, SP, slot(*p));
                        a.ldr(X2, SP, slot(*n));
                        if report && cl != 0 { a.mov_imm(X17, cl as i64); }
                        a.bl(l_panic_bytes);
                    }
                    Inst::Check(i, n) => {
                        a.ldr(X0, SP, slot(*i));
                        a.ldr(X1, SP, slot(*n));
                        a.cmp(X0, X1);
                        { let t = site(&mut a, &mut stubs, l_panic[P_INDEX], cl); a.bcond(HS, t); } // unsigned: negative indices fail too
                    }
                    Inst::CheckNonEmpty(n) => {
                        a.ldr(X0, SP, slot(*n));
                        { let t = site(&mut a, &mut stubs, l_panic[P_POP], cl); a.cbz(X0, t); }
                    }
                    Inst::Assert(n) => {
                        a.ldr(X0, SP, slot(*n));
                        { let t = site(&mut a, &mut stubs, l_panic[P_ASSERT], cl); a.cbz(X0, t); }
                    }
                }
            }
            match &b.term {
                Term::Jmp(t) => {
                    if *t != bi + 1 {
                        a.b(bl[*t]);
                    }
                }
                Term::Br(c, t, e) => {
                    a.ldr(X0, SP, slot(*c));
                    a.cbnz(X0, bl[*t]);
                    if *e != bi + 1 {
                        a.b(bl[*e]);
                    }
                }
                Term::Ret(v) => {
                    if let Some(v) = v {
                        a.ldr(X0, SP, slot(*v));
                    }
                    a.mov_sp(SP, X29);
                    a.ldp_post16(X29, X30);
                    a.ret();
                }
                Term::Unreachable => a.udf(),
            }
        }
        for (l, t, c) in stubs {
            a.bind(l);
            a.mov_imm(X17, c as i64);
            a.b(t);
        }
    }

    // lay out: code, padding to 16, then data blobs (each 8-aligned)
    while !a.code.len().is_multiple_of(16) {
        a.code.push(0);
    }
    let data_start = a.code.len();
    let mut offs = vec![];
    let mut blob: Vec<u8> = vec![];
    for d in &data {
        while !blob.len().is_multiple_of(8) {
            blob.push(0);
        }
        offs.push(blob.len());
        blob.extend_from_slice(d);
    }
    for table in &ir.vtables {
        blob.resize((blob.len() + 7) & !7, 0);
        offs.push(blob.len());
        let address = (data_start + blob.len()) as i64;
        for &func in table {
            let code = a.labels[l_funcs[func]].unwrap() as i64;
            blob.extend_from_slice(&(code - address).to_le_bytes());
        }
    }
    if report {
        // positions: "  at ", ":", the location rows (file, line, col as u64) and the file table
        // ({offset from the table, length} per file, then the names)
        let mut files: Vec<u8> = vec![];
        let mut names: Vec<u8> = vec![];
        let base = 16 * ir.files.len();
        for n in &ir.files {
            files.extend_from_slice(&((base + names.len()) as u64).to_le_bytes());
            files.extend_from_slice(&(n.len() as u64).to_le_bytes());
            names.extend_from_slice(n.as_bytes());
        }
        files.extend(names);
        let mut locs: Vec<u8> = vec![];
        for (f, l, c) in &ir.locs {
            for w in [*f, *l, *c] {
                locs.extend_from_slice(&(w as u64).to_le_bytes());
            }
        }
        for d in [b"  at ".to_vec(), b":".to_vec(), locs, files] {
            blob.resize((blob.len() + 7) & !7, 0);
            offs.push(blob.len());
            blob.extend_from_slice(&d);
        }
    }
    let entry = a.labels[l_start].unwrap();
    if ir.externs.is_empty() {
        a.finalize(BASE + HDR, data_start, &offs);
        a.code.extend_from_slice(&blob);
        elf64(&a.code, entry)
    } else {
        use crate::elfdyn::{write_image, Arch, HEADER_SIZE};
        a.finalize(BASE + HEADER_SIZE as u64, data_start, &offs);
        write_image(&a.code, &blob, entry, &ir.links, &ir.externs, Arch::A64)
    }
}

// ───────────────────────── ELF64 writer ─────────────────────────
// One PT_LOAD segment (R+X) holding headers + code + read-only data. EM_AARCH64 = 183.

const BASE: u64 = 0x400000;
const HDR: u64 = 64 + 2 * 56;

fn elf64(blob: &[u8], entry_off: usize) -> Vec<u8> {
    let total = HDR + blob.len() as u64;
    let mut o: Vec<u8> = vec![];
    o.extend_from_slice(&[0x7F, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    o.extend_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    o.extend_from_slice(&183u16.to_le_bytes()); // EM_AARCH64
    o.extend_from_slice(&1u32.to_le_bytes());
    o.extend_from_slice(&(BASE + HDR + entry_off as u64).to_le_bytes());
    o.extend_from_slice(&64u64.to_le_bytes()); // phoff
    o.extend_from_slice(&0u64.to_le_bytes()); // shoff
    o.extend_from_slice(&0u32.to_le_bytes()); // flags
    o.extend_from_slice(&64u16.to_le_bytes());
    o.extend_from_slice(&56u16.to_le_bytes());
    o.extend_from_slice(&2u16.to_le_bytes()); // phnum
    o.extend_from_slice(&64u16.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    o.extend_from_slice(&5u32.to_le_bytes()); // R + X
    o.extend_from_slice(&0u64.to_le_bytes());
    o.extend_from_slice(&BASE.to_le_bytes());
    o.extend_from_slice(&BASE.to_le_bytes());
    o.extend_from_slice(&total.to_le_bytes());
    o.extend_from_slice(&total.to_le_bytes());
    o.extend_from_slice(&0x1000u64.to_le_bytes());
    // second segment: zero-filled RW heap state at G (no bytes in the file)
    o.extend_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    o.extend_from_slice(&6u32.to_le_bytes()); // R + W
    o.extend_from_slice(&0u64.to_le_bytes());
    o.extend_from_slice(&(G as u64).to_le_bytes());
    o.extend_from_slice(&(G as u64).to_le_bytes());
    o.extend_from_slice(&0u64.to_le_bytes());
    o.extend_from_slice(&4096u64.to_le_bytes());
    o.extend_from_slice(&0x1000u64.to_le_bytes());
    o.extend_from_slice(blob);
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(f: impl FnOnce(&mut Asm)) -> u32 {
        let mut a = Asm::new();
        f(&mut a);
        u32::from_le_bytes(a.code[0..4].try_into().unwrap())
    }

    // Known-good encodings (ARM ARM / objdump).
    #[test]
    fn known_encodings() {
        assert_eq!(enc(|a| a.movz(0, 1, 0)), 0xD280_0020);
        assert_eq!(enc(|a| a.adds(0, 0, 1)), 0xAB01_0000);
        assert_eq!(enc(|a| a.subs(0, 0, 1)), 0xEB01_0000);
        assert_eq!(enc(|a| a.mul(2, 0, 1)), 0x9B01_7C02);
        assert_eq!(enc(|a| a.smulh(3, 0, 1)), 0x9B41_7C03);
        assert_eq!(enc(|a| a.sdiv(2, 0, 1)), 0x9AC1_0C02);
        assert_eq!(enc(|a| a.msub(0, 2, 1, 0)), 0x9B01_8040);
        assert_eq!(enc(|a| a.ret()), 0xD65F_03C0);
        assert_eq!(enc(|a| a.svc()), 0xD400_0001);
        assert_eq!(enc(|a| a.stp_pre16(29, 30)), 0xA9BF_7BFD);
        assert_eq!(enc(|a| a.ldp_post16(29, 30)), 0xA8C1_7BFD);
        assert_eq!(enc(|a| a.cset(0, EQ)), 0x9A9F_17E0);
        assert_eq!(enc(|a| a.mov(29, 1)), 0xAA01_03FD);
        assert_eq!(enc(|a| a.ldr(0, 31, 8)), 0xF940_07E0);
        assert_eq!(enc(|a| a.eor1(0, 0)), 0xD240_0000);
        assert_eq!(enc(|a| a.fmov_dx(0, 0)), 0x9E67_0000);
        assert_eq!(enc(|a| a.fmov_xd(0, 0)), 0x9E66_0000);
        assert_eq!(enc(|a| a.farith(0x2800, 0, 0, 1)), 0x1E61_2800);
        assert_eq!(enc(|a| a.farith(0x3800, 0, 0, 1)), 0x1E61_3800);
        assert_eq!(enc(|a| a.farith(0x0800, 0, 0, 1)), 0x1E61_0800);
        assert_eq!(enc(|a| a.farith(0x1800, 0, 0, 1)), 0x1E61_1800);
        assert_eq!(enc(|a| a.fsqrt(0, 0)), 0x1E61_C000);
        assert_eq!(enc(|a| a.fcmp(0, 1)), 0x1E61_2000);
        assert_eq!(enc(|a| a.scvtf_d(0, 0)), 0x9E62_0000);
        assert_eq!(enc(|a| a.fcvtzs(2, 0)), 0x9E78_0002);
        assert_eq!(enc(|a| a.fcvt_ds(0, 0)), 0x1E22_C000);
        assert_eq!(enc(|a| a.fcvt_sd(0, 0)), 0x1E62_4000);
        assert_eq!(enc(|a| a.umulh(2, 0, 1)), 0x9BC1_7C02);
        assert_eq!(enc(|a| a.lsr_imm(0, 0, 56)), 0xD378_FC00);
        assert_eq!(enc(|a| a.asr_imm(0, 0, 56)), 0x9378_FC00);
    }
}
