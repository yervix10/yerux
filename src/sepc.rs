// Separate compilation for the C backend (`--separate`, docs/MODULE_OBJECT.md stage 2).
//
// The generated C of the whole program is split into translation units: one per user module (the functions
// `nx_f_<module>__*`) and one for everything else (standard library, generic instances, `main`). Every unit gets the
// shared prefix (types, runtime) plus the prototypes it mentions; the unit's object file is cached under a hash of its
// exact text, so a module whose generated code did not change is not handed to the C compiler again. The front end
// (parse, check, monomorphise) still runs on the whole program: only the C compiler step is incremental.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::process::Command;

pub struct Split {
    pub units: Vec<(String, String)>, // (name, C text)
}

fn is_proto(l: &str) -> bool {
    l.starts_with("static ") && l.contains(" nx_f_") && l.ends_with(");") && !l.contains('{')
}

fn fn_name(l: &str) -> Option<&str> {
    let i = l.find("nx_f_")?;
    let rest = &l[i..];
    let e = rest.find('(')?;
    Some(&rest[..e])
}

/// Split the generated C. `modules` are the prefixes (`util` in `nx_f_util__twice`) that get their own unit.
/// None when the text has a shape this splitter does not understand (the caller then compiles it whole).
fn fail(n: u32) -> Option<Split> {
    if std::env::var_os("YERUX_SEPARATE_DEBUG").is_some() {
        eprintln!("separate: split failed at check {n}");
    }
    None
}

pub fn split(c: &str, modules: &HashSet<String>) -> Option<Split> {
    if std::env::var_os("YERUX_SEPARATE_DEBUG").is_some() {
        let _ = std::fs::write("/tmp/sepc_debug.c", c);
    }
    let lines: Vec<&str> = c.lines().collect();
    let first = lines.iter().position(|l| is_proto(l))?;
    let mut head: Vec<String> = vec![];
    let mut data: Vec<String> = vec![]; // definitions of shared mutable state: only in the main unit
    for l in &lines[..first] {
        if l.starts_with("static ") && !l.contains('(') && l.ends_with(';') {
            if l.contains('=') {
                return fail(1);
            }
            data.push(l.replacen("static ", "", 1));
            head.push(l.replacen("static ", "extern ", 1));
        } else {
            head.push(l.to_string());
        }
    }
    let mut protos: Vec<(String, String)> = vec![]; // name, text without `static`
    let mut i = first;
    while i < lines.len() && (is_proto(lines[i]) || lines[i].is_empty()) {
        if !lines[i].is_empty() {
            protos.push((fn_name(lines[i])?.to_string(), lines[i].replacen("static ", "", 1)));
        }
        i += 1;
    }
    // top-level chunks: a function definition runs to the next `}` at column 0
    let mut chunks: BTreeMap<String, String> = BTreeMap::new();
    let mut main_chunk = String::new();
    let mut pending = String::new(); // `#line` directives belong to the definition that follows
    while i < lines.len() {
        if lines[i].is_empty() {
            i += 1;
            continue;
        }
        if lines[i].starts_with('#') {
            pending.push_str(lines[i]);
            pending.push('\n');
            i += 1;
            continue;
        }
        let start = i;
        let l = lines[i];
        if is_proto(l) {
            protos.push((fn_name(l)?.to_string(), l.replacen("static ", "", 1)));
            i += 1;
            pending.clear();
            continue;
        }
        if l.ends_with('{') {
            while i < lines.len() && lines[i] != "}" {
                i += 1;
            }
            i += 1;
        } else if l.ends_with("}") && !l.starts_with(' ') {
            i += 1; // one-line definition (`int main(...) { ... }`)
        } else if l.ends_with(';') && !l.starts_with(' ') && !l.contains("nx_f_") {
            i += 1; // one-line data definition (the location tables): main unit
        } else {
            if std::env::var_os("YERUX_SEPARATE_DEBUG").is_some() {
                eprintln!("  shape: {}", l.chars().take(120).collect::<String>());
            }
            return fail(2);
        }
        let text = format!("{}{}", std::mem::take(&mut pending), lines[start..i.min(lines.len())].join("\n"));
        let unit = match (l.starts_with("static ") && l.contains(" nx_f_")).then(|| fn_name(l)).flatten() {
            Some(n) => {
                let p = n.trim_start_matches("nx_f_").split("__").next().unwrap_or("");
                let key = if modules.contains(p) { p.to_string() } else { "main".to_string() };
                (key, text.replacen("static ", "", 1))
            }
            None if l.starts_with("int main(") || l.ends_with(';') => ("main".to_string(), text),
            None if l.starts_with("static ") && l.contains('(') && (l.ends_with('{') || l.contains(") {")) => {
                // a generated helper (drop glue, dyn thunk): shared by name, so it is global in the main unit
                let sig = l.split(" {").next().unwrap_or(l).trim_end_matches('{').trim_end();
                let name = sig[..sig.find('(').unwrap()].rsplit(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or("");
                protos.push((name.to_string(), format!("{};", sig.replacen("static ", "", 1))));
                ("main".to_string(), text.replacen("static ", "", 1))
            }
            None => return fail(3),
        };
        if unit.0 == "main" {
            main_chunk.push_str(&unit.1);
            main_chunk.push('\n');
        } else {
            let e = chunks.entry(unit.0).or_default();
            e.push_str(&unit.1);
            e.push('\n');
        }
    }
    let mut units = vec![];
    let mut all: Vec<(String, String)> = chunks.into_iter().collect();
    all.push(("main".to_string(), main_chunk));
    for (name, body) in all {
        let mut t = head.join("\n");
        t.push('\n');
        if name == "main" {
            for d in &data {
                t.push_str(d);
                t.push('\n');
            }
        }
        for (pn, p) in &protos {
            if body.contains(pn.as_str()) {
                t.push_str(p);
                t.push('\n');
            }
        }
        t.push_str(&body);
        units.push((name, t));
    }
    Some(Split { units })
}

fn hash_hex(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.bytes().chain(std::iter::once(0)) {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

/// Compile the units (reusing cached objects) and link. `flags` are the compiler flags shared by every unit.
/// Returns (units, cached).
pub fn build(
    split: &Split,
    cc: &str,
    flags: &[String],
    link_args: &[String],
    out: &str,
    cache: &Path,
) -> Result<(usize, usize), String> {
    std::fs::create_dir_all(cache).map_err(|e| format!("could not create {}: {e}", cache.display()))?;
    let fl = flags.join(" ");
    let mut objs = vec![];
    let mut todo = vec![];
    let mut cached = 0;
    for (name, text) in &split.units {
        let h = hash_hex(&[cc, &fl, text]);
        let o = cache.join(format!("{name}-{h}.o"));
        if o.is_file() {
            cached += 1;
        } else {
            todo.push((name.clone(), text.clone(), o.clone()));
        }
        objs.push(o);
    }
    let mut handles = vec![];
    for (name, text, o) in todo {
        let (cc, flags) = (cc.to_string(), flags.to_vec());
        let cfile = cache.join(format!("{name}-{}.c", std::process::id()));
        handles.push(std::thread::spawn(move || -> Result<(), String> {
            std::fs::write(&cfile, &text).map_err(|e| format!("could not write {}: {e}", cfile.display()))?;
            let tmp = o.with_extension("o.tmp");
            let st = Command::new(&cc)
                .args(&flags)
                .arg("-c")
                .arg(&cfile)
                .arg("-o")
                .arg(&tmp)
                .status()
                .map_err(|_| format!("C compiler ({cc}) could not be started"))?;
            let _ = std::fs::remove_file(&cfile);
            if !st.success() {
                return Err("the C compiler failed".into());
            }
            std::fs::rename(&tmp, &o).map_err(|e| e.to_string())
        }));
    }
    for h in handles {
        h.join().map_err(|_| "compiler thread panicked".to_string())??;
    }
    let st = Command::new(cc)
        .args(flags.iter().filter(|f| f.starts_with("-pthread")))
        .arg("-o")
        .arg(out)
        .args(&objs)
        .args(link_args)
        .arg("-lm")
        .status()
        .map_err(|_| format!("C compiler ({cc}) could not be started"))?;
    if !st.success() {
        return Err("the linker failed".into());
    }
    Ok((split.units.len(), cached))
}
