// Yerux front end: diagnostics, lexer, AST and parser.
use crate::numeric::{self, Nk};
use std::collections::{HashMap, HashSet};

// ───────────────────────── Diagnostics ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
    pub len: usize,
}

pub fn span(a: Pos, b: Pos) -> Pos {
    if a.line == b.line && b.col + b.len >= a.col {
        Pos { line: a.line, col: a.col, len: b.col + b.len - a.col }
    } else {
        a
    }
}

pub struct Diag {
    pub msg: String,
    pub pos: Pos,
    pub notes: Vec<String>,
}

const MAX_NEST: usize = 2000;
const MAX_CHAIN: usize = 50000;

pub fn diag(msg: impl Into<String>, pos: Pos) -> Diag {
    Diag { msg: msg.into(), pos, notes: vec![] }
}

impl Diag {
    pub fn note(mut self, n: impl Into<String>) -> Self {
        self.notes.push(n.into());
        self
    }
}

pub fn render(d: &Diag, file: &str, src: &str) -> String {
    let line = src.lines().nth(d.pos.line.saturating_sub(1)).unwrap_or("");
    let w = d.pos.line.to_string().len();
    let mut s = format!("error: {}\n{:w$}--> {}:{}:{}\n", d.msg, "", file, d.pos.line, d.pos.col, w = w);
    s += &format!("{:w$} |\n", "", w = w);
    s += &format!("{} | {}\n", d.pos.line, line);
    s += &format!(
        "{:w$} | {}{}\n",
        "",
        " ".repeat(d.pos.col.saturating_sub(1)),
        "^".repeat(d.pos.len.max(1)),
        w = w
    );
    for n in &d.notes {
        s += &format!("{:w$} = {}\n", "", n, w = w);
    }
    s
}

// ───────────────────────── Lexer ─────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Id(String),
    Num(i64),
    /// Integer literal that needs a type: a suffix (`200u8`, `0xFFu32`) or a value above `i64::MAX` (suffix "").
    NumT(u64, &'static str),
    /// Float literal: its text (parsed later, so `1.5f32` is rounded once) and suffix ("", "f32" or "f64").
    Float(String, &'static str),
    Str(String),
    /// `'a'`: a Unicode scalar value
    Char(i64),
    Sym(&'static str),
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub t: Tok,
    pub pos: Pos,
}

const SYMS3: &[&str] = &["..="];
const SYMS2: &[&str] = &["==", "!=", "<=", ">=", "&&", "||", "->", "=>", ".."];
const SYMS1: &[&str] = &["(", ")", "{", "}", ",", ":", ";", "=", "<", ">", "+", "-", "*", "/", "%", "!", ".", "|", "@", "?"];
pub const KW: &[&str] = &[
    "fn", "let", "var", "if", "else", "while", "return", "move", "view", "mut", "true", "false", "struct", "enum",
    "match", "defer", "trait", "dyn", "impl", "for", "in", "break", "continue", "unsafe", "spawn", "parallel", "scope",
];

pub fn lex(src: &str) -> Result<Vec<Token>, Diag> {
    let cs: Vec<char> = src.chars().collect();
    let (mut i, mut line, mut col) = (0usize, 1usize, 1usize);
    let mut out: Vec<Token> = vec![];
    while i < cs.len() {
        let c = cs[i];
        if c == '\n' {
            i += 1;
            line += 1;
            col = 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            col += 1;
            continue;
        }
        if c == '/' && cs.get(i + 1) == Some(&'/') {
            while i < cs.len() && cs[i] != '\n' {
                i += 1;
            }
            continue;
        }
        let start = i;
        let scol = col;
        let tok;
        if c.is_ascii_digit() {
            let after_dot = matches!(out.last(), Some(Token { t: Tok::Sym("."), .. }));
            let (t, ni) = lex_number(&cs, i, after_dot, Pos { line, col: scol, len: 0 })?;
            tok = t;
            i = ni;
        } else if c.is_ascii_alphabetic() || c == '_' {
            while i < cs.len() && (cs[i].is_ascii_alphanumeric() || cs[i] == '_') {
                i += 1;
            }
            tok = Tok::Id(cs[start..i].iter().collect());
        } else if c == '\'' {
            i += 1;
            let bad = |msg: &str, len: usize| diag(msg, Pos { line, col: scol, len }).note("a Char literal holds exactly one Unicode character: 'a', 'é', '\\n', '\\u{1F600}'; use \"..\" for text");
            let ch: char = match cs.get(i) {
                None | Some('\n') => return Err(bad("unterminated character literal", 1)),
                Some('\'') => return Err(bad("empty character literal", 2)),
                Some('\\') => {
                    i += 1;
                    let e = cs.get(i).copied();
                    i += 1;
                    match e {
                        Some('n') => '\n',
                        Some('t') => '\t',
                        Some('r') => '\r',
                        Some('0') => '\0',
                        Some('\\') => '\\',
                        Some('\'') => '\'',
                        Some('"') => '"',
                        Some('u') if cs.get(i) == Some(&'{') => {
                            let mut hex = String::new();
                            i += 1;
                            while i < cs.len() && cs[i] != '}' && cs[i] != '\n' && hex.len() < 8 {
                                hex.push(cs[i]);
                                i += 1;
                            }
                            if cs.get(i) != Some(&'}') {
                                return Err(bad("unterminated \\u{..} escape", i - start));
                            }
                            i += 1;
                            match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                                Some(c) => c,
                                None => return Err(bad("this is not a valid Unicode scalar value", i - start)),
                            }
                        }
                        _ => return Err(bad("unknown escape in a character literal", (i - start).max(2))),
                    }
                }
                Some(c) => {
                    i += 1;
                    *c
                }
            };
            if cs.get(i) != Some(&'\'') {
                return Err(bad("a character literal must contain exactly one character", (i - start).max(1)));
            }
            i += 1;
            tok = Tok::Char(ch as i64);
        } else if c == '"' {
            i += 1;
            // pieces of the literal: (is_expr, text, column offset inside the literal)
            let mut parts: Vec<(bool, String, usize)> = vec![];
            let mut s = String::new();
            while i < cs.len() && cs[i] != '"' && cs[i] != '\n' {
                if cs[i] == '\\' {
                    let esc = match cs.get(i + 1) {
                        Some('n') => '\n',
                        Some('t') => '\t',
                        Some('r') => '\r',
                        Some('"') => '"',
                        Some('\\') => '\\',
                        other => {
                            let shown = other.map_or(String::from("end of line"), |c| format!("\\{}", c));
                            return Err(diag(format!("unknown escape sequence {} in a string", shown), Pos { line, col: scol + (i - start), len: 2 })
                                .note("escapes: \\n (newline), \\t (tab), \\r, \\\" (quote), \\\\ (backslash); write {{ and }} for literal braces"));
                        }
                    };
                    s.push(esc);
                    i += 2;
                    continue;
                }
                if cs[i] == '{' || cs[i] == '}' {
                    if cs.get(i + 1) == Some(&cs[i]) {
                        s.push(cs[i]);
                        i += 2;
                        continue;
                    }
                    let bpos = Pos { line, col: scol + (i - start), len: 1 };
                    if cs[i] == '}' {
                        return Err(diag("a lone `}` in a string", bpos).note("write `}}` for a literal brace, or `{name}` to insert a value"));
                    }
                    let mut j = i + 1;
                    while j < cs.len() && cs[j] != '}' && cs[j] != '"' && cs[j] != '\n' {
                        j += 1;
                    }
                    if j >= cs.len() || cs[j] != '}' {
                        return Err(diag("this `{` in a string is never closed", bpos)
                            .note("string interpolation looks like \"x = {x}\"; the expression cannot contain a quote (store the value in a variable first)")
                            .note("write `{{` for a literal brace"));
                    }
                    let text: String = cs[i + 1..j].iter().collect();
                    if text.trim().is_empty() {
                        return Err(diag("empty `{}` in a string", bpos).note("write `{{}}` for literal braces, or put a value inside: \"x = {x}\""));
                    }
                    if !s.is_empty() {
                        parts.push((false, std::mem::take(&mut s), 0));
                    }
                    parts.push((true, text, i + 1 - start));
                    i = j + 1;
                    continue;
                }
                s.push(cs[i]);
                i += 1;
            }
            if i >= cs.len() || cs[i] != '"' {
                return Err(diag("unterminated string", Pos { line, col: scol, len: 1 }));
            }
            i += 1;
            if !parts.iter().any(|p| p.0) {
                tok = Tok::Str(s);
            } else {
                // string interpolation: "a{x}b" becomes
                //   std__fmt_lit(std__fmt_val(std__fmt_lit(String.new(), "a"), x), "b")
                // so neither the checker nor the backends need to know about it
                if !s.is_empty() {
                    parts.push((false, std::mem::take(&mut s), 0));
                }
                let lit_pos = Pos { line, col: scol, len: i - start };
                let mk = |t: Tok| Token { t, pos: lit_pos };
                let mut acc: Vec<Token> = vec![
                    mk(Tok::Id("String".into())),
                    mk(Tok::Sym(".")),
                    mk(Tok::Id("new".into())),
                    mk(Tok::Sym("(")),
                    mk(Tok::Sym(")")),
                ];
                for (is_expr, text, off) in parts {
                    let mut next = vec![mk(Tok::Id(if is_expr { "std__fmt_val" } else { "std__fmt_lit" }.into())), mk(Tok::Sym("("))];
                    next.append(&mut acc);
                    next.push(mk(Tok::Sym(",")));
                    if is_expr {
                        let mut sub = lex(&text).map_err(|mut d| {
                            d.pos = Pos { line, col: scol + off + d.pos.col - 1, len: d.pos.len };
                            d
                        })?;
                        sub.pop(); // Eof
                        let mut depth = 0i32;
                        for t in sub.iter_mut() {
                            match &t.t {
                                Tok::Sym("(") => depth += 1,
                                Tok::Sym(")") => depth -= 1,
                                Tok::Sym(",") if depth == 0 => {
                                    return Err(diag("a `{}` in a string holds one expression", Pos { line, col: scol + off + t.pos.col - 1, len: 1 })
                                        .note("write `{{` for a literal brace"));
                                }
                                _ => {}
                            }
                            t.pos = Pos { line, col: scol + off + t.pos.col - 1, len: t.pos.len };
                        }
                        next.append(&mut sub);
                    } else {
                        next.push(mk(Tok::Str(text)));
                    }
                    next.push(mk(Tok::Sym(")")));
                    acc = next;
                }
                // `print("..{x}..")` prints the temporary String and frees it
                let mut k = i;
                while k < cs.len() && cs[k] == ' ' {
                    k += 1;
                }
                let n = out.len();
                if n >= 2 && cs.get(k) == Some(&')') && out[n - 1].t == Tok::Sym("(") && out[n - 2].t == Tok::Id("print".into()) {
                    out[n - 2].t = Tok::Id("std__print_owned".into());
                }
                out.extend(acc);
                col += i - start;
                continue;
            }
        } else {
            let three: String = cs[i..(i + 3).min(cs.len())].iter().collect();
            let two: String = cs[i..(i + 2).min(cs.len())].iter().collect();
            if let Some(s) = SYMS3.iter().copied().find(|s| *s == three) {
                i += 3;
                tok = Tok::Sym(s);
            } else if let Some(s) = SYMS2.iter().copied().find(|s| *s == two) {
                i += 2;
                tok = Tok::Sym(s);
            } else {
                let one = c.to_string();
                if let Some(s) = SYMS1.iter().copied().find(|s| *s == one) {
                    i += 1;
                    tok = Tok::Sym(s);
                } else {
                    return Err(diag(format!("unexpected character '{}'", c), Pos { line, col: scol, len: 1 }));
                }
            }
        }
        let len = i - start;
        col += len;
        out.push(Token { t: tok, pos: Pos { line, col: scol, len } });
    }
    out.push(Token { t: Tok::Eof, pos: Pos { line, col, len: 1 } });
    Ok(out)
}

/// Number literals: `42`, `1_000`, `0xFF`, `0b1010`, `3.14`, `1e-3`, `2.5e10`, with an optional type suffix
/// (`200u8`, `0xFFu32`, `1.5f32`). `after_dot`: the previous token is `.` (a field/tuple index), so only
/// plain digits are read.
fn lex_number(cs: &[char], start: usize, after_dot: bool, p: Pos) -> Result<(Tok, usize), Diag> {
    let mut i = start;
    let at = |k: usize| cs.get(k).copied().unwrap_or('\0');
    let err = |m: String, i: usize| diag(m, Pos { len: i - start, ..p });
    let radix = if at(i) == '0' && !after_dot && matches!(at(i + 1), 'x' | 'X') {
        16
    } else if at(i) == '0' && !after_dot && matches!(at(i + 1), 'b' | 'B') {
        2
    } else {
        10
    };
    let suffix_at = |mut j: usize, allow_f: bool| -> (usize, &'static str) {
        let sfx = ["i64", "i32", "i16", "i8", "u64", "u32", "u16", "u8", "f64", "f32"];
        for s in sfx {
            if s.starts_with('f') && !allow_f {
                continue;
            }
            let n = s.len();
            if (j..j + n).map(at).collect::<String>() == s && !(at(j + n).is_ascii_alphanumeric() || at(j + n) == '_') {
                j += n;
                return (j, s);
            }
        }
        (j, "")
    };
    if radix != 10 {
        i += 2;
        let ds = i;
        while at(i).is_digit(radix) || at(i) == '_' {
            i += 1;
        }
        let txt: String = cs[ds..i].iter().filter(|c| **c != '_').collect();
        if txt.is_empty() {
            return Err(err(format!("expected digits after `0{}`", if radix == 16 { 'x' } else { 'b' }), i));
        }
        let v = u64::from_str_radix(&txt, radix).map_err(|_| err("number too large (the largest is 18446744073709551615)".into(), i))?;
        let (j, sfx) = suffix_at(i, false);
        if at(j).is_ascii_alphanumeric() || at(j) == '_' {
            return Err(err("unexpected letters after a number".into(), j + 1));
        }
        return Ok(if sfx.is_empty() && v <= i64::MAX as u64 { (Tok::Num(v as i64), j) } else { (Tok::NumT(v, sfx), j) });
    }
    while at(i).is_ascii_digit() || (at(i) == '_' && at(i + 1).is_ascii_digit()) {
        i += 1;
    }
    let mut is_float = false;
    if !after_dot {
        if at(i) == '.' && at(i + 1).is_ascii_digit() {
            is_float = true;
            i += 1;
            while at(i).is_ascii_digit() || (at(i) == '_' && at(i + 1).is_ascii_digit()) {
                i += 1;
            }
        }
        if matches!(at(i), 'e' | 'E') && (at(i + 1).is_ascii_digit() || (matches!(at(i + 1), '+' | '-') && at(i + 2).is_ascii_digit())) {
            is_float = true;
            i += 2;
            while at(i).is_ascii_digit() {
                i += 1;
            }
        }
    }
    let txt: String = cs[start..i].iter().filter(|c| **c != '_').collect();
    let (j, sfx) = if after_dot { (i, "") } else { suffix_at(i, true) };
    if !after_dot && sfx.is_empty() && (at(j).is_ascii_alphabetic() || at(j) == '_') {
        return Err(err(format!("unexpected letters after the number `{}`", txt), j + 1)
            .note("number suffixes are i8 i16 i32 i64 u8 u16 u32 u64 f32 f64, e.g. `200u8` or `1.5f32`"));
    }
    if is_float || sfx.starts_with('f') {
        return Ok((Tok::Float(txt, sfx), j));
    }
    match txt.parse::<u64>() {
        Ok(v) if v <= i64::MAX as u64 && sfx.is_empty() => Ok((Tok::Num(v as i64), j)),
        Ok(v) => Ok((Tok::NumT(v, sfx), j)),
        Err(_) => Err(err("number too large (the largest is 18446744073709551615, written as UInt64)".into(), j)),
    }
}

// ───────────────────────── Types and AST ─────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Ty {
    Int,
    /// Signed 64-bit integer constrained to an inclusive interval.
    Range(i64, i64),
    /// Every other number type: Int8..UInt64, Float32, Float (see numeric.rs).
    Num(Nk),
    Char, // a Unicode scalar value; represented exactly like an Int by the backends
    Bool,
    Text,
    String,
    List(usize), // index into `Types::lists` (the element type): List<T> is interned
    Box(usize),  // index into `Types::boxes` (the pointee): Box<T> is interned; an owned heap cell holding one T
    Ptr(usize),  // index into `Types::ptrs`; an unowned raw pointer for C interop
    Dyn(usize), // erased pointee, used only behind Box or a borrow
    Struct(usize),
    Enum(usize),
    /// A type parameter (`T` in `fn max<T: Ordered>`): only exists while checking a generic definition;
    /// monomorphisation replaces it by a concrete type before code generation.
    Param(usize),
    /// A function value `fn(A, view B) -> R` (index into `Types::fns`): a pair (code pointer, environment
    /// pointer). Always an owned type: the environment, if any, is freed when the value is dropped.
    Fn(usize),
    Unit,
    Err,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Own,
    View,
    Mut,
}

#[derive(Clone)]
pub struct StructDef {
    pub name: String,  // as written in diagnostics: `Pair<Int, Text>`
    pub cname: String, // identifier-safe name used by the backends
    pub fields: Vec<(String, Ty)>,
    pub pos: Pos,
    pub origin: Option<(String, Vec<Ty>)>, // generic struct and type arguments this was instantiated from
    pub generic: bool,                     // mentions a type parameter: only used while checking, never emitted
}

#[derive(Clone)]
pub struct EnumDef {
    pub name: String,
    pub cname: String,
    pub variants: Vec<(String, Vec<Ty>)>,
    pub pos: Pos,
    pub origin: Option<(String, Vec<Ty>)>,
    pub generic: bool,
}

/// A type parameter of a generic definition and the traits it must implement.
#[derive(Clone)]
pub struct ParamDef {
    pub name: String,
    pub bounds: Vec<String>,
}

/// A function type: parameter modes and types, and the return type.
#[derive(Clone, PartialEq, Debug)]
pub struct FnTy {
    pub params: Vec<(Mode, Ty)>,
    pub ret: Ty,
}

#[derive(Clone)]
pub struct DynTable {
    pub object: usize,
    pub concrete: Ty,
    pub methods: Vec<String>,
}

#[derive(Clone, Default)]
pub struct Types {
    pub structs: Vec<StructDef>,
    pub enums: Vec<EnumDef>,
    pub lists: Vec<Ty>, // element type of every List<T> instantiation named in the program
    pub dyns: Vec<(String, Pos)>,
    pub dyn_tables: Vec<DynTable>,
    pub boxes: Vec<Ty>, // pointee type of every Box<T> instantiation named in the program
    pub ptrs: Vec<Ty>, // pointee type of every Ptr<T> instantiation
    pub params: Vec<ParamDef>,
    pub thread_bound: HashSet<String>,
    pub fns: Vec<FnTy>, // every function type named in the program (interned)
}

impl Types {
    pub fn name(&self, t: Ty) -> String {
        match t {
            Ty::Int => "Int".into(),
            Ty::Range(lo, hi) => format!("Int<{}..{}>", if lo == i64::MIN { String::new() } else { lo.to_string() }, if hi == i64::MAX { String::new() } else { hi.to_string() }),
            Ty::Num(k) => k.name().into(),
            Ty::Char => "Char".into(),
            Ty::Bool => "Bool".into(),
            Ty::Text => "Text".into(),
            Ty::String => "String".into(),
            // `List` is the alias of `List<Int>`, and keeps its short name in diagnostics
            Ty::List(i) if self.lists[i] == Ty::Int => "List".into(),
            Ty::List(i) => format!("List<{}>", self.name(self.lists[i])),
            Ty::Box(i) => format!("Box<{}>", self.name(self.boxes[i])),
            Ty::Ptr(i) => format!("Ptr<{}>", self.name(self.ptrs[i])),
            Ty::Dyn(i) => format!("dyn {}", self.dyns[i].0),
            Ty::Struct(i) => self.structs[i].name.clone(),
            Ty::Enum(i) => self.enums[i].name.clone(),
            Ty::Param(i) => self.params[i].name.clone(),
            Ty::Fn(i) => {
                let f = &self.fns[i];
                let ps: Vec<String> = f
                    .params
                    .iter()
                    .map(|(m, t)| match m {
                        Mode::Own => self.name(*t),
                        Mode::View => format!("view {}", self.name(*t)),
                        Mode::Mut => format!("mut {}", self.name(*t)),
                    })
                    .collect();
                if f.ret == Ty::Unit {
                    format!("fn({})", ps.join(", "))
                } else {
                    format!("fn({}) -> {}", ps.join(", "), self.name(f.ret))
                }
            }
            Ty::Unit => "nothing".into(),
            Ty::Err => "?".into(),
        }
    }
    /// A type is "owned" when it holds heap memory: it has a single owner and is moved, not copied.
    pub fn owned(&self, t: Ty) -> bool {
        match t {
            Ty::List(_) | Ty::String | Ty::Box(_) | Ty::Fn(_) | Ty::Dyn(_) => true,
            Ty::Struct(i) => self.is_lock_guard(i) || self.join_payload(i).is_some() || self.structs[i].fields.iter().any(|(_, f)| self.owned(*f)),
            Ty::Enum(i) => self.enums[i].variants.iter().any(|(_, ps)| ps.iter().any(|p| self.owned(*p))),
            // inside a generic body a `T` may be anything, so it is owned (moved) unless declared `T: Copy`
            Ty::Param(i) => !self.params[i].bounds.iter().any(|b| b == "Copy"),
            _ => false,
        }
    }
    pub fn capture_live_field(&self, id: usize, capture: &str) -> Option<String> {
        let def = &self.structs[id];
        if !def.cname.starts_with("Env__") {
            return None;
        }
        let flag = format!("live_{capture}");
        def.fields.iter().any(|(name, _)| *name == flag).then_some(flag)
    }

    pub fn runtime_struct(&self, id: usize, expected: &str) -> Option<Ty> {
        let def = &self.structs[id];
        let (name, args) = def.origin.as_ref()?;
        (def.pos.line >= PRELUDE_LINE && name == expected).then(|| args[0])
    }

    pub fn join_payload(&self, id: usize) -> Option<Ty> {
        self.runtime_struct(id, "JoinHandle")
    }

    pub fn contains_join_handle(&self, ty: Ty) -> bool {
        fn contains(types: &Types, ty: Ty, seen: &mut HashSet<Ty>) -> bool {
            if !seen.insert(ty) {
                return false;
            }
            match ty {
                Ty::Box(i) => contains(types, types.boxes[i], seen),
                Ty::List(i) => contains(types, types.lists[i], seen),
                Ty::Struct(i) => types.join_payload(i).is_some()
                    || types.structs[i].fields.iter().any(|(_, t)| contains(types, *t, seen)),
                Ty::Enum(i) => types.enums[i].variants.iter()
                    .any(|(_, fields)| fields.iter().any(|t| contains(types, *t, seen))),
                _ => false,
            }
        }
        contains(self, ty, &mut HashSet::new())
    }

    pub fn shared_payload(&self, ty: Ty) -> Option<Ty> {
        match ty {
            Ty::Struct(id) => self.runtime_struct(id, "Shared"),
            _ => None,
        }
    }

    pub fn locked_payload(&self, ty: Ty) -> Option<Ty> {
        match ty {
            Ty::Struct(id) => self.runtime_struct(id, "Locked"),
            _ => None,
        }
    }

    pub fn is_lock_guard(&self, id: usize) -> bool {
        let def = &self.structs[id];
        def.pos.line >= PRELUDE_LINE && def.name == "LockGuard"
    }

    pub fn opaque_struct(&self, id: usize) -> bool {
        self.is_lock_guard(id) || ["JoinHandle", "Shared", "Atomic", "Locked"].iter().any(|name| self.runtime_struct(id, name).is_some())
    }
    /// Does the type mention a type parameter (so it can only exist while checking generic code)?
    pub fn has_param(&self, t: Ty) -> bool {
        match t {
            Ty::Param(_) | Ty::Err => true, // Err: placeholder while a generic header is first read
            Ty::List(i) => self.has_param(self.lists[i]),
            Ty::Fn(i) => self.fns[i].params.iter().any(|(_, t)| self.has_param(*t)) || self.has_param(self.fns[i].ret),
            Ty::Box(i) => self.has_param(self.boxes[i]),
            Ty::Ptr(i) => self.has_param(self.ptrs[i]),
            Ty::Struct(i) => self.structs[i].generic,
            Ty::Enum(i) => self.enums[i].generic,
            _ => false,
        }
    }
    /// Name fragment used to build the C names of a List<T> instantiation.
    pub fn mangle(&self, t: Ty) -> String {
        match t {
            Ty::Int => "Int".into(),
            Ty::Range(lo, hi) => format!("Range_{}_{}", lo.to_string().replace('-', "m"), hi.to_string().replace('-', "m")),
            Ty::Num(k) => k.name().into(),
            Ty::Char => "Char".into(),
            Ty::Bool => "Bool".into(),
            Ty::Text => "Text".into(),
            Ty::String => "String".into(),
            Ty::List(i) => format!("List_{}", self.mangle(self.lists[i])),
            Ty::Box(i) => format!("Box_{}", self.mangle(self.boxes[i])),
            Ty::Ptr(i) => format!("Ptr_{}", self.mangle(self.ptrs[i])),
            Ty::Dyn(i) => format!("Dyn_{}", i),
            Ty::Struct(i) => format!("Y_{}", self.structs[i].cname),
            Ty::Enum(i) => format!("Y_{}", self.enums[i].cname),
            Ty::Param(i) => format!("P{}", i),
            Ty::Fn(i) => {
                let f = &self.fns[i];
                let ps: Vec<String> = f
                    .params
                    .iter()
                    .map(|(m, t)| format!("{}{}", match m { Mode::Own => "", Mode::View => "v", Mode::Mut => "m" }, self.mangle(*t)))
                    .collect();
                format!("Fn_{}_R{}", ps.join("_"), self.mangle(f.ret))
            }
            _ => "void".into(),
        }
    }
    /// The one function type with these parameters and result (created on first use).
    pub fn intern_fn(&mut self, f: FnTy) -> Ty {
        match self.fns.iter().position(|x| *x == f) {
            Some(i) => Ty::Fn(i),
            None => {
                self.fns.push(f);
                Ty::Fn(self.fns.len() - 1)
            }
        }
    }
    pub fn cty(&self, t: Ty) -> String {
        match t {
            Ty::Int | Ty::Range(..) | Ty::Char => "int64_t".into(),
            Ty::Num(k) => k.cty().into(),
            Ty::Bool => "bool".into(),
            Ty::Text => "const char *".into(),
            Ty::String => "String".into(),
            Ty::List(i) => format!("List_{}", self.mangle(self.lists[i])),
            Ty::Box(i) => format!("Box_{}", self.mangle(self.boxes[i])),
            Ty::Ptr(i) => format!("{} *", self.cty(self.ptrs[i])),
            Ty::Struct(i) => format!("Y_{}", self.structs[i].cname),
            Ty::Enum(i) => format!("Y_{}", self.enums[i].cname),
            Ty::Fn(_) => "NxFn".into(),
            Ty::Dyn(_) => "NxDyn".into(),
            _ => "void".into(),
        }
    }
}

#[derive(Clone)]
pub struct Expr {
    pub k: EK,
    pub pos: Pos,
    pub ty: Ty,
}

#[derive(Clone)]
pub enum EK {
    /// `let x: T = e`: the initialiser with its declared type (`Expr::ty`); the checker checks `e` against it.
    Ascribe(Box<Expr>),
    /// `x as Int<lo..hi> else { .. }` (`Expr::ty` is the range): the value when it fits, else the block, which must diverge.
    /// Rewritten by the checker (a test and the block, or just `x` when the compiler can prove that it fits).
    AsElse(Box<Expr>, Vec<Stmt>),
    /// Checker-generated expression scope with an ordered prelude, used by postfix `?`.
    Sequence(Vec<Stmt>, Box<Expr>),
    /// Integer literal (the type is `Expr::ty`; UInt64 values are stored as their bit pattern).
    Int(i64),
    /// Float literal: value and source text (a Float32 literal is rounded from the text once).
    Float(f64, String),
    /// Explicit numeric conversion to `Expr::ty`: checked (panics when the value does not fit), or with `true`
    /// a bit-truncating one (`truncate_int8`). Built from `x.to_int8()`, `x.to_float()`, `x.truncate_uint8()`.
    Conv(Box<Expr>, bool),
    Char(i64),
    Bool(bool),
    Text(String),
    Var(String, usize),
    Move(String, usize),
    Field(Box<Expr>, String),
    Un(&'static str, Box<Expr>),
    Bin(&'static str, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    /// Type-level function: `List<T>.new()`, `String.from(x)`, ...
    Static { ty: Ty, f: String, args: Vec<Expr> },
    Method { recv: Box<Expr>, m: String, args: Vec<Expr> },
    StructLit { sid: usize, fields: Vec<(String, Expr)> },
    Variant { eid: usize, vname: String, vid: usize, args: Vec<Expr> },
    /// `base` is the variant with index `vid` (created by the checker for general `match`)
    IsVariant(Box<Expr>, usize),
    /// payload `k` of variant `vid` of the enum value `base` (only evaluated after `IsVariant` held)
    Payload(Box<Expr>, usize, usize),
    /// Literal of a generic struct, `Pair<Int, Text> { .. }` or `Pair { .. }` (type arguments inferred);
    /// the checker turns it into `StructLit` of the right instantiation.
    GenStruct { name: String, targs: Option<Vec<Ty>>, fields: Vec<(String, Expr)> },
    /// Same for a generic enum variant: `Maybe<Int>.None`, `Maybe.Some(5)`.
    GenVariant { name: String, targs: Option<Vec<Ty>>, vname: String, args: Vec<Expr> },
    /// `|x, y| x + y`, `move |x: Int| -> Int { .. }`: parsed only; the checker lifts the body into a function
    /// and turns the literal into `MkClosure`.
    Closure { params: Vec<CParam>, ret: Option<Ty>, body: Vec<Stmt>, is_move: bool, expr_body: bool, parallel: bool, worker: bool },
    /// A function value: `func` is called as `func(env, args..)`. With `env: Some(sid)` the value owns a heap
    /// environment (struct `sid`: word 0 is the drop function, then one field per capture, `caps` in order);
    /// without captures it is a plain code pointer (environment 0). A by-reference capture is `AddrOf(place)`.
    MkClosure { func: String, env: Option<usize>, caps: Vec<Expr> },
    /// Call of a function value (a place of function type): `f(3)`.
    CallFn(Box<Expr>, Vec<Expr>),
    DynBox { table: usize, value: Box<Expr> },
    DynCall { slot: usize, sig: FnTy, args: Vec<Expr> },
    /// The address of a variable, as an `Int` (environment of a borrowing closure); the flag: borrowed mutably.
    AddrOf(Box<Expr>, bool),
    /// The place whose address is the `Int` value of the operand (only as the source of a `Bind`).
    Deref(Box<Expr>),
}

#[derive(Clone)]
pub struct CParam {
    pub name: String,
    pub mode: Option<Mode>,
    pub ty: Option<Ty>,
    pub pos: Pos,
}

#[derive(Clone)]
pub struct Arm {
    pub vname: String,                // variant name, or "_" for the wildcard
    pub variant: Option<usize>,       // resolved by the checker
    pub binds: Vec<(String, usize)>,  // payload bindings (name, variable id)
    pub body: Vec<Stmt>,
    pub pos: Pos,
    /// the general pattern when the arm is not a plain `Variant(a, b)` / `_` (guards, `|`, literals, tuples, ..)
    pub pat: Option<Pat>,
    pub guard: Option<Expr>,
}

/// A `match` pattern.
#[derive(Clone)]
pub enum Pat {
    Wild(Pos),
    /// `x` or `x @ pattern`
    Bind(String, Option<Box<Pat>>, Pos),
    Int(i64, Pos),
    Char(i64, Pos),
    Bool(bool, Pos),
    /// `lo..=hi` (inclusive ends already; either end may be open); the flag is set for Char bounds
    Range(Option<i64>, Option<i64>, bool, Pos),
    /// `Variant`, `Variant(p, ..)`, `Enum.Variant(p, ..)`
    Variant(String, Vec<Pat>, Pos),
    Tuple(Vec<Pat>, Pos),
    Or(Vec<Pat>, Pos),
}

#[derive(Clone)]
pub struct Stmt {
    pub k: SK,
    pub pos: Pos,
}

#[derive(Clone)]
pub enum SK {
    Let(String, bool, Expr, usize),
    Assign(Expr, Expr),
    ExprS(Expr),
    If(Expr, Vec<Stmt>, Option<Vec<Stmt>>),
    /// `while cond { body }`. `Loop` carries the label, the loop id and (for desugared `for` loops) the
    /// step statements that run after the body and on every `continue`.
    While(Expr, Vec<Stmt>, Loop),
    /// `break` / `continue`, with an optional label; the checker fills the target loop id.
    Break(Option<(String, Pos)>, usize),
    Continue(Option<(String, Pos)>, usize),
    /// A nested scope. The checker creates it when it desugars `for`, `let (a, b) = ..` and general `match`.
    /// The flag says that every path through the block leaves the function (used for "must return").
    Block(Vec<Stmt>, bool),
    /// Lexical unsafe block; erased by the checker after checking its contents.
    Unsafe(Vec<Stmt>),
    /// `let (a, (b, _)) = tuple`: every leaf becomes a variable that owns its part; the tuple itself is consumed.
    Unpack(UnpackPat, Expr),
    /// `x` is a borrow of an existing place (a list element in a `for` loop); created by the checker.
    Bind(String, Mode, Expr, usize),
    /// `for x in iter { body }`: parsed only; the checker rewrites it into plain `while` loops.
    For(Box<ForHead>, Vec<Stmt>),
    Return(Option<Expr>),
    Match(Expr, Vec<Arm>),
    /// `defer stmt`: runs at scope exit. The checker expands it away, so backends never see it.
    Defer(Box<Stmt>),
}

#[derive(Clone, Default)]
pub struct Loop {
    pub label: Option<(String, Pos)>,
    pub id: usize,
    pub step: Vec<Stmt>,
    /// a list or string iterated by this loop: (place, exclusive); see `Checker::frozen`
    pub freeze: Option<(Expr, bool)>,
}

#[derive(Clone)]
pub enum UnpackPat {
    /// name (`_` for a discarded element), mutable, variable id (set by the checker), position
    Bind(String, bool, usize, Pos),
    Tuple(Vec<UnpackPat>, Pos),
}

#[derive(Clone)]
pub enum ForIter {
    /// `a..b` or `a..=b` (inclusive when the flag is set)
    Range(Expr, Expr, bool),
    /// a list (or string) place, or `move list`
    Expr(Expr),
}

#[derive(Clone)]
pub struct ForHead {
    pub var: String,
    pub var_pos: Pos,
    pub mutable: bool,
    pub iter: ForIter,
    pub step: Option<Expr>,
    pub label: Option<(String, Pos)>,
}

#[derive(Clone)]
pub struct Param {
    pub name: String,
    pub mode: Mode,
    pub ty: Ty,
    pub id: usize,
}

#[derive(Clone)]
pub struct Func {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Ty,
    pub body: Vec<Stmt>,
    pub pos: Pos,
}

pub struct Program {
    pub types: Types,
    pub funcs: Vec<Func>,
    /// C ABI declarations. These have no Yerux body and are emitted only by FFI-capable backends.
    pub externs: Vec<ExternFn>,
    pub links: Vec<String>,
    /// Bodies of generic functions with their type parameters left abstract: checked once, never emitted.
    pub templates: Vec<Func>,
}

#[derive(Clone)]
pub struct ExternFn {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Ty,
    /// C variadic (`fn printf(fmt: Text, ...)`): the parameters above are the fixed ones.
    pub variadic: bool,
    pub pos: Pos,
}

impl Program {
    pub fn has_recoverable_workers(&self) -> bool {
        self.funcs.iter().any(|f| f.pos.line >= PRELUDE_LINE && f.name.starts_with("__spawn_owned__"))
    }
}

pub fn pat_pos(p: &Pat) -> Pos {
    match p {
        Pat::Wild(p) | Pat::Bind(_, _, p) | Pat::Int(_, p) | Pat::Char(_, p) | Pat::Bool(_, p) | Pat::Range(_, _, _, p) | Pat::Variant(_, _, p) | Pat::Tuple(_, p) | Pat::Or(_, p) => *p,
    }
}

fn mk(k: EK, pos: Pos) -> Expr {
    Expr { k, pos, ty: Ty::Unit }
}

// ───────────────────────── Parser ─────────────────────────

pub type PR<T> = Result<T, Diag>;

/// A generic struct or enum, registered before anything is parsed. Instantiations re-parse its body
/// with the type parameters bound to concrete types.
#[derive(Clone)]
struct GenType {
    is_enum: bool,
    params: Vec<String>,
    pids: Vec<usize>, // abstract parameters used to infer type arguments from values
    body: usize,      // index of the `{` token
    pos: Pos,
}

/// A bound on a type parameter: `T: Ordered + Copy`.
#[derive(Clone)]
pub struct TParam {
    pub name: String,
    pub bounds: Vec<(String, Pos)>,
}

/// A generic function: its parameters and the token where `fn` starts (re-parsed per instantiation).
#[derive(Clone)]
pub struct GenFn {
    pub name: String,
    pub tparams: Vec<TParam>,
    pub pids: Vec<usize>,
    pub tok: usize,
    pub pos: Pos,
    pub params: Vec<(Mode, Ty)>, // with the type parameters left abstract (patterns for inference)
    pub ret: Ty,
}

pub struct TraitMethod {
    pub name: String,
    pub tok: usize, // index of the `fn` token
    pub sig: FnTy,
}

pub struct TraitDef {
    pub name: String,
    #[allow(dead_code)]
    pub pos: Pos,
    pub methods: Vec<TraitMethod>,
    pub prelude: bool,
}

pub struct ImplMethod {
    pub name: String,
    pub tok: usize,
    pub parsed: bool, // false: parsed on first use (built-in impls), so unused ones cost nothing
    pub pos: Pos,
}

pub struct ImplDef {
    pub trait_name: Option<String>,
    pub trait_pos: Pos,
    /// `impl<T: Show> Show for List<T>` (built-in library only): the type parameters and their abstract ids;
    /// `self_ty` then mentions them and is matched against concrete types by unification.
    pub generics: Vec<TParam>,
    pub pids: Vec<usize>,
    pub self_ty: Ty,
    pub pos: Pos,
    pub methods: Vec<ImplMethod>,
    pub prelude: bool,
}

pub struct FnHeader {
    pub name: String,
    pub pos: Pos,
    pub generics: Vec<TParam>,
    pub params: Vec<Param>,
    pub ret: Ty,
}

pub struct Parser {
    t: Vec<Token>,
    i: usize,
    names: HashMap<String, Ty>, // user type names (collected in a pre-scan so order does not matter)
    pub types: Types,
    gtypes: HashMap<String, GenType>,
    pub gfns: Vec<GenFn>,
    pub traits: Vec<TraitDef>,
    pub impls: Vec<ImplDef>,
    tparams: HashMap<String, Ty>, // type parameters in scope while (re-)parsing a generic definition
    self_ty: Option<Ty>,          // `Self` inside an impl or a trait
    in_extern: bool,              // parsing a header inside `extern "C" { }`: `...` is allowed
    saw_variadic: bool,           // set by `fn_header_rest` when the header ended in `...`
    prelude_len: usize, // tokens [0, prelude_len) are the built-in prelude
    thread_scopes: usize,
    memo: HashMap<(String, Vec<Ty>), Ty>,
    depth: usize,
    nest: usize, // syntactic nesting (blocks, parentheses, types, patterns): bounded so deep input is a diagnostic
    pub inst_errs: Vec<Diag>,
    /// Non-generic functions of the built-in library: parsed and compiled on first call.
    pub lazy_fns: HashMap<String, usize>,
}

const LEVELS: &[&[&str]] = &[&["||"], &["&&"], &["==", "!="], &["<", ">", "<=", ">="], &["+", "-"], &["*", "/", "%"]];
const BUILTIN_TYPES: &[&str] = &["Int", "Char", "Bool", "Text", "String", "List"];


/// Built-in traits and their implementations for the primitive types (parsed lazily, on first use).
pub const PRELUDE_LINE: usize = 1 << 40; // tag on the line numbers of the built-in library
pub const PRELUDE: &str = concat!(
    include_str!("prelude.yx"),
    "\n",
    include_str!("prelude_fmt.yx"),
    "\n",
    include_str!("prelude_string.yx"),
    "\n",
    include_str!("prelude_list.yx"),
    "\n",
    include_str!("prelude_map.yx"),
    "\n",
    include_str!("prelude_opt.yx"),
    "\n",
    include_str!("prelude_arena.yx"),
    "\n",
    include_str!("prelude_threads.yx"),
    "\n",
    include_str!("prelude_num.yx"),
    "\n"
);

pub fn impl_fn_name(types: &Types, trait_name: Option<&str>, ty: Ty, m: &str) -> String {
    match trait_name {
        Some(t) => format!("{}__{}__{}", t, types.mangle(ty), m),
        None => format!("{}__{}", types.mangle(ty), m),
    }
}

impl Parser {
    pub fn new(user: Vec<Token>) -> Parser {
        let mut t = lex(PRELUDE).unwrap_or_else(|d| panic!("prelude: {}", d.msg));
        t.pop(); // the prelude's Eof
    for tk in t.iter_mut() {
        tk.pos.line |= PRELUDE_LINE;
    }
        let prelude_len = t.len();
        t.extend(user);
        let mut names = HashMap::new();
        let mut types = Types::default();
        let blank = |n: &str| (n.to_string(), Pos::default());
        for w in t.windows(3) {
            if let (Tok::Id(k), Tok::Id(n)) = (&w[0].t, &w[1].t) {
                if (k == "struct" || k == "enum") && !names.contains_key(n) && w[2].t != Tok::Sym("<") {
                    if k == "struct" {
                        names.insert(n.clone(), Ty::Struct(types.structs.len()));
                        let (name, pos) = blank(n);
                        types.structs.push(StructDef { cname: name.clone(), name, fields: vec![], pos, origin: None, generic: false });
                    } else {
                        names.insert(n.clone(), Ty::Enum(types.enums.len()));
                        let (name, pos) = blank(n);
                        types.enums.push(EnumDef { cname: name.clone(), name, variants: vec![], pos, origin: None, generic: false });
                    }
                }
            }
        }
        Parser {
            t,
            i: 0,
            names,
            types,
            gtypes: HashMap::new(),
            gfns: vec![],
            traits: vec![],
            impls: vec![],
            tparams: HashMap::new(),
            in_extern: false,
            saw_variadic: false,
            self_ty: None,
            prelude_len,
            thread_scopes: 0,
            memo: HashMap::new(),
            depth: 0,
            nest: 0,
            inst_errs: vec![],
            lazy_fns: HashMap::new(),
        }
    }

    /// Enter one level of syntactic nesting; `leave` must be called on the success path.
    fn enter(&mut self) -> PR<()> {
        self.nest += 1;
        if self.nest > MAX_NEST {
            self.nest -= 1;
            return Err(diag(format!("this is nested more than {} levels deep", MAX_NEST), self.pos())
                .note("no real program needs this much nesting; split the inner part into its own function or a `let`"));
        }
        Ok(())
    }
    fn leave(&mut self) {
        self.nest -= 1;
    }
    fn too_long(&self, what: &str, n: usize) -> PR<()> {
        if n > MAX_CHAIN {
            return Err(diag(format!("{} is more than {} items long", what, MAX_CHAIN), self.pos())
                .note("break it into several statements with `let`"));
        }
        Ok(())
    }

    fn peek(&self) -> &Tok {
        &self.t[self.i].t
    }
    fn peek_at(&self, k: usize) -> &Tok {
        &self.t[(self.i + k).min(self.t.len() - 1)].t
    }
    fn pos(&self) -> Pos {
        self.t[self.i].pos
    }
    fn bump(&mut self) -> Token {
        let t = self.t[self.i].clone();
        if self.i + 1 < self.t.len() {
            self.i += 1;
        }
        t
    }
    fn is_sym(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Sym(x) if *x == s)
    }
    fn is_kw(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Id(x) if x == s)
    }
    fn eat_sym(&mut self, s: &str) -> bool {
        if self.is_sym(s) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect_sym(&mut self, s: &str) -> PR<Pos> {
        if self.is_sym(s) {
            Ok(self.bump().pos)
        } else {
            Err(diag(format!("expected '{}'", s), self.pos()))
        }
    }
    fn ident(&mut self) -> PR<(String, Pos)> {
        match self.peek().clone() {
            Tok::Id(n) if !KW.contains(&n.as_str()) => {
                let p = self.bump().pos;
                Ok((n, p))
            }
            _ => Err(diag("expected a name", self.pos())),
        }
    }
    fn intern_list(&mut self, elem: Ty) -> Ty {
        match self.types.lists.iter().position(|e| *e == elem) {
            Some(i) => Ty::List(i),
            None => {
                self.types.lists.push(elem);
                Ty::List(self.types.lists.len() - 1)
            }
        }
    }
    pub fn intern_box(&mut self, elem: Ty) -> Ty {
        match self.types.boxes.iter().position(|e| *e == elem) {
            Some(i) => Ty::Box(i),
            None => {
                self.types.boxes.push(elem);
                Ty::Box(self.types.boxes.len() - 1)
            }
        }
    }
    /// After the name `List`: an optional `<T>` (bare `List` means `List<Int>`).
    fn list_args(&mut self) -> PR<Ty> {
        let elem = if self.eat_sym("<") {
            let e = self.ty()?;
            self.expect_sym(">")?;
            e
        } else {
            Ty::Int
        };
        Ok(self.intern_list(elem))
    }
    /// `<A, B>` after the name of a generic type.
    fn type_args(&mut self) -> PR<Vec<Ty>> {
        self.expect_sym("<")?;
        let mut v = vec![];
        while !self.is_sym(">") {
            v.push(self.ty()?);
            if !self.eat_sym(",") {
                break;
            }
        }
        self.expect_sym(">")?;
        Ok(v)
    }
    /// After the keyword `fn` in a type position: `(view A, B) -> R`.
    fn fn_ty(&mut self) -> PR<Ty> {
        self.expect_sym("(")?;
        let mut params = vec![];
        while !self.is_sym(")") {
            let mode = if self.is_kw("view") {
                self.bump();
                Mode::View
            } else if self.is_kw("mut") {
                self.bump();
                Mode::Mut
            } else {
                Mode::Own
            };
            let ty = if mode == Mode::Own { self.ty()? } else { self.indirect_ty()? };
            params.push((mode, ty));
            if !self.eat_sym(",") {
                break;
            }
        }
        self.expect_sym(")")?;
        let ret = if self.eat_sym("->") { self.ty()? } else { Ty::Unit };
        Ok(self.types.intern_fn(FnTy { params, ret }))
    }

    fn ty(&mut self) -> PR<Ty> {
        self.enter()?;
        let r = self.ty_inner();
        self.leave();
        r
    }
    fn indirect_ty(&mut self) -> PR<Ty> {
        if !self.is_kw("dyn") {
            return self.ty();
        }
        let pos = self.bump().pos;
        let (name, _) = self.ident()?;
        if let Some(i) = self.types.dyns.iter().position(|(n, _)| *n == name) {
            return Ok(Ty::Dyn(i));
        }
        let i = self.types.dyns.len();
        self.types.dyns.push((name, pos));
        Ok(Ty::Dyn(i))
    }

    fn ty_inner(&mut self) -> PR<Ty> {
        if self.is_kw("dyn") {
            return Err(diag("dyn Trait requires Box or a view or mut parameter", self.pos()));
        }
        if self.is_kw("fn") {
            self.bump();
            return self.fn_ty();
        }
        if self.is_sym("(") {
            let open = self.bump().pos;
            let mut ts = vec![];
            while !self.is_sym(")") {
                ts.push(self.ty()?);
                if !self.eat_sym(",") {
                    break;
                }
            }
            let close = self.expect_sym(")")?;
            if !(2..=6).contains(&ts.len()) {
                return Err(diag(format!("a tuple type has 2 to 6 elements, this one has {}", ts.len()), span(open, close))
                    .note("help: for more elements, define a struct with named fields"));
            }
            return self.inst_type(&format!("Tuple{}", ts.len()), ts, open);
        }
        let (n, p) = self.ident()?;
        if let Some(t) = self.tparams.get(&n) {
            return Ok(*t);
        }
        if n == "Self" {
            return self.self_ty.ok_or_else(|| {
                diag("`Self` only exists inside `impl` and `trait`", p).note("help: write the type's name instead")
            });
        }
        match n.as_str() {
            "Int" => {
                if !self.eat_sym("<") { return Ok(Ty::Int); }
                self.range_after_lt(p)
            }
            "Char" => Ok(Ty::Char),
            "Bool" => Ok(Ty::Bool),
            "Text" => Ok(Ty::Text),
            "String" => Ok(Ty::String),
            "List" => self.list_args(),
            "Box" if self.uses_builtin_box(p) => {
                if !self.is_sym("<") {
                    return Err(diag("Box needs a type argument", p).note("help: write Box<T>, e.g. `Box<Tree>`"));
                }
                self.bump();
                let e = self.indirect_ty()?;
                self.expect_sym(">")?;
                Ok(self.intern_box(e))
            }
            "Ptr" if !self.names.contains_key("Ptr") && !self.gtypes.contains_key("Ptr") => {
                if !self.is_sym("<") {
                    return Err(diag("Ptr needs a type argument", p).note("help: write Ptr<T>, for example Ptr<UInt8>"));
                }
                self.bump();
                let pointee = self.indirect_ty()?;
                self.expect_sym(">")?;
                Ok(self.intern_ptr(pointee))
            }
            _ if numeric::ty_from_name(&n).is_some() => Ok(numeric::ty_from_name(&n).unwrap()),
            _ => {
                if self.gtypes.contains_key(&n) {
                    if !self.is_sym("<") {
                        let g = &self.gtypes[&n];
                        return Err(diag(format!("{} is generic: it needs type arguments", n), p)
                            .note(format!("help: write {}<{}>", n, g.params.join(", "))));
                    }
                    let args = self.type_args()?;
                    return self.inst_type(&n, args, p);
                }
                self.names.get(&n).copied().ok_or_else(|| {
                    diag(format!("unknown type '{}'", n), p).note("built-in types: Int, Char, Bool, Text, String, List<T>, Float, Float32, Int8, Int16, Int32, UInt8, UInt16, UInt32, UInt64")
                })
            }
        }
    }

    fn uses_builtin_box(&self, pos: Pos) -> bool {
        pos.line >= PRELUDE_LINE || (!self.names.contains_key("Box") && !self.gtypes.contains_key("Box"))
    }

    pub fn uses_workers(&self) -> bool {
        self.t[self.prelude_len..].iter().any(|token| matches!(&token.t, Tok::Id(name) if name == "spawn"))
    }

    fn new_param(&mut self, name: &str, bounds: Vec<String>) -> Ty {
        self.types.params.push(ParamDef { name: name.to_string(), bounds });
        Ty::Param(self.types.params.len() - 1)
    }

    fn intern_ptr(&mut self, pointee: Ty) -> Ty {
        match self.types.ptrs.iter().position(|t| *t == pointee) {
            Some(i) => Ty::Ptr(i),
            None => { self.types.ptrs.push(pointee); Ty::Ptr(self.types.ptrs.len() - 1) }
        }
    }

    /// Instantiate a generic struct/enum: re-parse its body with the parameters bound to `args`.
    fn inst_type(&mut self, name: &str, args: Vec<Ty>, pos: Pos) -> PR<Ty> {
        let g = self.gtypes[name].clone();
        if args.len() != g.params.len() {
            return Err(diag(
                format!("{} takes {} type argument(s) but {} given", name, g.params.len(), args.len()),
                pos,
            )
            .note(format!("help: write {}<{}>", name, g.params.join(", "))));
        }
        let key = (name.to_string(), args.clone());
        if let Some(t) = self.memo.get(&key) {
            return Ok(*t);
        }
        if self.depth > 24 {
            return Err(diag(format!("the generic type {} expands forever", name), pos)
                .note("help: a generic type cannot contain itself with a different argument each time (e.g. `Bad<List<T>>`)"));
        }
        let generic = args.iter().any(|a| *a == Ty::Err || self.types.has_param(*a));
        let shown: Vec<String> = args.iter().map(|a| self.types.name(*a)).collect();
        let mang: Vec<String> = args.iter().map(|a| self.types.mangle(*a)).collect();
        let dname = if g.body < self.prelude_len && name.starts_with("Tuple") {
            format!("({})", shown.join(", "))
        } else {
            format!("{}<{}>", name, shown.join(", "))
        };
        let cname = format!("{}__{}", name, mang.join("__"));
        let ty = if g.is_enum {
            self.types.enums.push(EnumDef { name: dname, cname, variants: vec![], pos: g.pos, origin: Some((name.to_string(), args.clone())), generic });
            Ty::Enum(self.types.enums.len() - 1)
        } else {
            self.types.structs.push(StructDef { name: dname, cname, fields: vec![], pos: g.pos, origin: Some((name.to_string(), args.clone())), generic });
            Ty::Struct(self.types.structs.len() - 1)
        };
        self.memo.insert(key, ty);
        // re-parse the body under the substitution
        let saved = (self.i, std::mem::take(&mut self.tparams), self.self_ty.take());
        self.i = g.body;
        self.tparams = g.params.iter().cloned().zip(args).collect();
        self.depth += 1;
        let r = self.inst_body(ty, g.is_enum);
        self.depth -= 1;
        self.i = saved.0;
        self.tparams = saved.1;
        self.self_ty = saved.2;
        r?;
        Ok(ty)
    }
    fn inst_body(&mut self, ty: Ty, is_enum: bool) -> PR<()> {
        self.expect_sym("{")?;
        match ty {
            Ty::Enum(i) if is_enum => {
                let v = self.enum_variants()?;
                self.types.enums[i].variants = v;
            }
            Ty::Struct(i) => {
                let f = self.struct_fields()?;
                self.types.structs[i].fields = f;
            }
            _ => {}
        }
        self.expect_sym("}")?;
        Ok(())
    }

    /// Index just past the `}` that closes the first `{` at or after `self.i`.
    fn skip_item(&mut self) -> PR<()> {
        while !self.is_sym("{") {
            if matches!(self.peek(), Tok::Eof) {
                return Err(diag("missing '{'", self.pos()));
            }
            self.bump();
        }
        self.skip_braces()
    }
    fn skip_braces(&mut self) -> PR<()> {
        let start = self.pos();
        let mut depth = 0;
        loop {
            match self.peek() {
                Tok::Sym("{") => depth += 1,
                Tok::Sym("}") => {
                    depth -= 1;
                    if depth == 0 {
                        self.bump();
                        return Ok(());
                    }
                }
                Tok::Eof => return Err(diag("missing closing '}'", start)),
                _ => {}
            }
            self.bump();
        }
    }

    /// `<A, B: Trait + Trait>` after a generic name.
    fn generics_list(&mut self) -> PR<Vec<TParam>> {
        self.expect_sym("<")?;
        let mut v: Vec<TParam> = vec![];
        while !self.is_sym(">") {
            let (name, np) = self.ident()?;
            if v.iter().any(|p| p.name == name) {
                return Err(diag(format!("type parameter '{}' is declared twice", name), np));
            }
            let mut bounds = vec![];
            if self.eat_sym(":") {
                loop {
                    let (b, bp) = self.ident()?;
                    bounds.push((b, bp));
                    if !self.eat_sym("+") {
                        break;
                    }
                }
            }
            v.push(TParam { name, bounds });
            if !self.eat_sym(",") {
                break;
            }
        }
        self.expect_sym(">")?;
        if v.is_empty() {
            return Err(diag("a generic list needs at least one type parameter", self.pos()));
        }
        Ok(v)
    }

    /// Pass 1: find the generic structs/enums so their uses can be parsed in any order.
    fn register_generics(&mut self) -> PR<()> {
        while !matches!(self.peek(), Tok::Eof) {
            if self.eat_sym("@") {
                let (attribute, pos) = self.ident()?;
                if attribute != "thread_bound" || !(self.is_kw("struct") || self.is_kw("enum")) {
                    return Err(diag("expected @thread_bound on a struct or enum", pos));
                }
                if let Tok::Id(name) = self.peek_at(1) {
                    self.types.thread_bound.insert(name.clone());
                }
            }
            if (self.is_kw("struct") || self.is_kw("enum")) && matches!(self.peek_at(1), Tok::Id(_)) && *self.peek_at(2) == Tok::Sym("<") {
                let is_enum = self.is_kw("enum");
                self.bump();
                let (name, pos) = self.type_name()?;
                let gl = self.generics_list()?;
                if let Some(b) = gl.iter().find_map(|p| p.bounds.first()) {
                    return Err(diag("type parameters of a struct or enum cannot have bounds", b.1)
                        .note("help: put the bounds on the functions that use the type, e.g. `fn f<T: Ordered>(p: view Pair<T, T>)`"));
                }
                if self.gtypes.contains_key(&name) || self.names.contains_key(&name) {
                    return Err(diag(format!("type '{}' is defined twice", name), pos));
                }
                let params: Vec<String> = gl.iter().map(|p| p.name.clone()).collect();
                let pids = params.iter().map(|p| match self.new_param(p, vec![]) { Ty::Param(i) => i, _ => 0 }).collect();
                let body = self.i;
                self.gtypes.insert(name, GenType { is_enum, params, pids, body, pos });
                self.skip_braces()?;
            } else {
                self.skip_item()?;
            }
        }
        Ok(())
    }

    pub fn program(&mut self) -> PR<Program> {
        self.register_generics()?;
        self.i = 0;
        let mut funcs = vec![];
        let mut externs = vec![];
        let mut links = vec![];
        let mut defined: Vec<String> = vec![];
        // the generic types themselves must be well-formed even if nothing instantiates them
        let mut gnames: Vec<String> = self.gtypes.keys().cloned().collect();
        gnames.sort_by_key(|n| self.gtypes[n].body);
        for n in gnames {
            let args = self.gtypes[&n].pids.iter().map(|p| Ty::Param(*p)).collect();
            let pos = self.gtypes[&n].pos;
            self.inst_type(&n, args, pos)?;
        }
        while !matches!(self.peek(), Tok::Eof) {
            if matches!(self.peek(), Tok::Id(n) if n == "type") {
                self.bump();
                let (name, pos) = self.ident()?;
                if self.names.contains_key(&name) || BUILTIN_TYPES.contains(&name.as_str()) || numeric::NUMERIC_TYPE_NAMES.contains(&name.as_str()) {
                    return Err(diag(format!("type '{}' is already defined", name), pos));
                }
                self.expect_sym("=")?;
                let alias = self.ty()?;
                self.names.insert(name, alias);
                self.eat_sym(";");
                continue;
            }
            if self.is_kw("link") {
                self.bump();
                match self.bump() {
                    Token { t: Tok::Str(lib), .. } => links.push(lib),
                    t => return Err(diag("expected a library name string after `link`", t.pos)),
                }
                continue;
            }
            if self.is_kw("extern") {
                let ep = self.bump().pos;
                if !matches!(self.bump().t, Tok::Str(ref abi) if abi == "C") {
                    return Err(diag("only `extern \"C\"` is supported", ep));
                }
                self.expect_sym("{")?;
                while !self.is_sym("}") {
                    self.in_extern = true;
                    self.saw_variadic = false;
                    let h = self.fn_header();
                    self.in_extern = false;
                    let h = h?;
                    if !h.generics.is_empty() { return Err(diag("extern functions cannot be generic", h.pos)); }
                    self.eat_sym(";");
                    externs.push(ExternFn { name: h.name, params: h.params, ret: h.ret, variadic: self.saw_variadic, pos: h.pos });
                }
                self.bump();
                continue;
            }
            if self.eat_sym("@") { self.bump(); } // validated during registration
            if self.is_kw("struct") || self.is_kw("enum") {
                if *self.peek_at(2) == Tok::Sym("<") {
                    self.skip_item()?;
                    continue;
                }
                let is_struct = self.is_kw("struct");
                let (name, pos, ty) = {
                    let n = self.peek_at(1).clone();
                    let name = if let Tok::Id(n) = n { n } else { String::new() };
                    (name.clone(), self.t[(self.i + 1).min(self.t.len() - 1)].pos, self.names.get(&name).copied())
                };
                if defined.contains(&name) {
                    return Err(diag(format!("type '{}' is defined twice", name), pos));
                }
                defined.push(name);
                if is_struct {
                    let d = self.struct_def()?;
                    if let Some(Ty::Struct(i)) = ty {
                        self.types.structs[i] = d;
                    }
                } else {
                    let d = self.enum_def()?;
                    if let Some(Ty::Enum(i)) = ty {
                        self.types.enums[i] = d;
                    }
                }
            } else if self.is_kw("trait") {
                self.trait_def()?;
            } else if self.is_kw("impl") {
                self.impl_def(&mut funcs)?;
            } else {
                let tok = self.i;
                let h = self.fn_header()?;
                if !h.generics.is_empty() {
                    // generic function: re-parsed per instantiation; its body is skipped here
                    if self.gfns.iter().any(|g| g.name == h.name) {
                        return Err(diag(format!("function '{}' is defined twice", h.name), h.pos));
                    }
                    self.skip_item()?;
                    let pids = h.generics.iter().map(|p| match self.new_param(&p.name, vec![]) { Ty::Param(i) => i, _ => 0 }).collect();
                    self.gfns.push(GenFn { name: h.name, tparams: h.generics, pids, tok, pos: h.pos, params: vec![], ret: Ty::Unit });
                    continue;
                }
                if tok < self.prelude_len {
                    // built-in library function: compiled when something calls it
                    self.skip_item()?;
                    self.lazy_fns.insert(h.name, tok);
                    continue;
                }
                let body = self.block()?;
                funcs.push(Func { name: h.name, params: h.params, ret: h.ret, body, pos: h.pos });
            }
        }
        // templates: every generic function with its type parameters abstract, checked once against its bounds
        let mut templates = vec![];
        for gi in 0..self.gfns.len() {
            let g = self.gfns[gi].clone();
            for (p, pid) in g.tparams.iter().zip(&g.pids) {
                self.types.params[*pid].bounds = p.bounds.iter().map(|b| b.0.clone()).collect();
            }
            let subst: Vec<(String, Ty)> = g.tparams.iter().zip(&g.pids).map(|(p, id)| (p.name.clone(), Ty::Param(*id))).collect();
            let (h, body) = self.parse_fn_at(g.tok, subst, None, g.tok < self.prelude_len)?;
            self.gfns[gi].params = h.params.iter().map(|p| (p.mode, p.ty)).collect();
            self.gfns[gi].ret = h.ret;
            if g.tok < self.prelude_len {
                continue; // the built-in library is trusted: only its instantiations are checked
            }
            templates.push(Func { name: h.name, params: h.params, ret: h.ret, body: body.unwrap_or_default(), pos: h.pos });
        }
        Ok(Program { types: std::mem::take(&mut self.types), funcs, externs, links, templates })
    }

    /// Re-parse the function starting at token `tok` with `subst` as its type parameters (and `self_ty`
    /// as `Self`). With `header_only`, the body is not parsed.
    pub fn parse_fn_at(
        &mut self,
        tok: usize,
        subst: Vec<(String, Ty)>,
        self_ty: Option<Ty>,
        header_only: bool,
    ) -> PR<(FnHeader, Option<Vec<Stmt>>)> {
        let saved = (self.i, std::mem::take(&mut self.tparams), self.self_ty);
        self.i = tok;
        self.tparams = subst.into_iter().collect();
        self.self_ty = self_ty;
        let r = (|| {
            let h = self.fn_header()?;
            let b = if header_only { None } else { Some(self.block()?) };
            Ok((h, b))
        })();
        self.i = saved.0;
        self.tparams = saved.1;
        self.self_ty = saved.2;
        r
    }

    /// Abstract type of a generic struct/enum (its parameters left open) used to infer type arguments.
    pub fn generic_template(&mut self, name: &str) -> Option<(Ty, Vec<Ty>)> {
        let g = self.gtypes.get(name)?.clone();
        let args: Vec<Ty> = g.pids.iter().map(|p| Ty::Param(*p)).collect();
        let t = self.inst_type(name, args.clone(), g.pos).ok()?;
        Some((t, args))
    }
    pub fn generic_instance(&mut self, name: &str, args: Vec<Ty>, pos: Pos) -> PR<Ty> {
        self.inst_type(name, args, pos)
    }
    pub fn generic_is_enum(&self, name: &str) -> bool {
        self.gtypes.get(name).map_or(false, |g| g.is_enum)
    }
    pub fn generic_type_params(&self, name: &str) -> Vec<String> {
        self.gtypes.get(name).map(|g| g.params.clone()).unwrap_or_default()
    }

    /// The rest of `Int<lo..hi>` after the `<`: inclusive literal bounds, either side may be omitted.
    fn range_after_lt(&mut self, p: Pos) -> PR<Ty> {
        let bound = |this: &mut Self| -> PR<i64> {
            let neg = this.eat_sym("-");
            let bp = this.pos();
            let v = match this.bump().t {
                Tok::Num(n) => n as i128,
                Tok::NumT(n, "") => n as i128,
                _ => return Err(diag("range bounds must be integer constants", bp)
                    .note("help: write the bounds as literals, e.g. `Int<0..255>`; the bounds are inclusive")),
            };
            let v = if neg { -v } else { v };
            i64::try_from(v).map_err(|_| diag("range bound is outside Int", bp))
        };
        let lo = if self.eat_sym("..") { i64::MIN } else {
            let lo = bound(self)?;
            self.expect_sym("..")?;
            lo
        };
        let hi = if self.is_sym(">") { i64::MAX } else { bound(self)? };
        self.expect_sym(">")?;
        if lo > hi { return Err(diag(format!("empty integer range {}..{}", lo, hi), p).note("the bounds of `Int<lo..hi>` are inclusive and `lo` must not exceed `hi`")); }
        Ok(Ty::Range(lo, hi))
    }

    fn type_name(&mut self) -> PR<(String, Pos)> {
        let (n, p) = self.ident()?;
        if BUILTIN_TYPES.contains(&n.as_str()) || numeric::NUMERIC_TYPE_NAMES.contains(&n.as_str()) {
            return Err(diag(format!("'{}' is a built-in type name", n), p));
        }
        Ok((n, p))
    }

    fn struct_def(&mut self) -> PR<StructDef> {
        self.bump();
        let (name, pos) = self.type_name()?;
        self.expect_sym("{")?;
        let fields = self.struct_fields()?;
        self.expect_sym("}")?;
        Ok(StructDef { cname: name.clone(), name, fields, pos, origin: None, generic: false })
    }
    fn struct_fields(&mut self) -> PR<Vec<(String, Ty)>> {
        let mut fields: Vec<(String, Ty)> = vec![];
        while !self.is_sym("}") {
            let (f, fp) = self.ident()?;
            if fields.iter().any(|(n, _)| *n == f) {
                return Err(diag(format!("duplicate field '{}'", f), fp));
            }
            self.expect_sym(":")?;
            let t = self.ty()?;
            fields.push((f, t));
            if !self.eat_sym(",") {
                break;
            }
        }
        Ok(fields)
    }

    fn enum_def(&mut self) -> PR<EnumDef> {
        self.bump();
        let (name, pos) = self.type_name()?;
        self.expect_sym("{")?;
        let variants = self.enum_variants()?;
        self.expect_sym("}")?;
        Ok(EnumDef { cname: name.clone(), name, variants, pos, origin: None, generic: false })
    }
    fn enum_variants(&mut self) -> PR<Vec<(String, Vec<Ty>)>> {
        let mut variants: Vec<(String, Vec<Ty>)> = vec![];
        while !self.is_sym("}") {
            let (v, vp) = self.ident()?;
            if variants.iter().any(|(n, _)| *n == v) {
                return Err(diag(format!("duplicate variant '{}'", v), vp));
            }
            let mut payload = vec![];
            if self.eat_sym("(") {
                while !self.is_sym(")") {
                    payload.push(self.ty()?);
                    if !self.eat_sym(",") {
                        break;
                    }
                }
                self.expect_sym(")")?;
            }
            variants.push((v, payload));
            if !self.eat_sym(",") {
                break;
            }
        }
        Ok(variants)
    }

    /// `trait Name { fn m(view self, ..) -> T  ... }`
    fn trait_def(&mut self) -> PR<()> {
        self.bump();
        let prelude = self.i < self.prelude_len;
        let (name, pos) = self.type_name()?;
        if self.traits.iter().any(|t| t.name == name) {
            let msg = if self.traits.iter().any(|t| t.name == name && t.prelude) {
                format!("'{}' is a built-in trait", name)
            } else {
                format!("trait '{}' is defined twice", name)
            };
            return Err(diag(msg, pos).note("help: pick another name for your trait"));
        }
        self.expect_sym("{")?;
        let self_param = self.new_param("Self", vec![]);
        let mut methods: Vec<TraitMethod> = vec![];
        while !self.is_sym("}") {
            if !self.is_kw("fn") {
                return Err(diag("expected 'fn' (a trait lists method signatures)", self.pos()));
            }
            let tok = self.i;
            // validate the signature with `Self` left abstract
            let (h, _) = self.parse_fn_at(tok, vec![], Some(self_param), true)?;
            if methods.iter().any(|m| m.name == h.name) {
                return Err(diag(format!("method '{}' is listed twice in trait {}", h.name, name), h.pos));
            }
            if !h.generics.is_empty() {
                return Err(diag("trait methods cannot be generic yet", h.pos));
            }
            if !matches!(h.params.first(), Some(p) if p.name == "self") {
                return Err(diag(format!("method '{}' must take `self` first (`view self`, `mut self` or `self`)", h.name), h.pos)
                    .note("traits only describe methods you call as `x.method()`"));
            }
            // advance past this signature (the real parse above restored the cursor)
            self.bump(); // fn
            while !(self.is_kw("fn") || self.is_sym("}") || self.is_sym(";")) {
                if matches!(self.peek(), Tok::Eof) {
                    return Err(diag("missing closing '}'", pos));
                }
                if self.is_sym("{") {
                    return Err(diag("a trait lists signatures only: remove the body", self.pos())
                        .note("the body goes in `impl Trait for Type { .. }`"));
                }
                self.bump();
            }
            self.eat_sym(";");
            let sig = FnTy { params: h.params.iter().map(|p| (p.mode, p.ty)).collect(), ret: h.ret };
            methods.push(TraitMethod { name: h.name, tok, sig });
        }
        self.expect_sym("}")?;
        self.traits.push(TraitDef { name, pos, methods, prelude });
        Ok(())
    }

    /// `impl Trait for Type { fn .. }` or `impl Type { fn .. }`
    fn impl_def(&mut self, funcs: &mut Vec<Func>) -> PR<()> {
        let ipos = self.bump().pos;
        let prelude = self.i < self.prelude_len;
        // `impl<T: Show> ...`: only the built-in library has generic impls
        let mut generics: Vec<TParam> = vec![];
        let mut pids: Vec<usize> = vec![];
        let saved_tp = self.tparams.clone();
        if self.is_sym("<") {
            if !prelude {
                return Err(diag("generic `impl` blocks are not supported yet", self.pos())
                    .note("help: write a generic function instead (`fn show_all<T: Show>(items: view List<T>)`), or an `impl` for one concrete type"));
            }
            generics = self.generics_list()?;
            for g in &generics {
                let t = self.new_param(&g.name, g.bounds.iter().map(|b| b.0.clone()).collect());
                if let Ty::Param(i) = t {
                    pids.push(i);
                }
                self.tparams.insert(g.name.clone(), t);
            }
        }
        let subst: Vec<(String, Ty)> = generics.iter().zip(&pids).map(|(g, p)| (g.name.clone(), Ty::Param(*p))).collect();
        let (mut trait_name, mut trait_pos) = (None, ipos);
        if matches!(self.peek(), Tok::Id(_)) && matches!(self.peek_at(1), Tok::Id(k) if k == "for") {
            let (n, p) = self.ident()?;
            self.bump();
            trait_name = Some(n);
            trait_pos = p;
        }
        let self_ty = self.ty()?;
        self.tparams = saved_tp;
        self.expect_sym("{")?;
        let mut methods: Vec<ImplMethod> = vec![];
        while !self.is_sym("}") {
            if !self.is_kw("fn") {
                return Err(diag("expected 'fn' inside `impl`", self.pos()));
            }
            let tok = self.i;
            let (h, _) = self.parse_fn_at(tok, subst.clone(), Some(self_ty), true)?;
            if !h.generics.is_empty() {
                return Err(diag("methods cannot be generic yet", h.pos));
            }
            if methods.iter().any(|m| m.name == h.name) {
                return Err(diag(format!("method '{}' is defined twice in this impl", h.name), h.pos));
            }
            let lazy = prelude;
            if lazy {
                self.skip_item()?;
            } else {
                let saved = self.self_ty;
                self.self_ty = Some(self_ty);
                let r = (|| {
                    let h = self.fn_header()?;
                    let body = self.block()?;
                    Ok::<_, Diag>((h, body))
                })();
                self.self_ty = saved;
                let (h2, body) = r?;
                let name = impl_fn_name(&self.types, trait_name.as_deref(), self_ty, &h2.name);
                funcs.push(Func { name, params: h2.params, ret: h2.ret, body, pos: h2.pos });
            }
            methods.push(ImplMethod { name: h.name, tok, parsed: !lazy, pos: h.pos });
        }
        self.expect_sym("}")?;
        self.impls.push(ImplDef { trait_name, trait_pos, generics, pids, self_ty, pos: ipos, methods, prelude });
        Ok(())
    }

    fn fn_header(&mut self) -> PR<FnHeader> {
        if !self.is_kw("fn") {
            return Err(diag("expected 'fn', 'struct', 'enum', 'trait' or 'impl'", self.pos()));
        }
        self.bump();
        let (name, npos) = self.ident()?;
        let generics = if self.is_sym("<") { self.generics_list()? } else { vec![] };
        // first sight of a generic header: its parameter names must resolve (to a placeholder)
        let added: Vec<String> =
            generics.iter().filter(|g| !self.tparams.contains_key(&g.name)).map(|g| g.name.clone()).collect();
        for n in &added {
            self.tparams.insert(n.clone(), Ty::Err);
        }
        let r = self.fn_header_rest(name, npos, generics);
        for n in &added {
            self.tparams.remove(n);
        }
        r
    }
    fn fn_header_rest(&mut self, name: String, npos: Pos, generics: Vec<TParam>) -> PR<FnHeader> {
        self.expect_sym("(")?;
        let mut params: Vec<Param> = vec![];
        while !self.is_sym(")") {
            if self.in_extern && self.is_sym("..") && *self.peek_at(1) == Tok::Sym(".") {
                let vp = self.pos();
                if params.is_empty() {
                    return Err(diag("a variadic extern function needs at least one fixed parameter before `...`", vp));
                }
                self.bump();
                self.bump();
                self.saw_variadic = true;
                if !self.is_sym(")") {
                    return Err(diag("`...` must be the last parameter", self.pos()));
                }
                break;
            }
            let first = params.is_empty();
            // `self`, `view self`, `mut self`: only the first parameter of a method
            let mode_kw = if self.is_kw("view") { Some(Mode::View) } else if self.is_kw("mut") { Some(Mode::Mut) } else { None };
            let self_at = if mode_kw.is_some() { 1 } else { 0 };
            if first && self.self_ty.is_some() && matches!(self.peek_at(self_at), Tok::Id(s) if s == "self")
                && matches!(self.peek_at(self_at + 1), Tok::Sym(",") | Tok::Sym(")"))
            {
                for _ in 0..=self_at {
                    self.bump();
                }
                params.push(Param { name: "self".into(), mode: mode_kw.unwrap_or(Mode::Own), ty: self.self_ty.unwrap(), id: 0 });
            } else {
                let (pn, pp) = self.ident()?;
                if params.iter().any(|p| p.name == pn) {
                    return Err(diag(format!("duplicate parameter '{}'", pn), pp));
                }
                self.expect_sym(":")?;
                let mode = if self.is_kw("view") {
                    self.bump();
                    Mode::View
                } else if self.is_kw("mut") {
                    self.bump();
                    Mode::Mut
                } else {
                    Mode::Own
                };
                let ty = if mode == Mode::Own { self.ty()? } else { self.indirect_ty()? };
                params.push(Param { name: pn, mode, ty, id: 0 });
            }
            if !self.eat_sym(",") {
                break;
            }
        }
        self.expect_sym(")")?;
        let ret = if self.eat_sym("->") { self.ty()? } else { Ty::Unit };
        Ok(FnHeader { name, pos: npos, generics, params, ret })
    }

    fn block(&mut self) -> PR<Vec<Stmt>> {
        self.enter()?;
        let r = self.block_inner();
        self.leave();
        r
    }
    fn block_inner(&mut self) -> PR<Vec<Stmt>> {
        self.expect_sym("{")?;
        let mut v = vec![];
        while !self.is_sym("}") {
            if matches!(self.peek(), Tok::Eof) {
                return Err(diag("missing closing '}'", self.pos()));
            }
            v.push(self.stmt()?);
        }
        self.expect_sym("}")?;
        Ok(v)
    }

    fn stmt(&mut self) -> PR<Stmt> {
        let pos = self.pos();
        let k = if self.is_kw("let") || self.is_kw("var") {
            let mutable = self.is_kw("var");
            self.bump();
            if self.is_sym("(") {
                let pat = self.unpack_pat(mutable)?;
                self.expect_sym("=")?;
                let e = self.expr()?;
                self.eat_sym(";");
                return Ok(Stmt { k: SK::Unpack(pat, e), pos });
            }
            let (n, _) = self.ident()?;
            let annot = if self.eat_sym(":") { Some(self.ty()?) } else { None };
            self.expect_sym("=")?;
            let e = self.expr()?;
            let e = match annot {
                Some(t) => { let p = e.pos; Expr { k: EK::Ascribe(Box::new(e)), pos: p, ty: t } }
                None => e,
            };
            SK::Let(n, mutable, e, 0)
        } else if self.is_kw("return") {
            self.bump();
            if self.is_sym("}") || self.is_sym(";") {
                SK::Return(None)
            } else {
                SK::Return(Some(self.expr()?))
            }
        } else if self.is_kw("unsafe") {
            self.bump();
            SK::Unsafe(self.block()?)
        } else if self.is_kw("if") {
            return self.if_stmt();
        } else if self.is_kw("scope") {
            self.bump();
            self.thread_scopes += 1;
            let body = self.block();
            self.thread_scopes -= 1;
            SK::Block(body?, false)
        } else if self.is_kw("parallel") {
            self.bump();
            if !self.is_kw("for") {
                return Err(diag("expected 'for' after 'parallel'", self.pos()));
            }
            let stmt = self.loop_stmt(None)?;
            let SK::For(head, body) = stmt.k else { unreachable!() };
            if head.mutable || head.step.is_some() {
                return Err(diag("parallel for requires a read-only iterator without a step", pos));
            }
            let (name, mut args, mode, ty) = match head.iter {
                ForIter::Range(first, end, inclusive) =>
                    ("__parallel_range", vec![first, end, mk(EK::Bool(inclusive), pos)], Mode::Own, Some(Ty::Int)),
                ForIter::Expr(list) => ("__parallel_list", vec![list], Mode::View, None),
            };
            let param = CParam { name: head.var, pos: head.var_pos, mode: Some(mode), ty };
            args.push(mk(EK::Closure {
                params: vec![param], ret: None, body, is_move: false, expr_body: false, parallel: true, worker: false,
            }, pos));
            SK::ExprS(mk(EK::Call(name.into(), args), pos))
        } else if self.is_kw("while") || self.is_kw("for") {
            return self.loop_stmt(None);
        } else if matches!(self.peek(), Tok::Id(n) if !KW.contains(&n.as_str()))
            && *self.peek_at(1) == Tok::Sym(":")
            && matches!(self.peek_at(2), Tok::Id(k) if k == "while" || k == "for")
        {
            let (name, lp) = self.ident()?;
            self.bump(); // ':'
            return self.loop_stmt(Some((name, lp)));
        } else if self.is_kw("break") || self.is_kw("continue") {
            let is_break = self.is_kw("break");
            let kp = self.bump().pos;
            let label = match self.peek() {
                Tok::Id(n) if !KW.contains(&n.as_str()) && self.pos().line == kp.line => Some(self.ident()?),
                _ => None,
            };
            if is_break {
                SK::Break(label, 0)
            } else {
                SK::Continue(label, 0)
            }
        } else if self.is_kw("match") {
            return self.match_stmt();
        } else if self.is_kw("defer") {
            self.bump();
            let ipos = self.pos();
            if ["let", "var", "return", "if", "while", "match", "defer"].iter().any(|k| self.is_kw(k)) {
                return Err(diag("`defer` takes a single call or assignment", ipos)
                    .note("help: put the work in a function and defer the call: `defer cleanup(x)`"));
            }
            let e = self.expr()?;
            let inner = if self.eat_sym("=") {
                let v = self.expr()?;
                SK::Assign(e, v)
            } else {
                SK::ExprS(e)
            };
            SK::Defer(Box::new(Stmt { k: inner, pos: ipos }))
        } else {
            let e = self.expr()?;
            if self.eat_sym("=") {
                let v = self.expr()?;
                SK::Assign(e, v)
            } else {
                SK::ExprS(e)
            }
        };
        self.eat_sym(";");
        Ok(Stmt { k, pos })
    }

    /// `(a, b)`, `(a, (b, _))` after `let` / `var`.
    fn unpack_pat(&mut self, mutable: bool) -> PR<UnpackPat> {
        let open = self.expect_sym("(")?;
        let mut items = vec![];
        while !self.is_sym(")") {
            if self.is_sym("(") {
                items.push(self.unpack_pat(mutable)?);
            } else {
                let (n, p) = self.ident()?;
                items.push(UnpackPat::Bind(n, mutable, 0, p));
            }
            if !self.eat_sym(",") {
                break;
            }
        }
        let close = self.expect_sym(")")?;
        if items.len() < 2 {
            return Err(diag("a tuple pattern needs at least two names", span(open, close))
                .note("example: `let (a, b) = pair`; use `_` for an element you do not need"));
        }
        Ok(UnpackPat::Tuple(items, span(open, close)))
    }

    /// `while cond { .. }` or `for x in .. { .. }`, optionally labeled (`outer: while ..`).
    fn loop_stmt(&mut self, label: Option<(String, Pos)>) -> PR<Stmt> {
        let pos = label.as_ref().map_or(self.pos(), |l| l.1);
        if self.is_kw("while") {
            self.bump();
            let c = self.expr()?;
            let b = self.block()?;
            return Ok(Stmt { k: SK::While(c, b, Loop { label, id: 0, step: vec![], freeze: None }), pos });
        }
        self.bump(); // for
        let mutable = if self.is_kw("mut") {
            self.bump();
            true
        } else {
            false
        };
        let (var, var_pos) = self.ident()?;
        if !self.is_kw("in") {
            return Err(diag("expected `in` after the loop variable", self.pos())
                .note("example: `for i in 0..10 { .. }` or `for x in list { .. }`"));
        }
        self.bump();
        let first = self.expr()?;
        let iter = if self.eat_sym("..") {
            ForIter::Range(first, self.expr()?, false)
        } else if self.eat_sym("..=") {
            ForIter::Range(first, self.expr()?, true)
        } else {
            ForIter::Expr(first)
        };
        let step = if self.is_kw("step") {
            self.bump();
            Some(self.expr()?)
        } else {
            None
        };
        let b = self.block()?;
        let head = ForHead { var, var_pos, mutable, iter, step, label };
        Ok(Stmt { k: SK::For(Box::new(head), b), pos })
    }

    fn if_stmt(&mut self) -> PR<Stmt> {
        let pos = self.pos();
        self.bump();
        let c = self.expr()?;
        let t = self.block()?;
        let e = if self.is_kw("else") {
            self.bump();
            if self.is_kw("if") {
                Some(vec![self.if_stmt()?])
            } else {
                Some(self.block()?)
            }
        } else {
            None
        };
        Ok(Stmt { k: SK::If(c, t, e), pos })
    }

    fn match_stmt(&mut self) -> PR<Stmt> {
        let pos = self.pos();
        self.bump();
        // `match mut e { .. }`: the payloads are bound as mutable borrows
        let scrut = if self.is_kw("mut") {
            let mp = self.bump().pos;
            let e = self.expr()?;
            let p = span(mp, e.pos);
            mk(EK::Un("mut", Box::new(e)), p)
        } else {
            self.expr()?
        };
        self.expect_sym("{")?;
        let mut arms = vec![];
        while !self.is_sym("}") {
            let apos = self.pos();
            let pat = self.pat()?;
            let guard = if self.is_kw("if") {
                self.bump();
                Some(self.expr()?)
            } else {
                None
            };
            if !self.is_sym("=>") {
                return Err(diag("expected '=>' after the pattern", self.pos())
                    .note("example: `Some(x) if x > 3 => { .. }`"));
            }
            self.bump();
            let body = self.block()?;
            self.eat_sym(",");
            // plain `Variant(a, b)` / `_` arms keep the original representation
            let plain = guard.is_none()
                && match &pat {
                    Pat::Wild(_) => true,
                    Pat::Variant(_, subs, _) => subs.iter().all(|p| matches!(p, Pat::Wild(_) | Pat::Bind(_, None, _))),
                    _ => false,
                };
            if plain {
                let (vname, binds) = match &pat {
                    Pat::Wild(_) => ("_".to_string(), vec![]),
                    Pat::Variant(n, subs, _) => (
                        n.clone(),
                        subs.iter()
                            .map(|p| match p {
                                Pat::Bind(n, _, _) => (n.clone(), 0),
                                _ => ("_".to_string(), 0),
                            })
                            .collect(),
                    ),
                    _ => unreachable!(),
                };
                arms.push(Arm { vname, variant: None, binds, body, pos: apos, pat: None, guard: None });
            } else {
                arms.push(Arm { vname: String::new(), variant: None, binds: vec![], body, pos: apos, pat: Some(pat), guard });
            }
        }
        self.expect_sym("}")?;
        Ok(Stmt { k: SK::Match(scrut, arms), pos })
    }

    /// `p1 | p2 | ..`
    fn pat(&mut self) -> PR<Pat> {
        self.enter()?;
        let r = self.pat_inner();
        self.leave();
        r
    }
    fn pat_inner(&mut self) -> PR<Pat> {
        let first = self.pat_atom()?;
        if !self.is_sym("|") {
            return Ok(first);
        }
        let p0 = pat_pos(&first);
        let mut alts = vec![first];
        while self.eat_sym("|") {
            alts.push(self.pat_atom()?);
        }
        Ok(Pat::Or(alts, p0))
    }

    /// A literal bound of a range pattern: `5`, `-5`, `'a'`.
    fn pat_bound(&mut self) -> PR<Option<(i64, bool)>> {
        let neg = self.is_sym("-");
        let save = self.i;
        if neg {
            self.bump();
        }
        match self.peek().clone() {
            Tok::Num(n) => {
                self.bump();
                Ok(Some((if neg { -n } else { n }, false)))
            }
            Tok::Char(c) if !neg => {
                self.bump();
                Ok(Some((c, true)))
            }
            _ => {
                self.i = save;
                Ok(None)
            }
        }
    }

    fn pat_atom(&mut self) -> PR<Pat> {
        let pos = self.pos();
        // `..=hi`
        if self.eat_sym("..=") {
            return match self.pat_bound()? {
                Some((hi, ch)) => Ok(Pat::Range(None, Some(hi), ch, pos)),
                None => Err(diag("expected a number or a character after `..=`", self.pos())),
            };
        }
        if let Some((lo, ch)) = self.pat_bound()? {
            let incl = if self.eat_sym("..=") {
                Some(true)
            } else if self.eat_sym("..") {
                Some(false)
            } else {
                None
            };
            return match incl {
                None => Ok(if ch { Pat::Char(lo, pos) } else { Pat::Int(lo, pos) }),
                Some(inclusive) => match self.pat_bound()? {
                    None if !inclusive => Ok(Pat::Range(Some(lo), None, ch, pos)),
                    None => Err(diag("expected a number or a character after `..=`", self.pos())),
                    Some((hi, ch2)) => {
                        if ch != ch2 {
                            return Err(diag("both ends of a range pattern must be the same kind", pos)
                                .note("write numbers on both sides (`1..=9`) or characters on both sides (`'a'..='z'`)"));
                        }
                        let hi = if inclusive {
                            hi
                        } else {
                            match hi.checked_sub(1) {
                                Some(h) => h,
                                None => return Err(diag("this range is empty", pos)),
                            }
                        };
                        if lo > hi {
                            return Err(diag("this range pattern is empty: the start is after the end", pos)
                                .note("help: write the smaller value first, e.g. `1..=9`"));
                        }
                        Ok(Pat::Range(Some(lo), Some(hi), ch, pos))
                    }
                },
            };
        }
        if self.eat_sym("(") {
            let mut items = vec![];
            let mut comma = false;
            while !self.is_sym(")") {
                items.push(self.pat()?);
                if self.eat_sym(",") {
                    comma = true;
                } else {
                    break;
                }
            }
            let close = self.expect_sym(")")?;
            if items.len() == 1 && !comma {
                return Ok(items.pop().unwrap()); // parenthesised pattern
            }
            if !(2..=6).contains(&items.len()) {
                return Err(diag("a tuple pattern has 2 to 6 elements", span(pos, close)));
            }
            return Ok(Pat::Tuple(items, span(pos, close)));
        }
        match self.peek().clone() {
            Tok::Id(n) if n == "true" || n == "false" => {
                self.bump();
                Ok(Pat::Bool(n == "true", pos))
            }
            Tok::Id(n) if n == "_" => {
                self.bump();
                Ok(Pat::Wild(pos))
            }
            Tok::Id(n) if !KW.contains(&n.as_str()) => {
                let (mut name, np) = self.ident()?;
                if self.eat_sym("@") {
                    let sub = self.pat_atom()?;
                    return Ok(Pat::Bind(name, Some(Box::new(sub)), np));
                }
                if self.eat_sym(".") {
                    // qualified pattern: Shape.Circle(r)
                    name = self.ident()?.0;
                    let subs = self.pat_args()?;
                    return Ok(Pat::Variant(name, subs, span(pos, self.t[self.i - 1].pos)));
                }
                let upper = name.chars().next().map_or(false, |c| c.is_ascii_uppercase());
                if upper || self.is_sym("(") {
                    let subs = self.pat_args()?;
                    return Ok(Pat::Variant(name, subs, span(pos, self.t[self.i - 1].pos)));
                }
                Ok(Pat::Bind(name, None, np))
            }
            _ => Err(diag("expected a pattern", pos).note("patterns: `Variant(x)`, `_`, a name, `1`, `1..=9`, `'a'`, `true`, `(a, b)`, `A | B`, `n @ 1..=9`")),
        }
    }

    fn pat_args(&mut self) -> PR<Vec<Pat>> {
        let mut subs = vec![];
        if self.eat_sym("(") {
            while !self.is_sym(")") {
                subs.push(self.pat()?);
                if !self.eat_sym(",") {
                    break;
                }
            }
            self.expect_sym(")")?;
        }
        Ok(subs)
    }

    fn expr(&mut self) -> PR<Expr> {
        let e = self.bin(0)?;
        // `x as Int<0..255>` (checked conversion) / `x as Int<0..255> else { .. }` (the block must not continue)
        let line = self.t[self.i.saturating_sub(1)].pos.line;
        if matches!(self.peek(), Tok::Id(n) if n == "as") && self.pos().line == line {
            let apos = self.bump().pos;
            let tp = self.pos();
            let ty = self.ty()?;
            if !matches!(ty, Ty::Range(..)) {
                return Err(diag("`as` converts an integer into a range type", tp)
                    .note("help: write `x as Int<0..255>` (checked, panics when x does not fit) or `x as Int<0..255> else { return }`"));
            }
            let end = self.t[self.i - 1].pos;
            let p = span(e.pos, end);
            if self.is_kw("else") {
                self.bump();
                let blk = self.block()?;
                let end = self.t[self.i - 1].pos;
                let p = span(e.pos, end);
                return Ok(Expr { k: EK::AsElse(Box::new(e), blk), pos: p, ty });
            }
            let _ = apos;
            return Ok(mk(EK::Static { ty, f: "from".into(), args: vec![e] }, p));
        }
        Ok(e)
    }

    fn bin(&mut self, lvl: usize) -> PR<Expr> {
        if lvl == LEVELS.len() {
            return self.unary();
        }
        let mut l = self.bin(lvl + 1)?;
        let mut n = 0;
        loop {
            let op = match self.peek() {
                Tok::Sym(s) if LEVELS[lvl].contains(s) => *s,
                _ => break,
            };
            self.bump();
            n += 1;
            self.too_long("this chain of operators", n)?;
            let r = self.bin(lvl + 1)?;
            let pos = span(l.pos, r.pos);
            l = mk(EK::Bin(op, Box::new(l), Box::new(r)), pos);
        }
        Ok(l)
    }

    fn unary(&mut self) -> PR<Expr> {
        self.enter()?;
        let r = self.unary_inner();
        self.leave();
        r
    }
    fn unary_inner(&mut self) -> PR<Expr> {
        let pos = self.pos();
        if self.is_sym("!") || self.is_sym("-") {
            let op = if self.is_sym("!") { "!" } else { "-" };
            self.bump();
            let x = self.unary()?;
            let p = span(pos, x.pos);
            return Ok(mk(EK::Un(op, Box::new(x)), p));
        }
        if matches!(self.peek(), Tok::Id(n) if n == "comptime")
            && matches!(self.peek_at(1), Tok::Id(_) | Tok::Num(_) | Tok::Char(_) | Tok::Sym("(") | Tok::Sym("-") | Tok::Sym("!"))
        {
            // `comptime e`: evaluated by the compiler (src/comptime.rs); a marker call until then
            self.bump();
            let x = self.unary()?;
            let p = span(pos, x.pos);
            return Ok(mk(EK::Call("__comptime".into(), vec![x]), p));
        }
        if self.is_sym("*") {
            // `*b` is `b.get()`: the value inside a Box, as a place
            self.bump();
            let x = self.unary()?;
            let p = span(pos, x.pos);
            return Ok(mk(EK::Method { recv: Box::new(x), m: "get".into(), args: vec![] }, p));
        }
        self.primary()
    }

    fn args(&mut self) -> PR<(Vec<Expr>, Pos)> {
        self.expect_sym("(")?;
        let mut v = vec![];
        while !self.is_sym(")") {
            v.push(self.expr()?);
            if !self.eat_sym(",") {
                break;
            }
        }
        let end = self.expect_sym(")")?;
        Ok((v, end))
    }

    /// `.method(args)` / `.field` / `.0` after any primary expression.
    fn postfix(&mut self, mut e: Expr) -> PR<Expr> {
        let mut n = 0;
        loop {
            // `make_adder(1)(2)`: call the function value a call returned (same line, so a tuple statement
            // starting with `(` on the next line is not taken for arguments)
            if self.is_sym("(")
                && matches!(e.k, EK::Call(..) | EK::CallFn(..) | EK::Method { .. })
                && self.i > 0
                && self.t[self.i - 1].pos.line == self.pos().line
            {
                let (args, end) = self.args()?;
                let p = span(e.pos, end);
                e = mk(EK::CallFn(Box::new(e), args), p);
                continue;
            }
            if self.is_sym("?") {
                let qp = self.bump().pos;
                let p = span(e.pos, qp);
                e = mk(EK::Un("?", Box::new(e)), p);
                continue;
            }
            if !self.is_sym(".") {
                break;
            }
            self.bump();
            n += 1;
            self.too_long("this chain of method calls", n)?;
            if let Tok::Num(n) = self.peek().clone() {
                let np = self.bump().pos;
                let p = span(e.pos, np);
                e = mk(EK::Field(Box::new(e), format!("_{}", n)), p);
                continue;
            }
            let (m, mp) = self.ident()?;
            if self.is_sym("(") {
                let (args, end) = self.args()?;
                let p = span(e.pos, end);
                e = mk(EK::Method { recv: Box::new(e), m, args }, p);
            } else {
                let p = span(e.pos, mp);
                e = mk(EK::Field(Box::new(e), m), p);
            }
        }
        Ok(e)
    }

    fn primary(&mut self) -> PR<Expr> {
        let e = self.primary_base()?;
        self.postfix(e)
    }

    fn primary_base(&mut self) -> PR<Expr> {
        let tk = self.bump();
        let pos = tk.pos;
        match tk.t {
            Tok::Num(n) => self.postfix(mk(EK::Int(n), pos)),
            Tok::NumT(v, sfx) => {
                let k = if sfx.is_empty() { EK::Int(v as i64) } else { EK::Int(v as i64) };
                let mut e = mk(k, pos);
                // a typed literal carries its type already; an untyped one that is too big keeps `Ty::Unit`
                // until the checker decides (it needs to be UInt64)
                if let Some(t) = numeric::suffix_ty(sfx) {
                    e.ty = t;
                } else {
                    e.ty = Ty::Err;
                }
                self.postfix(e)
            }
            Tok::Float(txt, sfx) => {
                let v: f64 = txt.parse().unwrap_or(0.0);
                let mut e = mk(EK::Float(v, txt), pos);
                if let Some(t) = numeric::suffix_ty(sfx) {
                    e.ty = t;
                }
                self.postfix(e)
            }
            Tok::Char(c) => Ok(mk(EK::Char(c), pos)),
            Tok::Str(s) => Ok(mk(EK::Text(s), pos)),
            Tok::Sym("|") | Tok::Sym("||") => {
                self.i -= 1; // the closure parser reads the opening bar itself
                self.closure(false, pos)
            }
            Tok::Sym("(") => {
                let e = self.expr()?;
                if self.is_sym(",") {
                    let mut items = vec![e];
                    while self.eat_sym(",") {
                        if self.is_sym(")") {
                            break;
                        }
                        items.push(self.expr()?);
                    }
                    let close = self.expect_sym(")")?;
                    if items.len() < 2 {
                        return Err(diag("a tuple needs at least two elements", span(pos, close))
                            .note("help: write the value itself, without parentheses and comma"));
                    }
                    if items.len() > 6 {
                        return Err(diag(format!("a tuple has at most 6 elements, this one has {}", items.len()), span(pos, close))
                            .note("help: for more elements, define a struct with named fields"));
                    }
                    let fields = items.into_iter().enumerate().map(|(k, x)| (format!("_{}", k), x)).collect::<Vec<_>>();
                    let name = format!("Tuple{}", fields.len());
                    return Ok(mk(EK::GenStruct { name, targs: None, fields }, span(pos, close)));
                }
                self.expect_sym(")")?;
                self.postfix(e)
            }
            Tok::Id(x) => match x.as_str() {
                "true" => Ok(mk(EK::Bool(true), pos)),
                "false" => Ok(mk(EK::Bool(false), pos)),
                "spawn" => {
                    if self.thread_scopes == 0 {
                        return Err(diag("spawn can only be used inside a scope block", pos));
                    }
                    if !self.is_kw("move") {
                        return Err(diag("spawn requires a move closure", self.pos()));
                    }
                    self.bump();
                    let body = if self.is_sym("{") {
                        let body = self.block()?;
                        mk(EK::Closure { params: vec![], ret: None, body, is_move: true, expr_body: false, parallel: false, worker: true }, pos)
                    } else {
                        let mut closure = self.closure(true, pos)?;
                        if let EK::Closure { worker, .. } = &mut closure.k {
                            *worker = true;
                        }
                        closure
                    };
                    Ok(mk(EK::Call("__spawn_owned".into(), vec![body]), pos))
                }
                "move" if self.is_sym("|") || self.is_sym("||") => self.closure(true, pos),
                "move" => {
                    let (n, p) = self.ident()?;
                    Ok(mk(EK::Move(n, 0), span(pos, p)))
                }
                _ if KW.contains(&x.as_str()) => Err(diag(format!("unexpected keyword '{}' here", x), pos)),
                _ => self.after_name(x, pos),
            },
            _ => Err(diag("expected an expression", pos)),
        }
    }

    /// `|a, b: Int, c: view List| body` / `|| body` / `|x| -> Int { .. }`, with an optional `move` before it.
    fn closure(&mut self, is_move: bool, start: Pos) -> PR<Expr> {
        let mut params: Vec<CParam> = vec![];
        if !self.eat_sym("||") {
            self.expect_sym("|")?;
            while !self.is_sym("|") {
                let (name, pos) = self.ident()?;
                if params.iter().any(|p| p.name == name) {
                    return Err(diag(format!("duplicate closure parameter '{}'", name), pos));
                }
                let (mut mode, mut ty) = (None, None);
                if self.eat_sym(":") {
                    mode = if self.is_kw("view") {
                        self.bump();
                        Some(Mode::View)
                    } else if self.is_kw("mut") {
                        self.bump();
                        Some(Mode::Mut)
                    } else {
                        None
                    };
                    ty = Some(if mode.is_some() { self.indirect_ty()? } else { self.ty()? });
                }
                params.push(CParam { name, mode, ty, pos });
                if !self.eat_sym(",") {
                    break;
                }
            }
            if !self.is_sym("|") {
                return Err(diag("expected `|` to end the closure parameters", self.pos())
                    .note("help: closures are written `|x, y| x + y`, or `|x: Int| { .. }` for a block body"));
            }
            self.bump();
        }
        let ret = if self.eat_sym("->") { Some(self.ty()?) } else { None };
        let expr_body = !self.is_sym("{");
        let (body, end) = if self.is_sym("{") {
            let b = self.block()?;
            (b, self.t[self.i - 1].pos)
        } else {
            if ret.is_some() {
                return Err(diag("a closure with `-> Type` needs a block body", self.pos())
                    .note("help: write `|x| -> Int { return x + 1 }`, or drop the return type: `|x| x + 1`"));
            }
            let e = self.expr()?;
            let p = e.pos;
            if self.is_sym("=") {
                // `|x| total = total + x`: an assignment is a statement, the closure returns nothing
                self.bump();
                let v = self.expr()?;
                let end = v.pos;
                (vec![Stmt { k: SK::Assign(e, v), pos: p }], end)
            } else {
                (vec![Stmt { k: SK::Return(Some(e)), pos: p }], p)
            }
        };
        Ok(mk(EK::Closure { params, ret, body, is_move, expr_body, parallel: false, worker: false }, span(start, end)))
    }

    fn struct_lit_fields(&mut self) -> PR<(Vec<(String, Expr)>, Pos)> {
        self.expect_sym("{")?;
        let mut fields = vec![];
        while !self.is_sym("}") {
            let (f, _) = self.ident()?;
            self.expect_sym(":")?;
            let v = self.expr()?;
            fields.push((f, v));
            if !self.eat_sym(",") {
                break;
            }
        }
        let end = self.expect_sym("}")?;
        Ok((fields, end))
    }

    fn after_name(&mut self, x: String, pos: Pos) -> PR<Expr> {
        if self.is_sym("(") {
            let (args, end) = self.args()?;
            return self.postfix(mk(EK::Call(x, args), span(pos, end)));
        }
        if (x == "List" && (self.is_sym(".") || self.is_sym("<"))) || ((x == "String" || x == "Char") && self.is_sym(".")) {
            let ty = if x == "List" { self.list_args()? } else if x == "Char" { Ty::Char } else { Ty::String };
            self.expect_sym(".")?;
            let (f, _) = self.ident()?;
            let (args, end) = self.args()?;
            return Ok(mk(EK::Static { ty, f, args }, span(pos, end)));
        }
        if x == "Ptr" && self.is_sym("<") && !self.names.contains_key("Ptr") && !self.gtypes.contains_key("Ptr") {
            // `Ptr<UInt8>.null()`, `Ptr<UInt8>.alloc(n)`
            self.bump();
            let pointee = self.indirect_ty()?;
            self.expect_sym(">")?;
            let ty = self.intern_ptr(pointee);
            self.expect_sym(".")?;
            let (f, _) = self.ident()?;
            let (args, end) = self.args()?;
            return Ok(mk(EK::Static { ty, f, args }, span(pos, end)));
        }
        if x == "Int" && self.is_sym("<") {
            // `Int<0..255>.try_from(n)`
            self.bump();
            let ty = self.range_after_lt(pos)?;
            self.expect_sym(".")?;
            let (f, _) = self.ident()?;
            let (args, end) = self.args()?;
            return Ok(mk(EK::Static { ty, f, args }, span(pos, end)));
        }
        if let Some(rt @ Ty::Range(..)) = self.names.get(&x).copied() {
            if self.is_sym(".") {
                // `Percent.try_from(n)` for `type Percent = Int<0..100>`
                self.bump();
                let (f, _) = self.ident()?;
                let (args, end) = self.args()?;
                return Ok(mk(EK::Static { ty: rt, f, args }, span(pos, end)));
            }
        }
        if x == "Box" && self.uses_builtin_box(pos) && (self.is_sym(".") || self.is_sym("<")) {
            // `Box.new(v)` (the pointee type comes from `v`) or `Box<T>.new(v)`
            let ty = if self.eat_sym("<") {
                let e = self.indirect_ty()?;
                self.expect_sym(">")?;
                self.intern_box(e)
            } else {
                Ty::Err
            };
            self.expect_sym(".")?;
            let (f, _) = self.ident()?;
            let (args, end) = self.args()?;
            return Ok(mk(EK::Static { ty, f, args }, span(pos, end)));
        }
        if let Some(g) = self.gtypes.get(&x).cloned() {
            let targs = if self.is_sym("<") { Some(self.type_args()?) } else { None };
            if g.is_enum {
                if !self.is_sym(".") {
                    return Err(diag(format!("expected '.Variant' after the enum {}", x), self.pos()));
                }
                self.bump();
                let (vname, vp) = self.ident()?;
                let (args, end) = if self.is_sym("(") { self.args()? } else { (vec![], vp) };
                return Ok(mk(EK::GenVariant { name: x, targs, vname, args }, span(pos, end)));
            }
            if self.is_sym(".") {
                // `Map<String, Int>.new()`: a function of the instantiated type
                self.bump();
                let (fname, _) = self.ident()?;
                let (args, end) = self.args()?;
                return Ok(mk(EK::GenVariant { name: x, targs, vname: fname, args }, span(pos, end)));
            }
            if !self.is_sym("{") {
                return Err(diag(format!("expected '{{' with the fields of {}", x), self.pos()));
            }
            let (fields, end) = self.struct_lit_fields()?;
            return Ok(mk(EK::GenStruct { name: x, targs, fields }, span(pos, end)));
        }
        match self.names.get(&x).copied() {
            Some(Ty::Enum(eid)) if self.is_sym(".") => {
                self.bump();
                let (vname, vp) = self.ident()?;
                let (args, end) = if self.is_sym("(") { self.args()? } else { (vec![], vp) };
                return Ok(mk(EK::Variant { eid, vname, vid: 0, args }, span(pos, end)));
            }
            Some(Ty::Struct(sid)) if self.is_sym("{") => {
                let (fields, end) = self.struct_lit_fields()?;
                return Ok(mk(EK::StructLit { sid, fields }, span(pos, end)));
            }
            _ => {}
        }
        // variable, then any chain of .field / .method(...)
        let mut e = mk(EK::Var(x, 0), pos);
        self.postfix(e)
    }
}
