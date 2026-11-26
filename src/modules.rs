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
use std::sync::OnceLock;

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
    /// Build the program whose `main` is the n-th `bench` block of the entry file.
    Bench(usize),
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
    /// names of the `bench` blocks of the entry file, in source order
    pub benches: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Func,
    Type, // struct, enum, trait or `type` alias
    Const, // `const NAME[: T] = expr`
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
    bench: bool,
}

struct Module {
    toks: Vec<Token>,
}

struct Loader<'a> {
    packages: Option<&'a crate::project::Graph>,
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
    benches: Vec<String>,
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
            Kind::Const => "constant",
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

/// `|` opens a closure parameter list when a closing `|` appears before `=>` (match arms use `|` for alternatives).
fn closure_pipe_opens(toks: &[Token], j: usize) -> bool {
    let mut paren = 0i32;
    let mut i = j + 1;
    while i < toks.len() {
        let t = &toks[i];
        if paren == 0 && is_sym(t, "|") {
            return true;
        }
        if paren == 0 && (is_sym(t, "=>") || is_sym(t, "{") || is_sym(t, ";") || is_sym(t, "}")) {
            return false;
        }
        if is_sym(t, "(") {
            paren += 1;
        } else if is_sym(t, ")") {
            paren -= 1;
        }
        i += 1;
    }
    false
}

/// True when `=>` follows this pattern name before the next statement/block boundary.
fn arm_arrow_ahead(toks: &[Token], j: usize) -> bool {
    let mut depth = 0i32;
    let mut i = j + 1;
    while i < toks.len() {
        let t = &toks[i];
        if depth == 0 && is_sym(t, "=>") {
            return true;
        }
        if depth == 0 && (is_sym(t, "{") || is_sym(t, ";") || is_sym(t, "}")) {
            return false;
        }
        if is_sym(t, "(") || is_sym(t, "[") {
            depth += 1;
        } else if (is_sym(t, ")") || is_sym(t, "]")) && depth > 0 {
            depth -= 1;
        }
        i += 1;
    }
    false
}

/// `let (a, b) = ..` / `var (a, b) = ..`: a name inside the unpack tuple.
fn unpack_bind_ahead(toks: &[Token], j: usize) -> bool {
    let mut depth = 0i32;
    let mut i = j + 1;
    while i < toks.len() {
        let t = &toks[i];
        if is_sym(t, "(") {
            depth += 1;
        } else if is_sym(t, ")") {
            if depth == 0 {
                return toks.get(i + 1).is_some_and(|n| is_sym(n, "="));
            }
            depth -= 1;
        } else if depth == 0 && (is_sym(t, "=>") || is_sym(t, "{") || is_sym(t, ";")) {
            return false;
        }
        i += 1;
    }
    false
}

fn mangle(key: &str, name: &str) -> String {
    if key.is_empty() {
        name.to_string()
    } else if key.starts_with('#') {
        format!("__pkg_{}__{name}", crate::hex(key.as_bytes()))
    } else {
        format!("{}__{}", key.replace('.', "__"), name)
    }
}

fn display_key(key: &str) -> &str {
    key.strip_prefix('#').unwrap_or(key)
}

fn module_name(key: &str) -> &str {
    if let Some((_, path)) = key.strip_prefix('#').and_then(|key| key.split_once(':')) {
        Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or(path)
    } else {
        key.rsplit('.').next().unwrap_or(key)
    }
}

static PACKAGES: OnceLock<crate::project::Graph> = OnceLock::new();

pub fn set_packages(graph: crate::project::Graph) {
    assert!(PACKAGES.set(graph).is_ok());
}

fn package_key(graph: &crate::project::Graph, owner: Option<&str>, path: &Path) -> String {
    if let Some(name) = owner {
        let root = graph.package_root(Some(name));
        let path = path.strip_prefix(root.join("src")).unwrap_or(path);
        format!("#{name}:{}", path.display())
    } else {
        let path = path.strip_prefix(&graph.root).unwrap_or(path);
        format!("#{}:{}", graph.manifest.name, path.display())
    }
}

static OVERLAY: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// Replace the contents of the editor's open buffers (path, text): the loader reads these instead of the
/// disk (used by `yerux lsp`; empty for every other command).
pub fn set_overlay(files: Vec<(String, String)>) {
    *OVERLAY.lock().unwrap() = files;
}

/// Add sources (path, text) to the overlay without dropping the ones already there (module objects).
pub fn add_overlay(files: Vec<(String, String)>) {
    OVERLAY.lock().unwrap().extend(files);
}

fn norm(p: &str) -> &str {
    p.trim_start_matches("./")
}

fn overlay_get(path: &str) -> Option<String> {
    OVERLAY.lock().unwrap().iter().find(|(p, _)| norm(p) == norm(path)).map(|(_, t)| t.clone())
}

fn read_source(path: &str) -> std::io::Result<String> {
    if let Some(text) = overlay_get(path) {
        return Ok(text);
    }
    std::fs::read_to_string(path)
}

fn source_exists(path: &Path) -> bool {
    path.is_file() || overlay_get(&path.to_string_lossy()).is_some()
}

/// Load `entry` and everything it imports.
pub fn load(entry: &str, mode: Mode) -> Result<Loaded, Vec<String>> {
    load_with(entry, mode, false).0
}

/// Observe the module loader's symbol tables and name lookups before type checking.
pub fn resolution_trace(entry: &str) -> (Vec<String>, bool) {
    let (loaded, trace) = load_with(entry, Mode::Normal, true);
    let mut rows = trace.unwrap_or_default().rows;
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
    let discovered = if PACKAGES.get().is_none() {
        match crate::project::for_entry(entry) {
            Ok(graph) => graph,
            Err(error) => return (Err(vec![format!("error: {error}")]), None),
        }
    } else {
        None
    };
    let packages = PACKAGES.get().or(discovered.as_ref());
    let key = packages.map_or(stem, |graph| {
        let absolute = path.canonicalize().or_else(|_| std::path::absolute(path)).unwrap_or_else(|_| path.to_owned());
        package_key(graph, None, &absolute)
    });
    let mut l = Loader {
        packages,
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
        benches: vec![],
        items_by_key: HashMap::new(),
        trace: trace.then(ResolutionTrace::default),
    };
    // the entry module is known under its file name too, so `import main` from a dependency is a cycle
    let complete = l.load_module(&key, entry, true, None);
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
            benches: l.benches,
        }),
        trace,
    )
}

impl Loader<'_> {
    fn import(
        &self,
        file: usize,
        owner: Option<&str>,
        segments: &[String],
        local: &str,
    ) -> (String, PathBuf, Option<String>) {
        let rel = format!("{}.yx", segments.join("/"));
        if crate::stdlib::source(local).is_some() {
            return (
                local.into(),
                PathBuf::from(format!("<std>/{}", rel.trim_start_matches("std/"))),
                None,
            );
        }
        let Some(graph) = self.packages.filter(|_| !self.files[file].path.starts_with("<std>/")) else {
            return (local.into(), self.root.join(&rel), None);
        };
        let (dest, path) = if let Some(root) = graph.dependency(owner, &segments[0]) {
            let src = root.join("src");
            let path = if segments.len() == 1 {
                let named = src.join(format!("{}.yx", segments[0]));
                if source_exists(&named) { named } else { src.join("lib.yx") }
            } else {
                src.join(format!("{}.yx", segments[1..].join("/")))
            };
            (Some(segments[0].as_str()), path)
        } else {
            let src = graph.package_root(owner).join("src");
            let local = self.root.join(&rel);
            let in_src = std::path::absolute(&self.files[file].path).is_ok_and(|path| path.starts_with(&src));
            if owner.is_some() || in_src || !source_exists(&local) {
                (owner, src.join(&rel))
            } else {
                (owner, std::path::absolute(&local).unwrap_or(local))
            }
        };
        (package_key(graph, dest, &path), path, dest.map(str::to_owned))
    }

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
    fn load_module(&mut self, key: &str, path: &str, entry: bool, owner: Option<&str>) -> bool {
        // modules of the standard library (`import std.io`) are embedded in the compiler
        let src = match crate::stdlib::source(key).filter(|_| !entry) {
            Some(s) => s.to_string(),
            None => match read_source(path) {
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
            trace
                .owners
                .push(if entry { "@".into() } else { display_key(key).into() });
        }
        self.trace(file, "module", &[]);
        let lex_start = std::time::Instant::now();
        let lexed = lex(&src);
        crate::profile_stage("lexer", lex_start);
        crate::profile_result("lexer", lexed.is_ok());
        let toks = match lexed {
            Ok(t) => crate::syntax::expand_derives(t),
            Err(d) => {
                self.trace(file, "error", &["lex"]);
                self.report(file, d);
                return false;
            }
        };
        self.by_key.insert(key.to_string(), None);
        self.stack.push(key.to_string());
        let resolve_start = std::time::Instant::now();
        let ok = self.module_body(key, file, toks, entry, owner);
        crate::profile_stage("module resolution", resolve_start);
        crate::profile_result("module resolution", ok);
        self.stack.pop();
        self.by_key
            .insert(key.to_string(), if ok { Some(file) } else { None });
        ok
    }

    fn module_body(&mut self, key: &str, file: usize, toks: Vec<Token>, entry: bool, owner: Option<&str>) -> bool {
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
            let import = segs.join(".");
            if aliases.contains_key(&alias) {
                self.trace(file, "error", &["duplicate-alias", &alias, &import]);
                self.report(
                    file,
                    diag(format!("the name '{}' is imported twice", alias), mpos).note(format!(
                        "help: rename one of them: `import {} as other_name`",
                        import
                    )),
                );
                continue;
            }
            let (dep, p, dependency_owner) = self.import(file, owner, &segs, &import);
            if self.stack.contains(&dep) {
                self.trace(file, "error", &["cycle", &dep]);
                let mut chain: Vec<String> = self.stack.clone();
                chain.push(dep.clone());
                let from = chain.iter().position(|k| *k == dep).unwrap_or(0);
                let shown: Vec<&str> = chain[from..].iter().map(|k| display_key(k)).collect();
                self.report(
                    file,
                    diag(format!("import cycle: {}", shown.join(" -> ")), mpos)
                        .note("modules cannot import each other, directly or through others")
                        .note("help: move what they share into a third module that both of them import"),
                );
                continue;
            }
            if !self.by_key.contains_key(&dep) {
                if crate::stdlib::source(&dep).is_none() && !source_exists(&p) {
                    self.trace(file, "error", &["missing-module", &import]);
                    let mut error = diag(format!("cannot find module '{}'", import), mpos)
                        .note(format!("looked for {}", p.display()));
                    if self.packages.is_some() {
                        error = error.note("help: imports can use local modules and directly declared dependencies");
                    } else {
                        let rel = format!("{}.yx", segs.join("/"));
                        error = error.note(format!(
                            "help: `import {}` loads {} from the folder of the file you compile ({}); create it, or fix the name",
                            import, rel, self.root.display()
                        ));
                    }
                    if segs.len() == 2 && segs[0] == "std" {
                        let guess = match segs[1].as_str() {
                            "random" | "rng" | "randomness" => Some("rand"),
                            "string" | "strings" | "str" => Some("text"),
                            "file" | "files" | "filesystem" => Some("fs"),
                            "os" | "sys" | "environment" => Some("env"),
                            "list" | "map" | "set" | "maps" | "lists" => Some("collections"),
                            "thread" | "threads" | "mutex" | "atomic" => Some("sync"),
                            "datetime" | "date" | "clock" => Some("time"),
                            "network" | "socket" | "tcp" => Some("net"),
                            "logging" => Some("log"),
                            "cli" | "argv" => Some("args"),
                            "maths" => Some("math"),
                            "sha" | "sha256" | "crc" | "md5" => Some("hash"),
                            _ => None,
                        };
                        if let Some(g) = guess {
                            error = error.note(format!("help: did you mean `import std.{}`?", g));
                        }
                    }
                    self.report(file, error);
                    continue;
                }
                let ps = p.to_string_lossy().to_string();
                if !self.load_module(&dep, &ps, false, dependency_owner.as_deref()) {
                    continue;
                }
            } else if self.by_key[&dep].is_none() {
                continue; // failed earlier; already reported
            }
            self.trace(file, "import", &[&alias, display_key(&dep)]);
            aliases.insert(alias.clone(), dep.clone());
            self.imports[file].push((alias, dep));
        }

        // ── top-level items ──
        let spans = scan_items(&toks, i);
        let mut items: HashMap<String, Item> = HashMap::new();
        let mut export_list: Vec<(String, Kind)> = vec![];
        for sp in &spans {
            if sp.kind == Kind::Const && sp.public && !toks.get(sp.head + 2).is_some_and(|t| is_sym(t, ":")) {
                self.trace(file, "error", &["pub-const-untyped"]);
                self.report(
                    file,
                    diag(
                        format!("a `pub const` needs an explicit type: `pub const {}: Int = ...`", sp.name.clone().unwrap_or_default()),
                        toks[sp.head + 1].pos,
                    )
                    .note("the type of a public constant is part of the module's interface, so it is written out"),
                );
            }
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
                    format!("{}.{}", module_name(key), n),
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
        let mut bench_no = 0usize;
        let mut cursor = i;
        for sp in &spans {
            // tokens between items that scan_items could not classify are passed through (the parser reports them)
            for t in &toks[cursor..sp.start] {
                out.push(t.clone());
            }
            cursor = sp.end;
            if let Some(tname) = &sp.test {
                if entry {
                    let own = if sp.bench { bench_no } else { test_no };
                    if sp.bench {
                        self.benches.push(tname.clone());
                        bench_no += 1;
                    } else {
                        self.tests.push(tname.clone());
                        test_no += 1;
                    }
                    let wanted = if sp.bench {
                        self.mode == Mode::Bench(own)
                    } else {
                        self.mode == Mode::Test(own)
                    };
                    if wanted {
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
                }
                continue;
            }
            if entry
                && matches!(self.mode, Mode::Test(_) | Mode::Bench(_))
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
        // Local names that hide an imported module alias: `let path = ..`, `for path in ..`, a parameter
        // `path: ..`, a match/unpack binding `Ok(path)` / `let (path, n) = ..`, a closure `|path: ..| { .. }`.
        // A shadow is active from the next line (let/var/unpack: its own initializer may still name the module)
        // or from the body block (parameters, loop variables, match/closure binds) until that block closes.
        struct Shadow {
            name: String,
            level: i32,
            line: usize,
            active: bool,
        }
        let mut shadows: Vec<Shadow> = vec![];
        let mut lvl: Vec<i32> = Vec::with_capacity(toks.len());
        {
            let mut d = 0i32;
            for t in toks {
                if is_sym(t, "}") {
                    d -= 1;
                }
                lvl.push(d);
                if is_sym(t, "{") {
                    d += 1;
                }
            }
        }
        // paren depth before each token, for the variant declarations of an enum (computed once: a scan per
        // identifier made an enum with thousands of variants quadratic)
        let mut paren_before: Vec<i32> = vec![];
        if toks.iter().take(2).any(|f| is_id(f, "enum")) {
            let mut depth = 0i32;
            for k in toks {
                paren_before.push(depth);
                if is_sym(k, "(") {
                    depth += 1;
                } else if is_sym(k, ")") {
                    depth -= 1;
                }
            }
        }
        // names bound as locals somewhere in this file (let/var/for/param/closure param/pattern): a function of
        // the module with such a name is only renamed when called, since `f` alone could be the local
        let mut bound_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (k, tk) in toks.iter().enumerate() {
            let Tok::Id(n) = &tk.t else { continue };
            if !own.get(n).is_some_and(|it| it.kind == Kind::Func) {
                continue;
            }
            let prev = if k > 0 { Some(&toks[k - 1]) } else { None };
            let nx = toks.get(k + 1);
            let dotted = matches!(prev, Some(p) if is_sym(p, "."));
            if dotted {
                continue;
            }
            let b = matches!(prev, Some(p) if is_id(p, "let") || is_id(p, "var") || is_id(p, "for") || is_id(p, "mut") || is_sym(p, "|"))
                || matches!(nx, Some(x) if is_sym(x, ":") || is_sym(x, "@") || is_sym(x, "=>") || is_sym(x, "|"))
                || (matches!(prev, Some(p) if is_sym(p, "(") || is_sym(p, ","))
                    && matches!(nx, Some(x) if is_sym(x, ")") || is_sym(x, ","))
                    && (arm_arrow_ahead(toks, k) || unpack_bind_ahead(toks, k)));
            if b && !matches!(prev, Some(p) if is_id(p, "fn")) {
                bound_names.insert(n.clone());
            }
        }
        let mut fn_header = false;
        let mut paren = 0i32;
        let mut closure_params = false;
        let mut out: Vec<Token> = Vec::with_capacity(toks.len());
        let mut j = 0;
        while j < toks.len() {
            let t = &toks[j];
            let l = lvl[j];
            shadows.retain(|s| {
                if s.active {
                    l >= s.level
                } else {
                    l >= s.level - 1
                }
            });
            for s in shadows.iter_mut() {
                if !s.active && l >= s.level && t.pos.line > s.line {
                    s.active = true;
                }
            }
            if is_id(t, "fn") {
                shadows.retain(|s| s.active);
                fn_header = true;
                paren = 0;
                closure_params = false;
            } else if fn_header && is_sym(t, "(") {
                paren += 1;
            } else if fn_header && is_sym(t, ")") {
                paren -= 1;
                if paren == 0 {
                    fn_header = false;
                }
            } else if is_sym(t, "|") {
                if closure_params {
                    closure_params = false;
                } else if closure_pipe_opens(toks, j) {
                    closure_params = true;
                }
            }
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
            // a declaration of a local that hides the module alias `x`
            if aliases.contains_key(&x) {
                let prev = if j > 0 { Some(&toks[j - 1]) } else { None };
                let prev2 = if j > 1 { Some(&toks[j - 2]) } else { None };
                let is_let = matches!(prev, Some(p) if is_id(p, "let") || is_id(p, "var"));
                let is_for = matches!(prev, Some(p) if is_id(p, "for"))
                    || (matches!(prev, Some(p) if is_id(p, "mut"))
                        && matches!(prev2, Some(p) if is_id(p, "for")));
                let is_param = fn_header
                    && paren == 1
                    && matches!(next, Some(n) if is_sym(n, ":"))
                    && matches!(prev, Some(p) if is_sym(p, "(") || is_sym(p, ",") || is_id(p, "view") || is_id(p, "mut"));
                let is_closure_param = closure_params
                    && matches!(prev, Some(p) if is_sym(p, "|") || is_sym(p, ","))
                    && matches!(next, Some(n) if is_sym(n, ":") || is_sym(n, ",") || is_sym(n, "|"));
                // match arm / or-pattern / `name @ pat`, and `let (name, ..) = ..`
                let is_pattern_bind = {
                    let name_at = matches!(next, Some(n) if is_sym(n, "@"));
                    let bare_arm = matches!(next, Some(n) if is_sym(n, "=>") || is_id(n, "if"));
                    let or_arm = matches!(next, Some(n) if is_sym(n, "|")) && arm_arrow_ahead(toks, j);
                    let in_payload = matches!(prev, Some(p) if is_sym(p, "(") || is_sym(p, ","))
                        && matches!(next, Some(n) if is_sym(n, ")") || is_sym(n, ",") || is_sym(n, "@"));
                    name_at && arm_arrow_ahead(toks, j)
                        || bare_arm
                        || or_arm
                        || (in_payload && (arm_arrow_ahead(toks, j) || unpack_bind_ahead(toks, j)))
                };
                if is_let || is_for || is_param || is_closure_param || is_pattern_bind {
                    let like_let = is_let || (is_pattern_bind && unpack_bind_ahead(toks, j) && !arm_arrow_ahead(toks, j));
                    shadows.push(Shadow {
                        name: x.clone(),
                        level: if like_let { l } else { l + 1 },
                        line: if like_let { t.pos.line } else { 0 },
                        active: false,
                    });
                    out.push(t.clone());
                    j += 1;
                    continue;
                }
            }
            let hidden = shadows.iter().any(|s| s.active && s.name == x);
            if hidden {
                out.push(t.clone());
                j += 1;
                continue;
            }
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
                            let (what, declaration) = match it.kind {
                                Kind::Func => ("function", "fn"),
                                Kind::Const => ("constant", "const"),
                                Kind::Type => ("type", "struct"),
                            };
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
                if !matches!(next, Some(n) if is_sym(n, ":") || is_sym(n, "=")) {
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
                    Kind::Const => false,
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
                let in_enum = toks.iter().take(2).any(|f| is_id(f, "enum"));
                // the paren depth is only needed inside an enum declaration (scanning it for every identifier
                // of a long function body made module loading quadratic)
                let depth = if in_enum { paren_before[j] } else { 0 };
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
                    .is_some_and(|f| is_id(f, "impl") || is_id(f, "trait") || is_id(f, "extern"));
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
                if !ekey.is_empty() && (it.kind != Kind::Func || call || (!prev_fn && !bound_names.contains(&x))) {
                    out.push(Token {
                        t: Tok::Id(mangle(ekey, &x)),
                        pos: t.pos,
                    });
                    j += 1;
                    continue;
                }
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
        // `extern "C" { .. }` and `link "name"` declare C symbols: they add no item of the module, but the items
        // written after them are still the module's own
        if !public
            && is_id(item, "link")
            && matches!(toks.get(i + 1).map(|t| &t.t), Some(Tok::Str(_)))
        {
            v.push(Span {
                start: s,
                end: i + 2,
                public,
                head,
                name: None,
                kind: Kind::Func,
                test: None,
                bench: false,
            });
            i += 2;
            continue;
        }
        let mut bench = false;
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
                if (w == "test" || w == "bench")
                    && matches!(toks.get(i + 1).map(|t| &t.t), Some(Tok::Str(_))) =>
            {
                bench = w == "bench";
                let n = match &toks[i + 1].t {
                    Tok::Str(n) => n.clone(),
                    _ => unreachable!(),
                };
                (Kind::Func, None, Some(n))
            }
            Tok::Id(w) if w == "impl" => (Kind::Func, None, None),
            Tok::Id(w)
                if !public
                    && w == "extern"
                    && matches!(toks.get(i + 1).map(|t| &t.t), Some(Tok::Str(_))) =>
            {
                (Kind::Func, None, None)
            }
            // `const NAME[: T] = expr` and `type NAME = T`: one declaration on one line (no body)
            Tok::Id(w) if (w == "const" || w == "type") && toks.get(i + 1).and_then(id_of).is_some() => {
                let line = item.pos.line;
                let mut end = i + 2;
                while end < toks.len() && !matches!(toks[end].t, Tok::Eof) && toks[end].pos.line == line {
                    end += 1;
                }
                if end < toks.len() && is_sym(&toks[end], ";") {
                    end += 1;
                }
                v.push(Span {
                    start: s,
                    end,
                    public,
                    head,
                    name: toks.get(i + 1).and_then(id_of).map(|n| n.to_string()),
                    kind: if w == "const" { Kind::Const } else { Kind::Type },
                    test: None,
                    bench: false,
                });
                i = end;
                continue;
            }
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
            bench,
        });
        i = end;
    }
    v
}

/// The file of a token of the merged program (not meaningful for the prelude's, tagged with `PRELUDE_LINE`).
pub fn file_of(line: usize) -> usize {
    line >> FILE_SHIFT
}

impl Loaded {
    /// The files that are embedded standard-library modules: besides the prelude, only they may use the raw
    /// intrinsics outside `unsafe`.
    pub fn std_files(&self) -> std::collections::HashSet<usize> {
        self.files
            .iter()
            .enumerate()
            .filter(|(_, f)| f.path.starts_with("<std>/"))
            .map(|(i, _)| i)
            .collect()
    }

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
                                name, display_key(dep), alias, name
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
