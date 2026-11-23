// `yerux doc`: API documentation (Markdown or HTML) from the `///` comments of the public items of .yx files.
//
// The extractor is line based and does not need the program to compile: it reads `pub` top-level items (fn, struct,
// enum, trait, const, type), the methods of `impl` blocks of public types and the methods declared in public traits.
// `///` lines directly above an item document it; `//!` lines at the top of a file document the module. A blank line
// detaches a pending doc comment.

use std::{fs, path::Path};

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub kind: String,      // fn, struct, enum, trait, const, type, method
    pub name: String,      // the item name (methods: Type.name)
    pub signature: String, // as written, without `pub` and without the body
    pub doc: String,
}

#[derive(Debug, Default)]
pub struct Module {
    pub name: String,
    pub doc: String,
    pub items: Vec<Item>,
}

/// Replace the contents of string literals and drop `//` comments, so braces can be counted.
fn code_of(line: &str) -> String {
    let mut out = String::new();
    let (mut in_str, mut esc) = (false, false);
    let b: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
                out.push('"');
            }
        } else if c == '"' {
            in_str = true;
            out.push('"');
        } else if c == '/' && b.get(i + 1) == Some(&'/') {
            break;
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

fn depth_delta(code: &str) -> i32 {
    code.chars().map(|c| if c == '{' { 1 } else if c == '}' { -1 } else { 0 }).sum()
}

fn item_kind(rest: &str) -> Option<(&'static str, String)> {
    for kw in ["fn", "struct", "enum", "trait", "const", "type"] {
        if let Some(after) = rest.strip_prefix(kw) {
            if after.starts_with(' ') {
                let name: String = after.trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if !name.is_empty() {
                    return Some((kw, name));
                }
            }
        }
    }
    None
}

/// The declaration text of an item that starts at `lines[i]`: up to the opening `{` (or the end of the declaration).
fn signature(lines: &[&str], i: usize) -> String {
    let mut sig = String::new();
    let mut parens = 0i32;
    for l in &lines[i..] {
        let code = code_of(l);
        let t = code.trim();
        let mut cut = None;
        for (k, ch) in t.char_indices() {
            match ch {
                '(' | '<' => parens += 1,
                ')' => parens -= 1,
                '>' if !t[..k].ends_with('-') => parens -= 1,
                '{' if parens <= 0 => {
                    cut = Some(k);
                    break;
                }
                _ => {}
            }
        }
        match cut {
            Some(k) => {
                if !sig.is_empty() {
                    sig.push(' ');
                }
                sig.push_str(t[..k].trim());
                return sig;
            }
            None => {
                if !sig.is_empty() {
                    sig.push(' ');
                }
                sig.push_str(t);
                if parens <= 0 && !t.ends_with(',') && !t.ends_with('(') {
                    return sig;
                }
            }
        }
    }
    sig
}

pub fn extract(name: &str, src: &str) -> Module {
    let lines: Vec<&str> = src.lines().collect();
    let mut m = Module { name: name.to_string(), ..Default::default() };
    let mut pending: Vec<String> = vec![];
    let mut depth = 0i32;
    // the block we are in at depth 1: ("impl"|"trait", type name, public)
    let mut block: Option<(String, String, bool)> = None;
    let mut pub_types: Vec<String> = vec![];
    let mut items: Vec<(Item, Option<String>)> = vec![]; // (item, owner type when it is a method of an impl)
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let t = raw.trim();
        if let Some(d) = t.strip_prefix("//!") {
            m.doc.push_str(d.strip_prefix(' ').unwrap_or(d));
            m.doc.push('\n');
        } else if let Some(d) = t.strip_prefix("///") {
            pending.push(d.strip_prefix(' ').unwrap_or(d).to_string());
        } else if t.is_empty() {
            pending.clear();
        } else if t.starts_with("//") || t.starts_with('@') {
            // plain comments and attributes keep the pending doc
        } else {
            let doc = pending.join("\n");
            pending.clear();
            let code = code_of(raw);
            if depth == 0 {
                let (is_pub, rest) = match t.strip_prefix("pub ") {
                    Some(r) => (true, r.trim_start()),
                    None => (false, t),
                };
                if let Some((kind, iname)) = item_kind(rest) {
                    if is_pub {
                        if kind == "struct" || kind == "enum" || kind == "trait" {
                            pub_types.push(iname.clone());
                        }
                        let sig = signature(&lines, i);
                        let sig = sig.strip_prefix("pub ").unwrap_or(&sig).to_string();
                        items.push((Item { kind: kind.into(), name: iname.clone(), signature: sig, doc }, None));
                    }
                    if kind == "trait" {
                        block = Some(("trait".into(), iname, is_pub));
                    }
                } else if let Some(h) = rest.strip_prefix("impl") {
                    // `impl Type {` or `impl Trait for Type {`
                    let head = h.split('{').next().unwrap_or("").trim();
                    let ty = head.rsplit(" for ").next().unwrap_or(head).trim();
                    let ty: String = ty.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                    block = Some(("impl".into(), ty, true));
                }
            } else if depth == 1 {
                if let Some((bk, bty, bpub)) = &block {
                    let rest = t.strip_prefix("pub ").unwrap_or(t).trim_start();
                    if let Some(("fn", fname)) = item_kind(rest) {
                        let sig = signature(&lines, i);
                        let sig = sig.strip_prefix("pub ").unwrap_or(&sig).to_string();
                        let owner = if *bk == "impl" || *bpub { Some(bty.clone()) } else { None };
                        if let Some(o) = owner {
                            items.push((
                                Item { kind: "method".into(), name: format!("{}.{}", bty, fname), signature: sig, doc },
                                Some(o),
                            ));
                        }
                    }
                }
            }
            depth += depth_delta(&code);
            if depth <= 0 {
                depth = 0;
                block = None;
            }
            i += 1;
            continue;
        }
        i += 1;
    }
    if m.doc.trim().is_empty() {
        // no `//!`: the leading block of plain `//` comments describes the module
        for l in &lines {
            let t = l.trim();
            if t.starts_with("///") || !t.starts_with("//") {
                break;
            }
            let d = &t[2..];
            m.doc.push_str(d.strip_prefix(' ').unwrap_or(d));
            m.doc.push('\n');
        }
    }
    for (it, owner) in items {
        match owner {
            Some(o) if !pub_types.contains(&o) => {}
            _ => m.items.push(it),
        }
    }
    m
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn inline_html(s: &str) -> String {
    let mut out = String::new();
    let mut code = false;
    for part in esc(s).split('`') {
        if code {
            out.push_str(&format!("<code>{}</code>", part));
        } else {
            out.push_str(part);
        }
        code = !code;
    }
    out
}

fn doc_html(doc: &str) -> String {
    let (mut out, mut para, mut fence) = (String::new(), vec![] as Vec<&str>, false);
    let flush = |out: &mut String, para: &mut Vec<&str>| {
        if !para.is_empty() {
            out.push_str(&format!("<p>{}</p>\n", inline_html(&para.join(" "))));
            para.clear();
        }
    };
    for l in doc.lines() {
        if l.trim_start().starts_with("```") {
            flush(&mut out, &mut para);
            out.push_str(if fence { "</code></pre>\n" } else { "<pre><code>" });
            fence = !fence;
        } else if fence {
            out.push_str(&esc(l));
            out.push('\n');
        } else if l.trim().is_empty() {
            flush(&mut out, &mut para);
        } else {
            para.push(l.trim());
        }
    }
    flush(&mut out, &mut para);
    if fence {
        out.push_str("</code></pre>\n");
    }
    out
}

fn summary(doc: &str) -> String {
    let first = doc.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    first.to_string()
}

const KINDS: [(&str, &str); 6] =
    [("struct", "Structs"), ("enum", "Enums"), ("trait", "Traits"), ("type", "Types"), ("const", "Constants"), ("fn", "Functions")];

pub fn module_markdown(m: &Module) -> String {
    let mut s = format!("# Module `{}`\n\n", m.name);
    if !m.doc.trim().is_empty() {
        s.push_str(m.doc.trim());
        s.push_str("\n\n");
    }
    for (kind, title) in KINDS {
        let group: Vec<&Item> = m.items.iter().filter(|i| i.kind == kind).collect();
        if group.is_empty() {
            continue;
        }
        s.push_str(&format!("## {}\n\n", title));
        for it in group {
            s.push_str(&format!("### `{}`\n\n```yerux\n{}\n```\n\n", it.name, it.signature));
            if !it.doc.trim().is_empty() {
                s.push_str(it.doc.trim());
                s.push_str("\n\n");
            }
            for me in m.items.iter().filter(|x| x.kind == "method" && x.name.starts_with(&format!("{}.", it.name))) {
                s.push_str(&format!("#### `{}`\n\n```yerux\n{}\n```\n\n", me.name, me.signature));
                if !me.doc.trim().is_empty() {
                    s.push_str(me.doc.trim());
                    s.push_str("\n\n");
                }
            }
        }
    }
    s
}

const CSS: &str = "body{font-family:sans-serif;max-width:50rem;margin:2rem auto;padding:0 1rem;line-height:1.5}pre{background:#f4f4f4;padding:.6rem;overflow:auto}code{background:#f4f4f4}h3,h4{font-family:monospace}";

fn page(title: &str, body: &str) -> String {
    format!("<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>{}</title><style>{}</style></head><body>\n{}</body></html>\n", esc(title), CSS, body)
}

pub fn module_html(m: &Module) -> String {
    let mut s = format!("<p><a href=\"index.html\">index</a></p>\n<h1>Module <code>{}</code></h1>\n", esc(&m.name));
    s.push_str(&doc_html(&m.doc));
    for (kind, title) in KINDS {
        let group: Vec<&Item> = m.items.iter().filter(|i| i.kind == kind).collect();
        if group.is_empty() {
            continue;
        }
        s.push_str(&format!("<h2>{}</h2>\n", title));
        for it in group {
            s.push_str(&format!("<h3 id=\"{0}\">{0}</h3>\n<pre><code>{1}</code></pre>\n", esc(&it.name), esc(&it.signature)));
            s.push_str(&doc_html(&it.doc));
            for me in m.items.iter().filter(|x| x.kind == "method" && x.name.starts_with(&format!("{}.", it.name))) {
                s.push_str(&format!("<h4 id=\"{0}\">{0}</h4>\n<pre><code>{1}</code></pre>\n", esc(&me.name), esc(&me.signature)));
                s.push_str(&doc_html(&me.doc));
            }
        }
    }
    page(&format!("Module {}", m.name), &s)
}

fn counts(m: &Module) -> usize {
    m.items.len()
}

pub fn index_markdown(title: &str, mods: &[Module]) -> String {
    let mut s = format!("# {}\n\n| Module | Summary | Items |\n|---|---|---|\n", title);
    for m in mods {
        s.push_str(&format!("| [`{0}`]({0}.md) | {1} | {2} |\n", m.name, summary(&m.doc), counts(m)));
    }
    s
}

pub fn index_html(title: &str, mods: &[Module]) -> String {
    let mut s = format!("<h1>{}</h1>\n<table>\n<tr><th>Module</th><th>Summary</th><th>Items</th></tr>\n", esc(title));
    for m in mods {
        s.push_str(&format!(
            "<tr><td><a href=\"{0}.html\"><code>{0}</code></a></td><td>{1}</td><td>{2}</td></tr>\n",
            esc(&m.name),
            inline_html(&summary(&m.doc)),
            counts(m)
        ));
    }
    s.push_str("</table>\n");
    page(title, &s)
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    let mut es: Vec<_> = match fs::read_dir(dir) {
        Ok(r) => r.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
        Err(_) => return,
    };
    es.sort();
    for p in es {
        let n = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if p.is_dir() {
            if !n.starts_with('.') && n != "target" && n != "node_modules" {
                collect(root, &p, out);
            }
        } else if n.ends_with(".yx") {
            let rel = p.strip_prefix(root).unwrap_or(&p).with_extension("");
            let name = rel.to_string_lossy().replace(['/', '\\'], ".");
            out.push((name, p.to_string_lossy().to_string()));
        }
    }
}

pub fn run_cli(args: &[String]) -> i32 {
    let usage = "usage: yerux doc [--html] [-o DIR] [dir|file.yx ...]";
    let (mut html, mut outdir) = (false, "target/doc".to_string());
    let mut paths: Vec<String> = vec![];
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--html" => html = true,
            "--markdown" => html = false,
            "-o" => {
                i += 1;
                match args.get(i) {
                    Some(d) => outdir = d.clone(),
                    None => {
                        eprintln!("-o needs a directory\n{}", usage);
                        return 2;
                    }
                }
            }
            a if a.starts_with('-') => {
                eprintln!("unknown option {}\n{}", a, usage);
                return 2;
            }
            a => paths.push(a.to_string()),
        }
        i += 1;
    }
    if paths.is_empty() {
        if Path::new("yerux.toml").is_file() && Path::new("src").is_dir() {
            paths.push("src".into());
        } else {
            paths.push(".".into());
        }
    }
    let mut files: Vec<(String, String)> = vec![];
    for p in &paths {
        let pa = Path::new(p);
        if pa.is_dir() {
            collect(pa, pa, &mut files);
        } else if pa.is_file() {
            let stem = pa.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            files.push((stem, p.clone()));
        } else {
            eprintln!("could not read {}", p);
            return 2;
        }
    }
    let mut mods: Vec<Module> = vec![];
    for (name, path) in &files {
        match fs::read_to_string(path) {
            Ok(src) => mods.push(extract(name, &src)),
            Err(e) => {
                eprintln!("could not read {}: {}", path, e);
                return 1;
            }
        }
    }
    if mods.is_empty() {
        eprintln!("no .yx files to document");
        return 1;
    }
    if let Err(e) = fs::create_dir_all(&outdir) {
        eprintln!("could not create {}: {}", outdir, e);
        return 1;
    }
    let ext = if html { "html" } else { "md" };
    let title = "API documentation";
    let write = |name: &str, text: String| -> bool {
        let f = Path::new(&outdir).join(name);
        match fs::write(&f, text) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("could not write {}: {}", f.display(), e);
                false
            }
        }
    };
    for m in &mods {
        let text = if html { module_html(m) } else { module_markdown(m) };
        if !write(&format!("{}.{}", m.name, ext), text) {
            return 1;
        }
    }
    let idx = if html { index_html(title, &mods) } else { index_markdown(title, &mods) };
    if !write(&format!("index.{}", ext), idx) {
        return 1;
    }
    let n: usize = mods.iter().map(counts).sum();
    println!("documented {} items in {} modules -> {}/index.{}", n, mods.len(), outdir, ext);
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "//! Geometry helpers.\n//! Second line.\n\n/// A point.\n///\n/// Two coordinates.\npub struct Point {\n  x: Int,\n  y: Int,\n}\n\nstruct Hidden {\n  a: Int,\n}\n\nimpl Point {\n  /// Sum of both.\n  fn total(view self) -> Int {\n    return self.x + self.y\n  }\n}\n\nimpl Hidden {\n  fn secret(view self) -> Int {\n    return 1\n  }\n}\n\n/// Adds.\npub fn add(\n  a: Int,\n  b: Int,\n) -> Int {\n  return a + b\n}\n\nfn private() {}\n\n/// detached\n\npub const LIMIT: Int = 3\n\ntest \"t\" {\n  assert(true)\n}\n";

    #[test]
    fn extracts_public_api_only() {
        let m = extract("geo", SRC);
        let names: Vec<&str> = m.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Point", "Point.total", "add", "LIMIT"]);
        assert_eq!(m.doc.trim(), "Geometry helpers.\nSecond line.");
        assert_eq!(m.items[0].doc, "A point.\n\nTwo coordinates.");
        assert_eq!(m.items[1].doc, "Sum of both.");
        assert_eq!(m.items[2].signature, "fn add( a: Int, b: Int, ) -> Int");
        assert_eq!(m.items[3].doc, "", "a blank line detaches the comment");
    }

    #[test]
    fn leading_plain_comments_describe_the_module() {
        let m = extract("m", "// std.m: things.\n//\n//   import std.m\n\npub fn f() {}\n");
        assert_eq!(summary(&m.doc), "std.m: things.");
    }

    #[test]
    fn braces_in_strings_do_not_confuse_depth() {
        let m = extract("m", "pub fn a() {\n  print(\"}}\")\n}\n/// b\npub fn b() {}\n");
        assert_eq!(m.items.len(), 2);
        assert_eq!(m.items[1].doc, "b");
    }

    #[test]
    fn renders_markdown_and_html() {
        let m = extract("geo", SRC);
        let md = module_markdown(&m);
        assert!(md.contains("# Module `geo`") && md.contains("## Structs") && md.contains("#### `Point.total`"));
        let h = module_html(&m);
        assert!(h.contains("<h3 id=\"add\">add</h3>") && h.contains("<p>Adds.</p>"));
        assert!(index_markdown("T", &[m]).contains("| [`geo`](geo.md) | Geometry helpers. | 4 |"));
    }
}
