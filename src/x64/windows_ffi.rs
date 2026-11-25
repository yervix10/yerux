//! Win64 calls to a subset of the Universal CRT with portable C signatures.
use super::*;

const CRT: [(&str, &str); 4] = [
    ("strlen", "strlen"),
    ("memset", "memset"),
    ("strtod", "strtod"),
    ("snprintf", "__stdio_common_vsprintf"),
];
const API_BASE: i64 = windows_relative::API_END;
const _: () = assert!(API_BASE + 8 * CRT.len() as i64 <= G_SIZE as i64);

fn calls(ir: &IrProgram) -> impl Iterator<Item = usize> + '_ {
    ir.funcs
        .iter()
        .flat_map(|f| f.blocks.iter())
        .flat_map(|b| &b.insts)
        .filter_map(|i| match i {
            Inst::ExternCall(_, id, _, _, _) => Some(*id),
            _ => None,
        })
}

fn symbol(name: &str) -> Option<usize> {
    CRT.iter()
        .position(|(name_in_yerux, _)| *name_in_yerux == name)
}

pub(super) fn validate(ir: &IrProgram) -> Result<(), String> {
    for id in calls(ir) {
        if let Some(link) = ir.links.first() {
            return Err(format!("extern C library '{link}'"));
        }
        if symbol(&ir.externs[id]).is_none() {
            return Err(format!("extern C symbol '{}'", ir.externs[id]));
        }
    }
    Ok(())
}

pub(super) struct Stubs {
    pub(super) labels: Vec<usize>,
    slots: [Option<usize>; CRT.len()],
}

impl Stubs {
    pub(super) fn new(a: &mut Asm, ir: &IrProgram) -> Self {
        let mut stubs = Self {
            labels: vec![0; ir.externs.len()],
            slots: [None; CRT.len()],
        };
        for id in calls(ir) {
            let slot = symbol(&ir.externs[id]).expect("validated Windows C symbol");
            stubs.labels[id] = *stubs.slots[slot].get_or_insert_with(|| a.label());
        }
        stubs
    }

    pub(super) fn emit_init(&self, a: &mut Asm, data: &mut Vec<Vec<u8>>) {
        if self.slots.iter().all(Option::is_none) {
            return;
        }
        let missing = a.label();
        let ready = a.label();
        let library = data.len();
        data.push(b"ucrtbase.dll\0".to_vec());
        a.lea_data(RCX, library);
        a.mov_imm(RDX, 0);
        a.mov_imm(R8, 0x800); // LOAD_LIBRARY_SEARCH_SYSTEM32
        a.call_iat(IMP_LOADLIBRARYEX);
        a.test_rax();
        a.jcc(CC_E, missing);
        a.mov_rr(RBX, RAX);
        for (index, (_, export)) in CRT.into_iter().enumerate() {
            if self.slots[index].is_none() {
                continue;
            }
            let name = data.len();
            data.push([export.as_bytes(), b"\0"].concat());
            a.mov_rr(RCX, RBX);
            a.lea_data(RDX, name);
            a.call_iat(IMP_GETPROCADDRESS);
            a.test_rax();
            a.jcc(CC_E, missing);
            a.gaddr(RCX, API_BASE + 8 * index as i64);
            a.st(RCX, 0, RAX);
        }
        a.jmp(ready);
        a.bind(missing);
        a.mov_imm(RCX, 127);
        a.call_iat(IMP_EXITPROCESS);
        a.bind(ready);
    }

    pub(super) fn emit(&self, a: &mut Asm) {
        for (index, label) in self.slots.into_iter().enumerate() {
            let Some(label) = label else { continue };
            let address = API_BASE + 8 * index as i64;
            a.bind(label);
            if CRT[index].0 == "snprintf" {
                // Win64 va_list points to the first variadic argument's home
                // slot. Later arguments are already contiguous on the stack.
                a.st(RSP, 32, R9);
                a.lea(R10, RSP, 32);
                a.win_prologue(16);
                a.st(RSP, 40, R10);
                a.mov_imm(RAX, 0); // current locale
                a.st(RSP, 32, RAX);
                a.mov_rr(R9, R8); // format
                a.mov_rr(R8, RDX); // buffer length
                a.mov_rr(RDX, RCX); // buffer
                a.mov_imm(RCX, 2); // _CRT_INTERNAL_PRINTF_STANDARD_SNPRINTF_BEHAVIOR
                a.gaddr(RAX, address);
                a.ld(RAX, RAX, 0);
                a.call_r(RAX);
                a.win_epilogue();
            } else {
                a.gaddr(RAX, address);
                a.ld(RAX, RAX, 0);
                a.raw(&[0xff, 0xe0]); // jmp rax, preserve the Win64 call frame
            }
        }
    }
}
