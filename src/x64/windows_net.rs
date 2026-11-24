//! Translate the Linux TCP calls used by std.net into Winsock calls.
use super::*;

const SYSCALLS: [i64; 9] = [41, 42, 43, 44, 49, 50, 51, 52, 54];

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

#[repr(i64)]
#[derive(Clone, Copy)]
enum Api {
    Startup,
    LastError,
    Socket,
    Close,
    Connect,
    Accept,
    Bind,
    Listen,
    LocalName,
    PeerName,
    SetOption,
    GetOption,
    Ioctl,
    Select,
    Recv,
    Send,
    Shutdown,
}

const SYMBOLS: [&[u8]; 17] = [
    b"WSAStartup\0",
    b"WSAGetLastError\0",
    b"socket\0",
    b"closesocket\0",
    b"connect\0",
    b"accept\0",
    b"bind\0",
    b"listen\0",
    b"getsockname\0",
    b"getpeername\0",
    b"setsockopt\0",
    b"getsockopt\0",
    b"ioctlsocket\0",
    b"select\0",
    b"recv\0",
    b"send\0",
    b"shutdown\0",
];

const API_BASE: i64 = G_USER + 8 * crate::layout::GLOBAL_CELLS;
const _: () = assert!(API_BASE + 8 * SYMBOLS.len() as i64 <= G_SIZE as i64);
const SOL_SOCKET: i64 = 0xffff;
const SO_ERROR: i64 = 0x1007;
const SO_RCVTIMEO: i64 = 0x1006;
const SO_SNDTIMEO: i64 = 0x1005;
const FIONBIO: i64 = 0x8004_667e;

fn call(a: &mut Asm, api: Api) {
    a.gaddr(RAX, API_BASE + 8 * api as i64);
    a.ld(RAX, RAX, 0);
    a.call_r(RAX);
}

pub(super) fn emit_init(a: &mut Asm, data: &mut Vec<Vec<u8>>) {
    let (failed, ready) = (a.label(), a.label());
    let library = data.len();
    data.push(b"ws2_32.dll\0".to_vec());
    a.lea_data(RCX, library);
    a.mov_imm(RDX, 0);
    a.mov_imm(R8, 0x800); // LOAD_LIBRARY_SEARCH_SYSTEM32
    a.call_iat(IMP_LOADLIBRARYEX);
    a.test_rax();
    a.jcc(CC_E, failed);
    a.mov_rr(RBX, RAX);
    for (slot, name) in SYMBOLS.iter().enumerate() {
        let symbol = data.len();
        data.push(name.to_vec());
        a.mov_rr(RCX, RBX);
        a.lea_data(RDX, symbol);
        a.call_iat(IMP_GETPROCADDRESS);
        a.test_rax();
        a.jcc(CC_E, failed);
        a.gaddr(RCX, API_BASE + 8 * slot as i64);
        a.st(RCX, 0, RAX);
    }

    a.alu_imm(5, RSP, 512); // WSADATA, beyond the shadow space
    a.mov_imm(RCX, 0x0202);
    a.lea(RDX, RSP, 32);
    call(a, Api::Startup);
    a.alu_imm(0, RSP, 512);
    test_eax(a);
    a.jcc(CC_E, ready);
    a.bind(failed);
    let msg = b"panic: could not initialize Winsock\n";
    let message = data.len();
    data.push(msg.to_vec());
    a.mov_imm(RDI, 2);
    a.lea_data(RSI, message);
    a.mov_imm(RDX, msg.len() as i64);
    a.write_rdi();
    a.exit(101);
    a.bind(ready);
}

pub(super) struct Stubs {
    calls: [usize; SYSCALLS.len()],
    recv: usize,
    close: usize,
}

impl Stubs {
    pub(super) fn new(a: &mut Asm) -> Self {
        Self {
            calls: SYSCALLS.map(|_| a.label()),
            recv: a.label(),
            close: a.label(),
        }
    }

    pub(super) fn emit_dispatch(&self, a: &mut Asm) {
        for (number, stub) in SYSCALLS.into_iter().zip(self.calls) {
            a.cmp_imm(RAX, number as i32);
            a.jcc(CC_E, stub);
        }
        let file = a.label();
        a.test_rr(RDI);
        a.jcc(CC_S, file);
        // Tag sockets with bit 62 so read/close can distinguish Win32 file handles
        // from Winsock sockets without changing std.net's descriptor type.
        a.raw(&[0x48, 0x0f, 0xba, 0xe7, 62]); // bt rdi, 62
        a.jcc(CC_AE, file);
        a.cmp_imm(RAX, 0);
        a.jcc(CC_E, self.recv);
        a.cmp_imm(RAX, 3);
        a.jcc(CC_E, self.close);
        a.bind(file);
    }

    pub(super) fn emit(&self, a: &mut Asm) {
        let [
            socket,
            connect,
            accept,
            send,
            bind,
            listen,
            local,
            peer,
            set,
        ] = self.calls;
        let (error, map_error, option, wait) = (a.label(), a.label(), a.label(), a.label());
        emit_error(a, error, map_error);
        emit_get_option(a, option, error);
        emit_wait(a, wait, error);

        a.bind(socket);
        prologue(a);
        a.mov_rr(RCX, RDI);
        let family = a.label();
        a.cmp_imm(RCX, 10); // Linux AF_INET6
        a.jcc(CC_NE, family);
        a.mov_imm(RCX, 23); // WinSock AF_INET6
        a.bind(family);
        a.mov_rr(R8, RDX);
        a.mov_rr(RDX, RSI);
        call(a, Api::Socket);
        socket_result(a, error);

        a.bind(self.close);
        emit_close(a, error);

        a.bind(connect);
        emit_connect(a, option, wait, error, map_error);

        a.bind(bind);
        prologue(a);
        input_address(a);
        call(a, Api::Bind);
        int_result(a, error);

        a.bind(listen);
        prologue(a);
        socket_arg(a);
        a.mov_rr(RDX, RSI);
        call(a, Api::Listen);
        int_result(a, error);

        for (label, api) in [
            (local, Api::LocalName),
            (peer, Api::PeerName),
            (accept, Api::Accept),
        ] {
            a.bind(label);
            prologue(a);
            if matches!(api, Api::Accept) {
                wait_readable(a, option, wait);
            }
            socket_arg(a);
            a.mov_rr(RDX, RSI);
            a.ld(R8, RSP, ARG2);
            call(a, api);
            let ok = a.label();
            if matches!(api, Api::Accept) {
                a.cmp_imm(RAX, -1); // INVALID_SOCKET is pointer-sized
                a.jcc(CC_NE, ok);
            } else {
                test_eax(a);
                a.jcc(CC_NS, ok);
            }
            a.call(error);
            epilogue(a);
            a.bind(ok);
            output_address(a);
            if matches!(api, Api::Accept) {
                tag_socket(a);
            } else {
                a.mov_imm(RAX, 0);
            }
            epilogue(a);
        }

        a.bind(set);
        emit_set_option(a, error);

        for (label, api) in [(self.recv, Api::Recv), (send, Api::Send)] {
            a.bind(label);
            prologue(a);
            a.ld(R8, RSP, ARG2);
            let done = a.label();
            a.mov_imm(RAX, 0);
            a.test_rr(R8);
            a.jcc(CC_E, done);
            if matches!(api, Api::Recv) {
                wait_readable(a, option, wait);
            }
            socket_arg(a);
            a.mov_rr(RDX, RSI);
            a.ld(R8, RSP, ARG2);
            a.mov_imm(R9, i32::MAX as i64);
            a.alu(0x39, R8, R9);
            a.cmov_rr(CC_A, R8, R9);
            a.mov_imm(R9, 0); // no Winsock flags, including Linux MSG_NOSIGNAL
            call(a, api);
            let ok = a.label();
            test_eax(a);
            a.jcc(CC_NS, ok);
            a.call(error);
            a.jmp(done);
            a.bind(ok);
            a.raw(&[0x89, 0xc0]); // zero-extend the returned int
            a.bind(done);
            epilogue(a);
        }
    }
}

// Locals in the syscall frame, after shadow space and the fifth API argument.
const ARG2: i32 = 40;
const ARG3: i32 = 48;
const ARG4: i32 = 56;
const ARG5: i32 = 64;
const ADDRESS: i32 = 72; // 32 bytes
const TIMEOUT: i32 = 104;
const RESULT: i32 = 112;
const VALUE: i32 = 120;

fn prologue(a: &mut Asm) {
    // win_prologue uses an imm8 adjustment. Allocate the remainder separately.
    a.win_prologue(80);
    a.alu_imm(5, RSP, 16);
    for (offset, reg) in [(ARG2, RDX), (ARG3, R10), (ARG4, R8), (ARG5, R9)] {
        a.st(RSP, offset, reg);
    }
}

fn epilogue(a: &mut Asm) {
    for (offset, reg) in [(ARG2, RDX), (ARG3, R10), (ARG4, R8), (ARG5, R9)] {
        a.ld(reg, RSP, offset);
    }
    a.win_epilogue();
}

fn test_eax(a: &mut Asm) {
    a.raw(&[0x85, 0xc0]);
}

fn socket_arg(a: &mut Asm) {
    a.mov_rr(RCX, RDI);
    a.raw(&[0x48, 0x0f, 0xba, 0xf1, 62]); // btr rcx, 62
}

fn tag_socket(a: &mut Asm) {
    a.raw(&[0x48, 0x0f, 0xba, 0xe8, 62]); // bts rax, 62
}

fn int_result(a: &mut Asm, error: usize) {
    let ok = a.label();
    test_eax(a);
    a.jcc(CC_NS, ok);
    a.call(error);
    a.bind(ok);
    epilogue(a);
}

fn socket_result(a: &mut Asm, error: usize) {
    let ok = a.label();
    a.cmp_imm(RAX, -1);
    a.jcc(CC_NE, ok);
    a.call(error);
    epilogue(a);
    a.bind(ok);
    tag_socket(a);
    epilogue(a);
}

fn input_address(a: &mut Asm) {
    let unchanged = a.label();
    socket_arg(a);
    a.mov_rr(RDX, RSI);
    a.ld(R8, RSP, ARG2);
    a.cmp_imm(R8, 28);
    a.jcc(CC_NE, unchanged);
    a.movzx16_mem(RAX, RSI, 0);
    a.cmp_imm(RAX, 10);
    a.jcc(CC_NE, unchanged);
    for offset in [0, 8, 16] {
        a.ld(RAX, RSI, offset);
        a.st(RSP, ADDRESS + offset, RAX);
    }
    a.ld32(RAX, RSI, 24);
    a.st32(RSP, ADDRESS + 24, RAX);
    a.mov_imm(RAX, 23);
    a.mov16_mem(RSP, ADDRESS, RAX);
    a.lea(RDX, RSP, ADDRESS);
    a.bind(unchanged);
}

fn output_address(a: &mut Asm) {
    let done = a.label();
    a.test_rr(RSI);
    a.jcc(CC_E, done);
    a.ld(RCX, RSP, ARG2);
    a.test_rr(RCX);
    a.jcc(CC_E, done);
    a.ld32(RCX, RCX, 0);
    a.cmp_imm(RCX, 2);
    a.jcc(CC_B, done);
    a.movzx16_mem(RCX, RSI, 0);
    a.cmp_imm(RCX, 23);
    a.jcc(CC_NE, done);
    a.mov_imm(RCX, 10);
    a.mov16_mem(RSI, 0, RCX);
    a.bind(done);
}

fn emit_error(a: &mut Asm, error: usize, map: usize) {
    a.bind(error);
    a.win_prologue(0);
    call(a, Api::LastError);
    a.win_restore_frame();
    a.bind(map); // eax contains a Winsock error, including SO_ERROR values
    a.raw(&[0x89, 0xc0]); // zero-extend int error codes
    for (wsa, errno) in [
        (10004, 4),
        (10009, 9),
        (10013, 13),
        (10014, 14),
        (10022, 22),
        (10024, 24),
        (10035, 11),
        (10036, 115),
        (10037, 114),
        (10038, 88),
        (10039, 89),
        (10040, 90),
        (10041, 91),
        (10042, 92),
        (10043, 93),
        (10044, 94),
        (10045, 95),
        (10046, 96),
        (10047, 97),
        (10048, 98),
        (10049, 99),
        (10050, 100),
        (10051, 101),
        (10052, 102),
        (10053, 103),
        (10054, 104),
        (10055, 105),
        (10056, 106),
        (10057, 107),
        (10058, 32),
        (10060, 110),
        (10061, 111),
        (10064, 112),
        (10065, 113),
    ] {
        let next = a.label();
        a.cmp_imm(RAX, wsa);
        a.jcc(CC_NE, next);
        a.mov_imm(RAX, -errno);
        a.ret();
        a.bind(next);
    }
    a.mov_imm(RAX, -5); // EIO
    a.ret();
}

// RCX = socket, R8 = option. Return the zero-extended DWORD or a negative errno.
fn emit_get_option(a: &mut Asm, label: usize, error: usize) {
    let failed = a.label();
    a.bind(label);
    a.win_prologue(32);
    a.mov_imm(RDX, SOL_SOCKET);
    a.lea(R9, RSP, 40);
    a.lea(RAX, RSP, 48);
    a.st(RSP, 32, RAX); // optlen
    a.mov_imm(RAX, 4);
    a.st32(RSP, 48, RAX);
    call(a, Api::GetOption);
    test_eax(a);
    a.jcc(CC_S, failed);
    a.ld32(RAX, RSP, 40);
    a.win_epilogue();
    a.bind(failed);
    a.call(error);
    a.win_epilogue();
}

// RCX = socket, RDX = nonzero milliseconds, R8 = 0 for read or 1 for connect.
// Return zero when ready and a negative errno otherwise.
fn emit_wait(a: &mut Asm, label: usize, error: usize) {
    const SET: i32 = 48;
    const EXCEPT: i32 = SET + 520; // fd_set: u32 count, padding, 64 SOCKETs
    const TIMEVAL: i32 = EXCEPT + 520;
    a.bind(label);
    a.win_prologue(0);
    a.alu_imm(5, RSP, TIMEVAL + 16 - 32);
    for set in [SET, EXCEPT] {
        a.mov_imm(RAX, 1);
        a.st(RSP, set, RAX);
        a.st(RSP, set + 8, RCX);
    }
    a.mov_rr(RAX, RDX);
    a.mov_imm(RDX, 0);
    a.mov_imm(RCX, 1000);
    a.f7(6, RCX); // unsigned div rcx
    a.st32(RSP, TIMEVAL, RAX);
    a.raw(&[0x48, 0x69, 0xd2]); // imul rdx, rdx, 1000
    a.imm32(1000);
    a.st32(RSP, TIMEVAL + 4, RDX);
    a.lea(RAX, RSP, TIMEVAL);
    a.st(RSP, 32, RAX); // timeval*
    a.mov_imm(RCX, 0); // ignored nfds
    a.mov_imm(RDX, 0);
    a.mov_imm(R9, 0);
    let write = a.label();
    let invoke = a.label();
    a.test_rr(R8);
    a.jcc(CC_NE, write);
    a.lea(RDX, RSP, SET);
    a.jmp(invoke);
    a.bind(write);
    a.lea(R8, RSP, SET);
    a.lea(R9, RSP, EXCEPT);
    a.bind(invoke);
    call(a, Api::Select);
    test_eax(a);
    let failed = a.label();
    let expired = a.label();
    a.jcc(CC_S, failed);
    a.jcc(CC_E, expired);
    a.mov_imm(RAX, 0);
    a.win_epilogue();
    a.bind(expired);
    a.mov_imm(RAX, -110);
    a.win_epilogue();
    a.bind(failed);
    a.call(error);
    a.win_epilogue();
}

fn wait_readable(a: &mut Asm, option: usize, wait: usize) {
    socket_arg(a);
    a.mov_imm(R8, SO_RCVTIMEO);
    a.call(option);
    let failed = a.label();
    let ready = a.label();
    a.test_rax();
    a.jcc(CC_S, failed);
    a.jcc(CC_E, ready); // zero means blocking
    socket_arg(a);
    a.mov_rr(RDX, RAX);
    a.mov_imm(R8, 0);
    a.call(wait);
    a.test_rax();
    a.jcc(CC_E, ready);
    a.bind(failed);
    epilogue(a);
    a.bind(ready);
}

fn nonblocking(a: &mut Asm, enabled: bool) {
    socket_arg(a);
    a.mov_imm(RDX, FIONBIO);
    a.lea(R8, RSP, VALUE);
    a.mov_imm(RAX, enabled as i64);
    a.st32(RSP, VALUE, RAX);
    call(a, Api::Ioctl);
}

fn emit_close(a: &mut Asm, error: usize) {
    const BUFFER: i32 = 4096;
    const FRAME: i32 = BUFFER + 32;
    let (close, drain, drained) = (a.label(), a.label(), a.label());
    prologue(a);
    socket_arg(a);
    a.mov_imm(RDX, 1); // SD_SEND: let queued data leave before the FIN
    call(a, Api::Shutdown);
    test_eax(a);
    a.jcc(CC_S, close); // includes unconnected and listening sockets

    // Closing with unread input can reset a connection and discard a response
    // the peer has not read yet. Drain data already queued, without waiting for
    // the peer or letting it force unbounded work in a destructor.
    nonblocking(a, true);
    test_eax(a);
    a.jcc(CC_S, close);
    a.alu_imm(5, RSP, FRAME);
    a.mov_imm(RAX, 16);
    a.st(RSP, FRAME + RESULT, RAX);
    a.bind(drain);
    socket_arg(a);
    a.lea(RDX, RSP, 32);
    a.mov_imm(R8, BUFFER as i64);
    a.mov_imm(R9, 0);
    call(a, Api::Recv);
    test_eax(a);
    a.jcc(CC_LE, drained);
    a.ld(RAX, RSP, FRAME + RESULT);
    a.sub_imm(RAX, 1);
    a.st(RSP, FRAME + RESULT, RAX);
    a.jcc(CC_NE, drain);
    a.bind(drained);
    a.alu_imm(0, RSP, FRAME);
    a.bind(close);
    socket_arg(a);
    call(a, Api::Close);
    int_result(a, error);
}

fn emit_connect(a: &mut Asm, option: usize, wait: usize, error: usize, map: usize) {
    let (blocking, failed, connected, restore, finish) =
        (a.label(), a.label(), a.label(), a.label(), a.label());
    prologue(a);
    socket_arg(a);
    a.mov_imm(R8, SO_SNDTIMEO);
    a.call(option);
    a.test_rax();
    a.jcc(CC_S, finish);
    a.jcc(CC_E, blocking);
    a.st(RSP, TIMEOUT, RAX);
    nonblocking(a, true);
    test_eax(a);
    a.jcc(CC_S, failed);
    input_address(a);
    call(a, Api::Connect);
    test_eax(a);
    a.jcc(CC_NS, connected);
    a.call(error);
    a.cmp_imm(RAX, -11); // WSAEWOULDBLOCK: handshake still in progress
    a.jcc(CC_NE, restore);

    socket_arg(a);
    a.ld(RDX, RSP, TIMEOUT);
    a.mov_imm(R8, 1);
    a.call(wait);
    a.test_rax();
    a.jcc(CC_S, restore);
    socket_arg(a);
    a.mov_imm(R8, SO_ERROR);
    a.call(option);
    a.test_rax();
    a.jcc(CC_S, restore);
    a.jcc(CC_E, connected);
    a.call(map);
    a.jmp(restore);

    a.bind(connected);
    a.mov_imm(RAX, 0);
    a.bind(restore);
    a.st(RSP, RESULT, RAX);
    nonblocking(a, false);
    test_eax(a);
    a.ld(RAX, RSP, RESULT);
    let check_result = a.label();
    a.jcc(CC_S, check_result);
    a.jmp(finish);
    a.bind(check_result);
    a.test_rax();
    a.jcc(CC_S, finish);

    a.bind(failed);
    a.call(error);
    a.jmp(finish);

    a.bind(blocking);
    input_address(a);
    call(a, Api::Connect);
    test_eax(a);
    a.jcc(CC_S, failed);
    a.mov_imm(RAX, 0);
    a.bind(finish);
    epilogue(a);
}

fn emit_set_option(a: &mut Asm, error: usize) {
    let (reuse, buffer, time, invoke, invalid) =
        (a.label(), a.label(), a.label(), a.label(), a.label());
    prologue(a);
    a.cmp_imm(RSI, 1); // Linux SOL_SOCKET
    a.jcc(CC_NE, invalid);
    a.cmp_imm(RDX, 2); // SO_REUSEADDR
    a.jcc(CC_E, reuse);
    a.cmp_imm(RDX, 7); // SO_SNDBUF
    a.jcc(CC_E, buffer);
    a.cmp_imm(RDX, 20); // SO_RCVTIMEO
    a.mov_imm(RAX, SO_RCVTIMEO);
    a.jcc(CC_E, time);
    a.cmp_imm(RDX, 21); // SO_SNDTIMEO
    a.mov_imm(RAX, SO_SNDTIMEO);
    a.jcc(CC_NE, invalid);
    a.bind(time);
    a.cmp_imm(R8, 16); // a Linux timeval contains two i64 fields
    a.jcc(CC_B, invalid);
    a.st(RSP, RESULT, RAX);
    a.ld(RAX, R10, 0); // seconds
    a.cmp_imm(RAX, (u32::MAX / 1000) as i32);
    a.jcc(CC_A, invalid);
    a.raw(&[0x48, 0x69, 0xc0]); // imul rax, rax, 1000
    a.imm32(1000);
    a.mov_rr(R8, RAX);
    a.ld(RAX, R10, 8); // microseconds
    a.cmp_imm(RAX, 999_999);
    a.jcc(CC_A, invalid);
    a.add_imm(RAX, 999); // round submillisecond waits up
    a.mov_imm(RDX, 0);
    a.mov_imm(RCX, 1000);
    a.f7(6, RCX);
    a.alu(0x01, RAX, R8);
    a.mov_imm(RCX, u32::MAX as i64);
    a.alu(0x39, RAX, RCX);
    a.jcc(CC_A, invalid);
    a.st32(RSP, VALUE, RAX);
    a.ld(R8, RSP, RESULT);
    a.lea(R9, RSP, VALUE);
    a.jmp(invoke);

    a.bind(reuse);
    // WinSock SO_REUSEADDR can steal a port from a live listener. Use exclusive
    // binding for the server instead, preserving the competing-bind failure.
    a.mov_imm(R8, !4u32 as i64); // SO_EXCLUSIVEADDRUSE
    let integer = a.label();
    a.jmp(integer);
    a.bind(buffer);
    a.mov_imm(R8, 0x1001); // WinSock SO_SNDBUF
    a.bind(integer);
    a.ld(RAX, RSP, ARG4);
    a.cmp_imm(RAX, 4);
    a.jcc(CC_B, invalid);
    a.mov_rr(R9, R10);

    a.bind(invoke);
    socket_arg(a);
    a.mov_imm(RDX, SOL_SOCKET);
    a.mov_imm(RAX, 4);
    a.st(RSP, 32, RAX);
    call(a, Api::SetOption);
    int_result(a, error);

    a.bind(invalid);
    a.mov_imm(RAX, -22);
    epilogue(a);
}
