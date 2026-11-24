// `yerux lsp`: a minimal language server (LSP over stdio, JSON-RPC 2.0). Design notes: docs/LSP.md.
//
// Diagnostics come from the real front end (module loader, parser, checker) run on the editor's buffers
// (`modules::set_overlay`). Symbols, definition and the shape of hover use the lexer only, so they keep
// working while the file does not compile; the *types* shown by hover come from the checker.
mod ide;

use crate::json::{self, n, obj, s, Json};
use crate::syntax::{self, Tok, Token};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

// ───────────────────────── documents ─────────────────────────

pub struct Doc {
    text: String,
    lines: Vec<Vec<char>>,
    toks: Vec<Token>,
    syms: Vec<Sym>,
    /// checker types of the `let`/`var` bindings (line, col of the keyword, name, type), when it compiled
    types: Option<Vec<(usize, usize, String, String)>>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SymKind {
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

pub struct Sym {
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
        // string interpolation is desugared into calls whose tokens carry the position of the whole literal: the
        // innermost (shortest) token under the cursor is the one the user wrote
        let mut best: Option<(usize, usize)> = None;
        for (k, t) in self.toks.iter().enumerate() {
            if matches!(t.t, Tok::Id(_)) && t.pos.line == l && t.pos.col <= c && c <= t.pos.col + t.pos.len.max(1) - 1 && best.is_none_or(|(_, n)| t.pos.len < n) {
                best = Some((k, t.pos.len));
            }
        }
        best.map(|b| b.0)
    }
}

// ───────────────────────── symbol resolution (lexical) ─────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Decl {
    Let,
    For,
    Param,
    /// a name of a tuple pattern: `let (a, b) = ..`
    Tuple,
    /// a closure parameter: `|x, y| ..`
    Closure,
    /// a name bound by a `match` arm pattern: `Some(v) =>`
    Arm,
}

fn is_operand_end(t: &Token) -> bool {
    match &t.t {
        Tok::Id(s) => !syntax::KW.contains(&s.as_str()) || s == "self" || s == "true" || s == "false",
        Tok::Num(_) | Tok::NumT(..) | Tok::Float(..) | Tok::Str(_) | Tok::Char(_) => true,
        Tok::Sym(s) => *s == ")" || *s == "]",
        Tok::Eof => false,
    }
}

/// For a closure parameter at `i`: the index of the closing `|` of its parameter list.
fn closure_bars(toks: &[Token], i: usize) -> Option<(usize, usize)> {
    let ok = |t: &Token| match &t.t {
        Tok::Id(_) => true,
        Tok::Sym(s) => [",", ":", "<", ">", "(", ")", "[", "]", "&"].contains(s),
        _ => false,
    };
    let mut k = i;
    loop {
        if k == 0 {
            return None;
        }
        k -= 1;
        if is_sym(&toks[k], "|") {
            break;
        }
        if !ok(&toks[k]) || toks[k].pos.line + 3 < toks[i].pos.line {
            return None;
        }
    }
    // the `|` must start a closure, not be a binary or: nothing that ends an operand before it
    if k > 0 && is_operand_end(&toks[k - 1]) && !is_id(&toks[k - 1], "move") {
        return None;
    }
    let mut e = i + 1;
    while e < toks.len() {
        if is_sym(&toks[e], "|") {
            return Some((k, e));
        }
        if !ok(&toks[e]) {
            return None;
        }
        e += 1;
    }
    None
}

/// End (token index) of the body of the closure whose parameter list closes at bar `close`.
fn closure_end(toks: &[Token], close: usize) -> usize {
    if close + 1 >= toks.len() {
        return close;
    }
    if is_sym(&toks[close + 1], "{") {
        return matching(toks, close + 1);
    }
    let line = toks[close].pos.line;
    let mut d = 0i32;
    let mut j = close + 1;
    while j < toks.len() {
        let t = &toks[j];
        if is_sym(t, "(") || is_sym(t, "[") || is_sym(t, "{") {
            d += 1;
        } else if is_sym(t, ")") || is_sym(t, "]") || is_sym(t, "}") {
            d -= 1;
            if d < 0 {
                return j - 1;
            }
        } else if d == 0 && (is_sym(t, ",") || t.pos.line > line) {
            return j - 1;
        }
        j += 1;
    }
    toks.len() - 1
}

/// The `(` that encloses `from` when only pattern tokens lie between them.
fn pattern_open(toks: &[Token], from: usize) -> Option<usize> {
    let mut d = 0i32;
    let mut k = from;
    while k > 0 {
        k -= 1;
        let t = &toks[k];
        if is_sym(t, ")") {
            d += 1;
        } else if is_sym(t, "(") {
            if d == 0 {
                return Some(k);
            }
            d -= 1;
        } else if !(matches!(&t.t, Tok::Id(_)) || is_sym(t, ",")) {
            return None;
        }
    }
    None
}

/// Is token `i` the name of a declaration (`let x`, `for x`, a parameter, a tuple-pattern name, a closure
/// parameter, a name bound by a match arm)?
fn decl_at(doc: &Doc, i: usize) -> Option<Decl> {
    let toks = &doc.toks;
    let name = id_name(&toks[i])?;
    if syntax::KW.contains(&name) && name != "self" {
        return None;
    }
    if i > 0 && (is_id(&toks[i - 1], "let") || is_id(&toks[i - 1], "var")) {
        return Some(Decl::Let);
    }
    // `for x in xs` (not `derive Show for T` / `impl Trait for T`: there the name is followed by no `in`)
    if i > 0
        && (is_id(&toks[i - 1], "for") || (is_id(&toks[i - 1], "mut") && i > 1 && is_id(&toks[i - 2], "for")))
        && toks.get(i + 1).is_some_and(|t| is_id(t, "in"))
    {
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
                    if (j >= 2 && is_id(&toks[j - 2], "fn")) || (j >= 1 && is_id(&toks[j - 1], "fn")) || header_before(toks, j) {
                        return Some(Decl::Param);
                    }
                    break;
                }
                d -= 1;
            } else if is_sym(&toks[j], "{") || is_sym(&toks[j], "}") {
                break;
            }
        }
    }
    // `let (a, b)` / `for (a, b) in`
    if i > 0 && (is_sym(&toks[i - 1], "(") || is_sym(&toks[i - 1], ",") || is_id(&toks[i - 1], "mut")) {
        let mut open = pattern_open(toks, i + 1);
        while let Some(o) = open {
            let before = o.checked_sub(1).map(|b| &toks[b]);
            if let Some(b) = before {
                if is_id(b, "let") || is_id(b, "var") || is_id(b, "for") || (is_id(b, "mut") && o >= 2 && is_id(&toks[o - 2], "for")) {
                    return Some(Decl::Tuple);
                }
                if is_sym(b, "(") || is_sym(b, ",") {
                    open = pattern_open(toks, o);
                    continue;
                }
            }
            break;
        }
    }
    // closure parameter
    if let Some((open, _)) = closure_bars(toks, i) {
        // a parameter *name*, not a name inside a type annotation (`|d: mut Data|`: `Data` is a use)
        let (mut in_type, mut depth) = (false, 0i32);
        for t in &toks[open + 1..i] {
            if is_sym(t, "<") || is_sym(t, "(") || is_sym(t, "[") {
                depth += 1;
            } else if is_sym(t, ">") || is_sym(t, ")") || is_sym(t, "]") {
                depth -= 1;
            } else if depth == 0 && is_sym(t, ":") {
                in_type = true;
            } else if depth == 0 && is_sym(t, ",") {
                in_type = false;
            }
        }
        if !in_type {
            return Some(Decl::Closure);
        }
    }
    // match-arm binding: lowercase name inside a pattern that ends with `=>`
    let first = name.chars().next().unwrap_or('A');
    if (first.is_lowercase() || first == '_') && name != "_" && name != "self" {
        let next_ok = i + 1 < toks.len() && !(is_sym(&toks[i + 1], "(") || is_sym(&toks[i + 1], ".") || is_sym(&toks[i + 1], "{") || is_sym(&toks[i + 1], ":"));
        let prev_ok = i > 0 && !is_sym(&toks[i - 1], ".");
        if next_ok && prev_ok {
            let pat = |t: &Token| matches!(&t.t, Tok::Id(_) | Tok::Num(_) | Tok::Str(_) | Tok::Char(_)) || [",", "(", ")", "|", "..", "..=", "."].iter().any(|s| is_sym(t, s));
            // forward to `=>` through pattern tokens
            let mut e = i + 1;
            while e < toks.len() && pat(&toks[e]) && !is_id(&toks[e], "if") && toks[e].pos.line == toks[i].pos.line {
                e += 1;
            }
            if e < toks.len() && is_sym(&toks[e], "=>") {
                // backward to the arm start: `{`, `}` or a `,` between arms
                let mut d = 0i32;
                let mut k = i;
                while k > 0 {
                    k -= 1;
                    let u = &toks[k];
                    if is_sym(u, ")") {
                        d += 1;
                    } else if is_sym(u, "(") {
                        d -= 1;
                    } else if (is_sym(u, "{") || is_sym(u, "}") || is_sym(u, ",")) && d <= 0 {
                        return Some(Decl::Arm);
                    } else if !pat(u) {
                        break;
                    }
                }
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

/// The innermost function or method whose text contains token `i`.
fn enclosing_fn(doc: &Doc, i: usize) -> Option<&Sym> {
    let mut flat = vec![];
    all_syms(&doc.syms, &mut flat);
    flat.into_iter()
        .filter(|s| matches!(s.kind, SymKind::Fn | SymKind::Method) && s.start <= i && i <= s.end)
        .min_by_key(|s| s.end - s.start)
}

/// Declarations (token indices of their names) visible at token `i`, nearest first: the locals of the
/// enclosing blocks of its function, then its parameters.
fn visible_locals(doc: &Doc, i: usize) -> Vec<usize> {
    let toks = &doc.toks;
    let mut out = vec![];
    let Some(f) = enclosing_fn(doc, i) else {
        return out;
    };
    let mut depth = 0i32; // closed blocks we are skipping
    let mut skip_line = 0usize; // header line of a closed block: its `for x in ..`/`Some(v) =>` names are not visible
    let mut j = i;
    while j > f.start {
        j -= 1;
        let t = &toks[j];
        if is_sym(t, "}") {
            depth += 1;
        } else if is_sym(t, "{") {
            if depth > 0 {
                depth -= 1;
                // a struct literal (`Point { x: 1 }`) is not a block with a header of its own
                let literal = j > 0 && id_name(&toks[j - 1]).is_some_and(|x| x.chars().next().is_some_and(|c| c.is_uppercase()));
                if depth == 0 && !literal {
                    skip_line = t.pos.line;
                }
            }
        } else if depth == 0 && id_name(t).is_some() && (t.pos.line != skip_line || decl_at(doc, j) == Some(Decl::Let)) {
            // (a `let x = if c { a } else { b }` on the line of a closed block still declares `x`)
            match decl_at(doc, j) {
                Some(Decl::Param) | None => {}
                // `let keys = keys(db)`: the new name is not visible in its own initializer
                Some(Decl::Let) if toks[j].pos.line == toks[i].pos.line => {}
                Some(Decl::Closure) => {
                    if let Some((_, close)) = closure_bars(toks, j) {
                        if i <= closure_end(toks, close) {
                            out.push(j);
                        }
                    }
                }
                Some(_) => out.push(j),
            }
        }
    }
    for j in f.start..i {
        if is_sym(&toks[j], "{") {
            break;
        }
        if id_name(&toks[j]).is_some() && decl_at(doc, j) == Some(Decl::Param) {
            out.push(j);
        }
    }
    out
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
            i = j.max(i + 1);
            continue;
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
fn analyze(path: &str, docs: &HashMap<String, Doc>, uri_paths: &HashMap<String, String>, replace: Option<String>) -> Analysis {
    let mut overlay: Vec<(String, String)> = docs
        .iter()
        .filter_map(|(u, d)| uri_paths.get(u).map(|p| (p.clone(), d.text.clone())))
        .collect();
    if let Some(text) = replace {
        overlay.retain(|(p, _)| p != path);
        overlay.push((path.to_string(), text));
    }
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
    /// documents whose diagnostics are waiting for the typing pause, and when it ends
    pending: Vec<String>,
    deadline: Option<Instant>,
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
    // an unparsable body is skipped (Null has no method and no id), it does not end the session
    Some(json::parse(&String::from_utf8_lossy(&buf)).unwrap_or(Json::Null))
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
            doc.slice(sym.start, sym.end)
        }
        SymKind::Const | SymKind::Alias => doc.slice(sym.start, sym.end),
    })
}

impl Server {
    fn code_actions(&self, uri: &str, params: &Json) -> Json {
        let Some(doc) = self.docs.get(uri) else { return Json::Arr(vec![]) };
        let diagnostics = params.get("context").and_then(|c| c.get("diagnostics")).and_then(Json::arr).unwrap_or(&[]);
        let mut actions = Vec::new();
        for diagnostic in diagnostics {
            let message = diagnostic.get("message").and_then(Json::str).unwrap_or("");
            if let Some((ty, trait_name)) = message.split_once(" does not implement ") {
                let ty = ty.rsplit(' ').next().unwrap_or(ty).trim_matches('`');
                let trait_name = trait_name.split(|c: char| !c.is_alphanumeric() && c != '_').next().unwrap_or("");
                if ["Clone", "Equal", "Show"].contains(&trait_name) && doc.syms.iter().any(|sym| sym.name == ty && matches!(sym.kind, SymKind::Struct | SymKind::Enum)) && !doc.text.contains(&format!("derive {} for {}", trait_name, ty)) {
                    let line = doc.lines.len().saturating_sub(1);
                    let column = doc.lines.last().map_or(0, |chars| chars.iter().map(|c| c.len_utf16()).sum());
                    let edit = obj(vec![("changes", obj(vec![(uri, Json::Arr(vec![obj(vec![
                        ("range", obj(vec![("start", obj(vec![("line", n(line)), ("character", n(column))])), ("end", obj(vec![("line", n(line)), ("character", n(column))]))])),
                        ("newText", s(format!("\nderive {} for {}\n", trait_name, ty))),
                    ])]))]))]);
                    actions.push(obj(vec![("title", s(format!("Derive {} for {}", trait_name, ty))), ("kind", s("quickfix")), ("edit", edit)]));
                }
            }
            let Some(name) = message.strip_prefix("variable '").and_then(|m| m.split_once("' does not exist").map(|x| x.0)) else { continue };
            for module in crate::stdlib::MODULES {
                let Some(source) = crate::stdlib::source(module) else { continue };
                if !source.lines().any(|line| line.trim_start().starts_with(&format!("pub fn {}(", name))) { continue; }
                if doc.text.lines().any(|line| line.trim() == format!("import {}", module)) { continue; }
                let edit = obj(vec![("changes", obj(vec![(uri, Json::Arr(vec![obj(vec![
                    ("range", obj(vec![("start", obj(vec![("line", n(0)), ("character", n(0))])), ("end", obj(vec![("line", n(0)), ("character", n(0))]))])),
                    ("newText", s(format!("import {}\n", module))),
                ])]))]))]);
                actions.push(obj(vec![("title", s(format!("Import {} from {}", name, module))), ("kind", s("quickfix")), ("edit", edit)]));
            }
        }
        Json::Arr(actions)
    }

    fn publish(&mut self, uri: &str) {
        let path = match self.paths.get(uri) {
            Some(p) => p.clone(),
            None => return,
        };
        let an = analyze(&path, &self.docs, &self.paths, None);
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
        self.pending.retain(|u| u != uri);
        self.publish(uri);
    }

    /// Publish the diagnostics that were waiting for the typing pause.
    fn flush(&mut self) {
        self.deadline = None;
        for u in std::mem::take(&mut self.pending) {
            if self.docs.contains_key(&u) {
                self.publish(&u);
            }
        }
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
                            obj(vec![("openClose", Json::Bool(true)), ("change", n(2)), ("save", Json::Bool(true))]),
                        ),
                        (
                            "completionProvider",
                            obj(vec![("triggerCharacters", Json::Arr(vec![s(".")])), ("resolveProvider", Json::Bool(false))]),
                        ),
                        (
                            "signatureHelpProvider",
                            obj(vec![("triggerCharacters", Json::Arr(vec![s("("), s(",")]))]),
                        ),
                        ("referencesProvider", Json::Bool(true)),
                        ("renameProvider", obj(vec![("prepareProvider", Json::Bool(true))])),
                        (
                            "semanticTokensProvider",
                            obj(vec![
                                (
                                    "legend",
                                    obj(vec![
                                        ("tokenTypes", Json::Arr(ide::TOKEN_TYPES.iter().map(|x| s(*x)).collect())),
                                        ("tokenModifiers", Json::Arr(ide::TOKEN_MODIFIERS.iter().map(|x| s(*x)).collect())),
                                    ]),
                                ),
                                ("full", Json::Bool(true)),
                            ]),
                        ),
                        ("documentSymbolProvider", Json::Bool(true)),
                        ("codeActionProvider", Json::Bool(true)),
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
                if let Some(doc) = self.docs.get(&uri) {
                    let mut text = doc.text.clone();
                    for ch in params.get("contentChanges").and_then(|c| c.arr()).unwrap_or(&[]) {
                        let Some(new) = ch.get("text").and_then(|x| x.str()) else { continue };
                        match ch.get("range") {
                            Some(r) => {
                                let pos = |k: &str| -> Option<(usize, usize)> {
                                    Some((r.path(&format!("{}.line", k))?.int()? as usize, r.path(&format!("{}.character", k))?.int()? as usize))
                                };
                                if let (Some(a), Some(b)) = (pos("start"), pos("end")) {
                                    let (oa, ob) = (byte_offset(&text, a), byte_offset(&text, b));
                                    text.replace_range(oa..ob.max(oa), new);
                                }
                            }
                            None => text = new.to_string(),
                        }
                    }
                    self.docs.insert(uri.clone(), Doc::new(text));
                    // diagnostics wait for a pause in typing
                    if !self.pending.contains(&uri) {
                        self.pending.push(uri.clone());
                    }
                    self.deadline = Some(Instant::now() + debounce());
                }
                None
            }
            "textDocument/didSave" => {
                self.pending.retain(|u| *u != uri);
                if self.docs.contains_key(&uri) {
                    self.publish(&uri);
                }
                None
            }
            "textDocument/didClose" => {
                self.pending.retain(|u| *u != uri);
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
            "textDocument/completion" => Some(self.completion(&uri, &params)),
            "textDocument/signatureHelp" => Some(self.at_cursor(&uri, &params, ide::signature_help)),
            "textDocument/references" => Some(self.references(&uri, &params)),
            "textDocument/prepareRename" | "textDocument/rename" => {
                let r = self.rename(&uri, &params, method == "textDocument/rename");
                match (r, id) {
                    (Ok(v), _) => Some(v),
                    (Err(m), Some(id)) => {
                        reply_err(id, -32803, &m);
                        return true;
                    }
                    (Err(_), None) => return true,
                }
            }
            "textDocument/semanticTokens/full" => Some(match self.ctx(&uri) {
                Some(c) => ide::semantic_tokens(&c),
                None => Json::Null,
            }),
            "textDocument/hover" => Some(self.hover(&uri, &params)),
            "textDocument/codeAction" => Some(self.code_actions(&uri, &params)),
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

    fn cursor(&self, params: &Json) -> Option<(usize, usize)> {
        Some((params.path("position.line")?.int()? as usize, params.path("position.character")?.int()? as usize))
    }

    fn id_under_cursor(&self, uri: &str, params: &Json) -> Option<usize> {
        let doc = self.docs.get(uri)?;
        let (line, ch) = self.cursor(params)?;
        doc.id_at(line, ch)
    }

    /// Make sure the checker's types are known for the document (they are dropped by every edit).
    fn ensure_types(&mut self, uri: &str) {
        if self.docs.get(uri).is_some_and(|d| d.types.is_none()) {
            if let Some(path) = self.paths.get(uri).cloned() {
                let an = analyze(&path, &self.docs, &self.paths, None);
                if let Some(d) = self.docs.get_mut(uri) {
                    d.types = an.types;
                }
            }
        }
    }

    fn at_cursor(&self, uri: &str, params: &Json, f: fn(&ide::Ctx, usize, usize) -> Json) -> Json {
        match (self.ctx(uri), self.cursor(params)) {
            (Some(c), Some((l, ch))) => f(&c, l, ch),
            _ => Json::Null,
        }
    }

    fn definition(&self, uri: &str, params: &Json) -> Json {
        let (Some(i), Some(ctx)) = (self.id_under_cursor(uri, params), self.ctx(uri)) else {
            return Json::Null;
        };
        match ide::resolve(&ctx, 0, i) {
            Some(l) => ide::loc_json(&ctx, &l),
            None => Json::Null,
        }
    }

    fn hover(&mut self, uri: &str, params: &Json) -> Json {
        self.ensure_types(uri);
        let (Some(i), Some(ctx)) = (self.id_under_cursor(uri, params), self.ctx(uri)) else {
            return Json::Null;
        };
        let Some(l) = ide::resolve(&ctx, 0, i) else {
            return Json::Null;
        };
        let Some(text) = ide::describe(&ctx, &l) else {
            return Json::Null;
        };
        obj(vec![
            ("contents", obj(vec![("kind", s("markdown")), ("value", s(format!("```yerux\n{}\n```", text)))])),
            ("range", ctx.doc(0).tok_range(i)),
        ])
    }

    fn completion(&mut self, uri: &str, params: &Json) -> Json {
        self.ensure_types(uri);
        // typing `p.na` leaves a file that does not compile; the types come from the text without the
        // member being typed
        if self.docs.get(uri).is_some_and(|d| d.types.is_none()) {
            if let (Some((line, ch)), Some(path)) = (self.cursor(params), self.paths.get(uri).cloned()) {
                let doc = &self.docs[uri];
                let cs: Vec<char> = doc.lines.get(line).cloned().unwrap_or_default();
                let (_, c) = doc.from_lsp(line, ch);
                let mut a = (c - 1).min(cs.len());
                while a > 0 && (cs[a - 1].is_alphanumeric() || cs[a - 1] == '_') {
                    a -= 1;
                }
                if a > 0 && cs[a - 1] == '.' {
                    // the line being typed is dropped; the lets before it keep their types
                    let mut text = String::new();
                    for (k, l) in doc.lines.iter().enumerate() {
                        if k > 0 {
                            text.push('\n');
                        }
                        if k != line {
                            text.extend(l);
                        }
                    }
                    let an = analyze(&path, &self.docs, &self.paths, Some(text));
                    if let Some(d) = self.docs.get_mut(uri) {
                        d.types = an.types;
                    }
                }
            }
        }
        self.at_cursor(uri, params, ide::completion)
    }

    fn references(&self, uri: &str, params: &Json) -> Json {
        let (Some(i), Some(ctx)) = (self.id_under_cursor(uri, params), self.ctx(uri)) else {
            return Json::Null;
        };
        let Some(target) = ide::resolve(&ctx, 0, i) else {
            return Json::Arr(vec![]);
        };
        let include = params.path("context.includeDeclaration").map_or(true, |b| *b != Json::Bool(false));
        let turi = ctx.docs[target.doc].uri.clone();
        let mut out = vec![];
        let mut refs = ide::references(self, uri, &target, &ctx);
        if self.docs.contains_key(&turi) && !refs.iter().any(|(u, j, _)| *u == turi && *j == target.tok) {
            refs.push((turi.clone(), target.tok, true));
        }
        refs.sort();
        for (u, j, _) in refs {
            if !include && u == turi && j == target.tok {
                continue;
            }
            out.push(obj(vec![("uri", s(&u)), ("range", self.docs[&u].tok_range(j))]));
        }
        Json::Arr(out)
    }

    fn rename(&self, uri: &str, params: &Json, apply: bool) -> Result<Json, String> {
        let i = self.id_under_cursor(uri, params).ok_or("nothing to rename here")?;
        let ctx = self.ctx(uri).ok_or("unknown document")?;
        let fail = |e: ide::RenameError| match e {
            ide::RenameError::Msg(m) => m,
        };
        if !apply {
            let (_, range) = ide::prepare_rename(&ctx, i).map_err(fail)?;
            let name = id_name(&ctx.doc(0).toks[i]).unwrap_or("").to_string();
            return Ok(obj(vec![("range", range), ("placeholder", s(name))]));
        }
        let new = params.get("newName").and_then(|x| x.str()).ok_or("missing newName")?;
        ide::rename(self, uri, i, new).map_err(fail)
    }
}

/// Typing pause before diagnostics are recomputed (`YERUX_LSP_DEBOUNCE_MS` overrides it).
fn debounce() -> Duration {
    let ms = std::env::var("YERUX_LSP_DEBOUNCE_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
    Duration::from_millis(ms)
}

/// Byte offset in `text` of an LSP position (line, UTF-16 column).
fn byte_offset(text: &str, (line, col): (usize, usize)) -> usize {
    let mut off = 0;
    for (k, l) in text.split('\n').enumerate() {
        if k == line {
            let mut u = 0;
            for (b, c) in l.char_indices() {
                if u >= col {
                    return off + b;
                }
                u += c.len_utf16();
            }
            return off + l.len();
        }
        off += l.len() + 1;
    }
    text.len()
}

pub fn run() -> i32 {
    // a reader thread feeds the main loop, which can then wake up for the typing-pause deadline
    let (tx, rx) = mpsc::channel::<Json>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut inp = stdin.lock();
        while let Some(msg) = read_message(&mut inp) {
            if tx.send(msg).is_err() {
                return;
            }
        }
    });
    let mut server = Server { docs: HashMap::new(), paths: HashMap::new(), shutdown: false, pending: vec![], deadline: None };
    loop {
        let msg = match server.deadline {
            Some(d) => match rx.recv_timeout(d.saturating_duration_since(Instant::now())) {
                Ok(m) => m,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    server.flush();
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(m) => m,
                Err(_) => break,
            },
        };
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
