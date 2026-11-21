// Yerux modules: multi-file programs merged into ONE token stream before parsing.
//
//   import math              ->  <root>/math.yx          used as  math.add(1, 2)   math.Point { .. }
//   import utils.strings     ->  <root>/utils/strings.yx  used as  strings.up(s)
//   import utils.strings as s                              used as  s.up(x)
//
// `<root>` is the folder of the file being compiled. Every top-level item of an imported module is
// renamed to `<path>__<name>` (so two modules may both define `helper`), `pub` is stripped after it has
// been checked, and qualified references `alias.name` become that mangled name. The merged stream is
// parsed and checked as a single program, so the checker and both backends need no module support.
//
// Diagnostics keep their file: the lexer positions of module N are stored as `(N << 20) | line`, and
// `Loaded::render` decodes them (and turns mangled names back into `alias.name`).
//
// `test "name" { .. }` blocks of the entry file are also handled here (they are dropped for normal
// builds; `yerux test` builds one program per test whose `main` is the test body).
use crate::syntax::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const FILE_SHIFT: usize = 20;
const LINE_MASK: usize = (1 << FILE_SHIFT) - 1;

pub struct SourceFile {
    pub path: String,
    pub src: String,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Normal,
    /// Build the program whose `main` is the n-th `test` block of the entry file.
    Test(usize),
}

pub struct Loaded {
    pub toks: Vec<Token>,
    pub files: Vec<SourceFile>,
    /// (mangled name, name as the user writes it), longest mangled name first
    names: Vec<(String, String)>,
    /// per file: (alias, module path) of its imports, and per module path its public items
    imports: Vec<Vec<(String, String)>>,
    exports: HashMap<String, Vec<(String, Kind)>>,
    /// names of the `test` blocks of the entry file, in source order
    pub tests: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Func,
    Type, // struct, enum or trait
}

struct Item {
    kind: Kind,
    public: bool,
}

/// One top-level item: tokens [start, end) of the file (including a leading `pub`).
struct Span {
    start: usize,
    end: usize,
    public: bool,
    /// first token after `pub`
    head: usize,
    name: Option<String>,
    kind: Kind,
    test: Option<String>,
}

struct Module {
    toks: Vec<Token>,
}

struct Loader {
    root: PathBuf,
    mode: Mode,
    files: Vec<SourceFile>,
    mods: Vec<Module>, // post-order: dependencies first, the entry last
    by_key: HashMap<String, Option<usize>>, // None while loading (cycle detection)
    stack: Vec<String>,
    errs: Vec<String>,
    names: Vec<(String, String)>,
    imports: Vec<Vec<(String, String)>>,
    exports: HashMap<String, Vec<(String, Kind)>>,
    tests: Vec<String>,
    items_by_key: HashMap<String, HashMap<String, Item>>,
    trace: Option<ResolutionTrace>,
}

#[derive(Default)]
struct ResolutionTrace {
    owners: Vec<String>,
    rows: Vec<String>,
}

impl Kind {
    fn trace_name(self) -> &'static str {
        match self {
            Kind::Func => "function",
            Kind::Type => "type",
        }
    }
}

fn is_id(t: &Token, s: &str) -> bool {
    matches!(&t.t, Tok::Id(x) if x == s)
}
fn is_sym(t: &Token, s: &str) -> bool {
    matches!(&t.t, Tok::Sym(x) if *x == s)
}
fn id_of(t: &Token) -> Option<&str> {
    match &t.t {
        Tok::Id(x) if !KW.contains(&x.as_str()) => Some(x.as_str()),
        _ => None,
    }
}

fn mangle(key: &str, name: &str) -> String {
    if key.is_empty() {
        name.to_string()
    } else {
        format!("{}__{}", key.replace('.', "__"), name)
    }
}

/// Load `entry` and everything it imports.
pub fn load(entry: &str, mode: Mode) -> Result<Loaded, Vec<String>> {
    load_with(entry, mode, false).0
}

/// Observe the module loader's symbol tables and name lookups before type checking.
pub fn resolution_trace(entry: &str) -> (Vec<String>, bool) {
    let (loaded, trace) = load_with(entry, Mode::Normal, true);
    let mut rows = trace.unwrap().rows;
    rows.sort_unstable();
    (rows, loaded.is_ok())
}

fn load_with(
    entry: &str,
    mode: Mode,
    trace: bool,
) -> (Result<Loaded, Vec<String>>, Option<ResolutionTrace>) {
    let path = Path::new(entry);
    let root = path
        .parent()
        .map(|p| p.to_path_buf())
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("."));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut l = Loader {
        root,
        mode,
        files: vec![],
        mods: vec![],
        by_key: HashMap::new(),
        stack: vec![],
        errs: vec![],
        names: vec![],
        imports: vec![],
        exports: HashMap::new(),
        tests: vec![],
        items_by_key: HashMap::new(),
        trace: trace.then(ResolutionTrace::default),
    };
    // the entry module is known under its file name too, so `import main` from a dependency is a cycle
    let complete = l.load_module(&stem, entry, true);
    let trace = l.trace.take();
    if !complete || !l.errs.is_empty() {
        return (Err(l.errs), trace);
    }
    let mut toks = vec![];
    let mut last = Pos {
        line: 1,
        col: 1,
        len: 1,
    };
    for m in &l.mods {
        toks.extend(m.toks.iter().cloned());
    }
    if let Some(t) = toks.last() {
        last = t.pos;
    }
    toks.push(Token {
        t: Tok::Eof,
        pos: last,
    });
    let mut names = l.names;
    names.sort_by_key(|(m, _)| std::cmp::Reverse(m.len()));
    (
        Ok(Loaded {
            toks,
            files: l.files,
            names,
            imports: l.imports,
            exports: l.exports,
            tests: l.tests,
        }),
        trace,
    )
}

impl Loader {
    fn trace(&mut self, file: usize, tag: &str, fields: &[&str]) {
        if let Some(trace) = &mut self.trace {
            let mut row = format!("{tag}:{}", trace.owners[file]);
            for field in fields {
                row.push(':');
                row.push_str(field);
            }
            trace.rows.push(row);
        }
    }

    fn report(&mut self, file: usize, d: Diag) {
        let s = render(&d, &self.files[file].path, &self.files[file].src);
        self.errs.push(s);
    }

    /// Returns the module's key when it loaded.
    fn load_module(&mut self, key: &str, path: &str, entry: bool) -> bool {
        // modules of the standard library (`import std.io`) are embedded in the compiler
        let src = match crate::stdlib::source(key).filter(|_| !entry) {
            Some(s) => s.to_string(),
            None => match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    self.errs
                        .push(format!("error: could not read {}: {}", path, e));
                    return false;
                }
            },
        };
        let file = self.files.len();
        self.files.push(SourceFile {
            path: path.to_string(),
            src: src.clone(),
        });
        self.imports.push(vec![]);
        if let Some(trace) = &mut self.trace {
            trace.owners.push(if entry { "@".into() } else { key.into() });
        }
        self.trace(file, "module", &[]);
        let toks = match lex(&src) {
            Ok(t) => t,
            Err(d) => {
                self.trace(file, "error", &["lex"]);
                self.report(file, d);
                return false;
            }
        };
        self.by_key.insert(key.to_string(), None);
        self.stack.push(key.to_string());
        let ok = self.module_body(key, file, toks, entry);
        self.stack.pop();
        self.by_key
            .insert(key.to_string(), if ok { Some(file) } else { None });
        ok
    }

    fn module_body(&mut self, key: &str, file: usize, toks: Vec<Token>, entry: bool) -> bool {
        let ekey = if entry {
            String::new()
        } else {
            key.to_string()
        };
        // ── imports ──
        let mut i = 0;
        let mut aliases: HashMap<String, String> = HashMap::new(); // alias -> module key
        while is_id(&toks[i], "import") {
            let ipos = toks[i].pos;
            i += 1;
            let mut segs: Vec<String> = vec![];
            loop {
                match id_of(&toks[i]) {
                    Some(s) => {
                        segs.push(s.to_string());
                        i += 1;
                    }
                    None => {
                        self.report(
                            file,
                            diag("expected a module name after `import`", toks[i].pos)
                                .note("example: `import math` or `import utils.strings`"),
                        );
                        return false;
                    }
                }
                if is_sym(&toks[i], ".") {
                    i += 1;
                } else {
                    break;
                }
            }
            let mut alias = segs.last().unwrap().clone();
            if is_id(&toks[i], "as") {
                i += 1;
                match id_of(&toks[i]) {
                    Some(a) => {
                        alias = a.to_string();
                        i += 1;
                    }
                    None => {
                        self.report(file, diag("expected a name after `as`", toks[i].pos));
                        return false;
                    }
                }
            }
            let mpos = span(ipos, toks[i - 1].pos);
            let dep = segs.join(".");
            if aliases.contains_key(&alias) {
                self.trace(file, "error", &["duplicate-alias", &alias, &dep]);
                self.report(
                    file,
                    diag(format!("the name '{}' is imported twice", alias), mpos).note(format!(
                        "help: rename one of them: `import {} as other_name`",
                        dep
                    )),
                );
                continue;
            }
            if self.stack.contains(&dep) {
                self.trace(file, "error", &["cycle", &dep]);
                let mut chain: Vec<String> = self.stack.clone();
                chain.push(dep.clone());
                let from = chain.iter().position(|k| *k == dep).unwrap_or(0);
                let shown: Vec<String> = chain[from..].iter().map(|k| k.to_string()).collect();
                self.report(
                    file,
                    diag(format!("import cycle: {}", shown.join(" -> ")), mpos)
                        .note("modules cannot import each other, directly or through others")
                        .note("help: move what they share into a third module that both of them import"),
                );
                continue;
            }
            if !self.by_key.contains_key(&dep) {
                let rel = format!("{}.yx", segs.join("/"));
                let p = if crate::stdlib::source(&dep).is_some() {
                    PathBuf::from(format!("<std>/{}", rel.trim_start_matches("std/")))
                } else {
                    self.root.join(&rel)
                };
                if !crate::stdlib::source(&dep).is_some() && !p.is_file() {
                    self.trace(file, "error", &["missing-module", &dep]);
                    self.report(
                        file,
                        diag(format!("cannot find module '{}'", dep), mpos)
                            .note(format!("looked for {}", p.display()))
                            .note(format!(
                                "help: `import {}` loads {} from the folder of the file you compile ({}); create it, or fix the name",
                                dep,
                                rel,
                                self.root.display()
                            )),
                    );
                    continue;
                }
                let ps = p.to_string_lossy().to_string();
                if !self.load_module(&dep, &ps, false) {
                    continue;
                }
            } else if self.by_key[&dep].is_none() {
                continue; // failed earlier; already reported
            }
            self.trace(file, "import", &[&alias, &dep]);
            aliases.insert(alias.clone(), dep.clone());
            self.imports[file].push((alias, dep));
        }

        // ── top-level items ──
        let spans = scan_items(&toks, i);
        let mut items: HashMap<String, Item> = HashMap::new();
        let mut export_list: Vec<(String, Kind)> = vec![];
        for sp in &spans {
            if let Some(n) = &sp.name {
                if items
                    .insert(
                        n.clone(),
                        Item {
                            kind: sp.kind,
                            public: sp.public,
                        },
                    )
                    .is_none()
                    && sp.public
                {
                    export_list.push((n.clone(), sp.kind));
                }
            }
            if sp.test.is_some() && sp.public {
                self.trace(file, "error", &["public-test"]);
                self.report(
                    file,
                    diag("`pub` cannot be used on a test", toks[sp.start].pos),
                );
            }
        }
        if self.trace.is_some() {
            for (n, item) in &items {
                let visibility = if item.public { "public" } else { "private" };
                self.trace(
                    file,
                    "symbol",
                    &[n, item.kind.trace_name(), visibility, &mangle(&ekey, n)],
                );
            }
        }
        for t in &toks[i..] {
            if is_id(t, "import") {
                self.trace(file, "error", &["late-import"]);
                self.report(
                    file,
                    diag("`import` must come before every other declaration", t.pos)
                        .note("help: move all `import` lines to the top of the file"),
                );
                break;
            }
        }
        if !entry {
            for n in items.keys() {
                self.names.push((
                    mangle(&ekey, n),
                    format!("{}.{}", key.rsplit('.').next().unwrap_or(key), n),
                ));
            }
        }
        self.exports.insert(key.to_string(), export_list);

        // ── rewrite ──
        // public items of the imported modules, to explain a missing qualifier
        let mut imported: HashMap<String, (String, Kind)> = HashMap::new();
        for (alias, dep) in &aliases {
            if let Some(m) = self.items_by_key.get(dep) {
                for (n, it) in m {
                    if it.public && !items.contains_key(n) {
                        imported.insert(n.clone(), (alias.clone(), it.kind));
                    }
                }
            }
        }
        let mut out: Vec<Token> = vec![];
        let mut test_no = 0usize;
        let mut cursor = i;
        for sp in &spans {
            // tokens between items that scan_items could not classify are passed through (the parser reports them)
            for t in &toks[cursor..sp.start] {
                out.push(t.clone());
            }
            cursor = sp.end;
            if let Some(tname) = &sp.test {
                if entry {
                    self.tests.push(tname.clone());
                    if self.mode == Mode::Test(test_no) {
                        // `test "name" { body }`  ->  `fn main() { body }`
                        let tp = toks[sp.start].pos;
                        let body = &toks[sp.start + 2..sp.end];
                        let mk = |t: Tok| Token { t, pos: tp };
                        let mut v = vec![
                            mk(Tok::Id("fn".into())),
                            mk(Tok::Id("main".into())),
                            mk(Tok::Sym("(")),
                            mk(Tok::Sym(")")),
                        ];
                        v.extend(body.iter().cloned());
                        let r = self.rewrite(file, &ekey, &items, &aliases, &imported, &v);
                        out.extend(r);
                    }
                    test_no += 1;
                }
                continue;
            }
            if entry
                && matches!(self.mode, Mode::Test(_))
                && sp.name.as_deref() == Some("main")
                && sp.kind == Kind::Func
            {
                continue; // the program's own main is replaced by the test body
            }
            let body: Vec<Token> = toks[sp.head..sp.end].to_vec();
            let r = self.rewrite(file, &ekey, &items, &aliases, &imported, &body);
            out.extend(r);
        }
        for t in &toks[cursor..] {
            if !matches!(t.t, Tok::Eof) {
                out.push(t.clone());
            }
        }
        for t in out.iter_mut() {
            t.pos.line |= file << FILE_SHIFT;
        }
        self.items_by_key.insert(key.to_string(), items);
        self.mods.push(Module { toks: out });
        true
    }

    fn rewrite(
        &mut self,
        file: usize,
        ekey: &str,
        own: &HashMap<String, Item>,
        aliases: &HashMap<String, String>,
        imported: &HashMap<String, (String, Kind)>,
        toks: &[Token],
    ) -> Vec<Token> {
        let mut shadowed: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut out: Vec<Token> = Vec::with_capacity(toks.len());
        let mut j = 0;
        while j < toks.len() {
            let t = &toks[j];
            let after_dot = j > 0 && is_sym(&toks[j - 1], ".");
            let x = match &t.t {
                Tok::Id(x) if !after_dot => x.clone(),
                _ => {
                    out.push(t.clone());
                    j += 1;
                    continue;
                }
            };
            let next = toks.get(j + 1);
            if let Some(dep) = aliases.get(&x) {
                if matches!(next, Some(n) if is_sym(n, "."))
                    && toks.get(j + 2).is_some_and(|n| matches!(n.t, Tok::Id(_)))
                {
                    let y = match &toks[j + 2].t {
                        Tok::Id(y) => y.clone(),
                        _ => unreachable!(),
                    };
                    let p = span(t.pos, toks[j + 2].pos);
                    let dep = dep.clone();
                    match self.items_by_key.get(&dep).and_then(|m| m.get(&y)) {
                        None => {
                            self.trace(file, "error", &["unknown-item", &x, &dep, &y]);
                            let pubs: Vec<String> = self
                                .exports
                                .get(&dep)
                                .map(|v| v.iter().map(|(n, _)| n.clone()).collect())
                                .unwrap_or_default();
                            let mut d =
                                diag(format!("module '{}' has no item called '{}'", dep, y), p);
                            if crate::syntax::KW.contains(&y.as_str()) {
                                d = d.note(format!(
                                    "'{}' is a reserved word, so no module can export an item with that name",
                                    y
                                ));
                            }
                            if pubs.is_empty() {
                                d = d.note(format!("module '{}' has no `pub` items", dep));
                            } else {
                                d = d.note(format!(
                                    "public items of '{}': {}",
                                    dep,
                                    pubs.join(", ")
                                ));
                            }
                            self.report(file, d);
                        }
                        Some(it) if !it.public => {
                            let what = if it.kind == Kind::Func {
                                "function"
                            } else {
                                "type"
                            };
                            let declaration = if it.kind == Kind::Func { "fn" } else { "struct" };
                            self.trace(file, "error", &["private-item", &x, &dep, &y]);
                            self.report(
                                file,
                                diag(format!("'{}' is private in module '{}'", y, dep), p).note(format!(
                                    "help: add `pub` before the {} in {}.yx to share it: `pub {} {}`",
                                    what,
                                    dep.replace('.', "/"),
                                    declaration,
                                    y
                                )),
                            );
                        }
                        Some(it) => {
                            if self.trace.is_some() {
                                let kind = it.kind.trace_name();
                                self.trace(
                                    file,
                                    "qualified",
                                    &[&x, &dep, &y, kind, &mangle(&dep, &y)],
                                );
                            }
                        }
                    }
                    out.push(Token {
                        t: Tok::Id(mangle(&dep, &y)),
                        pos: p,
                    });
                    j += 3;
                    continue;
                }
                if !shadowed.contains(&x)
                    && !matches!(next, Some(n) if is_sym(n, ":") || is_sym(n, "="))
                {
                    self.trace(file, "error", &["module-value", &x]);
                    self.report(
                        file,
                        diag(format!("'{}' is a module, not a value", x), t.pos)
                            .note(format!("help: use one of its items: `{}.name`", x)),
                    );
                }
                out.push(t.clone());
                j += 1;
                continue;
            }
            if let Some((alias, kind)) = imported.get(&x) {
                let prev = if j > 0 { Some(&toks[j - 1]) } else { None };
                // `fn read_line(` (a method definition) and `x.read_line(` (a method call) are not uses
                let method = matches!(prev, Some(p) if is_id(p, "fn") || is_sym(p, "."));
                let flag = match kind {
                    Kind::Func => !method && matches!(next, Some(n) if is_sym(n, "(")),
                    Kind::Type => {
                        matches!(next, Some(n) if is_sym(n, "{"))
                            || matches!(prev, Some(p) if is_sym(p, ":") || is_sym(p, "->") || is_sym(p, "<") || is_id(p, "dyn"))
                    }
                };
                if flag {
                    self.trace(file, "error", &["unqualified", &x, kind.trace_name()]);
                    self.report(
                        file,
                        diag(format!("'{}' is not defined in this file", x), t.pos).note(format!(
                            "help: it comes from module '{}': write `{}.{}`",
                            aliases[alias], alias, x
                        )),
                    );
                }
            }
            // after a `.` a name is a field, method or variant (`FileKind.File`, `f.read_to_string()`), never an item
            let after_dot = j > 0 && is_sym(&toks[j - 1], ".");
            // an enum variant that shares its name with a type of the module (`enum Kind { File }` next to
            // `struct File`): its declaration (`{ File,` at paren depth 0 inside an enum) and match arms
            // (`File =>`, `File | Dir =>`) keep the name
            let variant_here = {
                let prev = if j > 0 { Some(&toks[j - 1]) } else { None };
                let in_enum = toks
                    .iter()
                    .take(2)
                    .any(|f| is_id(f, "enum"));
                let mut depth = 0i32;
                for k in toks.iter().take(j) {
                    if is_sym(k, "(") {
                        depth += 1;
                    } else if is_sym(k, ")") {
                        depth -= 1;
                    }
                }
                let decl = in_enum
                    && depth == 0
                    && matches!(prev, Some(p) if is_sym(p, "{") || is_sym(p, ","));
                let arm = matches!(next, Some(n) if is_sym(n, "=>"))
                    || (matches!(next, Some(n) if is_sym(n, "|"))
                        && !matches!(prev, Some(p) if is_sym(p, ":") || is_sym(p, "<") || is_sym(p, ">") || is_sym(p, "->") || is_id(p, "view") || is_id(p, "mut")));
                decl || arm
            };
            if let Some(it) = own
                .get(&x)
                .filter(|it| !after_dot && !(it.kind == Kind::Type && variant_here))
            {
                let prev_fn = j > 0 && is_id(&toks[j - 1], "fn");
                // `fn name<T>(..)`: the definition of a generic function; methods inside `impl`/`trait`
                // blocks are looked up by type, so their names stay as written
                let methods_here = toks
                    .first()
                    .is_some_and(|f| is_id(f, "impl") || is_id(f, "trait"));
                if prev_fn && methods_here {
                    out.push(t.clone());
                    j += 1;
                    continue;
                }
                let call = matches!(next, Some(n) if is_sym(n, "("))
                    || (prev_fn && matches!(next, Some(n) if is_sym(n, "<")));
                if self.trace.is_some() && it.kind == Kind::Func && call && !prev_fn {
                    self.trace(file, "call", &[&x, &mangle(ekey, &x)]);
                }
                if !ekey.is_empty() && (it.kind == Kind::Type || call) {
                    out.push(Token {
                        t: Tok::Id(mangle(ekey, &x)),
                        pos: t.pos,
                    });
                    j += 1;
                    continue;
                }
            }
            if (x == "let" || x == "var")
                && matches!(next.map(|n| &n.t), Some(Tok::Id(v)) if aliases.contains_key(v))
            {
                let n = next.unwrap();
                if let Tok::Id(v) = &n.t {
                    shadowed.insert(v.clone());
                    self.trace(file, "error", &["alias-shadow", v]);
                }
                self.report(
                    file,
                    diag(format!("'{}' is already the name of an imported module", match &n.t { Tok::Id(v) => v.as_str(), _ => "" }), n.pos)
                        .note("help: pick another variable name, or import the module with `as`: `import path.name as other`"),
                );
            }
            out.push(t.clone());
            j += 1;
        }
        out
    }
}

/// Split the top level of a file (after its imports) into items.
fn scan_items(toks: &[Token], start: usize) -> Vec<Span> {
    let mut v = vec![];
    let mut i = start;
    while i < toks.len() && !matches!(toks[i].t, Tok::Eof) {
        let s = i;
        let mut public = false;
        if is_id(&toks[i], "pub") {
            public = true;
            i += 1;
        }
        let head = i;
        if toks.get(i).is_some_and(|t| is_sym(t, "@"))
            && toks.get(i + 1).is_some_and(|t| is_id(t, "thread_bound"))
        {
            i += 2;
        }
        let Some(item) = toks.get(i) else { break };
        let (kind, name, test) = match &item.t {
            Tok::Id(w) if w == "fn" || w == "struct" || w == "enum" || w == "trait" => {
                match toks.get(i + 1).and_then(id_of) {
                    Some(n) => (
                        if w == "fn" { Kind::Func } else { Kind::Type },
                        Some(n.to_string()),
                        None,
                    ),
                    None => break,
                }
            }
            Tok::Id(w)
                if w == "test" && matches!(toks.get(i + 1).map(|t| &t.t), Some(Tok::Str(_))) =>
            {
                let n = match &toks[i + 1].t {
                    Tok::Str(n) => n.clone(),
                    _ => unreachable!(),
                };
                (Kind::Func, None, Some(n))
            }
            Tok::Id(w) if w == "impl" => (Kind::Func, None, None),
            _ => break,
        };
        // find the body
        let mut j = i + 1;
        while j < toks.len() && !is_sym(&toks[j], "{") && !matches!(toks[j].t, Tok::Eof) {
            j += 1;
        }
        if j >= toks.len() || matches!(toks[j].t, Tok::Eof) {
            break;
        }
        let mut depth = 0i32;
        let mut end = j;
        while end < toks.len() {
            if is_sym(&toks[end], "{") {
                depth += 1;
            } else if is_sym(&toks[end], "}") {
                depth -= 1;
                if depth == 0 {
                    end += 1;
                    break;
                }
            } else if matches!(toks[end].t, Tok::Eof) {
                break;
            }
            end += 1;
        }
        v.push(Span {
            start: s,
            end,
            public,
            head,
            name,
            kind,
            test,
        });
        i = end;
    }
    v
}

impl Loaded {
    /// Render a diagnostic of the merged program: pick the file, decode the line, show names as written.
    pub fn render(&self, d: &Diag) -> String {
        if d.pos.line & PRELUDE_LINE != 0 {
            let d2 = Diag {
                msg: d.msg.clone(),
                pos: Pos {
                    line: d.pos.line & !PRELUDE_LINE,
                    col: d.pos.col,
                    len: d.pos.len,
                },
                notes: d.notes.clone(),
            };
            return render(&d2, "<built-in library>", PRELUDE);
        }
        let file = (d.pos.line >> FILE_SHIFT).min(self.files.len().saturating_sub(1));
        let line = d.pos.line & LINE_MASK;
        let fix = |s: &str| -> String {
            let mut s = s.to_string();
            for (m, shown) in &self.names {
                if s.contains(m.as_str()) {
                    s = s.replace(m.as_str(), shown);
                }
            }
            // "line 1048577" -> "line 1" (+ " of file" when it is another file)
            let mut out = String::new();
            let mut rest = s.as_str();
            while let Some(p) = rest.find("line ") {
                out.push_str(&rest[..p + 5]);
                rest = &rest[p + 5..];
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(n) = digits.parse::<usize>() {
                    if n > LINE_MASK {
                        out.push_str(&(n & LINE_MASK).to_string());
                        let f = (n >> FILE_SHIFT).min(self.files.len() - 1);
                        if f != file {
                            out.push_str(&format!(" of {}", self.files[f].path));
                        }
                        rest = &rest[digits.len()..];
                    }
                }
            }
            out.push_str(rest);
            out
        };
        let mut notes: Vec<String> = d.notes.iter().map(|n| fix(n)).collect();
        let msg = fix(&d.msg);
        // a name that exists in an imported module but was written without its qualifier
        for pre in ["function '", "unknown type '"] {
            if let Some(r) = d.msg.strip_prefix(pre) {
                if let Some(name) = r.split('\'').next() {
                    for (alias, dep) in &self.imports[file] {
                        let found = self
                            .exports
                            .get(dep)
                            .is_some_and(|v| v.iter().any(|(n, _)| n == name));
                        if found {
                            notes.push(format!(
                                "help: '{}' is defined in module '{}': write `{}.{}`",
                                name, dep, alias, name
                            ));
                        }
                    }
                }
            }
        }
        let d2 = Diag {
            msg,
            pos: Pos {
                line,
                col: d.pos.col,
                len: d.pos.len,
            },
            notes,
        };
        render(&d2, &self.files[file].path, &self.files[file].src)
    }
}
