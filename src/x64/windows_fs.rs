//! Translate Linux file position and directory reads to Win32.
use super::*;

const SYSCALLS: [i64; 4] = [8, 74, 77, 217];

pub(super) fn supports(number: i64) -> bool {
    SYSCALLS.contains(&number)
}

pub(super) struct Stubs([usize; SYSCALLS.len()]);

impl Stubs {
    pub(super) fn new(a: &mut Asm) -> Self {
        Self(SYSCALLS.map(|_| a.label()))
    }

    pub(super) fn emit_dispatch(&self, a: &mut Asm) {
        for (number, stub) in SYSCALLS.into_iter().zip(self.0) {
            a.cmp_imm(RAX, number as i32);
            a.jcc(CC_E, stub);
        }
    }

    pub(super) fn emit(&self, a: &mut Asm, winerr: usize) {
        let [seek, sync, truncate, directory] = self.0;
        let bad_args = a.label();
        let failed = a.label();

        a.bind(seek);
        prologue(a);
        a.cmp_imm(RDX, 2); // SEEK_SET, SEEK_CUR and SEEK_END
        a.jcc(CC_A, bad_args);
        let valid = a.label();
        a.test_rr(RDX);
        a.jcc(CC_NE, valid);
        a.test_rr(RSI); // FILE_BEGIN interprets the offset as unsigned
        a.jcc(CC_S, bad_args);
        a.bind(valid);
        a.mov_rr(R9, RDX);
        a.mov_rr(RDX, RSI);
        a.mov_rr(RCX, RDI);
        a.lea(R8, RSP, 72);
        a.call_iat(IMP_SETFILEPOINTER);
        test_bool(a);
        a.jcc(CC_E, failed);
        a.ld(RAX, RSP, 72);
        epilogue(a);

        for (label, is_truncate) in [(sync, false), (truncate, true)] {
            a.bind(label);
            prologue(a);
            a.mov_rr(RCX, RDI);
            if is_truncate {
                a.test_rr(RSI);
                a.jcc(CC_S, bad_args);
                a.st(RSP, 72, RSI); // FILE_END_OF_FILE_INFO: new length
                a.lea(R8, RSP, 72);
                a.mov_imm(RDX, 6); // FileEndOfFileInfo
                a.mov_imm(R9, 8);
                a.call_iat(IMP_SETFILEINFO);
            } else {
                a.call_iat(IMP_FLUSHFILEBUFFERS);
            }
            test_bool(a);
            a.jcc(CC_E, failed);
            a.mov_imm(RAX, 0);
            epilogue(a);
        }

        a.bind(bad_args);
        a.mov_imm(RAX, -22);
        epilogue(a);
        a.bind(failed);
        a.call(winerr);
        epilogue(a);

        a.bind(directory);
        emit_directory(a, winerr);
    }
}

fn prologue(a: &mut Asm) {
    a.win_prologue(64);
    for (offset, reg) in [(40, RDX), (48, R8), (56, R9), (64, R10)] {
        a.st(RSP, offset, reg);
    }
}

fn epilogue(a: &mut Asm) {
    for (offset, reg) in [(40, RDX), (48, R8), (56, R9), (64, R10)] {
        a.ld(reg, RSP, offset);
    }
    a.win_epilogue();
}

fn test_bool(a: &mut Asm) {
    a.raw(&[0x85, 0xc0]); // test eax, eax (Win32 BOOL)
}

// Each call consumes a batch of FILE_ID_BOTH_DIR_INFO records on the directory
// handle. Convert the entire batch to linux_dirent64 so the next call can resume
// without per-handle scratch state. Twice the Win32 buffer size is sufficient:
// the UTF-8 name may grow by at most 50%, and the Linux header is smaller.
fn emit_directory(a: &mut Asm, winerr: usize) {
    const BUFFER: i32 = 96;
    const CAPACITY: i32 = 8192;
    const RECORD: i32 = 72;
    const WRITTEN: i32 = 80;
    const NAME: i32 = 104;
    const ATTRIBUTES: i32 = 56;
    const NAME_LENGTH: i32 = 60;
    const FILE_ID: i32 = 96;

    let (bad_args, not_dir, eof, api_error, record, convert, done) = (
        a.label(),
        a.label(),
        a.label(),
        a.label(),
        a.label(),
        a.label(),
        a.label(),
    );
    prologue(a);
    a.alu_imm(5, RSP, CAPACITY);
    // The saved syscall registers live just above the extended frame.
    a.cmp_imm(RDX, 2048);
    a.jcc(CC_B, bad_args);

    a.mov_rr(RCX, RDI);
    a.mov_imm(RDX, 1); // FileStandardInfo
    a.lea(R8, RSP, BUFFER);
    a.mov_imm(R9, 24);
    a.call_iat(IMP_GETFILEINFOEX);
    test_bool(a);
    a.jcc(CC_E, api_error);
    a.movzx8_mem(RCX, RSP, BUFFER + 21); // FILE_STANDARD_INFO.Directory
    a.test_rr(RCX);
    a.jcc(CC_E, not_dir);

    a.mov_rr(RCX, RDI);
    a.mov_imm(RDX, 10); // FileIdBothDirectoryInfo (retains enumeration cursor)
    a.lea(R8, RSP, BUFFER);
    a.ld(R9, RSP, CAPACITY + 40); // available bytes in the Linux output
    a.raw(&[0x49, 0xd1, 0xe9]); // shr r9, 1
    a.mov_imm(RAX, CAPACITY as i64);
    a.alu(0x39, R9, RAX);
    a.cmov_rr(CC_A, R9, RAX);
    a.call_iat(IMP_GETFILEINFOEX);
    test_bool(a);
    a.jcc(CC_E, api_error);

    a.lea(RAX, RSP, BUFFER);
    a.st(RSP, RECORD, RAX);
    a.mov_imm(RAX, 0);
    a.st(RSP, WRITTEN, RAX);
    a.bind(record);
    a.ld(RAX, RSP, RECORD);
    a.lea(R8, RAX, NAME);
    a.ld32(R9, RAX, NAME_LENGTH);
    a.raw(&[0x49, 0xd1, 0xe9]); // byte length to number of UTF-16 units
    a.ld(RAX, RSP, WRITTEN);
    a.lea(RDX, RSI, 19); // d_name
    a.alu(0x01, RDX, RAX);
    a.st(RSP, 32, RDX);
    a.mov_rr(RDX, R9);
    a.raw(&[0x48, 0x6b, 0xd2, 3]); // worst case three UTF-8 bytes per UTF-16 unit
    a.st(RSP, 40, RDX);
    a.mov_imm(RAX, 0);
    a.st(RSP, 48, RAX); // lpDefaultChar / lpUsedDefaultChar are NULL for UTF-8
    a.st(RSP, 56, RAX);
    a.mov_imm(RCX, 65001);
    a.mov_imm(RDX, 0x80); // WC_ERR_INVALID_CHARS
    a.call_iat(IMP_WIDETOMULTI);
    test_bool(a);
    a.jcc(CC_NE, convert);
    a.jmp(api_error);

    a.bind(convert);
    a.raw(&[0x89, 0xc0]); // number of UTF-8 bytes
    a.ld(RCX, RSP, WRITTEN);
    a.alu(0x01, RCX, RSI); // current linux_dirent64
    a.lea(RDX, RCX, 19);
    a.alu(0x01, RDX, RAX);
    a.mov_imm(R8, 0);
    a.mov8_mem(RDX, 0, R8); // terminate d_name
    a.st(RCX, 8, R8); // d_off: Win32 retains its cursor, no numeric cookie
    a.add_imm(RAX, 27);
    a.and_imm(RAX, !7); // d_reclen = align8(19 + name length + 1)
    a.mov16_mem(RCX, 16, RAX);
    a.ld(RDX, RSP, WRITTEN);
    a.alu(0x01, RDX, RAX);
    a.st(RSP, WRITTEN, RDX);

    a.ld(RAX, RSP, RECORD);
    a.ld(RDX, RAX, FILE_ID);
    a.st(RCX, 0, RDX); // d_ino
    a.ld32(RDX, RAX, ATTRIBUTES);
    a.mov_rr(R8, RDX);
    a.and_imm(R8, 0x10); // FILE_ATTRIBUTE_DIRECTORY
    a.mov_imm(R8, 4); // DT_DIR
    a.mov_imm(R9, 8); // DT_REG
    a.cmov_rr(CC_NE, R9, R8);
    a.and_imm(RDX, 0x400); // FILE_ATTRIBUTE_REPARSE_POINT
    a.mov_imm(R8, 10); // DT_LNK
    a.cmov_rr(CC_NE, R9, R8);
    a.mov8_mem(RCX, 18, R9);
    a.ld32(RDX, RAX, 0); // NextEntryOffset
    a.test_rr(RDX);
    let finish_batch = a.label();
    a.jcc(CC_E, finish_batch);
    a.alu(0x01, RAX, RDX);
    a.st(RSP, RECORD, RAX);
    a.jmp(record);
    a.bind(finish_batch);
    a.ld(RAX, RSP, WRITTEN);
    a.jmp(done);

    a.bind(api_error);
    a.call_iat(IMP_GETLASTERROR);
    a.raw(&[0x89, 0xc0]);
    a.cmp_imm(RAX, 18); // ERROR_NO_MORE_FILES
    a.jcc(CC_E, eof);
    a.cmp_imm(RAX, 2); // ERROR_FILE_NOT_FOUND for empty enumeration
    a.jcc(CC_E, eof);
    a.call(winerr);
    a.jmp(done);
    a.bind(eof);
    a.mov_imm(RAX, 0);
    a.jmp(done);
    a.bind(not_dir);
    a.mov_imm(RAX, -20);
    a.jmp(done);
    a.bind(bad_args);
    a.mov_imm(RAX, -22);
    a.bind(done);
    a.alu_imm(0, RSP, CAPACITY);
    epilogue(a);
}
