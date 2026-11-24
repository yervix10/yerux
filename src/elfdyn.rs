//! Minimal eager-binding ELF64 writer for Linux native imports.
use crate::x64::{Built, G, G_SIZE};

const BASE: u64 = 0x400000;
pub(crate) const HEADER_SIZE: usize = (64 + 56 * 5 + 15) & !15;

pub(crate) enum Arch {
    X64,
    A64,
}

fn put16(v: &mut Vec<u8>, n: u16) {
    v.extend_from_slice(&n.to_le_bytes());
}
fn put32(v: &mut Vec<u8>, n: u32) {
    v.extend_from_slice(&n.to_le_bytes());
}
fn put64(v: &mut Vec<u8>, n: u64) {
    v.extend_from_slice(&n.to_le_bytes());
}

/// Build an ET_EXEC with a system-loader interpreter and eager GLOB_DAT relocations.
pub fn write(b: &Built, links: &[String], imports: &[String]) -> Vec<u8> {
    debug_assert!(b.imports.iter().all(|&(_, index)| index < imports.len()));
    write_image(&b.code, &b.blob, b.entry, links, imports, Arch::X64)
}

pub(crate) fn write_image(
    code: &[u8],
    blob: &[u8],
    entry: usize,
    links: &[String],
    imports: &[String],
    arch: Arch,
) -> Vec<u8> {
    // Keep executable bytes and loader-writable metadata on separate pages.
    // Include PT_GNU_STACK so kernels do not need to infer an executable stack.
    let (machine, reloc, interp, globals, got_rel, page) = match arch {
        Arch::X64 => (
            62,
            6,
            b"/lib64/ld-linux-x86-64.so.2\0".as_slice(),
            G,
            blob.len() + 64,
            4096,
        ),
        Arch::A64 => (
            183,
            1025,
            b"/lib/ld-linux-aarch64.so.1\0".as_slice(),
            crate::a64::G,
            crate::a64::GOT_REL,
            65536,
        ),
    };
    let hdr = HEADER_SIZE;
    let interp_off = hdr + code.len() + blob.len();
    let rw_off = (interp_off + interp.len() + page - 1) & !(page - 1);
    let dyn_rel = got_rel + imports.len() * 8;
    let mut dynstr = vec![0];
    let mut needed = Vec::new();
    let mut needed_libs = vec!["libc.so.6".to_string()];
    for lib in links {
        if !needed_libs.contains(lib) {
            needed_libs.push(lib.clone());
        }
    }
    for lib in &needed_libs {
        let off = dynstr.len() as u32;
        dynstr.extend_from_slice(lib.as_bytes());
        dynstr.push(0);
        needed.push(off);
    }
    let mut symname = Vec::new();
    for name in imports {
        let off = dynstr.len() as u32;
        dynstr.extend_from_slice(name.as_bytes());
        dynstr.push(0);
        symname.push(off);
    }
    let mut dynsym = vec![0u8; 24];
    for name in &symname {
        put32(&mut dynsym, *name);
        dynsym.push(0x12);
        dynsym.push(0);
        put16(&mut dynsym, 0);
        put64(&mut dynsym, 0);
        put64(&mut dynsym, 0);
    }
    let mut rela = Vec::new();
    for (i, _) in imports.iter().enumerate() {
        put64(&mut rela, globals as u64 + (got_rel + i * 8) as u64);
        put64(&mut rela, (((i + 1) as u64) << 32) | reloc); // R_*_GLOB_DAT
        put64(&mut rela, 0);
    }
    let dyn_size = (needed.len() + 8) * 16;
    let dynsym_rel = dyn_rel + dyn_size;
    let dynstr_rel = dynsym_rel + dynsym.len();
    let rela_rel = dynstr_rel + dynstr.len();
    let file_len = rw_off + rela_rel + rela.len();
    let va = |rel: usize| globals as u64 + rel as u64;

    let mut out = Vec::with_capacity(file_len);
    out.extend_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    put16(&mut out, 2);
    put16(&mut out, machine);
    put32(&mut out, 1);
    put64(&mut out, BASE + hdr as u64 + entry as u64);
    put64(&mut out, 64);
    put64(&mut out, 0);
    put32(&mut out, 0);
    put16(&mut out, 64);
    put16(&mut out, 56);
    put16(&mut out, 5);
    put16(&mut out, 64);
    put16(&mut out, 0);
    put16(&mut out, 0);
    // PT_INTERP
    put32(&mut out, 3);
    put32(&mut out, 4);
    put64(&mut out, interp_off as u64);
    put64(&mut out, BASE + interp_off as u64);
    put64(&mut out, BASE + interp_off as u64);
    put64(&mut out, interp.len() as u64);
    put64(&mut out, interp.len() as u64);
    put64(&mut out, 1);
    // RX headers/code/rodata/interpreter.
    put32(&mut out, 1);
    put32(&mut out, 5);
    put64(&mut out, 0);
    put64(&mut out, BASE);
    put64(&mut out, BASE);
    put64(&mut out, rw_off as u64);
    put64(&mut out, rw_off as u64);
    put64(&mut out, page as u64);
    // RW dynamic metadata, GOT and runtime state. The page boundary avoids W+X overlap.
    put32(&mut out, 1);
    put32(&mut out, 6);
    put64(&mut out, rw_off as u64);
    put64(&mut out, globals as u64);
    put64(&mut out, globals as u64);
    put64(&mut out, (rela_rel + rela.len()) as u64);
    put64(&mut out, ((rela_rel + rela.len()) as u64).max(G_SIZE));
    put64(&mut out, page as u64);
    // PT_DYNAMIC
    put32(&mut out, 2);
    put32(&mut out, 6);
    put64(&mut out, (rw_off + dyn_rel) as u64);
    put64(&mut out, va(dyn_rel));
    put64(&mut out, va(dyn_rel));
    put64(&mut out, dyn_size as u64);
    put64(&mut out, dyn_size as u64);
    put64(&mut out, 8);
    // PT_GNU_STACK, RW and deliberately not executable.
    put32(&mut out, 0x6474e551);
    put32(&mut out, 6);
    put64(&mut out, 0);
    put64(&mut out, 0);
    put64(&mut out, 0);
    put64(&mut out, 0);
    put64(&mut out, 0);
    put64(&mut out, 16);
    out.resize(hdr, 0);
    out.extend_from_slice(code);
    out.extend_from_slice(blob);
    out.extend_from_slice(interp);
    out.resize(rw_off, 0);
    out.resize(rw_off + got_rel + imports.len() * 8, 0);
    while out.len() < rw_off + dyn_rel {
        out.push(0);
    }
    for off in needed {
        put64(&mut out, 1);
        put64(&mut out, off as u64);
    }
    for (tag, val) in [
        (5, va(dynstr_rel)),
        (6, va(dynsym_rel)),
        (10, dynstr.len() as u64),
        (11, 24),
        (7, va(rela_rel)),
        (8, rela.len() as u64),
        (9, 24),
    ] {
        put64(&mut out, tag);
        put64(&mut out, val);
    }
    put64(&mut out, 0);
    put64(&mut out, 0);
    out.extend_from_slice(&dynsym);
    out.extend_from_slice(&dynstr);
    out.extend_from_slice(&rela);
    debug_assert_eq!(out.len(), file_len);
    out
}
