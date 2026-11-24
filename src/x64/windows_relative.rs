//! The handle-relative filesystem operations used by std.fs.remove_dir_all.
use super::*;

const SYSCALLS: [i64; 2] = [257, 263];
pub(super) fn supports(number: i64) -> bool {
    SYSCALLS.contains(&number)
}

pub(super) fn reachable(ir: &IrProgram) -> bool {
    ir.funcs.iter().any(|f| {
        let numbers = crate::ir::syscall_constants(f);
        f.blocks
            .iter()
            .flat_map(|b| &b.insts)
            .any(|i| matches!(i, Inst::Syscall(_, args) if numbers[args[0]].is_some_and(supports)))
    })
}

// The sockets table follows user globals. Keep the NT file APIs after it.
const API_BASE: i64 = windows_net::API_END;
const NT_OPEN: i64 = API_BASE;
const NT_DISPOSITION: i64 = API_BASE + 8;
const NT_ERROR: i64 = API_BASE + 16;
const NT_QUERY: i64 = API_BASE + 24;
const WINE: i64 = API_BASE + 32;
const _: () = assert!(WINE + 8 <= G_SIZE as i64);

fn call(a: &mut Asm, slot: i64) {
    a.gaddr(RAX, slot);
    a.ld(RAX, RAX, 0);
    a.call_r(RAX);
}

pub(super) fn emit_init(a: &mut Asm, data: &mut Vec<Vec<u8>>) {
    let done = a.label();
    let library = data.len();
    data.push(b"ntdll.dll\0".to_vec());
    a.lea_data(RCX, library);
    a.call_iat(IMP_GETMODULEHANDLE);
    a.test_rax();
    a.jcc(CC_E, done);
    a.mov_rr(RBX, RAX);
    for (slot, name) in [
        (NT_OPEN, &b"NtOpenFile\0"[..]),
        (NT_DISPOSITION, &b"NtSetInformationFile\0"[..]),
        (NT_ERROR, &b"RtlNtStatusToDosError\0"[..]),
        (NT_QUERY, &b"NtQueryInformationFile\0"[..]),
        (WINE, &b"wine_get_version\0"[..]),
    ] {
        let symbol = data.len();
        data.push(name.to_vec());
        a.lea_data(RDX, symbol);
        a.mov_rr(RCX, RBX);
        a.call_iat(IMP_GETPROCADDRESS);
        a.gaddr(RCX, slot);
        a.st(RCX, 0, RAX);
    }
    a.bind(done);
}

pub(super) struct Stubs([usize; SYSCALLS.len()]);

impl Stubs {
    pub(super) fn new(a: &mut Asm) -> Self {
        Self(SYSCALLS.map(|_| a.label()))
    }

    pub(super) fn emit_dispatch(&self, a: &mut Asm) {
        for (number, label) in SYSCALLS.into_iter().zip(self.0) {
            a.cmp_imm(RAX, number as i32);
            a.jcc(CC_E, label);
        }
    }

    pub(super) fn emit(&self, a: &mut Asm, winerr: usize, dos_errno: usize) {
        const OPEN_DIR: i64 = 720896; // O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW
        const REMOVE_DIR: i64 = 512; // AT_REMOVEDIR
        const FRAME: i32 = 704;
        const MODE: i32 = 48;
        const SAVED: i32 = 56;
        const HANDLE: i32 = 88;
        const IO_STATUS: i32 = 96;
        const NAME: i32 = 112; // UNICODE_STRING
        const ATTR: i32 = 128; // OBJECT_ATTRIBUTES
        const INFO: i32 = 176; // FILE_ATTRIBUTE_TAG_INFO, then FILE_DISPOSITION_INFORMATION_EX
        const WIDE: i32 = 192; // 256 UTF-16 code units, including NUL

        let [open, unlink] = self.0;
        let invalid = a.label();
        let unsupported = a.label();
        let start = a.label();
        let next = a.label();
        let name_end = a.label();
        let prepared = a.label();
        let accessibility = a.label();
        let nt_error = a.label();
        let close_nt_error = a.label();
        let close_dos_error = a.label();
        let close_bad_type = a.label();
        let unsafe_alias = a.label();
        let checked = a.label();
        let check_wine_name = a.label();
        let close_success = a.label();
        let close = a.label();
        let done = a.label();
        let return_handle = a.label();

        for (label, deleting) in [(open, false), (unlink, true)] {
            a.bind(label);
            a.win_prologue(0);
            a.alu_imm(5, RSP, FRAME);
            for (offset, reg) in [(0, RDX), (8, R8), (16, R9), (24, R10)] {
                a.st(RSP, SAVED + offset, reg);
            }
            if deleting {
                a.cmp_imm(RDX, 0);
                a.jcc(CC_E, start);
                a.cmp_imm(RDX, REMOVE_DIR as i32);
                a.jcc(CC_E, start);
                a.jmp(invalid);
            } else {
                a.cmp_imm(RDX, OPEN_DIR as i32);
                a.jcc(CC_NE, invalid);
                a.jmp(start);
            }
        }

        a.bind(start);
        a.st(RSP, MODE, RDX);
        for slot in [NT_OPEN, NT_DISPOSITION, NT_ERROR] {
            a.gaddr(RAX, slot);
            a.ld(RAX, RAX, 0);
            a.test_rax();
            a.jcc(CC_E, unsupported);
        }

        // A single directory entry name. Never accept traversal, a drive or
        // an alternate data stream, even when invoked as a raw syscall.
        a.mov_rr(RCX, RSI);
        a.movzx8_mem(RAX, RCX, 0);
        a.test_rax();
        a.jcc(CC_E, invalid);
        let scan = a.label();
        a.cmp_imm(RAX, b'.' as i32);
        a.jcc(CC_NE, scan);
        a.movzx8_mem(RAX, RCX, 1);
        a.test_rax();
        a.jcc(CC_E, invalid);
        a.cmp_imm(RAX, b'.' as i32);
        a.jcc(CC_NE, scan);
        a.movzx8_mem(RAX, RCX, 2);
        a.test_rax();
        a.jcc(CC_E, invalid);
        a.bind(scan);
        a.bind(next);
        a.movzx8_mem(RAX, RCX, 0);
        a.test_rax();
        a.jcc(CC_E, name_end);
        for byte in [b'/', b'\\', b':'] {
            a.cmp_imm(RAX, byte as i32);
            a.jcc(CC_E, invalid);
        }
        a.add_imm(RCX, 1);
        a.jmp(next);

        a.bind(name_end);
        a.mov_imm(RCX, 65001); // UTF-8
        a.mov_imm(RDX, 8); // MB_ERR_INVALID_CHARS
        a.mov_rr(R8, RSI);
        a.mov_imm(R9, -1); // NUL-terminated
        a.lea(RAX, RSP, WIDE);
        a.st(RSP, 32, RAX);
        a.mov_imm(RAX, 256);
        a.st(RSP, 40, RAX);
        a.call_iat(IMP_MULTITOWIDE);
        test_eax(a);
        a.jcc(CC_NE, prepared);
        a.call(winerr);
        a.jmp(done);

        a.bind(prepared);
        a.alu(0x01, RAX, RAX); // bytes including NUL
        a.mov16_mem(RSP, NAME + 2, RAX);
        a.sub_imm(RAX, 2);
        a.mov16_mem(RSP, NAME, RAX);
        a.lea(RAX, RSP, WIDE);
        a.st(RSP, NAME + 8, RAX);
        a.mov_imm(RAX, 48);
        a.st(RSP, ATTR, RAX);
        a.st(RSP, ATTR + 8, RDI); // RootDirectory
        a.lea(RAX, RSP, NAME);
        a.st(RSP, ATTR + 16, RAX);
        a.mov_imm(RAX, 0x40); // OBJ_CASE_INSENSITIVE
        a.st(RSP, ATTR + 24, RAX);
        a.mov_imm(RAX, 0);
        a.st(RSP, ATTR + 32, RAX);
        a.st(RSP, ATTR + 40, RAX);
        a.lea(RCX, RSP, HANDLE);
        a.mov_imm(RDX, 0x110080); // SYNCHRONIZE | DELETE | FILE_READ_ATTRIBUTES
        a.ld(RAX, RSP, MODE);
        a.cmp_imm(RAX, OPEN_DIR as i32);
        a.jcc(CC_NE, accessibility);
        a.mov_imm(RDX, 0x100081); // SYNCHRONIZE | FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES
        a.bind(accessibility);
        a.lea(R8, RSP, ATTR);
        a.lea(R9, RSP, IO_STATUS);
        a.mov_imm(RAX, 7); // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
        a.st(RSP, 32, RAX);
        a.mov_imm(RAX, 0x200020); // FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT
        a.st(RSP, 40, RAX);
        call(a, NT_OPEN);
        test_eax(a);
        a.jcc(CC_S, nt_error);

        a.gaddr(RAX, WINE);
        a.ld(RAX, RAX, 0);
        a.test_rax();
        a.jcc(CC_E, checked);
        a.mov_rr(RCX, RDI);
        a.ld(RDX, RSP, HANDLE);
        a.lea(R8, RSP, WIDE);
        a.movzx16_mem(R9, RSP, NAME);
        a.call(check_wine_name);
        a.test_rax();
        a.jcc(CC_E, unsafe_alias);
        a.bind(checked);

        // A directory reparse point is treated like a symlink. It may be
        // unlinked itself but is never returned as a directory to traverse.
        a.ld(RCX, RSP, HANDLE);
        a.mov_imm(RDX, 9); // FileAttributeTagInfo
        a.lea(R8, RSP, INFO);
        a.mov_imm(R9, 8);
        a.call_iat(IMP_GETFILEINFOEX);
        test_eax(a);
        a.jcc(CC_E, close_dos_error);
        a.ld32(RAX, RSP, INFO);
        a.and_imm(RAX, 0x410); // FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT
        a.ld(RDX, RSP, MODE);
        a.test_rr(RDX);
        let expect_directory = a.label();
        let delete = a.label();
        a.jcc(CC_NE, expect_directory);
        a.cmp_imm(RAX, 0x10);
        a.jcc(CC_NE, delete);
        a.mov_imm(RAX, -21); // EISDIR
        a.jmp(close);
        a.bind(expect_directory);
        a.cmp_imm(RAX, 0x10);
        a.jcc(CC_NE, close_bad_type);
        a.cmp_imm(RDX, OPEN_DIR as i32);
        a.jcc(CC_E, return_handle);

        a.bind(delete);
        a.ld(RCX, RSP, HANDLE);
        a.lea(RDX, RSP, IO_STATUS);
        a.lea(R8, RSP, INFO);
        a.mov_imm(R9, 4);
        a.mov_imm(RAX, 3); // FILE_DISPOSITION_DELETE | FILE_DISPOSITION_POSIX_SEMANTICS
        a.st32(RSP, INFO, RAX);
        a.mov_imm(RAX, 64); // FileDispositionInformationEx (NT class)
        a.st(RSP, 32, RAX);
        call(a, NT_DISPOSITION);
        test_eax(a);
        a.jcc(CC_S, close_nt_error);
        a.jmp(close_success);

        a.bind(close_nt_error);
        nt_to_errno(a, dos_errno);
        a.jmp(close);
        a.bind(close_dos_error);
        a.call(winerr);
        a.jmp(close);
        a.bind(unsafe_alias);
        a.mov_imm(RAX, -40); // ELOOP
        a.jmp(close);
        a.bind(close_bad_type);
        a.mov_imm(RAX, -20); // ENOTDIR
        a.jmp(close);
        a.bind(close_success);
        a.mov_imm(RAX, 0);
        a.bind(close);
        a.st(RSP, INFO, RAX);
        a.ld(RCX, RSP, HANDLE);
        a.call_iat(IMP_CLOSEHANDLE);
        a.ld(RAX, RSP, INFO);
        a.jmp(done);
        a.bind(return_handle);
        a.ld(RAX, RSP, HANDLE);
        a.jmp(done);
        a.bind(nt_error);
        nt_to_errno(a, dos_errno);
        a.jmp(done);
        a.bind(invalid);
        a.mov_imm(RAX, -22);
        a.jmp(done);
        a.bind(unsupported);
        a.mov_imm(RAX, -38); // ENOSYS
        a.bind(done);
        for (offset, reg) in [(0, RDX), (8, R8), (16, R9), (24, R10)] {
            a.ld(reg, RSP, SAVED + offset);
        }
        a.win_epilogue();

        emit_check_wine_name(a, check_wine_name);
    }
}

// Wine can follow a host Unix symlink even with FILE_OPEN_REPARSE_POINT.
// Reject handles whose resolved path does not consist of the resolved parent
// plus the requested entry. Native Windows uses the reparse-point handle itself.
fn emit_check_wine_name(a: &mut Asm, label: usize) {
    const CAPACITY: i32 = 8192;
    const PARENT_NAME: i32 = 96;
    const CHILD_NAME: i32 = PARENT_NAME + CAPACITY;
    const FRAME: i32 = CHILD_NAME + CAPACITY;
    let (bad, prefix, separator, suffix, ok, done) = (
        a.label(),
        a.label(),
        a.label(),
        a.label(),
        a.label(),
        a.label(),
    );

    a.bind(label);
    a.win_prologue(0);
    a.alu_imm(5, RSP, FRAME);
    for (offset, reg) in [(40, RDX), (48, R8), (56, R9)] {
        a.st(RSP, offset, reg);
    }
    a.gaddr(RAX, NT_QUERY);
    a.ld(RAX, RAX, 0);
    a.test_rax();
    a.jcc(CC_E, bad);
    for offset in [PARENT_NAME, CHILD_NAME] {
        if offset == CHILD_NAME {
            a.ld(RCX, RSP, 40);
        }
        a.lea(RDX, RSP, 64); // IO_STATUS_BLOCK
        a.lea(R8, RSP, offset);
        a.mov_imm(R9, CAPACITY as i64);
        a.mov_imm(RAX, 9); // FileNameInformation
        a.st(RSP, 32, RAX);
        call(a, NT_QUERY);
        test_eax(a);
        a.jcc(CC_S, bad);
    }

    a.ld32(R8, RSP, PARENT_NAME);
    a.ld32(R9, RSP, CHILD_NAME);
    a.lea(RCX, RSP, PARENT_NAME + 4);
    a.lea(RDX, RSP, CHILD_NAME + 4);
    a.cmp_imm(R8, 0);
    a.jcc(CC_E, separator);
    a.alu(0x39, R8, R9);
    a.jcc(CC_AE, bad);
    a.alu(0x29, R9, R8);
    a.bind(prefix);
    a.movzx16_mem(RAX, RCX, 0);
    a.movzx16_mem(R10, RDX, 0);
    a.alu(0x39, RAX, R10);
    a.jcc(CC_NE, bad);
    a.add_imm(RCX, 2);
    a.add_imm(RDX, 2);
    a.sub_imm(R8, 2);
    a.jcc(CC_NE, prefix);
    a.cmp_imm(RAX, b'\\' as i32);
    let length = a.label();
    a.jcc(CC_E, length);
    a.bind(separator);
    a.cmp_imm(R9, 2);
    a.jcc(CC_BE, bad);
    a.movzx16_mem(RAX, RDX, 0);
    a.cmp_imm(RAX, b'\\' as i32);
    a.jcc(CC_NE, bad);
    a.add_imm(RDX, 2);
    a.sub_imm(R9, 2);
    a.bind(length);
    a.ld(R8, RSP, 56);
    a.alu(0x39, R9, R8);
    a.jcc(CC_NE, bad);
    a.ld(RCX, RSP, 48);
    a.bind(suffix);
    a.test_rr(R8);
    a.jcc(CC_E, ok);
    a.movzx16_mem(RAX, RCX, 0);
    a.movzx16_mem(R10, RDX, 0);
    a.alu(0x39, RAX, R10);
    a.jcc(CC_NE, bad);
    a.add_imm(RCX, 2);
    a.add_imm(RDX, 2);
    a.sub_imm(R8, 2);
    a.jmp(suffix);

    a.bind(ok);
    a.mov_imm(RAX, 1);
    a.jmp(done);
    a.bind(bad);
    a.mov_imm(RAX, 0);
    a.bind(done);
    a.win_epilogue();
}

fn test_eax(a: &mut Asm) {
    a.raw(&[0x85, 0xc0]);
}

fn nt_to_errno(a: &mut Asm, dos_errno: usize) {
    a.mov_rr(RCX, RAX);
    call(a, NT_ERROR);
    a.call(dos_errno);
}
