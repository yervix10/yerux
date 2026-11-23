// Module objects (.yxo): a module and what it imports, packed with its public interface and content hashes, so a
// program can be built without the module's sources next to it and a stale object is detected from hashes.
// Format and limits: docs/MODULE_OBJECT.md.
//
//   yerux obj build mod.yx [-o mod.yxo]   check `mod.yx` and write the object
//   yerux obj info mod.yxo                 modules, hashes and public interface
//   yerux obj check mod.yxo [--root DIR]   integrity, and whether the sources under DIR (default: next to the object) changed
//   yerux build|run|check app.yx --obj mod.yxo [--obj ..]   read the imported modules from the objects

use crate::{doc, modules};
use std::path::{Path, PathBuf};

const MAGIC: &str = "YXO1";

/// FNV-1a, 64 bits: the content hash of sources and interfaces (not cryptographic: it detects change, not tampering).
pub fn hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

pub struct ObjFile {
    /// path relative to the directory of the object's entry module (`util.yx`, `sub/x.yx`)
    pub rel: String,
    pub src: String,
    pub src_hash: u64,
    pub iface: Vec<String>,
    pub iface_hash: u64,
}

pub struct Object {
    pub entry: String,
    pub files: Vec<ObjFile>,
}

/// The public interface of one source file: kind and declaration of every public item, sorted (doc comments and
/// bodies are not part of it, so editing them does not invalidate the modules that import this one).
pub fn interface(name: &str, src: &str) -> Vec<String> {
    let m = doc::extract(name, src);
    let mut v: Vec<String> = m.items.iter().map(|i| i.signature.clone()).collect();
    v.sort();
    v
}

fn iface_hash(iface: &[String]) -> u64 {
    hash(iface.join("\n").as_bytes())
}

pub fn build(entry: &str, out: &str) -> Result<Object, Vec<String>> {
    let loaded = modules::load(entry, modules::Mode::Normal)?;
    let root = Path::new(entry).parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut files = vec![];
    for f in &loaded.files {
        if f.path.starts_with("<std>") {
            continue; // the standard library is part of the compiler
        }
        let rel = Path::new(&f.path).strip_prefix(root).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| f.path.clone());
        let rel = rel.trim_start_matches("./").to_string();
        let stem = Path::new(&rel).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let iface = interface(&stem, &f.src);
        files.push(ObjFile { rel, src: f.src.clone(), src_hash: hash(f.src.as_bytes()), iface_hash: iface_hash(&iface), iface });
    }
    let entry_rel = Path::new(entry).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let obj = Object { entry: entry_rel, files };
    std::fs::write(out, encode(&obj)).map_err(|e| vec![format!("error: could not write {}: {}", out, e)])?;
    Ok(obj)
}

pub fn encode(o: &Object) -> Vec<u8> {
    let mut b = format!("{MAGIC}\nentry {}\nfiles {}\n", o.entry, o.files.len()).into_bytes();
    for f in &o.files {
        b.extend(format!("file {} src {} {:016x} iface {} {:016x}\n", f.rel, f.src.len(), f.src_hash, f.iface.len(), f.iface_hash).bytes());
        for l in &f.iface {
            b.extend(l.bytes());
            b.push(b'\n');
        }
        b.extend(f.src.bytes());
        b.push(b'\n');
    }
    b
}

fn take_line<'a>(b: &'a [u8], at: &mut usize) -> Result<&'a str, String> {
    let rest = &b[*at..];
    let n = rest.iter().position(|c| *c == b'\n').ok_or("truncated object")?;
    *at += n + 1;
    std::str::from_utf8(&rest[..n]).map_err(|_| "object is not valid text".to_string())
}

pub fn decode(b: &[u8]) -> Result<Object, String> {
    let mut at = 0;
    if take_line(b, &mut at)? != MAGIC {
        return Err("not a Yerux module object (bad magic or version)".into());
    }
    let entry = take_line(b, &mut at)?.strip_prefix("entry ").ok_or("missing entry")?.to_string();
    let n: usize = take_line(b, &mut at)?.strip_prefix("files ").and_then(|s| s.parse().ok()).ok_or("missing file count")?;
    let mut files = vec![];
    for _ in 0..n {
        let head = take_line(b, &mut at)?;
        let p: Vec<&str> = head.split(' ').collect();
        if p.len() != 8 || p[0] != "file" || p[2] != "src" || p[5] != "iface" {
            return Err(format!("bad file header: {head}"));
        }
        let (len, nif): (usize, usize) = (p[3].parse().map_err(|_| "bad length")?, p[6].parse().map_err(|_| "bad length")?);
        let src_hash = u64::from_str_radix(p[4], 16).map_err(|_| "bad hash")?;
        let ih = u64::from_str_radix(p[7], 16).map_err(|_| "bad hash")?;
        let mut iface = vec![];
        for _ in 0..nif {
            iface.push(take_line(b, &mut at)?.to_string());
        }
        if at + len + 1 > b.len() {
            return Err("truncated object".into());
        }
        let src = std::str::from_utf8(&b[at..at + len]).map_err(|_| "source is not valid text")?.to_string();
        at += len + 1;
        if hash(src.as_bytes()) != src_hash {
            return Err(format!("{}: the embedded source does not match its hash (corrupt object)", p[1]));
        }
        if iface_hash(&iface) != ih {
            return Err(format!("{}: the interface does not match its hash (corrupt object)", p[1]));
        }
        files.push(ObjFile { rel: p[1].to_string(), src, src_hash, iface, iface_hash: ih });
    }
    Ok(Object { entry, files })
}

pub fn read(path: &str) -> Result<Object, String> {
    let b = std::fs::read(path).map_err(|e| format!("could not read {path}: {e}"))?;
    decode(&b).map_err(|e| format!("{path}: {e}"))
}

/// Make the imported modules of the objects visible to the module loader (as if their files sat next to `entry`).
/// Two objects that carry the same file must carry the same bytes.
pub fn install(objs: &[String], entry: &str) -> Result<(), String> {
    let root = Path::new(entry).parent().filter(|p| !p.as_os_str().is_empty()).map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
    let mut overlay: Vec<(String, String)> = vec![];
    let mut hashes: Vec<(String, u64, u64, String)> = vec![]; // rel, src hash, iface hash, object
    for o in objs {
        let ob = read(o)?;
        for f in ob.files {
            if let Some((_, sh, ih, from)) = hashes.iter().find(|(r, ..)| *r == f.rel) {
                if *sh != f.src_hash {
                    return Err(format!(
                        "module '{}' differs between {} and {}: rebuild the objects against the same sources{}",
                        f.rel,
                        from,
                        o,
                        if *ih != f.iface_hash { " (its interface changed)" } else { "" }
                    ));
                }
                continue;
            }
            hashes.push((f.rel.clone(), f.src_hash, f.iface_hash, o.clone()));
            overlay.push((root.join(&f.rel).to_string_lossy().into_owned(), f.src));
        }
    }
    modules::add_overlay(overlay);
    Ok(())
}

fn info(path: &str) -> Result<(), String> {
    let o = read(path)?;
    println!("object {path}: entry {}, {} module file(s)", o.entry, o.files.len());
    for f in &o.files {
        println!("  {}  source {:016x} ({} bytes)  interface {:016x}", f.rel, f.src_hash, f.src.len(), f.iface_hash);
        for l in &f.iface {
            println!("    {l}");
        }
    }
    Ok(())
}

/// 0 = fresh, 1 = a source under `root` changed (the object is stale), 2 = unreadable object
fn check(path: &str, root: &str) -> i32 {
    let o = match read(path) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut stale = 0;
    for f in &o.files {
        let p = Path::new(root).join(&f.rel);
        match std::fs::read_to_string(&p) {
            Err(_) => println!("{}: no source at {} (the object is all there is)", f.rel, p.display()),
            Ok(s) if hash(s.as_bytes()) == f.src_hash => println!("{}: up to date", f.rel),
            Ok(s) => {
                let iface = interface(Path::new(&f.rel).file_stem().and_then(|x| x.to_str()).unwrap_or(""), &s);
                if iface_hash(&iface) == f.iface_hash {
                    println!("{}: stale (body changed, interface the same: importers need no change)", f.rel);
                } else {
                    println!("{}: stale (interface changed: rebuild this object and the objects that import it)", f.rel);
                }
                stale = 1;
            }
        }
    }
    stale
}

pub fn run_cli(args: &[String]) -> i32 {
    let usage = "usage: yerux obj build <mod.yx> [-o out.yxo] | obj info <mod.yxo> | obj check <mod.yxo> [--root DIR]";
    match args.first().map(|s| s.as_str()) {
        Some("build") if args.len() >= 2 => {
            let entry = &args[1];
            let out = match args.iter().position(|a| a == "-o") {
                Some(i) if i + 1 < args.len() => args[i + 1].clone(),
                _ => Path::new(entry).with_extension("yxo").to_string_lossy().into_owned(),
            };
            match build(entry, &out) {
                Ok(o) => {
                    println!("built object: {out} ({} module file(s))", o.files.len());
                    0
                }
                Err(es) => {
                    for e in es {
                        eprintln!("{e}");
                    }
                    1
                }
            }
        }
        Some("info") if args.len() == 2 => match info(&args[1]) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        },
        Some("check") if args.len() >= 2 => {
            let root = match args.iter().position(|a| a == "--root") {
                Some(i) if i + 1 < args.len() => args[i + 1].clone(),
                _ => Path::new(&args[1]).parent().filter(|p| !p.as_os_str().is_empty()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| ".".into()),
            };
            check(&args[1], &root)
        }
        _ => {
            eprintln!("{usage}");
            2
        }
    }
}
