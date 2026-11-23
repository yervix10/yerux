// Minimal DWARF 4 + symbol table for the native x86-64 Linux backend, so gdb can show Yerux functions, `.yx`
// lines and backtraces:
//   .symtab/.strtab   one FUNC symbol per Yerux function
//   .debug_abbrev/.debug_info   one compile unit with one subprogram per function (name, low_pc, high_pc)
//   .debug_line       file:line:column rows, one sequence per function
//   .debug_frame      call frame information: rbp-based frames, with the three prologue states and the epilogue
// The sections are appended after the loaded image (not part of any PT_LOAD) with a section header table, so the
// program itself is unchanged. No variables or types: `bt`, `break main`, `break file.yx:12`, `list`, `info line`,
// `step`/`next` by line work; `print` of Yerux variables does not.

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

fn debug_abbrev() -> Vec<u8> {
    let mut o = vec![];
    // 1: compile unit (children)
    for v in [1u64, 0x11, 1] {
        uleb(&mut o, v);
    }
    for (at, form) in [(0x25u64, 0x08u64), (0x13, 0x05), (0x03, 0x08), (0x1b, 0x08), (0x11, 0x01), (0x12, 0x07), (0x10, 0x17)] {
        // producer string, language data2, name string, comp_dir string, low_pc addr, high_pc data8, stmt_list sec_offset
        uleb(&mut o, at);
        uleb(&mut o, form);
    }
    o.extend_from_slice(&[0, 0]);
    // 2: subprogram (no children)
    for v in [2u64, 0x2e, 0] {
        uleb(&mut o, v);
    }
    for (at, form) in [(0x03u64, 0x08u64), (0x11, 0x01), (0x12, 0x07), (0x3f, 0x19)] {
        // name string, low_pc addr, high_pc data8, external flag_present
        uleb(&mut o, at);
        uleb(&mut o, form);
    }
    o.extend_from_slice(&[0, 0]);
    o.push(0);
    o
}

fn debug_info(d: &DebugInfo, va: u64, text_len: usize, name: &str, comp_dir: &str) -> Vec<u8> {
    let mut b = vec![];
    b.extend_from_slice(&4u16.to_le_bytes()); // version
    b.extend_from_slice(&0u32.to_le_bytes()); // abbrev offset
    b.push(8); // address size
    uleb(&mut b, 1);
    cstr(&mut b, "yerux native backend");
    b.extend_from_slice(&0x8001u16.to_le_bytes()); // DW_LANG_Mips_Assembler: no C expression semantics
    cstr(&mut b, name);
    cstr(&mut b, comp_dir);
    b.extend_from_slice(&va.to_le_bytes());
    b.extend_from_slice(&(text_len as u64).to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes()); // stmt_list: the only line program
    for f in &d.funcs {
        uleb(&mut b, 2);
        cstr(&mut b, &f.name);
        b.extend_from_slice(&(va + f.start as u64).to_le_bytes());
        b.extend_from_slice(&((f.end - f.start) as u64).to_le_bytes());
    }
    b.push(0); // end of the unit's children
    with_len(b)
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
pub fn append(elf: &mut Vec<u8>, d: &DebugInfo, locs: &[(u32, u32, u32)], files: &[String], va: u64, text_off: u64, text_len: usize) -> bool {
    if d.funcs.is_empty() {
        return false;
    }
    let comp_dir = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let name = files.first().cloned().unwrap_or_else(|| "yerux".to_string());
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
    let secs: Vec<(&str, u32, u64, Vec<u8>, u32, u32, u64, u64)> = vec![
        // name, type, flags, bytes, link, info, align, entsize
        (".debug_abbrev", 1, 0, debug_abbrev(), 0, 0, 1, 0),
        (".debug_info", 1, 0, debug_info(d, va, text_len, &name, &comp_dir), 0, 0, 1, 0),
        (".debug_line", 1, 0, debug_line(d, locs, files, va), 0, 0, 1, 0),
        (".debug_frame", 1, 0, debug_frame(d, va), 0, 0, 8, 0),
        (".symtab", 2, 0, symtab, 7, 1, 8, 24),
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
