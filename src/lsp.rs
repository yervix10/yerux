// `yerux lsp`: a minimal language server (LSP over stdio, JSON-RPC 2.0). Design notes: docs/LSP.md.
//
// Diagnostics come from the real front end (module loader, parser, checker) run on the editor's buffers
// (`modules::set_overlay`). Symbols, definition and the shape of hover use the lexer only, so they keep
// working while the file does not compile; the *types* shown by hover come from the checker.
use crate::json::{self, n, obj, s, Json};
use crate::syntax::{self, Tok, Token};
use std::collections::HashMap;
use std::io::{BufRead, Write};

// ───────────────────────── documents ─────────────────────────

struct Doc {
    text: String,
    lines: Vec<Vec<char>>,
    toks: Vec<Token>,
    syms: Vec<Sym>,
    /// checker types of the `let`/`var` bindings (line, col of the keyword, name, type), when it compiled
    types: Option<Vec<(usize, usize, String, String)>>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum SymKind {
    Fn,
    Method,
    Struct,
    Enum,
    Trait,
    Impl,
    Const,
    Alias,
    Field,
    Variant,
}

struct Sym {
    name: String,
    kind: SymKind,
    /// token indices: the name, the first token of the item (`pub` included) and its last token
    name_tok: usize,
    start: usize,
    end: usize,
    children: Vec<Sym>,
}

fn is_sym(t: &Token, x: &str) -> bool {
    matches!(&t.t, Tok::Sym(s) if *s == x)
}
fn is_id(t: &Token, x: &str) -> bool {
    matches!(&t.t, Tok::Id(s) if s == x)
}
fn id_name(t: &Token) -> Option<&str> {
    match &t.t {
        Tok::Id(s) => Some(s),
        _ => None,
    }
}

/// Index of the `}` matching the `{` at `open` (the last token when it never closes).
fn matching(toks: &[Token], open: usize) -> usize {
    let mut d = 0;
    for (i, t) in toks.iter().enumerate().skip(open) {
        if is_sym(t, "{") {
            d += 1;
        } else if is_sym(t, "}") {
            d -= 1;
            if d == 0 {
                return i;
            }
        }
    }
    toks.len().saturating_sub(1)
}

/// The items declared in tokens `lo..hi`.
fn parse_items(toks: &[Token], lo: usize, hi: usize, in_impl: bool) -> Vec<Sym> {
    let mut out = vec![];
    let mut i = lo;
    while i < hi {
        let start = i;
        if is_id(&toks[i], "pub") {
            i += 1;
        }
        if i >= hi {
            break;
        }
        let kw = id_name(&toks[i]).unwrap_or("");
        match kw {
            "fn" if i + 1 < hi => {
                let (mut j, mut paren) = (i + 2, 0i32);
                let mut end = None;
                while j < hi {
                    let t = &toks[j];
                    if is_sym(t, "(") {
                        paren += 1;
                    } else if is_sym(t, ")") {
                        paren -= 1;
                    } else if paren == 0 && is_sym(t, "{") {
                        end = Some(matching(toks, j));
                        break;
                    } else if paren == 0
                        && (is_id(t, "pub") || is_id(t, "fn") || is_sym(t, "}"))
                        && !is_sym(&toks[j - 1], "->")
                    {
                        end = Some(j - 1);
                        break;
                    }
                    j += 1;
                }
                let end = end.unwrap_or(hi - 1).min(hi - 1);
                if let Some(name) = id_name(&toks[i + 1]) {
                    out.push(Sym {
                        name: name.to_string(),
                        kind: if in_impl { SymKind::Method } else { SymKind::Fn },
                        name_tok: i + 1,
                        start,
                        end,
                        children: vec![],
                    });
                }
                i = end + 1;
            }
            "struct" | "enum" | "trait" | "impl" => {
                let mut j = i + 1;
                while j < hi && !is_sym(&toks[j], "{") {
                    j += 1;
                }
                if j >= hi {
                    break;
                }
                let end = matching(toks, j).min(hi - 1);
                let kind = match kw {
                    "struct" => SymKind::Struct,
                    "enum" => SymKind::Enum,
                    "trait" => SymKind::Trait,
                    _ => SymKind::Impl,
                };
                let name_tok = if kind == SymKind::Impl { i } else { i + 1 };
                let name = id_name(&toks[name_tok]).unwrap_or("").to_string();
                let children = match kind {
                    SymKind::Struct => fields(toks, j + 1, end),
                    SymKind::Enum => variants(toks, j + 1, end),
                    _ => parse_items(toks, j + 1, end, true),
                };
                out.push(Sym { name, kind, name_tok, start, end, children });
                i = end + 1;
            }
            "const" | "type" if i + 1 < hi && id_name(&toks[i + 1]).is_some() => {
                let mut j = i + 2;
                let line = toks[i].pos.line;
                while j < hi && toks[j].pos.line == line {
                    j += 1;
                }
                out.push(Sym {
                    name: id_name(&toks[i + 1]).unwrap().to_string(),
                    kind: if kw == "const" { SymKind::Const } else { SymKind::Alias },
                    name_tok: i + 1,
                    start,
                    end: j - 1,
                    children: vec![],
                });
                i = j;
            }
            _ if is_sym(&toks[i], "{") => i = matching(toks, i) + 1,
            _ => i = start + 1,
        }
    }
    out
}

fn fields(toks: &[Token], lo: usize, hi: usize) -> Vec<Sym> {
    let mut out = vec![];
    let mut d = 0i32;
    for i in lo..hi {
        let t = &toks[i];
        if is_sym(t, "{") || is_sym(t, "(") || is_sym(t, "<") {
            d += 1;
        } else if is_sym(t, "}") || is_sym(t, ")") || is_sym(t, ">") {
            d -= 1;
        } else if d == 0 && i + 1 < hi && is_sym(&toks[i + 1], ":") {
            if let Some(nm) = id_name(t) {
                // the field ends before the next `,` at depth 0 (or at the end)
                let mut e = i + 1;
                let mut dd = 0i32;
                while e + 1 < hi {
                    let u = &toks[e + 1];
                    if is_sym(u, "(") || is_sym(u, "<") || is_sym(u, "{") {
                        dd += 1;
                    } else if is_sym(u, ")") || is_sym(u, ">") || is_sym(u, "}") {
                        dd -= 1;
                    } else if dd == 0 && is_sym(u, ",") {
                        break;
                    }
                    e += 1;
                }
                out.push(Sym {
                    name: nm.to_string(),
                    kind: SymKind::Field,
                    name_tok: i,
                    start: i,
                    end: e,
                    children: vec![],
                });
            }
        }
    }
    out
}

fn variants(toks: &[Token], lo: usize, hi: usize) -> Vec<Sym> {
    let mut out = vec![];
    let mut i = lo;
    while i < hi {
        if let Some(nm) = id_name(&toks[i]) {
            let mut e = i;
            if i + 1 < hi && is_sym(&toks[i + 1], "(") {
                let mut d = 0;
                let mut j = i + 1;
                while j < hi {
                    if is_sym(&toks[j], "(") {
                        d += 1;
                    } else if is_sym(&toks[j], ")") {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    j += 1;
                }
                e = j.min(hi - 1);
            }
            out.push(Sym {
                name: nm.to_string(),
                kind: SymKind::Variant,
                name_tok: i,
                start: i,
                end: e,
                children: vec![],
            });
            i = e + 1;
        } else {
            i += 1;
        }
    }
    out
}

impl Doc {
    fn new(text: String) -> Doc {
        let lines = text.split('\n').map(|l| l.chars().collect()).collect();
        let toks = syntax::lex(&text).unwrap_or_default();
        let syms = if toks.is_empty() { vec![] } else { parse_items(&toks, 0, toks.len() - 1, false) };
        Doc { text, lines, toks, syms, types: None }
    }

    /// LSP position (0-based line, UTF-16 column) of a token position (1-based line, char column).
    fn to_lsp(&self, line: usize, col: usize) -> (usize, usize) {
        let l = line.saturating_sub(1);
        let u16 = self.lines.get(l).map_or(col.saturating_sub(1), |cs| {
            cs.iter().take(col.saturating_sub(1)).map(|c| c.len_utf16()).sum()
        });
        (l, u16)
    }
    /// The reverse: (1-based line, char column) of an LSP position.
    fn from_lsp(&self, line: usize, u16col: usize) -> (usize, usize) {
        let mut acc = 0;
        let mut col = 0;
        if let Some(cs) = self.lines.get(line) {
            for c in cs {
                if acc >= u16col {
                    break;
                }
                acc += c.len_utf16();
                col += 1;
            }
        }
        (line + 1, col + 1)
    }
    fn range(&self, line: usize, col: usize, len: usize) -> Json {
        let (l, c) = self.to_lsp(line, col);
        let (l2, c2) = self.to_lsp(line, col + len);
        obj(vec![
            ("start", obj(vec![("line", n(l)), ("character", n(c))])),
            ("end", obj(vec![("line", n(l2)), ("character", n(c2))])),
        ])
    }
    fn tok_range(&self, i: usize) -> Json {
        let p = self.toks[i].pos;
        self.range(p.line, p.col, p.len.max(1))
    }
    fn tok_span(&self, a: usize, b: usize) -> Json {
        let (pa, pb) = (self.toks[a].pos, self.toks[b].pos);
        let (l1, c1) = self.to_lsp(pa.line, pa.col);
        let (l2, c2) = self.to_lsp(pb.line, pb.col + pb.len.max(1));
        obj(vec![
            ("start", obj(vec![("line", n(l1)), ("character", n(c1))])),
            ("end", obj(vec![("line", n(l2)), ("character", n(c2))])),
        ])
    }
    /// Source text from the start of token `a` to the end of token `b`.
    fn slice(&self, a: usize, b: usize) -> String {
        let (pa, pb) = (self.toks[a].pos, self.toks[b].pos);
        let mut out = String::new();
        for line in pa.line..=pb.line {
            let cs = &self.lines[line - 1];
            let from = if line == pa.line { pa.col - 1 } else { 0 };
            let to = if line == pb.line { (pb.col - 1 + pb.len.max(1)).min(cs.len()) } else { cs.len() };
            if from < to {
                out.extend(&cs[from..to]);
            }
            if line < pb.line {
                out.push('\n');
            }
        }
        out
    }
    /// Index of the identifier token under an LSP position.
    fn id_at(&self, line: usize, u16col: usize) -> Option<usize> {
        let (l, c) = self.from_lsp(line, u16col);
        self.toks.iter().position(|t| {
            matches!(t.t, Tok::Id(_)) && t.pos.line == l && t.pos.col <= c && c <= t.pos.col + t.pos.len.max(1) - 1
        })
    }
}

// ───────────────────────── symbol resolution (lexical) ─────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum Decl {
    Let,
    For,
    Param,
}

/// Is token `i` the name of a declaration (`let x`, `for x`, parameter `x:`)?
fn decl_at(doc: &Doc, i: usize) -> Option<Decl> {
    let toks = &doc.toks;
    id_name(&toks[i])?;
    if i > 0 && (is_id(&toks[i - 1], "let") || is_id(&toks[i - 1], "var")) {
        return Some(Decl::Let);
    }
    if i > 0 && (is_id(&toks[i - 1], "for") || (is_id(&toks[i - 1], "mut") && i > 1 && is_id(&toks[i - 2], "for"))) {
        return Some(Decl::For);
    }
    if i + 1 < toks.len() && is_sym(&toks[i + 1], ":") && i > 0 && (is_sym(&toks[i - 1], "(") || is_sym(&toks[i - 1], ",")) {
        // a parameter: inside the parentheses of a `fn` header
        let mut d = 0i32;
        let mut j = i;
        while j > 0 {
            j -= 1;
            if is_sym(&toks[j], ")") {
                d += 1;
            } else if is_sym(&toks[j], "(") {
                if d == 0 {
                    // `fn name (` or `fn name <T> (`
                    return (j >= 2 && is_id(&toks[j - 2], "fn") || (j >= 1 && is_id(&toks[j - 1], "fn"))
                        || header_before(toks, j))
                    .then_some(Decl::Param);
                }
                d -= 1;
            } else if is_sym(&toks[j], "{") || is_sym(&toks[j], "}") {
                return None;
            }
        }
    }
    None
}

/// `fn name<T, U>(`: is the `(` at `j` preceded by a generic list that follows `fn name`?
fn header_before(toks: &[Token], j: usize) -> bool {
    if j == 0 || !is_sym(&toks[j - 1], ">") {
        return false;
    }
    let mut k = j - 1;
    while k > 0 && !is_sym(&toks[k], "<") {
        k -= 1;
    }
    k >= 2 && is_id(&toks[k - 2], "fn")
}

fn all_syms<'a>(syms: &'a [Sym], out: &mut Vec<&'a Sym>) {
    for s in syms {
        out.push(s);
        all_syms(&s.children, out);
    }
}

/// Where is the identifier at token `i` declared? A token index in the same document.
fn declaration_of(doc: &Doc, i: usize) -> Option<usize> {
    let toks = &doc.toks;
    let name = id_name(&toks[i])?;
    if decl_at(doc, i).is_some() {
        return Some(i);
    }
    let after_dot = i > 0 && is_sym(&toks[i - 1], ".");
    if !after_dot {
        // locals: walk back through the enclosing blocks of the function around `i`
        let mut flat = vec![];
        all_syms(&doc.syms, &mut flat);
        let owner = flat
            .iter()
            .filter(|s| matches!(s.kind, SymKind::Fn | SymKind::Method) && s.start <= i && i <= s.end)
            .min_by_key(|s| s.end - s.start);
        if let Some(f) = owner {
            let mut depth = 0i32; // closed blocks we are skipping
            let mut j = i;
            while j > f.start {
                j -= 1;
                let t = &toks[j];
                if is_sym(t, "}") {
                    depth += 1;
                } else if is_sym(t, "{") {
                    if depth > 0 {
                        depth -= 1;
                    }
                } else if depth == 0 && id_name(t) == Some(name) && decl_at(doc, j).is_some() {
                    return Some(j);
                }
            }
            // parameters
            for j in f.start..i {
                if is_sym(&toks[j], "{") {
                    break;
                }
                if id_name(&toks[j]) == Some(name) && decl_at(doc, j) == Some(Decl::Param) {
                    return Some(j);
                }
            }
        }
        // items of the file
        for s in &doc.syms {
            if s.name == name && !matches!(s.kind, SymKind::Impl) {
                return Some(s.name_tok);
            }
        }
        for s in &doc.syms {
            for c in &s.children {
                if c.kind == SymKind::Variant && c.name == name {
                    return Some(c.name_tok);
                }
            }
        }
        return None;
    }
    // member: `x.name`: a method or field of some type of this file
    let mut flat = vec![];
    all_syms(&doc.syms, &mut flat);
    flat.iter()
        .find(|s| s.name == name && matches!(s.kind, SymKind::Method | SymKind::Field))
        .map(|s| s.name_tok)
}

/// `import a.b as c` lines: (alias, module path)
fn imports(doc: &Doc) -> Vec<(String, String)> {
    let toks = &doc.toks;
    let mut out = vec![];
    let mut i = 0;
    while i < toks.len() {
        if is_id(&toks[i], "import") {
            let mut segs = vec![];
            let mut j = i + 1;
            while j < toks.len() {
                if let Some(x) = id_name(&toks[j]) {
                    segs.push(x.to_string());
                    j += 1;
                    if j < toks.len() && is_sym(&toks[j], ".") {
                        j += 1;
                        continue;
                    }
                }
                break;
            }
            let mut alias = segs.last().cloned().unwrap_or_default();
            if j + 1 < toks.len() && is_id(&toks[j], "as") {
                if let Some(a) = id_name(&toks[j + 1]) {
                    alias = a.to_string();
                }
            }
            if !segs.is_empty() {
                out.push((alias, segs.join(".")));
            }
            i = j;
        }
        i += 1;
    }
    out
}

// ───────────────────────── diagnostics ─────────────────────────

struct Problem {
    path: String,
    line: usize,
    col: usize,
    len: usize,
    msg: String,
}

/// Parse the compiler's rendered errors (`error: msg` / ` --> file:line:col` / source / carets / ` = note`).
fn parse_rendered(errs: &[String]) -> Vec<Problem> {
    let mut out = vec![];
    for e in errs {
        let lines: Vec<&str> = e.lines().collect();
        let mut k = 0;
        while k < lines.len() {
            if let Some(first) = lines[k].strip_prefix("error: ") {
                let mut msg = first.to_string();
                let mut j = k + 1;
                while j < lines.len() && !lines[j].trim_start().starts_with("--> ") && !lines[j].starts_with("error: ") {
                    msg.push('\n');
                    msg.push_str(lines[j]);
                    j += 1;
                }
                let mut p = Problem { path: String::new(), line: 1, col: 1, len: 1, msg };
                if j < lines.len() && lines[j].trim_start().starts_with("--> ") {
                    let loc = lines[j].trim_start().trim_start_matches("--> ");
                    let mut it = loc.rsplitn(3, ':');
                    let col = it.next().and_then(|x| x.parse().ok());
                    let line = it.next().and_then(|x| x.parse().ok());
                    p.path = it.next().unwrap_or("").to_string();
                    p.line = line.unwrap_or(1);
                    p.col = col.unwrap_or(1);
                    j += 1;
                    let mut notes = vec![];
                    while j < lines.len() && !lines[j].starts_with("error: ") {
                        let l = lines[j];
                        if l.trim_start().starts_with('|') || l.contains(" | ") {
                            let after = l.splitn(2, '|').nth(1).unwrap_or("");
                            let carets = after.chars().filter(|c| *c == '^').count();
                            if carets > 0 && after.trim().chars().all(|c| c == '^') {
                                p.len = carets;
                            }
                        } else if let Some(nt) = l.trim_start().strip_prefix("= ") {
                            notes.push(nt.to_string());
                        }
                        j += 1;
                    }
                    for nt in notes {
                        p.msg.push('\n');
                        p.msg.push_str(&nt);
                    }
                }
                out.push(p);
                k = j;
            } else {
                k += 1;
            }
        }
    }
    out
}

struct Analysis {
    problems: Vec<Problem>,
    types: Option<Vec<(usize, usize, String, String)>>,
}

/// Run the compiler front end on the document at `path` with the open buffers overlaid.
fn analyze(path: &str, docs: &HashMap<String, Doc>, uri_paths: &HashMap<String, String>) -> Analysis {
    let overlay: Vec<(String, String)> = docs
        .iter()
        .filter_map(|(u, d)| uri_paths.get(u).map(|p| (p.clone(), d.text.clone())))
        .collect();
    crate::modules::set_overlay(overlay);
    let entry = path.to_string();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let loaded = match crate::modules::load(&entry, crate::modules::Mode::Normal) {
            Ok(l) => l,
            Err(errs) => return (parse_rendered(&errs), None),
        };
        let mut ps = syntax::Parser::new(loaded.toks.clone());
        let mut prog = match ps.program() {
            Ok(p) => p,
            Err(d) => return (parse_rendered(&[loaded.render(&d)]), None),
        };
        match crate::check::check_program(&mut prog, &mut ps) {
            Ok(_) => {
                let types = crate::check_trace::typed_bindings(&prog, &loaded.files[0].src);
                (vec![], Some(types))
            }
            Err(ds) => {
                let v: Vec<String> = ds.iter().map(|d| loaded.render(d)).collect();
                (parse_rendered(&v), None)
            }
        }
    }));
    crate::modules::set_overlay(vec![]);
    match r {
        Ok((problems, types)) => Analysis { problems, types },
        Err(_) => Analysis {
            problems: vec![Problem {
                path: entry,
                line: 1,
                col: 1,
                len: 1,
                msg: "internal error: the compiler front end panicked on this file".into(),
            }],
            types: None,
        },
    }
}

// ───────────────────────── server ─────────────────────────

struct Server {
    docs: HashMap<String, Doc>,
    paths: HashMap<String, String>,
    shutdown: bool,
}

fn uri_to_path(uri: &str) -> String {
    let rest = uri.strip_prefix("file://").unwrap_or(uri);
    let b = rest.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn path_to_uri(path: &str) -> String {
    let mut out = String::from("file://");
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/._-~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn send(v: &Json) {
    let body = v.to_text();
    let mut o = std::io::stdout().lock();
    let _ = write!(o, "Content-Length: {}\r\n\r\n{}", body.len(), body);
    let _ = o.flush();
}

fn reply(id: &Json, result: Json) {
    send(&obj(vec![("jsonrpc", s("2.0")), ("id", id.clone()), ("result", result)]));
}

fn reply_err(id: &Json, code: i64, msg: &str) {
    send(&obj(vec![
        ("jsonrpc", s("2.0")),
        ("id", id.clone()),
        ("error", obj(vec![("code", Json::Num(code as f64)), ("message", s(msg))])),
    ]));
}

fn read_message(inp: &mut impl BufRead) -> Option<Json> {
    let mut len = None;
    loop {
        let mut line = String::new();
        if inp.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse::<usize>().ok();
            }
        }
    }
    let mut buf = vec![0u8; len?];
    inp.read_exact(&mut buf).ok()?;
    json::parse(&String::from_utf8_lossy(&buf)).ok()
}

fn kind_code(k: SymKind) -> usize {
    match k {
        SymKind::Fn => 12,
        SymKind::Method => 6,
        SymKind::Struct => 23,
        SymKind::Enum => 10,
        SymKind::Trait => 11,
        SymKind::Impl => 5,
        SymKind::Const => 14,
        SymKind::Alias => 26,
        SymKind::Field => 8,
        SymKind::Variant => 22,
    }
}

fn symbol_json(doc: &Doc, sym: &Sym) -> Json {
    let label = if sym.kind == SymKind::Impl {
        // `impl Show for Point`: the header up to the `{`
        let mut e = sym.start;
        while e < sym.end && !is_sym(&doc.toks[e], "{") {
            e += 1;
        }
        doc.slice(sym.start, e.saturating_sub(1).max(sym.start))
            .trim_start_matches("pub ")
            .to_string()
    } else {
        sym.name.clone()
    };
    obj(vec![
        ("name", s(label)),
        ("kind", n(kind_code(sym.kind))),
        ("range", doc.tok_span(sym.start, sym.end)),
        ("selectionRange", doc.tok_range(sym.name_tok)),
        ("children", Json::Arr(sym.children.iter().map(|c| symbol_json(doc, c)).collect())),
    ])
}

/// The declaration text shown on hover for the item whose name is token `name_tok`.
fn item_signature(doc: &Doc, name_tok: usize) -> Option<String> {
    let mut flat = vec![];
    all_syms(&doc.syms, &mut flat);
    let sym = flat.iter().find(|s| s.name_tok == name_tok)?;
    let t = &doc.toks;
    Some(match sym.kind {
        SymKind::Fn | SymKind::Method | SymKind::Struct | SymKind::Enum | SymKind::Trait | SymKind::Impl => {
            let mut e = sym.start;
            while e < sym.end && !is_sym(&t[e], "{") {
                e += 1;
            }
            let head_end = if is_sym(&t[e], "{") { e.saturating_sub(1).max(sym.start) } else { e };
            let mut text = doc.slice(sym.start, head_end);
            if matches!(sym.kind, SymKind::Struct | SymKind::Enum) {
                text = doc.slice(sym.start, sym.end);
            }
            text
        }
        SymKind::Field | SymKind::Variant => {
            let mut text = doc.slice(sym.start, sym.end);
            if sym.kind == SymKind::Variant {
                text = format!("{}.{}", "variant", text);
            }
            text
        }
        SymKind::Const | SymKind::Alias => doc.slice(sym.start, sym.end),
    })
}

impl Server {
    fn publish(&mut self, uri: &str) {
        let path = match self.paths.get(uri) {
            Some(p) => p.clone(),
            None => return,
        };
        let an = analyze(&path, &self.docs, &self.paths);
        let mut diags = vec![];
        if let Some(doc) = self.docs.get_mut(uri) {
            let has_main = doc.syms.iter().any(|s| s.name == "main" && s.kind == SymKind::Fn);
            for p in &an.problems {
                // a library file has no `main`: not an error for the editor
                if !has_main && p.msg.starts_with("missing `fn main()`") {
                    continue;
                }
                let (line, col, len, msg) = if p.path == path {
                    (p.line, p.col, p.len, p.msg.clone())
                } else {
                    // a problem in another file: report it on the import chain's entry, naming the file
                    (1, 1, 1, format!("{}:{}:{}: {}", p.path, p.line, p.col, p.msg))
                };
                diags.push(obj(vec![
                    ("range", doc.range(line, col, len)),
                    ("severity", n(1)),
                    ("source", s("yerux")),
                    ("message", s(msg)),
                ]));
            }
            doc.types = an.types;
        }
        send(&obj(vec![
            ("jsonrpc", s("2.0")),
            ("method", s("textDocument/publishDiagnostics")),
            ("params", obj(vec![("uri", s(uri)), ("diagnostics", Json::Arr(diags))])),
        ]));
    }

    fn open(&mut self, uri: &str, text: String) {
        self.paths.insert(uri.to_string(), uri_to_path(uri));
        self.docs.insert(uri.to_string(), Doc::new(text));
        self.publish(uri);
    }

    fn handle(&mut self, msg: &Json) -> bool {
        let method = msg.get("method").and_then(|m| m.str()).unwrap_or("");
        let id = msg.get("id");
        let params = msg.get("params").cloned().unwrap_or(Json::Null);
        let uri = params.path("textDocument.uri").and_then(|u| u.str()).unwrap_or("").to_string();
        let result = match method {
            "initialize" => Some(obj(vec![
                (
                    "capabilities",
                    obj(vec![
                        (
                            "textDocumentSync",
                            obj(vec![("openClose", Json::Bool(true)), ("change", n(1)), ("save", Json::Bool(true))]),
                        ),
                        ("documentSymbolProvider", Json::Bool(true)),
                        ("definitionProvider", Json::Bool(true)),
                        ("hoverProvider", Json::Bool(true)),
                        ("documentFormattingProvider", Json::Bool(true)),
                    ]),
                ),
                ("serverInfo", obj(vec![("name", s("yerux lsp")), ("version", s(env!("CARGO_PKG_VERSION")))])),
            ])),
            "shutdown" => {
                self.shutdown = true;
                Some(Json::Null)
            }
            "exit" => return false,
            "textDocument/didOpen" => {
                let text = params.path("textDocument.text").and_then(|t| t.str()).unwrap_or("").to_string();
                self.open(&uri, text);
                None
            }
            "textDocument/didChange" => {
                let text = params
                    .get("contentChanges")
                    .and_then(|c| c.arr())
                    .and_then(|c| c.last())
                    .and_then(|c| c.get("text"))
                    .and_then(|t| t.str())
                    .map(|t| t.to_string());
                if let Some(t) = text {
                    self.open(&uri, t);
                }
                None
            }
            "textDocument/didSave" => {
                if self.docs.contains_key(&uri) {
                    self.publish(&uri);
                }
                None
            }
            "textDocument/didClose" => {
                self.docs.remove(&uri);
                self.paths.remove(&uri);
                send(&obj(vec![
                    ("jsonrpc", s("2.0")),
                    ("method", s("textDocument/publishDiagnostics")),
                    ("params", obj(vec![("uri", s(&uri)), ("diagnostics", Json::Arr(vec![]))])),
                ]));
                None
            }
            "textDocument/documentSymbol" => Some(match self.docs.get(&uri) {
                Some(d) => Json::Arr(d.syms.iter().map(|x| symbol_json(d, x)).collect()),
                None => Json::Arr(vec![]),
            }),
            "textDocument/definition" => Some(self.definition(&uri, &params)),
            "textDocument/hover" => Some(self.hover(&uri, &params)),
            "textDocument/formatting" => {
                let Some(d) = self.docs.get(&uri) else {
                    return true;
                };
                match crate::fmt::format_source(&d.text) {
                    Ok(out) if out == d.text => Some(Json::Arr(vec![])),
                    Ok(out) => {
                        let last = d.lines.len().saturating_sub(1);
                        let end_col: usize = d.lines.last().map_or(0, |l| l.iter().map(|c| c.len_utf16()).sum());
                        Some(Json::Arr(vec![obj(vec![
                            (
                                "range",
                                obj(vec![
                                    ("start", obj(vec![("line", n(0)), ("character", n(0))])),
                                    ("end", obj(vec![("line", n(last)), ("character", n(end_col))])),
                                ]),
                            ),
                            ("newText", s(out)),
                        ])]))
                    }
                    Err(e) => {
                        if let Some(id) = id {
                            reply_err(id, -32803, &format!("cannot format: {}", e));
                        }
                        return true;
                    }
                }
            }
            _ => {
                if let Some(id) = id {
                    reply_err(id, -32601, &format!("method not supported: {}", method));
                }
                return true;
            }
        };
        if let (Some(id), Some(r)) = (id, result) {
            reply(id, r);
        }
        true
    }

    fn locate(&self, uri: &str, params: &Json) -> Option<(&Doc, usize)> {
        let doc = self.docs.get(uri)?;
        let line = params.path("position.line")?.int()? as usize;
        let ch = params.path("position.character")?.int()? as usize;
        let i = doc.id_at(line, ch)?;
        Some((doc, i))
    }

    fn definition(&self, uri: &str, params: &Json) -> Json {
        let Some((doc, i)) = self.locate(uri, params) else {
            return Json::Null;
        };
        // `alias.name` where `alias` is an imported module: look the item up in that module's file
        if i >= 2 && is_sym(&doc.toks[i - 1], ".") {
            if let Some(alias) = id_name(&doc.toks[i - 2]) {
                if let Some((_, module)) = imports(doc).into_iter().find(|(a, _)| a == alias) {
                    return self.in_module(uri, &module, id_name(&doc.toks[i]).unwrap_or(""));
                }
            }
        }
        match declaration_of(doc, i) {
            Some(d) => obj(vec![("uri", s(uri)), ("range", doc.tok_range(d))]),
            None => Json::Null,
        }
    }

    /// The document of imported module `module` (an open buffer, else the file next to `uri`'s file); `std.*`
    /// modules are embedded in the compiler and have no file.
    fn module_doc(&self, uri: &str, module: &str) -> Option<(String, Doc)> {
        let root = std::path::Path::new(self.paths.get(uri)?).parent()?.to_path_buf();
        let file = root.join(format!("{}.yx", module.replace('.', "/")));
        let furi = path_to_uri(&file.to_string_lossy());
        let text = match self.docs.get(&furi) {
            Some(d) => d.text.clone(),
            None => std::fs::read_to_string(&file).ok()?,
        };
        Some((furi, Doc::new(text)))
    }

    fn in_module(&self, uri: &str, module: &str, name: &str) -> Json {
        match self.module_doc(uri, module) {
            Some((furi, d)) => d
                .syms
                .iter()
                .find(|x| x.name == name && x.kind != SymKind::Impl)
                .map_or(Json::Null, |x| obj(vec![("uri", s(&furi)), ("range", d.tok_range(x.name_tok))])),
            None => Json::Null,
        }
    }

    fn hover(&mut self, uri: &str, params: &Json) -> Json {
        let located = self.locate(uri, params).map(|(_, i)| i);
        let Some(i) = located else {
            return Json::Null;
        };
        // types come from the checker; refresh them when the buffer changed since the last analysis
        if self.docs.get(uri).is_some_and(|d| d.types.is_none()) {
            if let Some(path) = self.paths.get(uri).cloned() {
                let an = analyze(&path, &self.docs, &self.paths);
                if let Some(d) = self.docs.get_mut(uri) {
                    d.types = an.types;
                }
            }
        }
        let doc = &self.docs[uri];
        let hover = |text: String, doc: &Doc| {
            obj(vec![
                ("contents", obj(vec![("kind", s("markdown")), ("value", s(format!("```yerux\n{}\n```", text)))])),
                ("range", doc.tok_range(i)),
            ])
        };
        if i >= 2 && is_sym(&doc.toks[i - 1], ".") {
            if let Some(alias) = id_name(&doc.toks[i - 2]) {
                if let Some((_, module)) = imports(doc).into_iter().find(|(a, _)| a == alias) {
                    let name = id_name(&doc.toks[i]).unwrap_or("");
                    let sig = self.module_doc(uri, &module).and_then(|(_, md)| {
                        let x = md.syms.iter().find(|x| x.name == name && x.kind != SymKind::Impl)?;
                        item_signature(&md, x.name_tok)
                    });
                    return sig.map_or(Json::Null, |sg| hover(sg, doc));
                }
            }
        }
        let Some(d) = declaration_of(doc, i) else {
            return Json::Null;
        };
        let name = id_name(&doc.toks[i]).unwrap_or("");
        let text = match decl_at(doc, d) {
            Some(Decl::Let) => {
                let kw = &doc.toks[d - 1];
                let ty = doc
                    .types
                    .as_ref()
                    .and_then(|t| t.iter().find(|r| r.0 == kw.pos.line && r.1 == kw.pos.col))
                    .map(|r| r.3.clone());
                let kw_name = if is_id(kw, "var") { "var" } else { "let" };
                match ty {
                    Some(ty) => format!("{} {}: {}", kw_name, name, ty),
                    None => {
                        // not checked (the file has errors): show the annotation when there is one
                        let line = doc.toks[d].pos.line;
                        let mut e = d;
                        while e + 1 < doc.toks.len() && doc.toks[e + 1].pos.line == line && !is_sym(&doc.toks[e + 1], "=") {
                            e += 1;
                        }
                        format!("{} {}", kw_name, doc.slice(d, e))
                    }
                }
            }
            Some(Decl::Param) => {
                let mut e = d + 1;
                let mut dd = 0i32;
                while e + 1 < doc.toks.len() {
                    let u = &doc.toks[e + 1];
                    if is_sym(u, "(") || is_sym(u, "<") {
                        dd += 1;
                    } else if is_sym(u, ">") {
                        dd -= 1;
                    } else if is_sym(u, ")") {
                        if dd == 0 {
                            break;
                        }
                        dd -= 1;
                    } else if dd == 0 && is_sym(u, ",") {
                        break;
                    }
                    e += 1;
                }
                format!("(parameter) {}", doc.slice(d, e))
            }
            Some(Decl::For) => format!("(loop variable) {}", name),
            None => match item_signature(doc, d) {
                Some(sig) => sig,
                None => return Json::Null,
            },
        };
        hover(text, doc)
    }
}

pub fn run() -> i32 {
    let stdin = std::io::stdin();
    let mut inp = stdin.lock();
    let mut server = Server { docs: HashMap::new(), paths: HashMap::new(), shutdown: false };
    while let Some(msg) = read_message(&mut inp) {
        if !server.handle(&msg) {
            return if server.shutdown { 0 } else { 1 };
        }
    }
    if server.shutdown {
        0
    } else {
        1
    }
}
