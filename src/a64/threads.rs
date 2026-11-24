//! Linux worker lifecycle, futexes, and AArch64 memory ordering.
use super::*;

const CODE: i32 = 8;
const ENV: i32 = 16;
const RESULT: i32 = 24;
const RECOVERY: i32 = 32;
const UNWINDING: i32 = 40;
const CLEANUP_HEAD: i32 = 48;

// x19 is reserved for the current worker's control block. At process entry it
// points into the zero-filled runtime state instead.
pub(super) const MAIN_CONTEXT: i64 = G + 192;
pub(super) const HEAP_LOCK: i64 = G + 176;
pub(super) const PRINT_LOCK: i64 = G + 184;
const PAGE: u32 = 65536; // covers the largest supported arm64 Linux page size
const MAPPING: u32 = 2 * PAGE + 2 * 1024 * 1024;

const FUTEX_WAIT: i64 = 0;
const FUTEX_WAIT_PRIVATE: i64 = 128;
const FUTEX_WAKE_PRIVATE: i64 = 129;

impl Asm {
    fn ldaxr(&mut self, rt: u32, rn: u32) {
        self.emit(0xC85F_FC00 | rn << 5 | rt);
    }

    fn stlxr(&mut self, status: u32, rt: u32, rn: u32) {
        self.emit(0xC800_FC00 | status << 16 | rn << 5 | rt);
    }

    fn ldar(&mut self, rt: u32, rn: u32, word: bool) {
        let op = if word { 0x88DF_FC00 } else { 0xC8DF_FC00 };
        self.emit(op | rn << 5 | rt);
    }

    fn clrex(&mut self) {
        self.emit(0xD503_3F5F);
    }

    fn dmb(&mut self) {
        self.emit(0xD503_3BBF); // dmb ish
    }
}

// Runtime locks use states 0 (free), 1 (locked), and 2 (contended).
// The fast path preserves runtime arguments and does not enter the kernel.
pub(super) fn lock(a: &mut Asm, address: i64) {
    let retry = a.label();
    let slow = a.label();
    let got = a.label();
    a.mov_imm(X10, address);
    a.mov_imm(X12, 1);
    a.bind(retry);
    a.ldaxr(X11, X10);
    a.cbnz(X11, slow);
    a.stlxr(X11, X12, X10);
    a.cbz(X11, got);
    a.b(retry);
    a.bind(slow);
    a.mov_imm(X12, 2);
    a.ldaxr(X11, X10);
    a.stlxr(X13, X12, X10);
    a.cbnz(X13, slow);
    a.cbz(X11, got);
    lock_futex(a, FUTEX_WAIT_PRIVATE, 2);
    a.b(slow);
    a.bind(got);
}

pub(super) fn unlock(a: &mut Asm, address: i64) {
    let retry = a.label();
    let done = a.label();
    a.mov_imm(X10, address);
    a.bind(retry);
    a.ldaxr(X11, X10);
    a.stlxr(X13, XZR, X10);
    a.cbnz(X13, retry);
    a.sub_imm(X11, X11, 2);
    a.cbnz(X11, done);
    lock_futex(a, FUTEX_WAKE_PRIVATE, 1);
    a.bind(done);
}

fn lock_futex(a: &mut Asm, op: i64, val: i64) {
    a.sub_imm(SP, SP, 48);
    let saved = [X0, X1, X2, X3, X8];
    for (i, reg) in saved.into_iter().enumerate() {
        a.str_i(reg, SP, 8 * i as i32);
    }
    a.mov(X0, X10);
    a.mov_imm(X2, val);
    futex(a, op);
    for (i, reg) in saved.into_iter().enumerate() {
        a.ldr_i(reg, SP, 8 * i as i32);
    }
    a.add_imm(SP, SP, 48);
}

fn futex(a: &mut Asm, op: i64) {
    a.mov_imm(X1, op);
    a.mov_imm(X3, 0); // no wait timeout
    a.mov_imm(X8, 98);
    a.svc();
}

pub(super) struct Runtime {
    start: usize,
    wait: usize,
    recover: usize,
}

impl Runtime {
    pub(super) fn new(a: &mut Asm) -> Self {
        Self {
            start: a.label(),
            wait: a.label(),
            recover: a.label(),
        }
    }

    pub(super) fn panic_probe(&self, a: &mut Asm, double_panic: usize) {
        a.ldr_i(X15, X19, UNWINDING);
        a.cbnz(X15, double_panic);
        a.ldr_i(X15, X19, RECOVERY);
        a.sub_imm(X15, X15, 1);
        a.cbz(X15, self.recover);
    }

    pub(super) fn emit(&self, a: &mut Asm, mmap: usize, failure: usize) {
        let failed_mapping = a.label();
        let parent = a.label();
        let child_exit = a.label();
        let child_cleanup = a.label();

        // start(code, env, recover) -> control block. On the child side the
        // kernel switches SP, so only the parent reloads its saved registers.
        a.bind(self.start);
        a.sub_imm(SP, SP, 32);
        for (i, reg) in [X0, X1, X2, X30].into_iter().enumerate() {
            a.str_i(reg, SP, 8 * i as i32);
        }
        a.mov_imm(X1, MAPPING as i64);
        a.bl(mmap);
        a.cmp(X0, XZR);
        a.bcond(LT, failure);
        a.mov(X9, X0);
        for (i, field) in [CODE, ENV, RECOVERY].into_iter().enumerate() {
            a.ldr_i(X0, SP, 8 * i as i32);
            a.str_i(X0, X9, field);
        }
        a.sub_sp_const(X0, X9, PAGE, true);
        a.mov_imm(X1, PAGE as i64);
        a.mov_imm(X2, 0); // PROT_NONE
        a.mov_imm(X8, 226); // mprotect
        a.svc();
        a.cmp(X0, XZR);
        a.bcond(LT, failed_mapping);

        // VM, FS, FILES, SIGHAND, THREAD, SYSVSEM, PARENT_SETTID, CHILD_CLEARTID
        a.mov_imm(X0, 0x0035_0f00);
        a.sub_sp_const(X1, X9, MAPPING, true);
        a.mov(X2, X9); // parent_tid
        a.mov_imm(X3, 0); // no new libc TLS
        a.mov(X4, X9); // child_tid
        a.mov_imm(X8, 220); // clone
        a.svc();
        a.cmp(X0, XZR);
        a.bcond(LT, failed_mapping);
        a.cbnz(X0, parent);

        a.mov(X19, X9);
        a.mov_imm(X29, 0);
        a.ldr_i(X16, X19, CODE);
        a.ldr_i(X0, X19, ENV);
        a.sub_imm(SP, SP, 16);
        a.str_i(X0, SP, 0);
        a.blr(X16);
        a.add_imm(SP, SP, 16);
        a.str_i(X0, X19, RESULT);
        a.b(child_cleanup);

        // Keep abandoned frames live while running their cleanup callbacks.
        a.bind(self.recover);
        a.str_i(XZR, X19, RECOVERY);
        a.mov_imm(X0, 1);
        a.str_i(X0, X19, UNWINDING);
        let cleanup = a.label();
        let cleaned = a.label();
        a.bind(cleanup);
        a.ldr_i(X1, X19, CLEANUP_HEAD);
        a.cbz(X1, cleaned);
        a.ldr_i(X0, X1, 8);
        a.str_i(X0, X19, CLEANUP_HEAD);
        a.ldr_i(X0, X1, 0);
        a.cbz(X0, cleanup);
        a.str_i(XZR, X1, 0);
        a.ldr_i(X0, X1, 16);
        a.ldr_i(X16, X1, 24);
        a.sub_imm(SP, SP, 16);
        a.str_i(X0, SP, 0);
        a.blr(X16);
        a.add_imm(SP, SP, 16);
        a.b(cleanup);
        a.bind(cleaned);
        a.sub_sp_const(SP, X19, MAPPING, true);
        a.mov_imm(X29, 0);

        a.bind(child_cleanup);
        a.str_i(XZR, X19, RECOVERY);
        a.ldr_i(X0, X19, ENV);
        a.cbz(X0, child_exit);
        a.ldr_i(X16, X0, 0); // environment drop glue
        a.sub_imm(SP, SP, 16);
        a.str_i(X0, SP, 0);
        a.str_i(XZR, SP, 8); // no worklist
        a.blr(X16);
        a.bind(child_exit);
        a.dmb(); // publish the result and cleanup before the kernel clears TID
        a.mov_imm(X0, 0);
        a.mov_imm(X8, 93); // exit just this thread
        a.svc();

        a.bind(parent);
        a.ldr_i(X30, SP, 24);
        a.add_imm(SP, SP, 32);
        a.mov(X0, X9);
        a.ret();

        a.bind(failed_mapping);
        unmap(a, X9);
        a.b(failure);

        // The kernel's clear-child-tid wake uses the shared futex key.
        a.bind(self.wait);
        a.mov(X9, X0);
        let check = a.label();
        let complete = a.label();
        a.bind(check);
        a.ldar(X2, X9, true);
        a.cbz(X2, complete);
        a.mov(X0, X9);
        futex(a, FUTEX_WAIT);
        a.b(check);
        a.bind(complete);
        a.ldr_i(X10, X9, RESULT);
        unmap(a, X9);
        a.mov(X0, X10);
        a.ret();
    }

    pub(super) fn operation(&self, a: &mut Asm, op: ThreadOp) {
        match op {
            ThreadOp::Start | ThreadOp::StartRecover => {
                a.mov_imm(X2, i64::from(op == ThreadOp::StartRecover));
                a.bl(self.start);
            }
            ThreadOp::Wait => a.bl(self.wait),
            ThreadOp::Park | ThreadOp::Wake => {
                a.mov(X2, X1);
                futex(
                    a,
                    if op == ThreadOp::Park {
                        FUTEX_WAIT_PRIVATE
                    } else {
                        FUTEX_WAKE_PRIVATE
                    },
                );
            }
            ThreadOp::CleanupPush => {
                a.ldr_i(X3, X19, CLEANUP_HEAD);
                a.str_i(X3, X0, 8);
                a.str_i(X1, X0, 16);
                a.str_i(X2, X0, 24);
                a.str_i(X0, X19, CLEANUP_HEAD);
            }
            ThreadOp::CleanupPop => {
                a.ldr_i(X0, X0, 8);
                a.str_i(X0, X19, CLEANUP_HEAD);
            }
            ThreadOp::DropEnter => {
                a.ldr_i(X0, X19, RECOVERY);
                a.mov_imm(X1, 2); // a panic inside a destructor is fatal
                a.str_i(X1, X19, RECOVERY);
            }
            ThreadOp::DropLeave => a.str_i(X0, X19, RECOVERY),
            ThreadOp::Unlock => {
                let exchange = a.label();
                let retry = a.label();
                let done = a.label();
                a.ldr_i(X3, X19, UNWINDING);
                a.mov_imm(X2, 1); // wake one waiter on normal release
                a.cbz(X3, exchange);
                a.mov_imm(X3, 3); // poisoned: wake every waiter
                a.mov_imm(X2, i32::MAX as i64);
                a.bind(exchange);
                a.bind(retry);
                a.ldaxr(X4, X0);
                a.stlxr(X5, X3, X0);
                a.cbnz(X5, retry);
                a.sub_imm(X4, X4, 2); // 2 = contended
                a.cbnz(X4, done);
                futex(a, FUTEX_WAKE_PRIVATE);
                a.bind(done);
            }
        }
    }
}

fn unmap(a: &mut Asm, addr: u32) {
    a.mov(X0, addr);
    a.mov_imm(X1, MAPPING as i64);
    a.mov_imm(X8, 215);
    a.svc();
}

// Args arrive in x0..x2. Match the x86-64 runtime's sequential consistency,
// and its convention of returning the observed value for RMW operations.
pub(super) fn atomic(a: &mut Asm, op: AtomicOp) {
    a.mov(X3, X0);
    a.dmb();
    if op == AtomicOp::Load {
        a.ldar(X0, X3, false);
    } else {
        let retry = a.label();
        let done = a.label();
        a.bind(retry);
        a.ldaxr(X0, X3);
        let val = match op {
            AtomicOp::CompareExchange => {
                let matched = a.label();
                a.cmp(X0, X1);
                a.bcond(EQ, matched);
                a.clrex();
                a.b(done);
                a.bind(matched);
                X2
            }
            AtomicOp::FetchAdd => {
                a.add_reg(X2, X0, X1);
                X2
            }
            AtomicOp::Store => X1,
            AtomicOp::Load => unreachable!(),
        };
        a.stlxr(X4, val, X3);
        a.cbnz(X4, retry);
        a.bind(done);
    }
    a.dmb();
}
