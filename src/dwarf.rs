// Minimal DWARF 4 + symbol table for the native x86-64 Linux backend, so gdb can show Yerux functions, `.yx`
// lines, backtraces and variables:
//   .symtab/.strtab   one FUNC symbol per Yerux function
//   .debug_abbrev/.debug_info   one compile unit with function, type and variable descriptions
//   .debug_line       file:line:column rows, one sequence per function
//   .debug_frame      call frame information: rbp-based frames, with the three prologue states and the epilogue
// The sections are appended after the loaded image (not part of any PT_LOAD) with a section header table, so the
// program itself can run without the sections. .debug_ranges and .debug_loc describe scoped locals
// and parameter locations once their frame slots have been populated.

use crate::ir::{native_elem_bytes, native_words, DebugBinding, DebugBindings, IrProgram};
use crate::numeric::Nk;
use crate::syntax::{Ty, Types};
use std::collections::HashMap;

/// Frame state of the code at an offset (see `x64::gen_func`).
pub const CFA_ENTRY: u8 = 0; // CFA = rsp + 8: the return address is on top of the stack
pub const CFA_PUSHED: u8 = 1; // after `push rbp`: CFA = rsp + 16
pub const CFA_FRAME: u8 = 2; // after `mov rbp, rsp`: CFA = rbp + 16

pub struct FuncInfo {
    pub name: String,
    pub start: usize,
    pub end: usize,
    /// (code offset, state), ascending; the first row is at `start`
    pub cfi: Vec<(usize, u8)>,
    /// Frame slots relative to rbp, and source scope changes in code order.
    pub slot_off: Vec<i32>,
    pub scopes: Vec<(usize, usize)>,
    pub decl_loc: u32,
}

#[derive(Default)]
pub struct DebugInfo {
    pub funcs: Vec<FuncInfo>,
    /// (code offset, location id): the instruction at the offset belongs to that source position
    pub lines: Vec<(usize, u32)>,
}

fn uleb(o: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            o.push(b);
            return;
        }
        o.push(b | 0x80);
    }
}

fn sleb(o: &mut Vec<u8>, mut v: i64) {
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        let done = (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0);
        if done {
            o.push(b);
            return;
        }
        o.push(b | 0x80);
    }
}

fn cstr(o: &mut Vec<u8>, s: &str) {
    o.extend_from_slice(s.as_bytes());
    o.push(0);
}

/// `len32 | body`
fn with_len(body: Vec<u8>) -> Vec<u8> {
    let mut o = (body.len() as u32).to_le_bytes().to_vec();
    o.extend(body);
    o
}

// Abbreviation codes (DWARF tags and attribute forms are defined in debug_abbrev).
const CU: u64 = 1;
const FUNCTION: u64 = 2;
const PARAMETER: u64 = 3;
const LOCAL: u64 = 4;
const SCOPE: u64 = 5;
const BASE_TYPE: u64 = 6;
const STRUCT_TYPE: u64 = 7;
const MEMBER: u64 = 8;
const ENUM_TYPE: u64 = 9;
const ENUMERATOR: u64 = 10;
const POINTER: u64 = 11;
const ARRAY: u64 = 12;
const SUBRANGE: u64 = 13;
const VARIANT_PART: u64 = 14;
const VARIANT: u64 = 15;

fn debug_abbrev() -> Vec<u8> {
    let mut o = vec![];
    let mut add = |code, tag, children, attrs: &[(u64, u64)]| {
        uleb(&mut o, code);
        uleb(&mut o, tag);
        o.push(u8::from(children));
        for &(attr, form) in attrs {
            uleb(&mut o, attr);
            uleb(&mut o, form);
        }
        o.extend_from_slice(&[0, 0]);
    };
    let name = (0x03, 0x08); // string
    let ty = (0x49, 0x13); // ref4
    let size = (0x0b, 0x0f); // udata
    let pc = [(0x11, 0x01), (0x12, 0x07)]; // low_pc addr, high_pc data8
    add(CU, 0x11, true, &[(0x25, 0x08), (0x13, 0x05), name, (0x1b, 0x08), pc[0], pc[1], (0x10, 0x17)]);
    add(FUNCTION, 0x2e, true, &[name, pc[0], pc[1], (0x3f, 0x19), (0x40, 0x18)]);
    add(PARAMETER, 0x05, false, &[name, ty, (0x02, 0x17)]); // location sec_offset
    add(LOCAL, 0x34, false, &[name, ty, (0x02, 0x18)]); // location exprloc
    add(SCOPE, 0x0b, true, &[(0x55, 0x17)]); // ranges sec_offset
    add(BASE_TYPE, 0x24, false, &[name, size, (0x3e, 0x0b)]); // encoding data1
    add(STRUCT_TYPE, 0x13, true, &[name, size]);
    add(MEMBER, 0x0d, false, &[name, ty, (0x38, 0x0f)]); // data_member_location udata
    add(ENUM_TYPE, 0x04, true, &[name, size, ty]);
    add(ENUMERATOR, 0x28, false, &[name, (0x1c, 0x0f)]); // const_value udata
    add(POINTER, 0x0f, false, &[ty, size]);
    add(ARRAY, 0x01, true, &[ty, (0x51, 0x0f)]); // byte_stride udata
    add(SUBRANGE, 0x21, false, &[(0x37, 0x0f)]); // count udata
    add(VARIANT_PART, 0x33, true, &[(0x15, 0x13)]); // discr ref4 to the tag member
    add(VARIANT, 0x19, true, &[(0x16, 0x0f)]); // discr_value udata
    o.push(0);
    o
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum DebugType {
    Value(Ty),
    Pointer(Ty),
    BufferElement(Ty),
    EnumTag(usize),
    Payload(usize, usize),
    TextRecord,
    OpaquePointer,
}

// gdb's C string printer reads byte-sized array elements as contiguous chars. Use
// the native word width for packed-into-words integers, including UInt8.
fn buffer_element(ty: Ty) -> DebugType {
    match ty {
        Ty::Num(n) if !n.is_float() && n.bits() < 64 => DebugType::BufferElement(ty),
        _ => DebugType::Value(ty),
    }
}

struct Dies<'a> {
    bytes: Vec<u8>,
    types: &'a Types,
    offsets: HashMap<DebugType, u32>,
    pending: Vec<(usize, DebugType)>,
}

impl Dies<'_> {
    fn offset(&self) -> u32 {
        // DW_FORM_ref4 is relative to the CU header, including the initial length.
        self.bytes.len() as u32 + 4
    }

    fn start(&mut self, abbrev: u64) {
        uleb(&mut self.bytes, abbrev);
    }

    fn named(&mut self, abbrev: u64, name: &str) {
        self.start(abbrev);
        cstr(&mut self.bytes, name);
    }

    fn udata(&mut self, value: usize) {
        uleb(&mut self.bytes, value as u64);
    }

    fn type_ref(&mut self, ty: DebugType) {
        self.pending.push((self.bytes.len(), ty));
        self.bytes.extend_from_slice(&[0; 4]);
    }

    fn base(&mut self, name: &str, size: usize, encoding: u8) {
        self.named(BASE_TYPE, name);
        self.udata(size);
        self.bytes.push(encoding);
    }

    fn member(&mut self, name: &str, ty: DebugType, offset: usize) {
        self.named(MEMBER, name);
        self.type_ref(ty);
        self.udata(offset);
    }

    fn record(&mut self, name: &str, size: usize) {
        self.named(STRUCT_TYPE, name);
        self.udata(size);
    }

    fn fields<'a>(&mut self, fields: impl IntoIterator<Item = (&'a str, Ty)>) {
        let mut offset = 0;
        for (name, ty) in fields {
            let size = 8 * native_words(self.types, ty);
            if size > 0 {
                self.member(name, DebugType::Value(ty), offset);
            }
            offset += size;
        }
    }

    fn enumeration(&mut self, name: &str, variants: &[(String, Vec<Ty>)]) {
        self.named(ENUM_TYPE, name);
        self.udata(8);
        self.type_ref(DebugType::Value(Ty::Num(Nk::U64)));
        for (index, (name, _)) in variants.iter().enumerate() {
            self.named(ENUMERATOR, name);
            self.udata(index);
        }
        self.bytes.push(0);
    }

    fn emit_type(&mut self, kind: DebugType) {
        use DebugType::*;
        const INT: DebugType = Value(Ty::Int);
        const BYTE: Ty = Ty::Num(Nk::U8);
        let types = self.types;
        match kind {
            Pointer(ty) => {
                self.start(POINTER);
                self.type_ref(Value(ty));
                self.udata(8);
            }
            BufferElement(Ty::Num(n)) => {
                self.base(n.name(), 8, if n.signed() { 0x05 } else { 0x07 });
            }
            BufferElement(_) => unreachable!("only narrow integers need a separate buffer type"),
            OpaquePointer => self.base("Address", 8, 0x01), // DW_ATE_address
            TextRecord => {
                self.record("Text", 8);
                self.member("len", INT, 0);
                self.bytes.push(0);
            }
            EnumTag(id) => {
                let def = &types.enums[id];
                self.enumeration(&format!("{}Tag", def.name), &def.variants);
            }
            Payload(id, variant) => {
                let def = &types.enums[id];
                let (name, fields) = &def.variants[variant];
                let size = fields.iter().map(|&ty| 8 * native_words(types, ty)).sum();
                self.record(&format!("{}::{name}", def.name), size);
                let names: Vec<_> = (0..fields.len()).map(|i| format!("_{i}")).collect();
                self.fields(names.iter().map(String::as_str).zip(fields.iter().copied()));
                self.bytes.push(0);
            }
            Value(ty) => match ty {
                Ty::Int | Ty::Range(..) => self.base(&types.name(ty), 8, 0x05), // signed
                Ty::Char => self.base("Char", 8, 0x07), // unsigned Unicode scalar
                Ty::Bool => self.base("Bool", 1, 0x02), // boolean
                Ty::Num(n) => {
                    let encoding = if n.is_float() { 0x04 } else if n.signed() { 0x05 } else { 0x07 };
                    self.base(n.name(), n.bits() as usize / 8, encoding);
                }
                Ty::Ptr(p) => self.emit_type(Pointer(types.ptrs[p])),
                Ty::Box(p) => self.emit_type(Pointer(types.boxes[p])),
                Ty::Text => {
                    self.start(POINTER);
                    self.type_ref(TextRecord);
                    self.udata(8);
                }
                Ty::Struct(id) => {
                    if let Some((elem, count)) = types.array_of(ty) {
                        self.start(ARRAY);
                        self.type_ref(buffer_element(elem));
                        self.udata(native_elem_bytes(types, elem));
                        self.start(SUBRANGE);
                        self.udata(count);
                    } else {
                        let def = &types.structs[id];
                        self.record(&def.name, 8 * native_words(types, ty));
                        self.fields(def.fields.iter().map(|(name, ty)| (name.as_str(), *ty)));
                    }
                    self.bytes.push(0);
                }
                Ty::Enum(id) => {
                    let def = &types.enums[id];
                    if def.variants.iter().all(|(_, fields)| fields.is_empty()) {
                        self.enumeration(&def.name, &def.variants);
                        return;
                    }
                    self.record(&def.name, 8 * native_words(types, ty));
                    let tag = self.offset();
                    self.member("tag", EnumTag(id), 0);
                    self.start(VARIANT_PART);
                    self.bytes.extend_from_slice(&tag.to_le_bytes());
                    for (index, (name, fields)) in def.variants.iter().enumerate() {
                        self.start(VARIANT);
                        self.udata(index);
                        if !fields.is_empty() {
                            self.member(name, Payload(id, index), 8);
                        }
                        self.bytes.push(0);
                    }
                    self.bytes.extend_from_slice(&[0, 0]); // variant part, struct
                }
                Ty::String | Ty::List(_) => {
                    let data = match ty {
                        Ty::List(_) => OpaquePointer,
                        _ => Pointer(BYTE),
                    };
                    self.record(&types.name(ty), 24);
                    self.member("data", data, 0);
                    self.member("len", INT, 8);
                    self.member("cap", INT, 16);
                    self.bytes.push(0);
                }
                Ty::Fn(_) | Ty::Dyn(_) => {
                    self.record(&types.name(ty), 16);
                    let fields = if matches!(ty, Ty::Fn(_)) { ["code", "env"] } else { ["data", "vtable"] };
                    self.member(fields[0], OpaquePointer, 0);
                    self.member(fields[1], OpaquePointer, 8);
                    self.bytes.push(0);
                }
                _ => self.base(&types.name(ty), 0, 0x07),
            },
        }
    }

    fn finish_types(&mut self) {
        let mut index = 0;
        while let Some(&(at, ty)) = self.pending.get(index) {
            let offset = if let Some(&offset) = self.offsets.get(&ty) {
                offset
            } else {
                let offset = self.offset();
                self.offsets.insert(ty, offset);
                self.emit_type(ty);
                offset
            };
            self.bytes[at..at + 4].copy_from_slice(&offset.to_le_bytes());
            index += 1;
        }
    }
}

type PcRange = (usize, usize);

fn append_range(ranges: &mut Vec<PcRange>, start: usize, end: usize) {
    if let Some((_, prev_end)) = ranges.last_mut().filter(|range| range.1 == start) {
        *prev_end = end;
    } else {
        ranges.push((start, end));
    }
}

fn scope_ranges(f: &FuncInfo, bindings: &DebugBindings) -> Vec<Vec<PcRange>> {
    let mut ranges = vec![vec![]; bindings.locals.len() + 2];
    for (i, &(start, mut scope)) in f.scopes.iter().enumerate() {
        let end = f.scopes.get(i + 1).map_or(f.end, |&(offset, _)| offset);
        if start == end {
            continue;
        }
        while scope != 0 {
            append_range(&mut ranges[scope], start, end);
            scope = if scope == 1 { 0 } else { bindings.locals[scope - 2].0 };
        }
    }
    ranges
}

fn pc_range(bytes: &mut Vec<u8>, (start, end): PcRange) {
    // DWARF 4 ranges and location entries are relative to the compile unit's low_pc.
    bytes.extend_from_slice(&(start as u64).to_le_bytes());
    bytes.extend_from_slice(&(end as u64).to_le_bytes());
}

fn frame_expr(f: &FuncInfo, binding: &DebugBinding) -> Vec<u8> {
    let mut expr = vec![0x91]; // DW_OP_fbreg
    sleb(&mut expr, i64::from(f.slot_off[binding.slot]) - 16); // CFA = rbp + 16
    if binding.indirect {
        expr.push(0x06); // DW_OP_deref: saved address of the actual value
    }
    expr
}

fn function_bindings(
    dies: &mut Dies,
    f: &FuncInfo,
    bindings: &DebugBindings,
    ranges: &mut Vec<u8>,
    locs: &mut Vec<u8>,
) {
    let by_scope = scope_ranges(f, bindings);
    for param in &bindings.params {
        dies.named(PARAMETER, &param.name);
        dies.type_ref(DebugType::Value(param.ty));
        dies.bytes.extend_from_slice(&(locs.len() as u32).to_le_bytes());
        let expr = frame_expr(f, param);
        for &range in &by_scope[1] {
            pc_range(locs, range);
            locs.extend_from_slice(&(expr.len() as u16).to_le_bytes());
            locs.extend_from_slice(&expr);
        }
        pc_range(locs, (0, 0));
    }

    let mut children = vec![vec![]; by_scope.len()];
    for (i, &(parent, _)) in bindings.locals.iter().enumerate() {
        if !by_scope[i + 2].is_empty() {
            children[parent].push(i + 2);
        }
    }
    // Zero on the work stack closes a scope DIE. Iterate so deeply nested declarations
    // do not require a matching depth of Rust function calls.
    let mut work: Vec<_> = children[1].iter().rev().copied().collect();
    while let Some(scope) = work.pop() {
        if scope == 0 {
            dies.bytes.push(0);
            continue;
        }
        let binding = &bindings.locals[scope - 2].1;
        dies.start(SCOPE);
        dies.bytes.extend_from_slice(&(ranges.len() as u32).to_le_bytes());
        for &range in &by_scope[scope] {
            pc_range(ranges, range);
        }
        pc_range(ranges, (0, 0));
        dies.named(LOCAL, &binding.name);
        dies.type_ref(DebugType::Value(binding.ty));
        let expr = frame_expr(f, binding);
        dies.udata(expr.len());
        dies.bytes.extend(expr);
        work.push(0);
        work.extend(children[scope].iter().rev().copied());
    }
}

fn debug_info(
    d: &DebugInfo,
    ir: &IrProgram,
    va: u64,
    text_len: usize,
    name: &str,
    comp_dir: &str,
) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let fallback = Types::default();
    let types = ir.debug.as_ref().map_or(&fallback, |debug| &debug.types);
    let mut dies = Dies {
        bytes: vec![],
        types,
        offsets: HashMap::new(),
        pending: vec![],
    };
    let mut ranges = vec![];
    let mut locs = vec![];
    let b = &mut dies.bytes;
    b.extend_from_slice(&4u16.to_le_bytes()); // version
    b.extend_from_slice(&0u32.to_le_bytes()); // abbrev offset
    b.push(8); // address size
    uleb(b, CU);
    cstr(b, "yerux native backend");
    b.extend_from_slice(&0x0002u16.to_le_bytes()); // DW_LANG_C: gdb field and expression syntax
    cstr(b, name);
    cstr(b, comp_dir);
    b.extend_from_slice(&va.to_le_bytes());
    b.extend_from_slice(&(text_len as u64).to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes()); // stmt_list: the only line program
    for (index, f) in d.funcs.iter().enumerate() {
        dies.named(FUNCTION, &f.name);
        dies.bytes.extend_from_slice(&(va + f.start as u64).to_le_bytes());
        dies.bytes.extend_from_slice(&((f.end - f.start) as u64).to_le_bytes());
        dies.bytes.extend_from_slice(&[1, 0x9c]); // frame_base: DW_OP_call_frame_cfa
        if let Some(bindings) = ir.debug.as_ref().and_then(|debug| debug.functions.get(index)) {
            function_bindings(&mut dies, f, bindings, &mut ranges, &mut locs);
        }
        dies.bytes.push(0); // end of function children
    }
    dies.finish_types();
    dies.bytes.push(0); // end of compilation unit children
    (with_len(dies.bytes), ranges, locs)
}

fn debug_line(d: &DebugInfo, locs: &[(u32, u32, u32)], files: &[String], va: u64) -> Vec<u8> {
    const LINE_BASE: i64 = -5;
    const LINE_RANGE: i64 = 14;
    const OPCODE_BASE: u8 = 13;
    let mut hdr = vec![];
    hdr.push(1); // minimum instruction length
    hdr.push(1); // maximum operations per instruction
    hdr.push(1); // default is_stmt
    hdr.push(LINE_BASE as i8 as u8);
    hdr.push(LINE_RANGE as u8);
    hdr.push(OPCODE_BASE);
    hdr.extend_from_slice(&[0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1]); // standard opcode lengths
    hdr.push(0); // no include directories
    for f in files {
        cstr(&mut hdr, f);
        hdr.extend_from_slice(&[0, 0, 0]); // directory, mtime, length
    }
    hdr.push(0);
    let mut prog = vec![];
    // one sequence per function: its rows are the ones whose offset falls inside it
    let mut rows = d.lines.iter().peekable();
    for f in &d.funcs {
        while rows.peek().is_some_and(|(o, _)| *o < f.start) {
            rows.next();
        }
        let mut first = true;
        let mut prologue_end = f.decl_loc != 0;
        let (mut addr, mut file, mut line, mut col) = (0usize, 1i64, 1i64, 0i64);
        let mut any = false;
        while let Some((o, l)) = rows.peek().copied() {
            if *o >= f.end {
                break;
            }
            rows.next();
            let Some(&(lf, ll, lc)) = locs.get(*l as usize) else { continue };
            if ll == 0 {
                continue;
            }
            if first {
                prog.extend_from_slice(&[0, 9, 2]); // DW_LNE_set_address
                prog.extend_from_slice(&(va + *o as u64).to_le_bytes());
                addr = *o;
                first = false;
            }
            any = true;
            if *o > addr {
                prog.push(2); // DW_LNS_advance_pc
                uleb(&mut prog, (*o - addr) as u64);
                addr = *o;
            }
            if lf as i64 + 1 != file {
                prog.push(4); // DW_LNS_set_file
                uleb(&mut prog, lf as u64 + 1);
                file = lf as i64 + 1;
            }
            if lc as i64 != col {
                prog.push(5); // DW_LNS_set_column
                uleb(&mut prog, lc as u64);
                col = lc as i64;
            }
            if ll as i64 != line {
                prog.push(3); // DW_LNS_advance_line
                sleb(&mut prog, ll as i64 - line);
                line = ll as i64;
            }
            if prologue_end && *o > f.start {
                prog.push(10); // DW_LNS_set_prologue_end
                prologue_end = false;
            }
            prog.push(1); // DW_LNS_copy
        }
        if any {
            prog.push(2);
            uleb(&mut prog, (f.end - addr) as u64);
            prog.extend_from_slice(&[0, 1, 1]); // DW_LNE_end_sequence
        }
    }
    let _ = (LINE_BASE, LINE_RANGE);
    let mut b = vec![];
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&(hdr.len() as u32).to_le_bytes());
    b.extend(hdr);
    b.extend(prog);
    with_len(b)
}

fn debug_frame(d: &DebugInfo, va: u64) -> Vec<u8> {
    let mut o = vec![];
    // CIE
    let mut cie = vec![];
    cie.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    cie.push(1); // version
    cie.push(0); // augmentation ""
    uleb(&mut cie, 1); // code alignment
    sleb(&mut cie, -8); // data alignment
    uleb(&mut cie, 16); // return address register
    cie.extend_from_slice(&[0x0C, 7, 8]); // def_cfa rsp, 8
    cie.extend_from_slice(&[0x80 | 16, 1]); // offset r16 (rip) at cfa-8
    while (cie.len() + 4) % 8 != 0 {
        cie.push(0); // DW_CFA_nop padding
    }
    o.extend(with_len(cie));
    for f in &d.funcs {
        let mut fde = vec![];
        fde.extend_from_slice(&0u32.to_le_bytes()); // CIE pointer: offset 0
        fde.extend_from_slice(&(va + f.start as u64).to_le_bytes());
        fde.extend_from_slice(&((f.end - f.start) as u64).to_le_bytes());
        let mut at = f.start;
        let mut cur: Option<u8> = None;
        for &(off, st) in &f.cfi {
            if cur == Some(st) {
                continue;
            }
            let delta = off.saturating_sub(at);
            if delta > 0 {
                if delta < 64 {
                    fde.push(0x40 | delta as u8);
                } else if delta < 256 {
                    fde.extend_from_slice(&[0x02, delta as u8]);
                } else if delta < 65536 {
                    fde.push(0x03);
                    fde.extend_from_slice(&(delta as u16).to_le_bytes());
                } else {
                    fde.push(0x04);
                    fde.extend_from_slice(&(delta as u32).to_le_bytes());
                }
                at = off;
            }
            match st {
                CFA_ENTRY => {
                    fde.extend_from_slice(&[0x0C, 7, 8]); // def_cfa rsp, 8
                    fde.push(0xC0 | 6); // rbp: the caller's value again (after `leave`) or never changed
                }
                CFA_PUSHED => {
                    fde.extend_from_slice(&[0x0E, 16]); // def_cfa_offset 16
                    fde.extend_from_slice(&[0x80 | 6, 2]); // rbp saved at cfa-16
                }
                _ => {
                    fde.extend_from_slice(&[0x0C, 6, 16]); // def_cfa rbp, 16
                    fde.extend_from_slice(&[0x80 | 6, 2]);
                }
            }
            cur = Some(st);
        }
        while (fde.len() + 4) % 8 != 0 {
            fde.push(0);
        }
        o.extend(with_len(fde));
    }
    o
}

/// Append the debug sections and a section header table to the ELF image `elf` (a static executable whose code starts
/// at file offset `text_off`, virtual address `va`, `text_len` bytes). Returns false when there is nothing to describe.
pub fn append(elf: &mut Vec<u8>, d: &DebugInfo, ir: &IrProgram, va: u64, text_off: u64, text_len: usize) -> bool {
    if d.funcs.is_empty() {
        return false;
    }
    let comp_dir = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let name = ir.files.first().cloned().unwrap_or_else(|| "yerux".to_string());
    // symbol table: null, then one global FUNC per function (shndx 1 = .text)
    let mut strtab = vec![0u8];
    let mut symtab = vec![0u8; 24];
    for f in &d.funcs {
        let off = strtab.len() as u32;
        cstr(&mut strtab, &f.name);
        symtab.extend_from_slice(&off.to_le_bytes());
        symtab.push(0x12); // GLOBAL, FUNC
        symtab.push(0);
        symtab.extend_from_slice(&1u16.to_le_bytes());
        symtab.extend_from_slice(&(va + f.start as u64).to_le_bytes());
        symtab.extend_from_slice(&((f.end - f.start) as u64).to_le_bytes());
    }
    let (info, ranges, locs) = debug_info(d, ir, va, text_len, &name, &comp_dir);
    let secs: Vec<(&str, u32, u64, Vec<u8>, u32, u32, u64, u64)> = vec![
        // name, type, flags, bytes, link, info, align, entsize
        (".debug_abbrev", 1, 0, debug_abbrev(), 0, 0, 1, 0),
        (".debug_info", 1, 0, info, 0, 0, 1, 0),
        (".debug_line", 1, 0, debug_line(d, &ir.locs, &ir.files, va), 0, 0, 1, 0),
        (".debug_frame", 1, 0, debug_frame(d, va), 0, 0, 8, 0),
        (".debug_ranges", 1, 0, ranges, 0, 0, 8, 0),
        (".debug_loc", 1, 0, locs, 0, 0, 8, 0),
        (".symtab", 2, 0, symtab, 9, 1, 8, 24),
        (".strtab", 3, 0, strtab, 0, 0, 1, 0),
    ];
    // section header string table
    let mut shstr = vec![0u8];
    let mut name_off = vec![];
    for n in [".text"].into_iter().chain(secs.iter().map(|s| s.0)).chain([".shstrtab"]) {
        name_off.push(shstr.len() as u32);
        cstr(&mut shstr, n);
    }
    while elf.len() % 8 != 0 {
        elf.push(0);
    }
    let mut offs = vec![];
    for s in &secs {
        offs.push(elf.len() as u64);
        elf.extend_from_slice(&s.3);
    }
    let shstr_off = elf.len() as u64;
    elf.extend_from_slice(&shstr);
    while elf.len() % 8 != 0 {
        elf.push(0);
    }
    let shoff = elf.len() as u64;
    let sh = |o: &mut Vec<u8>, name: u32, ty: u32, flags: u64, addr: u64, off: u64, size: u64, link: u32, info: u32, align: u64, ent: u64| {
        o.extend_from_slice(&name.to_le_bytes());
        o.extend_from_slice(&ty.to_le_bytes());
        o.extend_from_slice(&flags.to_le_bytes());
        o.extend_from_slice(&addr.to_le_bytes());
        o.extend_from_slice(&off.to_le_bytes());
        o.extend_from_slice(&size.to_le_bytes());
        o.extend_from_slice(&link.to_le_bytes());
        o.extend_from_slice(&info.to_le_bytes());
        o.extend_from_slice(&align.to_le_bytes());
        o.extend_from_slice(&ent.to_le_bytes());
    };
    sh(elf, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
    sh(elf, name_off[0], 1, 6, va, text_off, text_len as u64, 0, 0, 16, 0); // .text: PROGBITS, ALLOC|EXEC
    for (k, s) in secs.iter().enumerate() {
        sh(elf, name_off[k + 1], s.1, s.2, 0, offs[k], s.3.len() as u64, s.4, s.5, s.6, s.7);
    }
    sh(elf, name_off[secs.len() + 1], 3, 0, 0, shstr_off, shstr.len() as u64, 0, 0, 1, 0);
    let n = secs.len() as u16 + 3;
    elf[40..48].copy_from_slice(&shoff.to_le_bytes()); // e_shoff
    elf[60..62].copy_from_slice(&n.to_le_bytes()); // e_shnum
    elf[62..64].copy_from_slice(&(n - 1).to_le_bytes()); // e_shstrndx
    true
}
