// yerux fmt: the one canonical layout for Yerux source (like gofmt: no options).
//
// It works on tokens (comments included), not on the AST, so it keeps working when the grammar grows.
// Decisions it takes:
//   * 2-space indentation, one statement per line, no `;` between statements, `} else {` on one line;
//   * a block that is written on one line in the source stays on one line (`Circle(r) => { return r }`),
//     a block with a newline or a comment inside is expanded one item per line;
//   * exactly one blank line between top-level items, imports grouped on top, blank lines inside blocks
//     collapsed to at most one, none right after `{` or before `}`;
//   * single spaces around binary operators, none inside parentheses, `a.b`, `f(x)`, `List<Int>`;
//   * a line that ends with a binary operator or `=` keeps its break; the next line is indented once more;
//     newlines inside parentheses are joined;
//   * comments are kept as written (trailing comments stay on their line).
// The result is idempotent, and `format_source` refuses to return text whose tokens differ from the input.
use crate::syntax::{lex, Tok};

#[derive(Clone, Copy, PartialEq, Debug)]
enum K {
    Word,
    Num,
    Str,
    Sym,
    Comment,
}

#[derive(Clone, Debug)]
struct T {
    k: K,
    s: String,
    nl: usize, // newlines between the previous token and this one
    line: usize,
}

const SYMS3: &[&str] = &["..="];
const SYMS2: &[&str] = &["==", "!=", "<=", ">=", "&&", "||", "->", "=>", ".."];
const SYMS1: &str = "(){},:;=<>+-*/%!.|@?";
const KEYWORDS: &[&str] = &[
    "fn", "let", "var", "if", "else", "while", "return", "move", "view", "mut", "struct", "enum", "match", "defer", "import", "pub", "test",
    "as", "impl", "trait", "dyn", "for", "in", "break", "continue",
];
const BINOPS: &[&str] = &["+", "-", "*", "/", "%", "==", "!=", "<", ">", "<=", ">=", "&&", "||", "="];

fn tokenize(src: &str) -> Result<Vec<T>, String> {
    let cs: Vec<char> = src.chars().collect();
    let (mut i, mut line, mut nl) = (0usize, 1usize, 0usize);
    let mut out = vec![];
    while i < cs.len() {
        let c = cs[i];
        if c == '\n' {
            i += 1;
            line += 1;
            nl += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let (k, s);
        if c == '/' && cs.get(i + 1) == Some(&'/') {
            while i < cs.len() && cs[i] != '\n' {
                i += 1;
            }
            k = K::Comment;
            s = cs[start..i].iter().collect::<String>().trim_end().to_string();
        } else if c.is_ascii_digit() {
            let after_dot = out.last().map_or(false, |t: &T| t.k == K::Sym && t.s == ".");
            let scan = |i: &mut usize| {
                while *i < cs.len() && (cs[*i].is_ascii_alphanumeric() || cs[*i] == '_') {
                    *i += 1;
                }
            };
            scan(&mut i);
            let dec = |t: &[char]| t.iter().all(|c| c.is_ascii_digit() || *c == '_');
            let next_digit = |j: usize| cs.get(j).map_or(false, |c| c.is_ascii_digit());
            // float literals: `1.5`, `1e-3`, `2.5e+10f32` (hex / binary literals have no fractions or exponents)
            if !after_dot && dec(&cs[start..i]) && cs.get(i) == Some(&'.') && next_digit(i + 1) {
                i += 1;
                scan(&mut i);
            }
            let txt: String = cs[start..i].iter().collect();
            let hexbin = txt.starts_with("0x") || txt.starts_with("0X") || txt.starts_with("0b") || txt.starts_with("0B");
            if !after_dot
                && !hexbin
                && (txt.ends_with('e') || txt.ends_with('E'))
                && txt[..txt.len() - 1].chars().all(|c| c.is_ascii_digit() || c == '_' || c == '.')
                && matches!(cs.get(i), Some('+') | Some('-'))
                && next_digit(i + 1)
            {
                i += 1;
                scan(&mut i);
            }
            k = K::Num;
            s = cs[start..i].iter().collect();
        } else if c.is_ascii_alphabetic() || c == '_' {
            while i < cs.len() && (cs[i].is_ascii_alphanumeric() || cs[i] == '_') {
                i += 1;
            }
            k = K::Word;
            s = cs[start..i].iter().collect();
        } else if c == '\'' {
            // character literal: 'a', '\n', '\u{1F600}'
            i += 1;
            if cs.get(i) == Some(&'\\') {
                i += 1;
            }
            while i < cs.len() && cs[i] != '\'' && cs[i] != '\n' {
                i += 1;
            }
            if i >= cs.len() || cs[i] != '\'' {
                return Err(format!("line {}: unterminated character literal", line));
            }
            i += 1;
            k = K::Str;
            s = cs[start..i].iter().collect();
        } else if c == '"' {
            i += 1;
            while i < cs.len() && cs[i] != '"' && cs[i] != '\n' {
                i += if cs[i] == '\\' { 2 } else { 1 };
            }
            if i >= cs.len() || cs[i] != '"' {
                return Err(format!("line {}: unterminated string", line));
            }
            i += 1;
            k = K::Str;
            s = cs[start..i].iter().collect();
        } else {
            let two: String = cs[i..(i + 2).min(cs.len())].iter().collect();
            let three: String = cs[i..(i + 3).min(cs.len())].iter().collect();
            if SYMS3.contains(&three.as_str()) {
                i += 3;
                s = three;
            } else if SYMS2.contains(&two.as_str()) {
                i += 2;
                s = two;
            } else if SYMS1.contains(c) {
                i += 1;
                s = c.to_string();
            } else {
                return Err(format!("line {}: unexpected character '{}'", line, c));
            }
            k = K::Sym;
        }
        out.push(T { k, s, nl, line });
        nl = 0;
    }
    Ok(out)
}

#[derive(Clone, Copy, PartialEq)]
enum Br {
    Block,
    Decl,
    Lit,
}

struct Open {
    kind: Br,
    multi: bool,
    pd: i32,
    cont: bool,
}

struct F<'a> {
    toks: &'a [T],
    out: String,
    at_start: bool,
    stack: Vec<Open>,
    pd: i32,
    cont: bool,
}

impl<'a> F<'a> {
    fn depth(&self) -> usize {
        self.stack.iter().filter(|o| o.multi).count()
    }
    fn write(&mut self, s: &str) {
        if self.at_start {
            let n = self.depth() + usize::from(self.cont);
            for _ in 0..n {
                self.out.push_str("  ");
            }
            self.at_start = false;
        }
        self.out.push_str(s);
    }
    fn brk(&mut self, blank: bool) {
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        if !self.out.is_empty() {
            self.out.push('\n');
            if blank {
                self.out.push('\n');
            }
        }
        self.at_start = true;
    }
}

fn is_kw(s: &str) -> bool {
    KEYWORDS.contains(&s)
}

/// Matching `}` for every `{`.
fn match_braces(toks: &[T]) -> Result<Vec<usize>, String> {
    let mut m = vec![usize::MAX; toks.len()];
    let mut st = vec![];
    for (i, t) in toks.iter().enumerate() {
        if t.k != K::Sym {
            continue;
        }
        if t.s == "{" {
            st.push(i);
        } else if t.s == "}" {
            match st.pop() {
                Some(o) => {
                    m[o] = i;
                    m[i] = o;
                }
                None => return Err(format!("line {}: this `}}` has no matching `{{`", t.line)),
            }
        }
    }
    if let Some(o) = st.pop() {
        return Err(format!("line {}: this `{{` is never closed", toks[o].line));
    }
    Ok(m)
}

/// Which `<` / `>` are generic brackets (`List<Int>`, `fn id<T>`) and not comparisons.
fn generics(toks: &[T]) -> Vec<bool> {
    let mut g = vec![false; toks.len()];
    for i in 1..toks.len() {
        if !(toks[i].k == K::Sym && toks[i].s == "<") || toks[i - 1].k != K::Word {
            continue;
        }
        let pw = &toks[i - 1].s;
        let upper = pw.chars().next().map_or(false, |c| c.is_ascii_uppercase());
        let decl = pw == "impl" || (i >= 2 && toks[i - 2].k == K::Word && matches!(toks[i - 2].s.as_str(), "fn" | "struct" | "enum" | "trait" | "impl"));
        if !(upper || decl) {
            continue;
        }
        let mut depth = 0i32;
        let mut j = i;
        let mut close = None;
        while j < toks.len() {
            let t = &toks[j];
            match (t.k, t.s.as_str()) {
                (K::Sym, "<") => depth += 1,
                (K::Sym, ">") => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(j);
                        break;
                    }
                }
                (K::Word, _) | (K::Sym, ",") | (K::Sym, ":") | (K::Sym, "+") | (K::Sym, ".") => {}
                // range bounds: `Int<0..255>`, `Int<-5..>`, `Int<..9>`
                (K::Num, _) | (K::Sym, "..") | (K::Sym, "-") if pw == "Int" => {}
                // function types as type arguments: `List<fn(Int) -> Int>`
                (K::Sym, "(") | (K::Sym, ")") | (K::Sym, "->") if toks[i + 1..j].iter().any(|x| x.k == K::Word && x.s == "fn") => {}
                _ => break,
            }
            j += 1;
        }
        if let Some(c) = close {
            let ok = match toks.get(c + 1) {
                None => true,
                Some(n) => n.nl > 0 || n.k == K::Comment || (n.k == K::Word && n.s == "else") || (n.k == K::Sym && matches!(n.s.as_str(), "(" | "." | "{" | "}" | "," | ")" | "=" | ">" | ";")),
            };
            if ok {
                g[i] = true;
                g[c] = true;
            }
        }
    }
    g
}

fn is_sym(t: &T, s: &str) -> bool {
    t.k == K::Sym && t.s == s
}

fn format_tokens(toks: &[T]) -> Result<String, String> {
    let mb = match_braces(toks)?;
    let gen = generics(toks);
    // `{` ... `}` is written on several lines when anything inside it starts a new line or is a comment
    let mut multi = vec![false; toks.len()];
    for i in 0..toks.len() {
        if is_sym(&toks[i], "{") {
            multi[i] = mb[i] > i + 1 && toks[i + 1..mb[i]].iter().any(|t| t.nl > 0 || t.k == K::Comment) || toks.get(i + 1).map_or(false, |t| t.k == K::Comment);
        }
    }
    let struct_names: Vec<&str> = toks
        .windows(2)
        .filter(|w| w[0].k == K::Word && w[0].s == "struct" && w[1].k == K::Word)
        .map(|w| w[1].s.as_str())
        .collect();
    // unary operators: `!x`, `-x`
    let mut unary = vec![false; toks.len()];
    for i in 0..toks.len() {
        let t = &toks[i];
        if t.k != K::Sym {
            continue;
        }
        if t.s == "!" {
            unary[i] = true;
        } else if t.s == "-" {
            let prev = toks[..i].iter().rev().find(|p| p.k != K::Comment);
            unary[i] = match prev {
                None => true,
                Some(p) => match p.k {
                    K::Sym => !(p.s == ")" || (p.s == ">" && gen[toks[..i].iter().rposition(|q| q.k != K::Comment).unwrap()])),
                    K::Word => is_kw(&p.s) && p.s != "as",
                    _ => false,
                },
            };
        }
    }

    // closure bars: `|x, y| body` opens with a `|` that follows `(`, `,`, `=`, `move`, `return`, ... (never an operand,
    // which would make it a pattern alternative `A | B`), and closes at the next `|`
    let mut bar_open = vec![false; toks.len()];
    let mut bar_close = vec![false; toks.len()];
    for i in 0..toks.len() {
        if !(toks[i].k == K::Sym && (toks[i].s == "|" || toks[i].s == "||")) || bar_close[i] {
            continue;
        }
        let prev = toks[..i].iter().rev().find(|p| p.k != K::Comment);
        let opens = match prev {
            None => false,
            Some(p) => match p.k {
                K::Sym => !matches!(p.s.as_str(), ")" | "]" | "}"),
                K::Word => matches!(p.s.as_str(), "move" | "return" | "in"),
                _ => false,
            },
        };
        if opens && toks[i].s == "|" {
            if let Some(j) = (i + 1..toks.len()).find(|&j| is_sym(&toks[j], "|")) {
                bar_open[i] = true;
                bar_close[j] = true;
            }
        }
    }

    let mut f = F { toks, out: String::new(), at_start: true, stack: vec![], pd: 0, cont: false };
    let mut prev: Option<usize> = None; // previous emitted non-comment token
    let mut prev_comment = false; // the last thing written was a comment
    let mut just_opened = false; // right after a multi-line `{`
    let mut top_item_end = false;
    let mut in_import = false;
    let mut pending_hard = false; // a `;` was dropped
    let mut pending_semi = false; // an inline `;` waits for the next token
    let mut decl_pending = false;
    let mut last_closed: Option<(Br, bool)> = None; // kind of the `}` just written

    for (i, t) in toks.iter().enumerate() {
        let ctx_multi = f.stack.last().map_or(true, |o| o.multi);
        let ctx_kind = f.stack.last().map(|o| o.kind);

        // ── comments ──
        if t.k == K::Comment {
            let trailing = t.nl == 0 && !f.at_start && !f.out.is_empty();
            if trailing {
                f.out.push(' ');
                f.out.push_str(&t.s);
            } else {
                let mut blank = t.nl >= 2 && !just_opened && !f.out.is_empty();
                if top_item_end || in_import {
                    blank = true;
                }
                if in_import {
                    in_import = false;
                }
                f.brk(blank);
                if !trailing && t.nl > 0 {
                    f.cont = false;
                }
                f.write(&t.s);
                top_item_end = false;
            }
            prev_comment = true;
            just_opened = false;
            pending_semi = false;
            continue;
        }

        // ── `;` ──
        if is_sym(t, ";") && f.pd == 0 {
            if ctx_kind == Some(Br::Block) && !ctx_multi {
                pending_semi = true;
            } else {
                pending_hard = true;
            }
            continue;
        }

        let p = prev.map(|j| &toks[j]);
        let is_close = is_sym(t, "}");

        // ── decide the break before this token ──
        #[derive(PartialEq)]
        enum B {
            None,
            Hard(bool),
            Cont,
        }
        let mut b = B::None;
        let src_blank = t.nl >= 2 && !just_opened;
        if in_import {
            if t.k == K::Word && (t.s == "import" || t.s == "type") {
                b = B::Hard(false);
                in_import = false;
            } else if t.nl > 0 {
                b = B::Hard(true);
                in_import = false;
            }
        }
        if b == B::None {
            if is_close && f.stack.last().map_or(false, |o| o.multi) {
                b = B::Hard(false);
            } else if pending_hard {
                b = B::Hard(src_blank);
            } else if top_item_end {
                b = B::Hard(true);
            } else if prev_comment {
                b = if f.pd > 0 { B::Cont } else { B::Hard(src_blank) };
            } else if just_opened {
                b = B::Hard(false);
            } else if f.pd == 0
                && matches!(ctx_kind, Some(Br::Decl) | Some(Br::Lit))
                && ctx_multi
                && p.map_or(false, |p| is_sym(p, ","))
            {
                b = B::Hard(src_blank); // one field / variant / initializer per line
            } else if let Some((kind, _)) = last_closed {
                // after a `}` that ends a block: a new statement, unless `else`, `,`, `)` continue it
                let continues = t.k == K::Sym && matches!(t.s.as_str(), "," | ")" | "." ) || (t.k == K::Word && t.s == "else");
                // inside a block written on one line the statements stay on that line
                if kind != Br::Lit && !continues && !is_close && ctx_multi {
                    b = B::Hard(src_blank);
                }
            }
        }
        if b == B::None && t.nl > 0 && !f.out.is_empty() {
            let pt = p;
            if f.pd > 0 {
                // join
            } else if ctx_kind == Some(Br::Lit) {
                if pt.map_or(false, |p| is_sym(p, ",")) {
                    b = B::Hard(src_blank);
                }
            } else if t.k == K::Word && t.s == "else" || is_sym(t, "{") {
                // join
            } else if is_sym(t, ".") {
                b = B::Cont;
            } else if pt.map_or(false, |p| p.k == K::Sym && BINOPS.contains(&p.s.as_str()) && !(p.s == ">" && prev.map_or(false, |j| gen[j]))) {
                b = B::Cont;
            } else if pt.map_or(false, |p| p.k == K::Sym && matches!(p.s.as_str(), "->" | "=>" | ".")) {
                // join
            } else {
                b = B::Hard(src_blank);
            }
        }
        if in_import && b != B::None {
            in_import = false;
        }

        // closing brace: pop before writing so the indent is the outer one
        let mut closing: Option<Open> = None;
        if is_close {
            closing = f.stack.pop();
            if let Some(o) = &closing {
                f.pd = o.pd;
            }
        }
        match b {
            B::Hard(blank) => {
                let blank = blank && !f.out.is_empty();
                f.brk(blank);
                f.cont = false;
            }
            B::Cont => {
                f.brk(false);
                f.cont = true;
            }
            B::None => {
                if !f.out.is_empty() && !f.at_start {
                    // inline: a pending `;`, then the usual spacing
                    if pending_semi && !is_close {
                        f.out.push(';');
                    }
                    let sp = match p {
                        None => false,
                        // inside closure bars: `|x, y|`
                        Some(_) if bar_close[i] => false,
                        Some(_) if prev.map_or(false, |j| bar_open[j]) => false,
                        Some(p) => space(p, t, prev.map_or(false, |j| gen[j]), gen[i], unary[prev.unwrap_or(0)], is_close, closing.as_ref().map_or(false, |o| o.multi)),
                    };
                    if sp {
                        f.out.push(' ');
                    }
                }
            }
        }
        if closing.is_some() {
            if let Some(o) = &closing {
                f.cont = o.cont;
            }
        }
        pending_hard = false;
        pending_semi = false;
        top_item_end = false;
        let was_import_start = t.k == K::Word && (t.s == "import" || t.s == "type") && f.stack.is_empty() && f.at_start;
        f.write(&t.s);
        prev_comment = false;
        just_opened = false;
        last_closed = None;
        if was_import_start {
            in_import = true;
        }

        // ── bookkeeping after the token ──
        match (t.k, t.s.as_str()) {
            (K::Sym, "(") => f.pd += 1,
            (K::Sym, ")") => f.pd -= 1,
            (K::Word, "struct") | (K::Word, "enum") if f.stack.is_empty() || ctx_kind == Some(Br::Block) => decl_pending = true,
            (K::Sym, "{") => {
                let kind = if decl_pending && f.pd == 0 {
                    Br::Decl
                } else {
                    let lit = p.map_or(false, |p| {
                        p.k == K::Word && struct_names.contains(&p.s.as_str()) && {
                            let pp = prev.and_then(|j| if j > 0 { Some(&toks[j - 1]) } else { None });
                            !pp.map_or(false, |x| x.k == K::Sym && x.s == "->" || x.k == K::Word && matches!(x.s.as_str(), "impl" | "for" | "struct" | "enum"))
                        }
                    });
                    if lit { Br::Lit } else { Br::Block }
                };
                decl_pending = false;
                f.stack.push(Open { kind, multi: multi[i], pd: f.pd, cont: f.cont });
                f.pd = 0;
                f.cont = false;
                just_opened = multi[i];
            }
            (K::Sym, "}") => {
                let o = closing.take().unwrap();
                last_closed = Some((o.kind, o.multi));
                if f.stack.is_empty() && o.kind != Br::Lit {
                    top_item_end = true;
                }
            }
            _ => {}
        }
        prev = Some(i);
    }
    while f.out.ends_with('\n') || f.out.ends_with(' ') {
        f.out.pop();
    }
    if !f.out.is_empty() {
        f.out.push('\n');
    }
    Ok(f.out)
}

/// Is there a space between two tokens written on the same line?
fn space(p: &T, t: &T, p_gen: bool, t_gen: bool, p_unary: bool, t_close: bool, close_multi: bool) -> bool {
    let _ = close_multi;
    // ranges: `0..n`, `1..=9`
    if (p.k == K::Sym && matches!(p.s.as_str(), ".." | "..=")) || (t.k == K::Sym && matches!(t.s.as_str(), ".." | "..=")) {
        // an open range pattern before `=>` (`100.. => ..`): `..=>` would lex as `..=` `>`
        return p.k == K::Sym && p.s == ".." && t.k == K::Sym && t.s.starts_with('=');
    }
    // after `p`
    if p.k == K::Sym {
        match p.s.as_str() {
            "(" | "." => return false,
            "!" => return false,
            "-" if p_unary => return false,
            "<" if p_gen => return false,
            "{" => return !(t_close),
            _ => {}
        }
    }
    // before `t`
    if t.k == K::Sym {
        match t.s.as_str() {
            "," | ")" | ";" | "." | ":" | "?" => return false,
            "(" => {
                return match p.k {
                    K::Word => is_kw(&p.s) && p.s != "fn",
                    K::Sym => !(p.s == ")" || (p.s == ">" && p_gen)),
                    _ => false,
                }
            }
            "<" if t_gen => return false,
            ">" if t_gen => return false,
            "{" => return true,
            "}" => return !(p.k == K::Sym && p.s == "{"),
            _ => {}
        }
    }
    true
}

/// Canonical text for a Yerux source file.
pub fn format_source(src: &str) -> Result<String, String> {
    let before = lex(src).map_err(|d| format!("line {}: {}", d.pos.line, d.msg))?;
    let toks = tokenize(src)?;
    let out = format_tokens(&toks)?;
    // safety net: formatting must not change the program
    let after = lex(&out).map_err(|d| format!("internal error: the formatted text does not lex (line {}: {})", d.pos.line, d.msg))?;
    let strip = |v: Vec<crate::syntax::Token>| -> Vec<Tok> { v.into_iter().map(|t| t.t).filter(|t| *t != Tok::Sym(";")).collect() };
    if strip(before) != strip(after) {
        return Err("internal error: formatting would change the program; nothing was written".into());
    }
    Ok(out)
}

fn collect(path: &std::path::Path, out: &mut Vec<String>) {
    if path.is_dir() {
        let mut es: Vec<_> = match std::fs::read_dir(path) {
            Ok(r) => r.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
            Err(_) => return,
        };
        es.sort();
        for p in es {
            let n = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if p.is_dir() && (n.starts_with('.') || n == "target" || n == "node_modules") {
                continue;
            }
            collect(&p, out);
        }
    } else if path.extension().map_or(false, |e| e == "yx") {
        out.push(path.to_string_lossy().trim_start_matches("./").to_string());
    }
}

/// `yerux fmt [--check|--stdout] file.yx|dir ...` (rewrites files in place by default).
pub fn run_cli(args: &[String]) -> i32 {
    let (mut check, mut stdout) = (false, false);
    let mut paths = vec![];
    for a in args {
        match a.as_str() {
            "--check" => check = true,
            "--stdout" => stdout = true,
            x if x.starts_with("--") => {
                eprintln!("unknown option {}\nusage: yerux fmt [--check|--stdout] file.yx|dir ...", x);
                return 2;
            }
            x => paths.push(x.to_string()),
        }
    }
    if paths.is_empty() {
        paths.push(".".into());
    }
    let mut files = vec![];
    for p in &paths {
        let pa = std::path::Path::new(p);
        if !pa.exists() {
            eprintln!("could not read {}", p);
            return 2;
        }
        if pa.is_file() {
            files.push(p.clone());
        } else {
            collect(pa, &mut files);
        }
    }
    let (mut dirty, mut errors) = (0, 0);
    for f in &files {
        let src = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("could not read {}: {}", f, e);
                errors += 1;
                continue;
            }
        };
        match format_source(&src) {
            Err(e) => {
                eprintln!("{}: {}", f, e);
                errors += 1;
            }
            Ok(out) => {
                if stdout {
                    print!("{}", out);
                } else if out != src {
                    dirty += 1;
                    if check {
                        println!("{}", f);
                    } else if let Err(e) = std::fs::write(f, &out) {
                        eprintln!("could not write {}: {}", f, e);
                        errors += 1;
                    } else {
                        println!("formatted {}", f);
                    }
                }
            }
        }
    }
    if errors > 0 {
        1
    } else if check && dirty > 0 {
        eprintln!("{} file(s) need formatting (run `yerux fmt`)", dirty);
        1
    } else {
        0
    }
}
