// Language features of `yerux lsp` that need to know about several files and about types: resolution of a name
// to its declaration (locals, items, members by receiver type, imported modules, the standard library and the
// prelude), completion, references, rename, signature help and semantic tokens.
use super::*;
use crate::json::{n, obj, s};
use std::collections::HashSet;
use std::ops::Deref;
use std::sync::{Arc, Mutex, OnceLock};

// ───────────────────────── the files a request can see ─────────────────────────

pub enum DocRef<'a> {
    Open(&'a Doc),
    Own(Doc),
    Shared(Arc<Doc>),
}
impl Deref for DocRef<'_> {
    type Target = Doc;
    fn deref(&self) -> &Doc {
        match self {
            DocRef::Open(d) => d,
            DocRef::Own(d) => d,
            DocRef::Shared(d) => d,
        }
    }
}

pub struct Entry<'a> {
    pub uri: String,
    pub doc: DocRef<'a>,
    /// part of the compiler (std modules, prelude): read-only
    pub lib: bool,
}

/// Document 0 is the one being edited; then the modules it imports (transitively); the prelude is last.
pub struct Ctx<'a> {
    pub docs: Vec<Entry<'a>>,
    /// (document, alias) -> imported document
    pub links: Vec<(usize, String, usize)>,
}

impl Ctx<'_> {
    pub fn doc(&self, i: usize) -> &Doc {
        &self.docs[i].doc
    }
    pub fn prelude(&self) -> usize {
        self.docs.len() - 1
    }
    pub fn module_of(&self, from: usize, alias: &str) -> Option<usize> {
        self.links.iter().find(|(f, a, _)| *f == from && a == alias).map(|l| l.2)
    }
}

fn cache_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("yerux-lsp-{}", env!("CARGO_PKG_VERSION")))
}

/// Methods the compiler implements itself (no Yerux source anywhere): signatures for hover and completion.
const BUILTINS: &str = "// Methods implemented by the compiler itself (see the checker's list_method / string_method / array_method).
// This file only gives them signatures for the editor; it is not part of the language's source.

impl<T> List<T> {
    fn push(mut self, x: T)
    fn pop(mut self) -> T
    fn len(view self) -> Int
    fn get(view self, i: Int) -> T
    fn at(view self, i: Int) -> T
    fn set(mut self, i: Int, x: T)
    fn clear(mut self)
    fn swap(mut self, i: Int, j: Int)
}

impl String {
    fn len(view self) -> Int
    fn push_str(mut self, x: view String)
    fn push_int(mut self, n: Int)
    fn push_char(mut self, c: Char)
    fn push_byte(mut self, b: Int)
    fn push_slice(mut self, x: view String, from: Int, to: Int)
    fn eq(view self, x: view String) -> Bool
    fn byte(view self, i: Int) -> Int
    fn char_at(view self, i: Int) -> Char
    fn char_size(view self, i: Int) -> Int
    fn clear(mut self)
}

impl<T> Array<T> {
    fn len(view self) -> Int
    fn get(view self, i: Int) -> T
    fn set(mut self, i: Int, x: T)
    fn swap(mut self, i: Int, j: Int)
}
";

static LIB_DOCS: OnceLock<Mutex<HashMap<String, (String, Arc<Doc>)>>> = OnceLock::new();

/// The standard-library module `key` (`std.io`) or the prelude (`""`) as a document backed by a real file in a
/// cache directory, so every editor can open it for go-to-definition.
pub fn lib_doc(key: &str) -> Option<(String, Arc<Doc>)> {
    let m = LIB_DOCS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = m.lock().unwrap();
    if let Some(x) = g.get(key) {
        return Some(x.clone());
    }
    let (rel, text): (String, String) = if key == "!builtins" {
        ("builtins.yx".into(), BUILTINS.to_string())
    } else if key.is_empty() {
        ("prelude.yx".into(), syntax::PRELUDE.to_string())
    } else {
        (format!("{}.yx", key.replace('.', "/")), crate::stdlib::source(key)?.to_string())
    };
    let path = cache_dir().join(&rel);
    if std::fs::read_to_string(&path).map_or(true, |old| old != text) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, &text);
    }
    let uri = path_to_uri(&path.to_string_lossy());
    let e = (uri, Arc::new(Doc::new(text)));
    g.insert(key.to_string(), e.clone());
    Some(e)
}

impl Server {
    pub fn ctx<'a>(&'a self, uri: &str) -> Option<Ctx<'a>> {
        let cur = self.docs.get(uri)?;
        let mut docs = vec![Entry { uri: uri.to_string(), doc: DocRef::Open(cur), lib: false }];
        let mut links = vec![];
        let mut i = 0;
        while i < docs.len() && docs.len() < 40 {
            let imps = imports(&docs[i].doc);
            let importer = uri_to_path(&docs[i].uri);
            for (alias, module) in imps {
                let found = if module.starts_with("std.") && crate::stdlib::source(&module).is_some() {
                    lib_doc(&module).map(|(u, d)| (u, DocRef::Shared(d), true))
                } else {
                    let root = std::path::Path::new(&importer).parent().map(|p| p.to_path_buf()).unwrap_or_default();
                    let file = root.join(format!("{}.yx", module.replace('.', "/")));
                    let furi = path_to_uri(&file.to_string_lossy());
                    match self.docs.get(&furi) {
                        Some(d) => Some((furi, DocRef::Open(d), false)),
                        None => std::fs::read_to_string(&file).ok().map(|t| (furi, DocRef::Own(Doc::new(t)), false)),
                    }
                };
                if let Some((furi, d, lib)) = found {
                    let idx = match docs.iter().position(|e| e.uri == furi) {
                        Some(k) => k,
                        None => {
                            docs.push(Entry { uri: furi, doc: d, lib });
                            docs.len() - 1
                        }
                    };
                    links.push((i, alias, idx));
                }
            }
            i += 1;
        }
        if let Some((u, d)) = lib_doc("!builtins") {
            docs.push(Entry { uri: u, doc: DocRef::Shared(d), lib: true });
        }
        if let Some((u, d)) = lib_doc("") {
            docs.push(Entry { uri: u, doc: DocRef::Shared(d), lib: true });
        }
        Some(Ctx { docs, links })
    }
}

// ───────────────────────── types as text ─────────────────────────

/// `List<Int>` -> ("List", ["Int"]); module prefixes (`a__B`), `view`/`mut` and `Box<..>`-free forms are normalised.
pub fn split_ty(t: &str) -> (String, Vec<String>) {
    let mut t = t.trim();
    for pre in ["view ", "mut ", "move "] {
        if let Some(r) = t.strip_prefix(pre) {
            t = r.trim();
        }
    }
    if t.starts_with('(') && t.ends_with(')') {
        let inner = &t[1..t.len() - 1];
        let parts = split_top(inner);
        return (format!("Tuple{}", parts.len()), parts);
    }
    if t.starts_with('[') && t.ends_with(']') {
        let inner = &t[1..t.len() - 1];
        let elem = inner.split(';').next().unwrap_or("").trim().to_string();
        return ("Array".into(), vec![elem]);
    }
    let (head, rest) = match t.find('<') {
        Some(p) if t.ends_with('>') => (&t[..p], Some(&t[p + 1..t.len() - 1])),
        _ => (t, None),
    };
    let head = head.rsplit("__").next().unwrap_or(head).rsplit('.').next().unwrap_or(head).trim();
    (head.to_string(), rest.map(split_top).unwrap_or_default())
}

fn split_top(s: &str) -> Vec<String> {
    let mut out = vec![];
    let (mut d, mut cur) = (0i32, String::new());
    for c in s.chars() {
        match c {
            '<' | '(' | '[' => d += 1,
            '>' | ')' | ']' => d -= 1,
            ',' if d == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Replace whole identifiers of `text` according to `map`.
fn subst(text: &str, map: &[(String, String)]) -> String {
    let mut out = String::new();
    let cs: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        if cs[i].is_alphabetic() || cs[i] == '_' {
            let st = i;
            while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') {
                i += 1;
            }
            let w: String = cs[st..i].iter().collect();
            match map.iter().find(|(k, _)| *k == w) {
                Some((_, v)) => out.push_str(v),
                None => out.push_str(&w),
            }
        } else {
            out.push(cs[i]);
            i += 1;
        }
    }
    out
}

// ───────────────────────── items, impls, signatures ─────────────────────────

pub struct FnParts {
    /// parameter texts without `self`
    pub params: Vec<String>,
    pub has_self: bool,
    pub ret: Option<String>,
    pub sig: String,
}

/// The pieces of the function whose name token is `name_tok` (and whose item ends at token `end`).
pub fn fn_parts(doc: &Doc, name_tok: usize, start: usize, end: usize) -> FnParts {
    let t = &doc.toks;
    let mut j = name_tok + 1;
    if j < t.len() && is_sym(&t[j], "<") {
        let mut d = 0;
        while j < t.len() {
            if is_sym(&t[j], "<") {
                d += 1;
            } else if is_sym(&t[j], ">") {
                d -= 1;
            } else if is_sym(&t[j], ">>") {
                d -= 2;
            }
            j += 1;
            if d <= 0 {
                break;
            }
        }
    }
    let mut params = vec![];
    let mut has_self = false;
    let mut close = j;
    if j < t.len() && is_sym(&t[j], "(") {
        let mut d = 0i32;
        let mut seg_start = j + 1;
        let mut k = j;
        while k < t.len() {
            let u = &t[k];
            let open = is_sym(u, "(") || is_sym(u, "[") || is_sym(u, "<") || is_sym(u, "{");
            let shut = is_sym(u, ")") || is_sym(u, "]") || is_sym(u, ">") || is_sym(u, "}");
            if open {
                d += 1;
            } else if shut {
                d -= 1;
            } else if is_sym(u, ">>") {
                d -= 2;
            }
            let at_end = d == 0 && is_sym(u, ")");
            if (d == 1 && is_sym(u, ",")) || at_end {
                if k > seg_start {
                    let text = doc.slice(seg_start, k - 1);
                    let first_ids: Vec<&str> = (seg_start..k).filter_map(|x| id_name(&t[x])).take(3).collect();
                    let is_self = first_ids.iter().take_while(|w| matches!(**w, "mut" | "view" | "move")).count() < first_ids.len()
                        && first_ids[first_ids.iter().take_while(|w| matches!(**w, "mut" | "view" | "move")).count()] == "self"
                        && !(seg_start..k).any(|x| is_sym(&t[x], ":"));
                    if is_self {
                        has_self = true;
                    } else {
                        params.push(text);
                    }
                }
                seg_start = k + 1;
            }
            if at_end {
                close = k;
                break;
            }
            k += 1;
        }
    }
    let mut ret = None;
    let mut sig_end = close.min(end);
    if close < end && close + 1 < t.len() && is_sym(&t[close + 1], "->") {
        let mut e = close + 2;
        while e <= end && e < t.len() && !is_sym(&t[e], "{") {
            e += 1;
        }
        if e > close + 2 {
            ret = Some(doc.slice(close + 2, e - 1));
            sig_end = e - 1;
        }
    }
    let sig = doc.slice(start, sig_end.max(start));
    FnParts { params, has_self, ret, sig }
}

pub struct ImplInfo {
    pub generics: Vec<String>,
    pub target: String,
    pub target_text: String,
    pub target_args: Vec<String>,
}

pub fn impl_info(doc: &Doc, sym: &Sym) -> ImplInfo {
    let t = &doc.toks;
    let mut j = sym.start + 1;
    let mut generics = vec![];
    if j < t.len() && is_sym(&t[j], "<") {
        let mut d = 0;
        let mut expect = true;
        while j < t.len() {
            if is_sym(&t[j], "<") {
                d += 1;
            } else if is_sym(&t[j], ">") {
                d -= 1;
            } else if d == 1 {
                if is_sym(&t[j], ",") {
                    expect = true;
                } else if expect {
                    if let Some(x) = id_name(&t[j]) {
                        generics.push(x.to_string());
                    }
                    expect = false;
                }
            }
            j += 1;
            if d == 0 {
                break;
            }
        }
    }
    let mut e = j;
    let mut d = 0i32;
    let mut for_at = None;
    while e < sym.end && !(is_sym(&t[e], "{") && d == 0) {
        if is_sym(&t[e], "<") || is_sym(&t[e], "(") {
            d += 1;
        } else if is_sym(&t[e], ">") || is_sym(&t[e], ")") {
            d -= 1;
        } else if is_id(&t[e], "for") && d == 0 {
            for_at = Some(e);
        }
        e += 1;
    }
    let tg = match for_at {
        Some(f) => doc.slice(f + 1, e - 1),
        None => if e > j { doc.slice(j, e - 1) } else { String::new() },
    };
    let (target, target_args) = split_ty(&tg);
    ImplInfo { generics, target, target_text: tg, target_args, }
}

#[derive(Clone)]
pub struct Member {
    pub name: String,
    pub is_field: bool,
    pub detail: String,
    pub doc: usize,
    pub tok: usize,
    pub ret: Option<String>,
    pub params: Vec<String>,
    pub has_self: bool,
}

/// Fields and methods of type `ty` (text such as `List<Int>`), with the type arguments applied to the signatures.
pub fn members_of(ctx: &Ctx, ty: &str) -> Vec<Member> {
    let (base, args) = split_ty(ty);
    let mut out: Vec<Member> = vec![];
    for (di, e) in ctx.docs.iter().enumerate() {
        let doc = &*e.doc;
        for sym in &doc.syms {
            match sym.kind {
                SymKind::Struct if sym.name == base => {
                    let gens = generic_names(doc, sym);
                    let map: Vec<(String, String)> = gens.into_iter().zip(args.iter().cloned()).collect();
                    for f in &sym.children {
                        let text = doc.slice(f.start, f.end);
                        let tyt = text.split_once(':').map(|x| x.1.trim().to_string()).unwrap_or_default();
                        out.push(Member {
                            name: f.name.clone(),
                            is_field: true,
                            detail: text.clone(),
                            doc: di,
                            tok: f.name_tok,
                            ret: Some(subst(&tyt, &map)),
                            params: vec![],
                            has_self: false,
                        });
                    }
                }
                SymKind::Impl => {
                    let info = impl_info(doc, sym);
                    if info.target != base {
                        continue;
                    }
                    let mut map: Vec<(String, String)> = vec![("Self".into(), ty.to_string())];
                    for (k, a) in info.target_args.iter().enumerate() {
                        if info.generics.contains(a) {
                            if let Some(actual) = args.get(k) {
                                map.push((a.clone(), actual.clone()));
                            }
                        }
                    }
                    for m in &sym.children {
                        if m.kind != SymKind::Method {
                            continue;
                        }
                        let parts = fn_parts(doc, m.name_tok, m.start, m.end);
                        out.push(Member {
                            name: m.name.clone(),
                            is_field: false,
                            detail: parts.sig.clone(),
                            doc: di,
                            tok: m.name_tok,
                            ret: parts.ret.as_ref().map(|r| subst(r, &map)),
                            params: parts.params,
                            has_self: parts.has_self,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    out
}

fn generic_names(doc: &Doc, sym: &Sym) -> Vec<String> {
    let t = &doc.toks;
    let mut out = vec![];
    let mut j = sym.name_tok + 1;
    if j < t.len() && is_sym(&t[j], "<") {
        let mut expect = true;
        j += 1;
        while j < t.len() && !is_sym(&t[j], ">") && !is_sym(&t[j], "{") {
            if is_sym(&t[j], ",") {
                expect = true;
            } else if expect {
                if let Some(x) = id_name(&t[j]) {
                    out.push(x.to_string());
                }
                expect = false;
            }
            j += 1;
        }
    }
    out
}

/// Does a type with this name exist (struct, enum, trait, or the target of an impl)?
fn type_exists(ctx: &Ctx, name: &str) -> bool {
    ctx.docs.iter().any(|e| {
        e.doc.syms.iter().any(|s| match s.kind {
            SymKind::Struct | SymKind::Enum | SymKind::Trait | SymKind::Alias => s.name == name,
            SymKind::Impl => impl_info(&e.doc, s).target == name,
            _ => false,
        })
    })
}

// ───────────────────────── expression types ─────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum Cur {
    Ty(String),
    Module(usize),
    TypeRef(String),
}

fn matching_close(t: &[Token], open: usize) -> usize {
    let (o, c) = if is_sym(&t[open], "(") { ("(", ")") } else if is_sym(&t[open], "[") { ("[", "]") } else { ("{", "}") };
    let mut d = 0;
    for (i, x) in t.iter().enumerate().skip(open) {
        if is_sym(x, o) {
            d += 1;
        } else if is_sym(x, c) {
            d -= 1;
            if d == 0 {
                return i;
            }
        }
    }
    t.len() - 1
}

fn matching_open(t: &[Token], close: usize) -> usize {
    let (o, c) = if is_sym(&t[close], ")") { ("(", ")") } else if is_sym(&t[close], "]") { ("[", "]") } else { ("{", "}") };
    let mut d = 0;
    let mut i = close + 1;
    while i > 0 {
        i -= 1;
        if is_sym(&t[i], c) {
            d += 1;
        } else if is_sym(&t[i], o) {
            d -= 1;
            if d == 0 {
                return i;
            }
        }
    }
    0
}

/// First token of the receiver chain (`a.b(c).d[i]`) that ends at token `end`.
pub fn chain_start(doc: &Doc, end: usize) -> Option<usize> {
    let t = &doc.toks;
    let mut j = end;
    loop {
        // j is the last token of one element
        let start = if is_sym(&t[j], "?") {
            return chain_start(doc, j.checked_sub(1)?);
        } else if is_sym(&t[j], ")") {
            let o = matching_open(t, j);
            if o > 0 && (id_name(&t[o - 1]).is_some() || is_sym(&t[o - 1], ">")) {
                if is_sym(&t[o - 1], ">") {
                    let mut k = o - 1;
                    while k > 0 && !is_sym(&t[k], "<") {
                        k -= 1;
                    }
                    k.saturating_sub(1)
                } else {
                    o - 1
                }
            } else {
                o
            }
        } else if is_sym(&t[j], "]") {
            let o = matching_open(t, j);
            j = o.checked_sub(1)?;
            continue;
        } else if is_sym(&t[j], "}") {
            let o = matching_open(t, j);
            let ty = o.checked_sub(1)?;
            id_name(&t[ty]).filter(|x| x.chars().next().is_some_and(|c| c.is_uppercase()))?;
            ty
        } else if matches!(t[j].t, Tok::Id(_) | Tok::Str(_) | Tok::Num(_) | Tok::NumT(..) | Tok::Float(..) | Tok::Char(_)) {
            j
        } else {
            return None;
        };
        if start >= 2 && is_sym(&t[start - 1], ".") {
            j = start - 2;
        } else {
            return Some(start);
        }
    }
}

fn self_type(doc: &Doc, i: usize) -> Option<String> {
    let imp = doc.syms.iter().find(|s| s.kind == SymKind::Impl && s.start <= i && i <= s.end)?;
    Some(impl_info(doc, imp).target_text)
}

/// Type (as text) of the variable declared at token `decl`.
pub fn var_type(ctx: &Ctx, di: usize, decl: usize) -> Option<String> {
    let doc = ctx.doc(di);
    let t = &doc.toks;
    match decl_at(doc, decl)? {
        Decl::Let => {
            if di == 0 {
                if let Some(rows) = &doc.types {
                    let kw = &t[decl - 1];
                    if let Some(r) = rows.iter().find(|r| r.0 == kw.pos.line && r.1 == kw.pos.col) {
                        return Some(r.3.clone());
                    }
                }
            }
            if decl + 1 < t.len() && is_sym(&t[decl + 1], ":") {
                let mut e = decl + 2;
                while e < t.len() && !is_sym(&t[e], "=") && t[e].pos.line == t[decl].pos.line {
                    e += 1;
                }
                return Some(doc.slice(decl + 2, e - 1));
            }
            if decl + 1 < t.len() && is_sym(&t[decl + 1], "=") {
                let start = decl + 2;
                let mut e = start;
                let mut d = 0i32;
                while e + 1 < t.len() {
                    let u = &t[e + 1];
                    if is_sym(u, "(") || is_sym(u, "[") || is_sym(u, "{") {
                        d += 1;
                    } else if is_sym(u, ")") || is_sym(u, "]") || is_sym(u, "}") {
                        d -= 1;
                    }
                    if d < 0 || (d == 0 && u.pos.line > t[e].pos.line && !is_sym(u, ".")) {
                        break;
                    }
                    e += 1;
                }
                return match chain_type(ctx, di, start, e)? {
                    Cur::Ty(x) => Some(x),
                    _ => None,
                };
            }
            None
        }
        Decl::Param => {
            let mut e = decl + 2;
            let mut d = 0i32;
            while e < t.len() {
                if is_sym(&t[e], "(") || is_sym(&t[e], "<") || is_sym(&t[e], "[") {
                    d += 1;
                } else if is_sym(&t[e], ">") || is_sym(&t[e], "]") {
                    d -= 1;
                } else if is_sym(&t[e], ")") {
                    if d == 0 {
                        break;
                    }
                    d -= 1;
                } else if d == 0 && is_sym(&t[e], ",") {
                    break;
                }
                e += 1;
            }
            let text = doc.slice(decl + 2, e - 1);
            Some(match self_type(doc, decl) {
                Some(me) => subst(&text, &[("Self".into(), me)]),
                None => text,
            })
        }
        Decl::For => {
            let mut e = decl + 1;
            if !(e < t.len() && is_id(&t[e], "in")) {
                return None;
            }
            e += 1;
            let start = e;
            let mut range = false;
            while e < t.len() && !is_sym(&t[e], "{") {
                if is_sym(&t[e], "..") || is_sym(&t[e], "..=") {
                    range = true;
                }
                e += 1;
            }
            if range {
                return Some("Int".into());
            }
            match chain_type(ctx, di, start, e - 1)? {
                Cur::Ty(x) => {
                    let (b, a) = split_ty(&x);
                    matches!(b.as_str(), "List" | "Array").then(|| a.first().cloned()).flatten()
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn find_fn<'a>(doc: &'a Doc, name: &str) -> Option<&'a Sym> {
    doc.syms.iter().find(|s| s.kind == SymKind::Fn && s.name == name)
}

/// The type of the expression `toks[start..=end]` when it is a chain of names, calls, fields, methods and indexing.
pub fn chain_type(ctx: &Ctx, di: usize, start: usize, end: usize) -> Option<Cur> {
    let doc = ctx.doc(di);
    let t = &doc.toks;
    if start > end || end >= t.len() {
        return None;
    }
    let mut i = start;
    let mut cur = match &t[i].t {
        Tok::Str(_) => Cur::Ty("String".into()),
        Tok::Num(_) | Tok::NumT(..) => Cur::Ty("Int".into()),
        Tok::Float(..) => Cur::Ty("Float".into()),
        Tok::Char(_) => Cur::Ty("Char".into()),
        Tok::Sym(x) if *x == "(" => {
            let close = matching_close(t, i);
            if close > end {
                return None;
            }
            let inner = chain_type(ctx, di, i + 1, close.checked_sub(1)?)?;
            i = close;
            inner
        }
        Tok::Id(name) => {
            let name = name.as_str();
            let call = i < end && is_sym(&t[i + 1], "(");
            if name == "true" || name == "false" {
                Cur::Ty("Bool".into())
            } else if name == "self" {
                Cur::Ty(self_type(doc, i)?)
            } else if call {
                let close = matching_close(t, i + 1);
                let f = find_fn(doc, name).map(|s| (di, s)).or_else(|| find_fn(ctx.doc(ctx.prelude()), name).map(|s| (ctx.prelude(), s)))?;
                let parts = fn_parts(ctx.doc(f.0), f.1.name_tok, f.1.start, f.1.end);
                i = close;
                Cur::Ty(parts.ret?)
            } else if i < end && is_sym(&t[i + 1], "{") && name.chars().next().is_some_and(|c| c.is_uppercase()) {
                i = matching_close(t, i + 1);
                Cur::Ty(name.to_string())
            } else if let Some(d) = visible_locals(doc, i).into_iter().find(|&j| id_name(&t[j]) == Some(name)) {
                Cur::Ty(var_type(ctx, di, d)?)
            } else if let Some(m) = ctx.module_of(di, name) {
                Cur::Module(m)
            } else if type_exists(ctx, name) {
                // `List<String>.new()`: keep the type arguments
                if i < end && is_sym(&t[i + 1], "<") {
                    let (mut d, mut k) = (0i32, i + 1);
                    while k < t.len() {
                        if is_sym(&t[k], "<") {
                            d += 1;
                        } else if is_sym(&t[k], ">") {
                            d -= 1;
                        } else if is_sym(&t[k], ">>") {
                            d -= 2;
                        }
                        if d <= 0 {
                            break;
                        }
                        k += 1;
                    }
                    if k > end {
                        return None;
                    }
                    let full = doc.slice(i, k);
                    i = k;
                    Cur::TypeRef(full)
                } else {
                    Cur::TypeRef(name.to_string())
                }
            } else {
                return None;
            }
        }
        _ => return None,
    };
    while i < end {
        i += 1;
        if is_sym(&t[i], ".") && i < end {
            i += 1;
            let name = id_name(&t[i])?;
            let call = i < end && is_sym(&t[i + 1], "(");
            cur = step(ctx, &cur, name, call)?;
            if call {
                i = matching_close(t, i + 1);
            }
        } else if is_sym(&t[i], "[") {
            let Cur::Ty(x) = &cur else { return None };
            let (b, a) = split_ty(x);
            cur = match b.as_str() {
                "List" | "Array" => Cur::Ty(a.first()?.clone()),
                _ => return None,
            };
            i = matching_close(t, i);
        } else if is_sym(&t[i], "?") {
            let Cur::Ty(x) = &cur else { return None };
            let (b, a) = split_ty(x);
            cur = match b.as_str() {
                "Option" | "Result" => Cur::Ty(a.first()?.clone()),
                _ => return None,
            };
        } else {
            return None;
        }
    }
    (i == end).then_some(cur)
}

fn step(ctx: &Ctx, cur: &Cur, name: &str, call: bool) -> Option<Cur> {
    match cur {
        Cur::Module(m) => {
            let doc = ctx.doc(*m);
            let sym = doc.syms.iter().find(|s| s.name == name && s.kind != SymKind::Impl)?;
            match sym.kind {
                SymKind::Fn if call => Some(Cur::Ty(fn_parts(doc, sym.name_tok, sym.start, sym.end).ret?)),
                SymKind::Struct | SymKind::Enum | SymKind::Trait | SymKind::Alias => Some(Cur::TypeRef(name.to_string())),
                _ => None,
            }
        }
        Cur::TypeRef(ty) => {
            for e in &ctx.docs {
                for s in &e.doc.syms {
                    if s.kind == SymKind::Enum && s.name == split_ty(ty).0 && s.children.iter().any(|c| c.name == name) {
                        return Some(Cur::Ty(ty.clone()));
                    }
                }
            }
            let m = members_of(ctx, ty).into_iter().find(|m| m.name == name && !m.has_self && !m.is_field)?;
            Some(Cur::Ty(m.ret?))
        }
        Cur::Ty(ty) => {
            let m = members_of(ctx, ty).into_iter().find(|m| m.name == name && m.is_field != call)?;
            Some(Cur::Ty(m.ret?))
        }
    }
}

// ───────────────────────── resolution ─────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct Loc {
    pub doc: usize,
    pub tok: usize,
    /// false when the target is a guess (a member looked up by name only)
    pub exact: bool,
}

/// Where is the identifier at token `ti` of document `di` declared?
/// The type named before the `{` of the struct literal or pattern that encloses token `ti` (`Stats {`, `types.Lambda {`).
fn literal_type(doc: &Doc, ti: usize) -> Option<String> {
    let t = &doc.toks;
    let mut depth = 0i32;
    let mut j = ti;
    while j > 0 {
        j -= 1;
        if is_sym(&t[j], ")") || is_sym(&t[j], "]") || is_sym(&t[j], "}") {
            depth += 1;
        } else if is_sym(&t[j], "(") || is_sym(&t[j], "[") {
            depth -= 1;
            if depth < 0 {
                return None;
            }
        } else if is_sym(&t[j], "{") {
            if depth == 0 {
                let name = id_name(t.get(j.checked_sub(1)?)?)?;
                return name.chars().next().is_some_and(|c| c.is_uppercase()).then(|| name.to_string());
            }
            depth -= 1;
        }
    }
    None
}

pub fn resolve(ctx: &Ctx, di: usize, ti: usize) -> Option<Loc> {
    let doc = ctx.doc(di);
    let t = &doc.toks;
    let name = id_name(&t[ti])?;
    if decl_at(doc, ti).is_some() {
        return Some(Loc { doc: di, tok: ti, exact: true });
    }
    // the name of an item is its own declaration
    let mut flat = vec![];
    all_syms(&doc.syms, &mut flat);
    if flat.iter().any(|x| x.name_tok == ti && x.kind != SymKind::Impl) {
        return Some(Loc { doc: di, tok: ti, exact: true });
    }
    if ti >= 2 && is_sym(&t[ti - 1], ".") {
        let found = chain_start(doc, ti - 2).and_then(|st| chain_type(ctx, di, st, ti - 2));
        match found {
            Some(Cur::Module(m)) => {
                let s = ctx.doc(m).syms.iter().find(|s| s.name == name && s.kind != SymKind::Impl)?;
                return Some(Loc { doc: m, tok: s.name_tok, exact: true });
            }
            Some(Cur::Ty(ty)) => {
                let m = members_of(ctx, &ty).into_iter().find(|m| m.name == name)?;
                return Some(Loc { doc: m.doc, tok: m.tok, exact: true });
            }
            Some(Cur::TypeRef(ty)) => {
                for (k, e) in ctx.docs.iter().enumerate() {
                    for s in &e.doc.syms {
                        if s.kind == SymKind::Enum && s.name == split_ty(&ty).0 {
                            if let Some(c) = s.children.iter().find(|c| c.name == name) {
                                return Some(Loc { doc: k, tok: c.name_tok, exact: true });
                            }
                        }
                    }
                }
                let m = members_of(ctx, &ty).into_iter().find(|m| m.name == name)?;
                return Some(Loc { doc: m.doc, tok: m.tok, exact: true });
            }
            None => {
                // unknown receiver type: the first member of the file with that name
                let mut flat = vec![];
                all_syms(&doc.syms, &mut flat);
                return flat
                    .iter()
                    .find(|s| s.name == name && matches!(s.kind, SymKind::Method | SymKind::Field))
                    .map(|s| Loc { doc: di, tok: s.name_tok, exact: false });
            }
        }
    }
    // `Type { field: value }` (literal or pattern): the name before `:` is a field of `Type`, not a local or a function
    if ti > 0 && ti + 1 < t.len() && is_sym(&t[ti + 1], ":") && (is_sym(&t[ti - 1], "{") || is_sym(&t[ti - 1], ",")) {
        if let Some(ty) = literal_type(doc, ti) {
            let m = members_of(ctx, &ty).into_iter().find(|m| m.is_field && m.name == name)?;
            return Some(Loc { doc: m.doc, tok: m.tok, exact: true });
        }
    }
    if let Some(j) = visible_locals(doc, ti).into_iter().find(|&j| id_name(&t[j]) == Some(name)) {
        return Some(Loc { doc: di, tok: j, exact: true });
    }
    let mut order = vec![di];
    order.push(ctx.prelude());
    for k in order {
        let d = ctx.doc(k);
        if let Some(s) = d.syms.iter().find(|s| s.name == name && s.kind != SymKind::Impl) {
            return Some(Loc { doc: k, tok: s.name_tok, exact: true });
        }
        for s in &d.syms {
            if let Some(c) = s.children.iter().find(|c| c.kind == SymKind::Variant && c.name == name) {
                return Some(Loc { doc: k, tok: c.name_tok, exact: true });
            }
        }
        if let Some(s) = d.syms.iter().find(|s| s.kind == SymKind::Impl && impl_info(d, s).target == name) {
            return Some(Loc { doc: k, tok: s.start, exact: true });
        }
    }
    None
}

pub fn loc_json(ctx: &Ctx, l: &Loc) -> Json {
    obj(vec![("uri", s(&ctx.docs[l.doc].uri)), ("range", ctx.doc(l.doc).tok_range(l.tok))])
}

/// Text shown for a declaration (hover, completion detail).
pub fn describe(ctx: &Ctx, l: &Loc) -> Option<String> {
    let doc = ctx.doc(l.doc);
    let name = id_name(&doc.toks[l.tok])?;
    let signature = match decl_at(doc, l.tok) {
        Some(Decl::Let) => {
            let kw = if is_id(&doc.toks[l.tok - 1], "var") { "var" } else { "let" };
            Some(match var_type(ctx, l.doc, l.tok) {
                Some(ty) => format!("{} {}: {}", kw, name, ty),
                None => format!("{} {}", kw, name),
            })
        }
        Some(Decl::Param) => Some(match var_type(ctx, l.doc, l.tok) {
            Some(ty) => format!("(parameter) {}: {}", name, ty),
            None => format!("(parameter) {}", name),
        }),
        Some(Decl::For) => Some(match var_type(ctx, l.doc, l.tok) {
            Some(ty) => format!("(loop variable) {}: {}", name, ty),
            None => format!("(loop variable) {}", name),
        }),
        Some(Decl::Tuple) => Some(format!("(tuple element) {}", name)),
        Some(Decl::Closure) => Some(format!("(closure parameter) {}", name)),
        Some(Decl::Arm) => Some(format!("(pattern binding) {}", name)),
        None => item_signature(doc, l.tok),
    }?;
    let line = doc.toks[l.tok].pos.line;
    let source: Vec<&str> = doc.text.lines().collect();
    let mut comments = Vec::new();
    let mut i = line.saturating_sub(2);
    while let Some(raw) = source.get(i) {
        let trimmed = raw.trim_start();
        if let Some(text) = trimmed.strip_prefix("///") {
            comments.push(text.strip_prefix(' ').unwrap_or(text));
            if i == 0 { break; }
            i -= 1;
        } else if trimmed.is_empty() && comments.is_empty() {
            if i == 0 { break; }
            i -= 1;
        } else { break; }
    }
    comments.reverse();
    if comments.is_empty() { Some(signature) } else { Some(format!("{}\n\n{}", signature, comments.join("\n"))) }
}

// ───────────────────────── completion ─────────────────────────

const KEYWORDS: &[&str] = &[
    "fn", "let", "var", "if", "else", "while", "for", "in", "return", "match", "struct", "enum", "trait", "impl", "import", "pub", "move",
    "view", "mut", "defer", "break", "continue", "unsafe", "spawn", "parallel", "scope", "true", "false", "const", "type", "test", "as",
];

const SNIPPETS: &[(&str, &str, &str)] = &[
    ("fn", "function", "fn ${1:name}(${2}) {\n\t$0\n}"),
    ("fnr", "function with a result", "fn ${1:name}(${2}) -> ${3:Int} {\n\t$0\n}"),
    ("struct", "struct", "struct ${1:Name} {\n\t${2:field}: ${3:Int},\n}"),
    ("enum", "enum", "enum ${1:Name} {\n\t${2:First},\n\t${3:Second},\n}"),
    ("match", "match", "match ${1:value} {\n\t${2:pattern} => {\n\t\t$0\n\t}\n}"),
    ("for", "for loop", "for ${1:x} in ${2:xs} {\n\t$0\n}"),
    ("forr", "for over a range", "for ${1:i} in 0..${2:n} {\n\t$0\n}"),
    ("if", "if", "if ${1:cond} {\n\t$0\n}"),
    ("while", "while", "while ${1:cond} {\n\t$0\n}"),
    ("impl", "impl block", "impl ${1:Type} {\n\t$0\n}"),
    ("trait", "trait", "trait ${1:Name} {\n\tfn ${2:method}(self) -> ${3:Int}\n}"),
    ("test", "test block", "test \"${1:name}\" {\n\t$0\n}"),
];

fn item(label: &str, kind: usize, detail: &str) -> Json {
    obj(vec![("label", s(label)), ("kind", n(kind)), ("detail", s(detail))])
}

fn call_snippet(name: &str, params: &[String]) -> String {
    let inner: Vec<String> = params
        .iter()
        .enumerate()
        .map(|(k, p)| format!("${{{}:{}}}", k + 1, p.split(':').next().unwrap_or("x").trim().replace(['$', '}', '\\'], "")))
        .collect();
    format!("{}({})", name, inner.join(", "))
}

fn symbol_item(doc: &Doc, sym: &Sym) -> Option<Json> {
    if sym.name.contains("__") || sym.kind == SymKind::Impl {
        return None;
    }
    let kind = match sym.kind {
        SymKind::Fn | SymKind::Method => 3,
        SymKind::Struct => 22,
        SymKind::Enum => 13,
        SymKind::Trait => 8,
        SymKind::Const => 21,
        SymKind::Alias => 25,
        SymKind::Field => 5,
        SymKind::Variant => 20,
        SymKind::Impl => return None,
    };
    let mut it = item(&sym.name, kind, &item_signature(doc, sym.name_tok).unwrap_or_default().lines().next().unwrap_or("").to_string());
    if sym.kind == SymKind::Fn {
        let p = fn_parts(doc, sym.name_tok, sym.start, sym.end);
        if let Json::Obj(kv) = &mut it {
            kv.push(("insertText".into(), s(call_snippet(&sym.name, &p.params))));
            kv.push(("insertTextFormat".into(), n(2)));
        }
    }
    Some(it)
}

fn member_item(m: &Member) -> Json {
    let mut it = item(&m.name, if m.is_field { 5 } else { 2 }, &m.detail);
    if !m.is_field {
        if let Json::Obj(kv) = &mut it {
            kv.push(("insertText".into(), s(call_snippet(&m.name, &m.params))));
            kv.push(("insertTextFormat".into(), n(2)));
        }
    }
    it
}

pub fn module_names(importer_dir: &str, prefix: &str) -> Vec<String> {
    let mut out = vec![];
    for k in crate::stdlib::MODULES {
        if let Some(rest) = k.strip_prefix(prefix) {
            out.push(rest.split('.').next().unwrap_or("").to_string());
        }
    }
    if prefix.is_empty() {
        out.push("std".into());
        if let Ok(rd) = std::fs::read_dir(importer_dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "yx") {
                    if let Some(st) = p.file_stem() {
                        out.push(st.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out.retain(|x| x != "std" || prefix.is_empty());
    out
}

pub fn completion(ctx: &Ctx, line: usize, ch: usize) -> Json {
    let doc = ctx.doc(0);
    let t = &doc.toks;
    let (l, c) = doc.from_lsp(line, ch);
    // identifier being typed: the token that touches the cursor
    let touching = t.iter().position(|x| matches!(x.t, Tok::Id(_)) && x.pos.line == l && x.pos.col <= c && c <= x.pos.col + x.pos.len);
    let prefix: String = touching.map_or(String::new(), |p| id_name(&t[p]).unwrap_or("").chars().take(c - t[p].pos.col).collect());
    let before = match touching {
        Some(p) => p.checked_sub(1),
        None => t.iter().rposition(|x| (x.pos.line, x.pos.col) < (l, c) && !matches!(x.t, Tok::Eof)),
    };
    let mut items: Vec<Json> = vec![];
    let mut seen: HashSet<String> = HashSet::new();
    let mut push = |items: &mut Vec<Json>, it: Json| {
        let key = format!("{}", it.get("label").and_then(|x| x.str()).unwrap_or(""));
        if key.starts_with(&prefix) && seen.insert(key) {
            items.push(it);
        }
    };
    let prev = before.map(|b| &t[b]);
    // `import ` / `import std.` : module names
    if let Some(b) = before {
        let mut k = b;
        let mut path = String::new();
        loop {
            if is_id(&t[k], "import") {
                let dir = std::path::Path::new(&uri_to_path(&ctx.docs[0].uri)).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                let pre = if path.is_empty() { String::new() } else { format!("{}.", path.trim_end_matches('.')) };
                for m in module_names(&dir, &pre) {
                    push(&mut items, item(&m, 9, "module"));
                }
                return obj(vec![("isIncomplete", Json::Bool(false)), ("items", Json::Arr(items))]);
            }
            let ok = (matches!(t[k].t, Tok::Id(_)) && !is_id(&t[k], "as")) || is_sym(&t[k], ".");
            if !ok || k == 0 || t[k].pos.line != l && k != b {
                break;
            }
            match &t[k].t {
                Tok::Id(x) => path = format!("{}{}", x, if path.is_empty() { String::new() } else { format!(".{}", path) }),
                _ => {}
            }
            k -= 1;
        }
    }
    // after a dot: members of the receiver's type / items of a module / variants of a type
    if let Some(b) = before.filter(|b| is_sym(&t[*b], ".")) {
        let cur = b.checked_sub(1).and_then(|e| chain_start(doc, e).and_then(|st| chain_type(ctx, 0, st, e)));
        match cur {
            Some(Cur::Ty(ty)) => {
                for m in members_of(ctx, &ty) {
                    if !m.name.contains("__") {
                        push(&mut items, member_item(&m));
                    }
                }
            }
            Some(Cur::Module(m)) => {
                let d = ctx.doc(m);
                for sym in &d.syms {
                    let public = is_id(&d.toks[sym.start], "pub") || ctx.docs[m].lib && false;
                    if public {
                        if let Some(it) = symbol_item(d, sym) {
                            push(&mut items, it);
                        }
                    }
                }
            }
            Some(Cur::TypeRef(ty)) => {
                for e in &ctx.docs {
                    for sym in &e.doc.syms {
                        if sym.kind == SymKind::Enum && sym.name == split_ty(&ty).0 {
                            for v in &sym.children {
                                push(&mut items, item(&v.name, 20, &format!("{}.{}", ty, v.name)));
                            }
                        }
                    }
                }
                for m in members_of(ctx, &ty) {
                    if !m.has_self && !m.is_field && !m.name.contains("__") {
                        push(&mut items, member_item(&m));
                    }
                }
            }
            None => {}
        }
        return obj(vec![("isIncomplete", Json::Bool(false)), ("items", Json::Arr(items))]);
    }
    let _ = prev;
    // plain identifier: locals, items, modules, prelude, keywords, snippets
    let at = touching.or(before.map(|b| b + 1)).unwrap_or(0).min(t.len().saturating_sub(1));
    for j in visible_locals(doc, at) {
        if let Some(name) = id_name(&t[j]) {
            let d = describe(ctx, &Loc { doc: 0, tok: j, exact: true }).unwrap_or_default();
            push(&mut items, item(name, if decl_at(doc, j) == Some(Decl::Param) { 6 } else { 6 }, &d));
        }
    }
    for sym in &doc.syms {
        if let Some(it) = symbol_item(doc, sym) {
            push(&mut items, it);
        }
        for ch in &sym.children {
            if ch.kind == SymKind::Variant {
                push(&mut items, item(&ch.name, 20, &format!("{}.{}", sym.name, ch.name)));
            }
        }
    }
    for (alias, _) in imports(doc) {
        push(&mut items, item(&alias, 9, "module"));
    }
    let pre = ctx.doc(ctx.prelude());
    for sym in &pre.syms {
        if let Some(it) = symbol_item(pre, sym) {
            push(&mut items, it);
        }
        if sym.kind == SymKind::Enum {
            for ch in &sym.children {
                push(&mut items, item(&ch.name, 20, &format!("{}.{}", sym.name, ch.name)));
            }
        }
    }
    for ty in ["Int", "Float", "Bool", "Char", "String", "Text", "List", "Map", "Option", "Result", "UInt8", "Int64", "UInt64", "Float32"] {
        push(&mut items, item(ty, 7, "type"));
    }
    for (label, detail, body) in SNIPPETS {
        let mut it = item(label, 15, detail);
        if let Json::Obj(kv) = &mut it {
            kv.push(("insertText".into(), s(*body)));
            kv.push(("insertTextFormat".into(), n(2)));
            kv.push(("sortText".into(), s(format!("~{}", label))));
        }
        push(&mut items, it);
    }
    for k in KEYWORDS {
        push(&mut items, item(k, 14, "keyword"));
    }
    obj(vec![("isIncomplete", Json::Bool(false)), ("items", Json::Arr(items))])
}

// ───────────────────────── signature help ─────────────────────────

pub fn signature_help(ctx: &Ctx, line: usize, ch: usize) -> Json {
    let doc = ctx.doc(0);
    let t = &doc.toks;
    let (l, c) = doc.from_lsp(line, ch);
    let Some(last) = t.iter().rposition(|x| (x.pos.line, x.pos.col) < (l, c) && !matches!(x.t, Tok::Eof)) else {
        return Json::Null;
    };
    // the innermost unclosed `(` before the cursor, counting the commas at its level
    let (mut d, mut active) = (0i32, 0usize);
    let mut open = None;
    let mut k = last + 1;
    while k > 0 {
        k -= 1;
        let u = &t[k];
        if is_sym(u, ")") || is_sym(u, "]") || is_sym(u, "}") {
            d += 1;
        } else if is_sym(u, "(") || is_sym(u, "[") || is_sym(u, "{") {
            if d == 0 {
                if is_sym(u, "(") {
                    open = Some(k);
                }
                break;
            }
            d -= 1;
        } else if d == 0 && is_sym(u, ",") {
            active += 1;
        }
    }
    let Some(open) = open else { return Json::Null };
    let mut nm = open.checked_sub(1);
    if let Some(p) = nm.filter(|p| is_sym(&t[*p], ">")) {
        let mut q = p;
        while q > 0 && !is_sym(&t[q], "<") {
            q -= 1;
        }
        nm = q.checked_sub(1);
    }
    let Some(nm) = nm.filter(|p| id_name(&t[*p]).is_some()) else {
        return Json::Null;
    };
    let method = nm >= 1 && is_sym(&t[nm - 1], ".");
    let Some(loc) = resolve(ctx, 0, nm) else {
        return Json::Null;
    };
    let target = ctx.doc(loc.doc);
    let mut flat = vec![];
    all_syms(&target.syms, &mut flat);
    let Some(sym) = flat.into_iter().find(|s| s.name_tok == loc.tok && matches!(s.kind, SymKind::Fn | SymKind::Method)) else {
        return Json::Null;
    };
    let parts = fn_parts(target, sym.name_tok, sym.start, sym.end);
    let _ = method;
    let mut active = active;
    if active >= parts.params.len() && !parts.params.is_empty() {
        active = parts.params.len() - 1;
    }
    let params: Vec<Json> = parts.params.iter().map(|p| obj(vec![("label", s(p.clone()))])).collect();
    obj(vec![
        (
            "signatures",
            Json::Arr(vec![obj(vec![("label", s(parts.sig)), ("parameters", Json::Arr(params))])]),
        ),
        ("activeSignature", n(0)),
        ("activeParameter", n(active)),
    ])
}

// ───────────────────────── references and rename ─────────────────────────

/// Every use of the declaration `target` (document uri + name token) in the open documents.
pub fn references(server: &Server, uri: &str, target: &Loc, ctx: &Ctx) -> Vec<(String, usize, bool)> {
    let tdoc = ctx.doc(target.doc);
    let name = id_name(&tdoc.toks[target.tok]).unwrap_or("").to_string();
    let turi = ctx.docs[target.doc].uri.clone();
    let mut out = vec![];
    let mut uris: Vec<&String> = server.docs.keys().collect();
    uris.sort();
    if !server.docs.contains_key(uri) {
        return out;
    }
    for u in uris {
        let Some(c2) = server.ctx(u) else { continue };
        let d = c2.doc(0);
        for (j, tk) in d.toks.iter().enumerate() {
            if id_name(tk) != Some(&name) {
                continue;
            }
            if let Some(l) = resolve(&c2, 0, j) {
                if c2.docs[l.doc].uri == turi && l.tok == target.tok {
                    out.push((u.clone(), j, l.exact));
                }
            }
        }
    }
    out
}

fn valid_ident(x: &str) -> bool {
    let mut cs = x.chars();
    cs.next().is_some_and(|c| c.is_alphabetic() || c == '_') && cs.all(|c| c.is_alphanumeric() || c == '_') && !syntax::KW.contains(&x) && !KEYWORDS.contains(&x)
}

pub enum RenameError {
    Msg(String),
}

pub fn prepare_rename(ctx: &Ctx, ti: usize) -> Result<(Loc, Json), RenameError> {
    let doc = ctx.doc(0);
    let name = id_name(&doc.toks[ti]).ok_or(RenameError::Msg("not an identifier".into()))?;
    if KEYWORDS.contains(&name) || syntax::KW.contains(&name) {
        return Err(RenameError::Msg("cannot rename a keyword".into()));
    }
    let loc = resolve(ctx, 0, ti).ok_or(RenameError::Msg(format!("cannot find the declaration of '{}'", name)))?;
    if ctx.docs[loc.doc].lib {
        return Err(RenameError::Msg(format!("'{}' belongs to the standard library or the prelude and cannot be renamed", name)));
    }
    if !loc.exact {
        return Err(RenameError::Msg(format!("the type of the receiver is unknown, so '{}' cannot be renamed safely", name)));
    }
    // a member name defined by several types is ambiguous without types for every use
    let d = ctx.doc(loc.doc);
    let mut flat = vec![];
    all_syms(&d.syms, &mut flat);
    let me = flat.iter().find(|s| s.name_tok == loc.tok);
    if let Some(sym) = me {
        if matches!(sym.kind, SymKind::Method | SymKind::Field) && flat.iter().filter(|s| s.name == sym.name && matches!(s.kind, SymKind::Method | SymKind::Field)).count() > 1 {
            return Err(RenameError::Msg(format!("'{}' is a member of several types: rename it by hand", name)));
        }
        if sym.kind == SymKind::Impl {
            return Err(RenameError::Msg("cannot rename an impl".into()));
        }
    }
    Ok((loc, doc.tok_range(ti)))
}

pub fn rename(server: &Server, uri: &str, ti: usize, new_name: &str) -> Result<Json, RenameError> {
    let ctx = server.ctx(uri).ok_or(RenameError::Msg("unknown document".into()))?;
    if !valid_ident(new_name) {
        return Err(RenameError::Msg(format!("'{}' is not a valid name", new_name)));
    }
    let (loc, _) = prepare_rename(&ctx, ti)?;
    let old = id_name(&ctx.doc(loc.doc).toks[loc.tok]).unwrap_or("").to_string();
    if old == new_name {
        return Ok(obj(vec![("changes", Json::Obj(vec![]))]));
    }
    let mut refs = references(server, uri, &loc, &ctx);
    let turi = ctx.docs[loc.doc].uri.clone();
    if server.docs.contains_key(&turi) && !refs.iter().any(|(u, j, _)| *u == turi && *j == loc.tok) {
        refs.push((turi.clone(), loc.tok, true));
    }
    if refs.iter().any(|(_, _, exact)| !exact) {
        return Err(RenameError::Msg("some uses could not be resolved exactly".into()));
    }
    if !server.docs.contains_key(&turi) {
        return Err(RenameError::Msg("the declaration is in a file that is not open: open it to rename".into()));
    }
    // conflicts: another declaration with the new name visible where the old one is used or declared
    for (u, j, _) in &refs {
        let c2 = server.ctx(u).ok_or(RenameError::Msg("unknown document".into()))?;
        let d = c2.doc(0);
        let clash_local = visible_locals(d, *j).into_iter().any(|k| id_name(&d.toks[k]) == Some(new_name) && k != *j);
        let clash_item = d.syms.iter().any(|x| x.name == new_name && x.kind != SymKind::Impl);
        let own_decl_is_item = ctx.docs[loc.doc].uri == *u && !matches!(decl_at(ctx.doc(loc.doc), loc.tok), Some(_));
        let item_clash = clash_item && (own_decl_is_item || decl_at(ctx.doc(loc.doc), loc.tok).is_none());
        if clash_local || item_clash && ctx.docs[loc.doc].uri == *u {
            return Err(RenameError::Msg(format!("'{}' already exists in that scope", new_name)));
        }
        if !clash_local && clash_item && decl_at(ctx.doc(loc.doc), loc.tok).is_some() {
            // a local renamed to the name of an item would hide the item from its own scope
            return Err(RenameError::Msg(format!("'{}' would hide the item with that name", new_name)));
        }
    }
    let mut by_uri: Vec<(String, Vec<Json>)> = vec![];
    for (u, j, _) in &refs {
        let c2 = server.ctx(u).unwrap();
        let edit = obj(vec![("range", c2.doc(0).tok_range(*j)), ("newText", s(new_name))]);
        match by_uri.iter_mut().find(|(x, _)| x == u) {
            Some((_, v)) => v.push(edit),
            None => by_uri.push((u.clone(), vec![edit])),
        }
    }
    Ok(obj(vec![("changes", Json::Obj(by_uri.into_iter().map(|(u, v)| (u, Json::Arr(v))).collect()))]))
}

// ───────────────────────── semantic tokens ─────────────────────────

pub const TOKEN_TYPES: &[&str] = &["namespace", "type", "struct", "enum", "enumMember", "function", "method", "parameter", "variable", "property", "keyword", "interface"];
pub const TOKEN_MODIFIERS: &[&str] = &["declaration", "defaultLibrary"];

pub fn semantic_tokens(ctx: &Ctx) -> Json {
    let doc = ctx.doc(0);
    let t = &doc.toks;
    let mut rows: Vec<(usize, usize, usize, usize, usize)> = vec![]; // line, col(utf16), len, type, modifiers
    let imports_here: Vec<String> = imports(doc).into_iter().map(|x| x.0).collect();
    for (i, tk) in t.iter().enumerate() {
        let Some(name) = id_name(tk) else { continue };
        let after_dot = i > 0 && is_sym(&t[i - 1], ".");
        let next_call = i + 1 < t.len() && is_sym(&t[i + 1], "(");
        let on_import_line = (0..i).rev().take_while(|k| t[*k].pos.line == tk.pos.line).any(|k| is_id(&t[k], "import"));
        let (ty, mods) = if on_import_line && !is_id(tk, "import") && name != "as" {
            (0, 0)
        } else if syntax::KW.contains(&name) && name != "self" || KEYWORDS.contains(&name) || ["import", "as", "pub", "const", "type", "test", "derive", "extern", "link"].contains(&name) {
            (10, 0)
        } else if after_dot {
            // a name after a dot: a module item, or a member (kind judged by the call parentheses)
            let module_item = i >= 2 && id_name(&t[i - 2]).is_some_and(|a| imports_here.iter().any(|x| x == a));
            if module_item {
                match resolve(ctx, 0, i) {
                    Some(l) => (kind_of(ctx, &l), lib_bit(ctx, &l)),
                    None => (if next_call { 5 } else { 1 }, 0),
                }
            } else {
                (if next_call { 6 } else { 9 }, 0)
            }
        } else if imports_here.iter().any(|a| a == name) && i + 1 < t.len() && is_sym(&t[i + 1], ".") && decl_at(doc, i).is_none() {
            (0, 0)
        } else {
            match resolve(ctx, 0, i) {
                Some(l) => {
                    let decl_here = l.doc == 0 && l.tok == i;
                    (kind_of(ctx, &l), lib_bit(ctx, &l) | usize::from(decl_here))
                }
                None if name.chars().next().is_some_and(|c| c.is_uppercase()) => (1, 2),
                None => continue,
            }
        };
        let (l0, c0) = doc.to_lsp(tk.pos.line, tk.pos.col);
        let (_, c1) = doc.to_lsp(tk.pos.line, tk.pos.col + tk.pos.len);
        rows.push((l0, c0, c1 - c0, ty, mods));
    }
    rows.sort();
    let mut data = vec![];
    let (mut pl, mut pc) = (0, 0);
    for (l, c, len, ty, mods) in rows {
        let dl = l - pl;
        let dc = if dl == 0 { c - pc } else { c };
        data.extend([dl, dc, len, ty, mods].map(n));
        pl = l;
        pc = c;
    }
    obj(vec![("data", Json::Arr(data))])
}

fn lib_bit(ctx: &Ctx, l: &Loc) -> usize {
    if ctx.docs[l.doc].lib {
        2
    } else {
        0
    }
}

fn kind_of(ctx: &Ctx, l: &Loc) -> usize {
    let d = ctx.doc(l.doc);
    match decl_at(d, l.tok) {
        Some(Decl::Param) => return 7,
        Some(_) => return 8,
        None => {}
    }
    let mut flat = vec![];
    all_syms(&d.syms, &mut flat);
    match flat.iter().find(|s| s.name_tok == l.tok || (s.kind == SymKind::Impl && s.start == l.tok)).map(|s| s.kind) {
        Some(SymKind::Fn) => 5,
        Some(SymKind::Method) => 6,
        Some(SymKind::Struct) => 2,
        Some(SymKind::Alias) | Some(SymKind::Impl) => 1,
        Some(SymKind::Enum) => 3,
        Some(SymKind::Trait) => 11,
        Some(SymKind::Const) => 8,
        Some(SymKind::Field) => 9,
        Some(SymKind::Variant) => 4,
        None => 8,
    }
}
