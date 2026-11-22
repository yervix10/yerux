// Integration tests of `yerux lsp`: JSON-RPC over the binary's stdin/stdout.
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Client {
    child: Child,
    to: ChildStdin,
    from: BufReader<ChildStdout>,
    next_id: i64,
    /// publishDiagnostics notifications read so far
    diags_seen: usize,
}

// a tiny JSON reader is not needed: responses are searched as text and parsed by hand where required
impl Client {
    fn start() -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_yerux"))
            .arg("lsp")
            .env("YERUX_LSP_DEBOUNCE_MS", "60")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let to = child.stdin.take().unwrap();
        let from = BufReader::new(child.stdout.take().unwrap());
        Client { child, to, from, next_id: 1, diags_seen: 0 }
    }
    fn send(&mut self, body: &str) {
        write!(self.to, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
        self.to.flush().unwrap();
    }
    fn read(&mut self) -> String {
        let mut len = 0usize;
        loop {
            let mut l = String::new();
            assert!(self.from.read_line(&mut l).unwrap() > 0, "server closed the pipe");
            let l = l.trim_end();
            if l.is_empty() {
                break;
            }
            if let Some(v) = l.strip_prefix("Content-Length: ") {
                len = v.parse().unwrap();
            }
        }
        let mut buf = vec![0u8; len];
        self.from.read_exact(&mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }
    /// Send a request and return its response (notifications that arrive first are skipped).
    fn request(&mut self, method: &str, params: &str) -> String {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&format!(r#"{{"jsonrpc":"2.0","id":{},"method":"{}","params":{}}}"#, id, method, params));
        loop {
            let m = self.read();
            if m.contains("publishDiagnostics") {
                self.diags_seen += 1;
            }
            if m.contains(&format!(r#""id":{},"#, id)) || m.ends_with(&format!(r#""id":{}}}"#, id)) {
                return m;
            }
        }
    }
    fn notify(&mut self, method: &str, params: &str) {
        self.send(&format!(r#"{{"jsonrpc":"2.0","method":"{}","params":{}}}"#, method, params));
    }
    /// Wait for the next publishDiagnostics notification.
    fn diagnostics(&mut self) -> String {
        loop {
            let m = self.read();
            if m.contains("publishDiagnostics") {
                self.diags_seen += 1;
                return m;
            }
        }
    }
    fn open(&mut self, uri: &str, text: &str) -> String {
        let t = text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
        self.notify(
            "textDocument/didOpen",
            &format!(r#"{{"textDocument":{{"uri":"{}","languageId":"yerux","version":1,"text":"{}"}}}}"#, uri, t),
        );
        self.diagnostics()
    }
    fn change(&mut self, uri: &str, text: &str) -> String {
        let t = text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
        self.notify(
            "textDocument/didChange",
            &format!(r#"{{"textDocument":{{"uri":"{}","version":2}},"contentChanges":[{{"text":"{}"}}]}}"#, uri, t),
        );
        self.diagnostics()
    }
    fn at(&mut self, method: &str, uri: &str, line: usize, ch: usize) -> String {
        self.request(
            method,
            &format!(r#"{{"textDocument":{{"uri":"{}"}},"position":{{"line":{},"character":{}}}}}"#, uri, line, ch),
        )
    }
    fn finish(mut self) {
        let r = self.request("shutdown", "null");
        assert!(r.contains(r#""result":null"#), "{r}");
        self.notify("exit", "null");
        assert!(self.child.wait().unwrap().success(), "exit after shutdown must be 0");
    }
}

fn fixture(name: &str) -> (String, String) {
    let path = format!("{}/tests/lsp/{}", env!("CARGO_MANIFEST_DIR"), name);
    let text = std::fs::read_to_string(&path).unwrap();
    (format!("file://{}", path), text)
}

#[test]
fn initialize_advertises_the_capabilities() {
    let mut c = Client::start();
    let r = c.request("initialize", r#"{"processId":null,"rootUri":null,"capabilities":{}}"#);
    for cap in ["documentSymbolProvider", "definitionProvider", "hoverProvider", "documentFormattingProvider", "textDocumentSync"] {
        assert!(r.contains(cap), "{cap} missing in {r}");
    }
    c.notify("initialized", "{}");
    c.finish();
}

#[test]
fn lsp_hover_docs_and_code_actions_work_over_json_rpc() {
    let mut c = Client::start();
    let uri = "file:///tmp/yerux_lsp_features.yx";
    c.open(uri, "/// Adds two values.\nfn add(a: Int, b: Int) -> Int { return a + b }\nstruct Pair { a: Int }\nfn main() { print(add(1, 2)) }\n");
    let r = c.at("textDocument/hover", uri, 1, 4);
    assert!(r.contains("Adds two values."), "{r}");
    let p = format!(r#"{{"textDocument":{{"uri":"{}"}},"range":{{"start":{{"line":0,"character":0}},"end":{{"line":0,"character":1}}}},"context":{{"diagnostics":[{{"message":"variable 'read_to_string' does not exist"}},{{"message":"Pair does not implement Clone"}}]}}}}"#, uri);
    let r = c.request("textDocument/codeAction", &p);
    assert!(r.contains("Import read_to_string from std.fs") && r.contains("Derive Clone for Pair"), "{r}");
    assert!(r.contains("import std.fs\\n") && r.contains("derive Clone for Pair"), "{r}");
    c.finish();
}

#[test]
fn unknown_request_gets_an_error_not_a_hang() {
    let mut c = Client::start();
    c.request("initialize", "{}");
    let r = c.request("workspace/nonsense", "{}");
    assert!(r.contains(r#""error""#) && r.contains("-32601"), "{r}");
    c.finish();
}

#[test]
fn clean_file_has_no_diagnostics_and_errors_have_ranges() {
    let mut c = Client::start();
    c.request("initialize", "{}");
    let (uri, text) = fixture("main.yx");
    let d = c.open(&uri, &text);
    assert!(d.contains(r#""diagnostics":[]"#), "{d}");
    // an unknown name on line 30 (0-based 29): `origin` -> `orgin`
    let broken = text.replace("scale(origin, 2)", "scale(orgin, 2)");
    let d = c.change(&uri, &broken);
    assert!(d.contains(r#""severity":1"#) && d.contains(r#""source":"yerux""#), "{d}");
    assert!(d.contains("orgin"), "{d}");
    assert!(d.contains(r#""start":{"line":29,"character":"#), "range of the error: {d}");
    // fixing the buffer clears them (nothing was saved to disk)
    let d = c.change(&uri, &text);
    assert!(d.contains(r#""diagnostics":[]"#), "{d}");
    c.finish();
}

#[test]
fn parse_error_is_reported_with_its_position() {
    let mut c = Client::start();
    c.request("initialize", "{}");
    let uri = "file:///tmp/yerux_lsp_test_parse.yx";
    let d = c.open(uri, "fn main() {\n    let x = \n}\n");
    assert!(d.contains(r#""severity":1"#), "{d}");
    assert!(d.contains(r#""line":2"#), "{d}");
    c.finish();
}

#[test]
fn library_file_without_main_is_not_an_error() {
    let mut c = Client::start();
    c.request("initialize", "{}");
    let (uri, text) = fixture("helper.yx");
    let d = c.open(&uri, &text);
    assert!(d.contains(r#""diagnostics":[]"#), "{d}");
    c.finish();
}

#[test]
fn an_unsaved_import_is_seen_through_the_open_buffer() {
    let mut c = Client::start();
    c.request("initialize", "{}");
    let (muri, mtext) = fixture("main.yx");
    let (huri, htext) = fixture("helper.yx");
    c.open(&huri, &htext);
    let d = c.open(&muri, &mtext);
    assert!(d.contains(r#""diagnostics":[]"#), "{d}");
    // rename `double` in the open helper buffer: main.yx must now fail
    c.change(&huri, &htext.replace("double", "twice"));
    c.notify("textDocument/didSave", &format!(r#"{{"textDocument":{{"uri":"{}"}}}}"#, muri));
    let d = c.diagnostics();
    assert!(d.contains("double"), "{d}");
    assert!(!d.contains(r#""diagnostics":[]"#), "{d}");
    c.finish();
}

#[test]
fn document_symbols_cover_items_and_their_members() {
    let mut c = Client::start();
    c.request("initialize", "{}");
    let (uri, text) = fixture("main.yx");
    c.open(&uri, &text);
    let r = c.request("textDocument/documentSymbol", &format!(r#"{{"textDocument":{{"uri":"{}"}}}}"#, uri));
    // SymbolKind: Struct 23, Enum 10, Interface 11, Class 5 (impl), Function 12, Method 6, Field 8, EnumMember 22
    for (name, kind) in [("Point", 23), ("Shape", 10), ("Area", 11), ("scale", 12), ("main", 12), ("area", 6), ("x", 8), ("Circle", 22), ("Dot", 22)] {
        assert!(r.contains(&format!(r#""name":"{}","kind":{}"#, name, kind)), "{name}/{kind} in {r}");
    }
    assert!(r.contains(r#""name":"impl Area for Point","kind":5"#), "{r}");
    assert!(r.contains(r#""selectionRange""#), "{r}");
    c.finish();
}

#[test]
fn definition_finds_locals_params_items_fields_and_module_items() {
    let mut c = Client::start();
    c.request("initialize", "{}");
    let (uri, text) = fixture("main.yx");
    c.open(&uri, &text);
    // `total` used on line 24 (col 25) -> declared on line 23
    let r = c.at("textDocument/definition", &uri, 24, 25);
    assert!(r.contains(r#""start":{"line":23,"character":8}"#), "{r}");
    // parameter `k` (line 24, col 34) -> line 22
    let r = c.at("textDocument/definition", &uri, 24, 34);
    assert!(r.contains(r#""start":{"line":22,"character":19}"#), "{r}");
    // the call `scale(..)` on line 29 -> `fn scale`
    let r = c.at("textDocument/definition", &uri, 29, 13);
    assert!(r.contains(r#""start":{"line":22,"character":3}"#), "{r}");
    // type `Point` in a parameter -> the struct
    let r = c.at("textDocument/definition", &uri, 22, 15);
    assert!(r.contains(r#""start":{"line":2,"character":7}"#), "{r}");
    // field `x` of `p.x` -> the struct field
    let r = c.at("textDocument/definition", &uri, 23, 18);
    assert!(r.contains(r#""start":{"line":3,"character":4}"#), "{r}");
    // `helper.double` -> the other file
    let r = c.at("textDocument/definition", &uri, 24, 20);
    assert!(r.contains("tests/lsp/helper.yx") && r.contains(r#""start":{"line":0,"character":7}"#), "{r}");
    // nothing under the cursor
    let r = c.at("textDocument/definition", &uri, 1, 0);
    assert!(r.contains(r#""result":null"#), "{r}");
    c.finish();
}

#[test]
fn hover_shows_checked_types_and_signatures() {
    let mut c = Client::start();
    c.request("initialize", "{}");
    let (uri, text) = fixture("main.yx");
    c.open(&uri, &text);
    let r = c.at("textDocument/hover", &uri, 29, 8);
    assert!(r.contains("let n: Int"), "{r}");
    let r = c.at("textDocument/hover", &uri, 28, 8);
    assert!(r.contains("let origin: Point"), "{r}");
    let r = c.at("textDocument/hover", &uri, 24, 25);
    assert!(r.contains("let total: Int"), "{r}");
    let r = c.at("textDocument/hover", &uri, 24, 34);
    assert!(r.contains("(parameter) k: Int"), "{r}");
    let r = c.at("textDocument/hover", &uri, 29, 12);
    assert!(r.contains("fn scale(p: Point, k: Int) -> Int"), "{r}");
    let r = c.at("textDocument/hover", &uri, 24, 20);
    assert!(r.contains("fn double(n: Int) -> Int"), "{r}");
    assert!(r.contains(r#""range""#), "{r}");
    c.finish();
}

#[test]
fn hover_and_symbols_survive_a_file_that_does_not_compile() {
    let mut c = Client::start();
    c.request("initialize", "{}");
    let (uri, text) = fixture("main.yx");
    c.open(&uri, &text.replace("scale(origin, 2)", "scale(orgin, 2)"));
    let r = c.request("textDocument/documentSymbol", &format!(r#"{{"textDocument":{{"uri":"{}"}}}}"#, uri));
    assert!(r.contains(r#""name":"scale""#), "{r}");
    let r = c.at("textDocument/hover", &uri, 29, 12);
    assert!(r.contains("fn scale"), "{r}");
    c.finish();
}

#[test]
fn formatting_returns_one_edit_and_nothing_for_formatted_code() {
    let mut c = Client::start();
    c.request("initialize", "{}");
    let uri = "file:///tmp/yerux_lsp_test_fmt.yx";
    c.open(uri, "fn main(){\nlet x=1+2\nprint(x)\n}\n");
    let r = c.request("textDocument/formatting", &format!(r#"{{"textDocument":{{"uri":"{}"}},"options":{{"tabSize":4,"insertSpaces":true}}}}"#, uri));
    assert!(r.contains("newText") && r.contains("let x = 1 + 2"), "{r}");
    assert!(r.contains(r#""start":{"line":0,"character":0}"#), "{r}");
    c.open(uri, "fn main() {\n  let x = 1 + 2\n  print(x)\n}\n");
    let r = c.request("textDocument/formatting", &format!(r#"{{"textDocument":{{"uri":"{}"}},"options":{{}}}}"#, uri));
    assert!(r.contains(r#""result":[]"#), "{r}");
    // text that does not parse cannot be formatted: an error response, not an edit
    c.open(uri, "fn main( {\n");
    let r = c.request("textDocument/formatting", &format!(r#"{{"textDocument":{{"uri":"{}"}},"options":{{}}}}"#, uri));
    assert!(r.contains(r#""error""#) && !r.contains("newText"), "{r}");
    c.finish();
}

#[test]
fn garbage_message_does_not_end_the_session() {
    let mut c = Client::start();
    c.send("{ this is not json");
    let r = c.request("initialize", "{}");
    assert!(r.contains("capabilities"), "{r}");
    c.finish();
}

#[test]
fn exit_without_shutdown_is_a_failure_code() {
    let mut c = Client::start();
    c.request("initialize", "{}");
    c.notify("exit", "null");
    assert_eq!(c.child.wait().unwrap().code(), Some(1));
}

// ───────────────────────── phase 2 ─────────────────────────

fn rich() -> (String, String) {
    fixture("rich.yx")
}

/// The labels of a completion response, as `,label,` markers to search in.
fn has_label(r: &str, label: &str) -> bool {
    r.contains(&format!(r#""label":"{}""#, label))
}

fn open_rich(c: &mut Client) -> (String, String) {
    c.request("initialize", "{}");
    let (uri, text) = rich();
    let d = c.open(&uri, &text);
    assert!(d.contains(r#""diagnostics":[]"#), "the fixture must compile: {d}");
    (uri, text)
}

/// Open the fixture with `from` replaced by `to` (a half-typed expression) and ask for completion at (line, ch).
fn complete_typing(from: &str, to: &str, line: usize, ch: usize) -> String {
    let mut c = Client::start();
    c.request("initialize", "{}");
    let (uri, text) = rich();
    c.open(&uri, &text.replace(from, to));
    let r = c.at("textDocument/completion", &uri, line, ch);
    c.finish();
    r
}

#[test]
fn initialize_advertises_phase_two_capabilities() {
    let mut c = Client::start();
    let r = c.request("initialize", "{}");
    for cap in ["completionProvider", "referencesProvider", "renameProvider", "prepareProvider", "signatureHelpProvider", "semanticTokensProvider", "tokenTypes"] {
        assert!(r.contains(cap), "{cap} missing in {r}");
    }
    assert!(r.contains(r#""change":2"#), "incremental sync: {r}");
    c.finish();
}

#[test]
fn completion_after_dot_uses_the_type_of_the_receiver() {
    // `p` is a Point: its fields and its impl methods, nothing from List
    let r = complete_typing("    print(total)\n    print(ok)", "    p.\n    print(ok)", 46, 6);
    assert!(has_label(&r, "x") && has_label(&r, "y") && has_label(&r, "sum"), "{r}");
    assert!(!has_label(&r, "push") && !has_label(&r, "len"), "{r}");
    // the prefix filters
    let r = complete_typing("    print(total)\n    print(ok)", "    p.s\n    print(ok)", 46, 7);
    assert!(has_label(&r, "sum") && !has_label(&r, "x"), "{r}");
    // `names` is a List<String>: compiler builtins and prelude methods
    let r = complete_typing("names.push(\"a\")", "names.", 40, 10);
    for m in ["push", "pop", "len", "insert", "sort"] {
        assert!(has_label(&r, m), "{m} in {r}");
    }
    assert!(!has_label(&r, "sum"), "{r}");
    // a call in the chain: `names.len()` is an Int
    let r = complete_typing("    print(total)\n    print(ok)", "    names.len().\n    print(ok)", 46, 16);
    assert!(has_label(&r, "show") && !has_label(&r, "push"), "{r}");
    // snippet insert text with the parameters as placeholders
    let r = complete_typing("names.push(\"a\")", "names.", 40, 10);
    assert!(r.contains(r#""insertText":"push(${1:x})""#) && r.contains(r#""insertTextFormat":2"#), "{r}");
}

#[test]
fn completion_of_modules_types_and_scope() {
    // `fs.` lists the public items of std.fs
    let r = complete_typing("    print(total)\n    print(ok)", "    fs.\n    print(ok)", 46, 7);
    for m in ["exists", "read_to_string", "File", "open"] {
        assert!(has_label(&r, m), "{m} in {r}");
    }
    // `Shape.` lists the variants
    let r = complete_typing("    print(total)\n    print(ok)", "    Shape.\n    print(ok)", 46, 10);
    for m in ["Circle", "Rect", "Dot"] {
        assert!(has_label(&r, m), "{m} in {r}");
    }
    // a plain name: locals in scope (not those of other functions), items, imports, keywords, snippets
    let r = complete_typing("    print(total)\n    print(ok)", "    \n    print(ok)", 46, 4);
    for m in ["total", "left", "right", "names", "area", "fs", "helper", "Point", "match", "while", "Some"] {
        assert!(has_label(&r, m), "{m} in {r}");
    }
    assert!(!has_label(&r, "r") && !has_label(&r, "w"), "match-arm names of another function leak: {r}");
    assert!(r.contains(r#""kind":15"#) && r.contains("match ${1:value}"), "snippets: {r}");
    // `import ` offers modules, `import std.` the standard library
    let r = complete_typing("import std.fs", "import ", 0, 7);
    assert!(has_label(&r, "std") && has_label(&r, "helper"), "{r}");
    let r = complete_typing("import std.fs", "import std.", 0, 11);
    assert!(has_label(&r, "fs") && has_label(&r, "process") && !has_label(&r, "helper"), "{r}");
}

#[test]
fn definition_and_hover_reach_the_standard_library_and_builtins() {
    let mut c = Client::start();
    let (uri, _) = open_rich(&mut c);
    // `fs.exists` -> a real file with the module's source
    let r = c.at("textDocument/definition", &uri, 44, 21);
    assert!(r.contains("fs.yx"), "{r}");
    let start = r.find("file://").unwrap() + 7;
    let end = r[start..].find('"').unwrap() + start;
    let std_file = std::fs::read_to_string(&r[start..end]).unwrap();
    assert!(std_file.contains("pub fn exists(path: view String) -> Bool"), "std file content");
    let line = std_file.lines().position(|l| l.starts_with("pub fn exists(")).unwrap();
    assert!(r.contains(&format!(r#""start":{{"line":{line},"character":7}}"#)), "{r}");
    let r = c.at("textDocument/hover", &uri, 44, 21);
    assert!(r.contains("pub fn exists(path: view String) -> Bool"), "{r}");
    // a method of a compiler builtin: the generated signature file
    let r = c.at("textDocument/definition", &uri, 40, 11);
    assert!(r.contains("builtins.yx"), "{r}");
    let r = c.at("textDocument/hover", &uri, 40, 11);
    assert!(r.contains("fn push(mut self, x: T)"), "{r}");
    // a prelude method found through the receiver's type: `names.len()` is builtin, `Option` is prelude source
    let r = c.at("textDocument/hover", &uri, 48, 17);
    assert!(r.contains("fn len(view self) -> Int"), "{r}");
    // a field and a method of a user type resolve by the type of the receiver
    let r = c.at("textDocument/definition", &uri, 42, 27);
    assert!(r.contains(r#""start":{"line":4,"character":4}"#), "p.x -> the field: {r}");
    let r = c.at("textDocument/definition", &uri, 45, 18);
    assert!(r.contains(r#""start":{"line":15,"character":7}"#), "p.sum -> the method: {r}");
    // the type of a variable bound to a struct literal
    let r = c.at("textDocument/hover", &uri, 45, 16);
    assert!(r.contains("let p: Point"), "{r}");
    c.finish();
}

#[test]
fn definition_finds_match_closure_and_tuple_bindings() {
    let mut c = Client::start();
    let (uri, _) = open_rich(&mut c);
    // `r` in `return 3 * r * r` -> the `r` of `Circle(r)`
    let r = c.at("textDocument/definition", &uri, 27, 23);
    assert!(r.contains(r#""start":{"line":26,"character":15}"#), "{r}");
    // `w` in `Rect(w, h)`
    let r = c.at("textDocument/definition", &uri, 30, 19);
    assert!(r.contains(r#""start":{"line":29,"character":13}"#), "{r}");
    let r = c.at("textDocument/definition", &uri, 30, 23);
    assert!(r.contains(r#""start":{"line":29,"character":16}"#), "{r}");
    // the closure parameter `n` in `|n| n * 2`
    let r = c.at("textDocument/definition", &uri, 43, 28);
    assert!(r.contains(r#""start":{"line":43,"character":25}"#), "{r}");
    // tuple pattern: `right` used later -> `let (left, right)`
    let r = c.at("textDocument/definition", &uri, 45, 28);
    assert!(r.contains(r#""start":{"line":42,"character":15}"#), "{r}");
    // the match binding of one function is not visible in another
    let r = c.at("textDocument/hover", &uri, 27, 23);
    assert!(r.contains("(pattern binding) r"), "{r}");
    let r = c.at("textDocument/hover", &uri, 43, 28);
    assert!(r.contains("(closure parameter) n"), "{r}");
    let r = c.at("textDocument/hover", &uri, 45, 28);
    assert!(r.contains("(tuple element) right"), "{r}");
    c.finish();
}

fn locations(r: &str) -> Vec<(usize, usize)> {
    // every `"start":{"line":L,"character":C}` of the response, in order
    let mut out = vec![];
    let mut rest = r;
    while let Some(p) = rest.find(r#""start":{"line":"#) {
        rest = &rest[p + 16..];
        let l: usize = rest.split(',').next().unwrap().parse().unwrap();
        let q = rest.find(r#""character":"#).unwrap() + 12;
        let ch: usize = rest[q..].split('}').next().unwrap().parse().unwrap();
        out.push((l, ch));
    }
    out
}

#[test]
fn references_are_scope_aware() {
    let mut c = Client::start();
    let (uri, _) = open_rich(&mut c);
    let ctx = r#","context":{"includeDeclaration":true}"#;
    let refs = |c: &mut Client, line: usize, ch: usize, decl: bool| {
        let extra = if decl { ctx.to_string() } else { r#","context":{"includeDeclaration":false}"#.to_string() };
        c.request(
            "textDocument/references",
            &format!(r#"{{"textDocument":{{"uri":"{}"}},"position":{{"line":{},"character":{}}}{}}}"#, uri, line, ch, extra),
        )
    };
    // the local `p`: declaration + three uses
    let r = refs(&mut c, 41, 8, true);
    assert_eq!(locations(&r), vec![(41, 8), (42, 25), (42, 30), (45, 16)], "{r}");
    let r = refs(&mut c, 45, 16, false);
    assert_eq!(locations(&r), vec![(42, 25), (42, 30), (45, 16)], "without the declaration: {r}");
    // `r` of `Circle(r)` is not the `r` of anything else, and `n` the closure parameter has two sites
    let r = refs(&mut c, 26, 15, true);
    assert_eq!(locations(&r).len(), 3, "{r}");
    // the type `Point`: struct, impl and the literal
    let r = refs(&mut c, 3, 8, true);
    assert_eq!(locations(&r), vec![(3, 7), (14, 5), (41, 12)], "{r}");
    // the method `sum`: declaration + the call
    let r = refs(&mut c, 15, 8, true);
    assert_eq!(locations(&r), vec![(15, 7), (45, 18)], "{r}");
    c.finish();
}

#[test]
fn rename_is_safe() {
    let mut c = Client::start();
    let (uri, _) = open_rich(&mut c);
    let ren = |c: &mut Client, line: usize, ch: usize, name: &str| {
        c.request(
            "textDocument/rename",
            &format!(r#"{{"textDocument":{{"uri":"{}"}},"position":{{"line":{},"character":{}}},"newName":"{}"}}"#, uri, line, ch, name),
        )
    };
    // a local: every use is edited, nothing else
    let r = ren(&mut c, 41, 8, "origin");
    assert_eq!(r.matches(r#""newText":"origin""#).count(), 4, "{r}");
    // a function and a type
    let r = ren(&mut c, 20, 4, "run");
    assert_eq!(r.matches(r#""newText":"run""#).count(), 2, "{r}");
    let r = ren(&mut c, 3, 8, "Pt");
    assert_eq!(r.matches(r#""newText":"Pt""#).count(), 3, "{r}");
    // refused: clash with another local of the scope, a keyword, a bad name, std, builtin members
    let r = ren(&mut c, 41, 8, "total");
    assert!(r.contains(r#""error""#) && r.contains("already exists"), "{r}");
    let r = ren(&mut c, 41, 8, "match");
    assert!(r.contains(r#""error""#), "{r}");
    let r = ren(&mut c, 41, 8, "9lives");
    assert!(r.contains(r#""error""#) && r.contains("not a valid name"), "{r}");
    let r = ren(&mut c, 44, 21, "present");
    assert!(r.contains(r#""error""#) && r.contains("standard library"), "{r}");
    let r = ren(&mut c, 40, 11, "append");
    assert!(r.contains(r#""error""#) && r.contains("standard library"), "{r}");
    // a local must not take the name of an item that is used in its scope
    let r = ren(&mut c, 42, 9, "apply");
    assert!(r.contains(r#""error""#), "{r}");
    // prepareRename: the range and the placeholder, or an error
    let r = c.at("textDocument/prepareRename", &uri, 45, 19);
    assert!(r.contains(r#""placeholder":"sum""#), "{r}");
    let r = c.at("textDocument/prepareRename", &uri, 44, 21);
    assert!(r.contains(r#""error""#), "{r}");
    c.finish();
}

#[test]
fn rename_edits_every_open_file_that_uses_the_item() {
    let mut c = Client::start();
    c.request("initialize", "{}");
    let (muri, mtext) = fixture("main.yx");
    let (huri, htext) = fixture("helper.yx");
    c.open(&muri, &mtext);
    c.open(&huri, &htext);
    // `double` in helper.yx is used as `helper.double` in main.yx
    let r = c.request(
        "textDocument/rename",
        &format!(r#"{{"textDocument":{{"uri":"{}"}},"position":{{"line":0,"character":8}},"newName":"twice"}}"#, huri),
    );
    assert!(r.contains(&format!(r#""{}""#, muri)) && r.contains(&format!(r#""{}""#, huri)), "{r}");
    assert_eq!(r.matches(r#""newText":"twice""#).count(), 2, "{r}");
    // and from the use site
    let r = c.request(
        "textDocument/rename",
        &format!(r#"{{"textDocument":{{"uri":"{}"}},"position":{{"line":24,"character":20}},"newName":"twice"}}"#, muri),
    );
    assert_eq!(r.matches(r#""newText":"twice""#).count(), 2, "{r}");
    c.finish();
}

#[test]
fn signature_help_names_the_active_parameter() {
    let mut c = Client::start();
    let (uri, _) = open_rich(&mut c);
    // `apply(|n| n * 2, left)`: cursor in the second argument
    let r = c.at("textDocument/signatureHelp", &uri, 43, 35);
    assert!(r.contains("fn apply(f: fn(Int) -> Int, v: Int) -> Int"), "{r}");
    assert!(r.contains(r#""activeParameter":1"#), "{r}");
    assert!(r.contains(r#"{"label":"f: fn(Int) -> Int"}"#) && r.contains(r#"{"label":"v: Int"}"#), "{r}");
    // first argument
    let r = c.at("textDocument/signatureHelp", &uri, 43, 24);
    assert!(r.contains(r#""activeParameter":0"#), "{r}");
    // a function of a standard-library module, and a builtin method (the receiver is not a parameter)
    let r = c.at("textDocument/signatureHelp", &uri, 44, 25);
    assert!(r.contains("fn exists(path: view String) -> Bool"), "{r}");
    let r = c.at("textDocument/signatureHelp", &uri, 40, 15);
    assert!(r.contains("fn push(mut self, x: T)") && r.contains(r#"{"label":"x: T"}"#), "{r}");
    // outside any call: nothing
    let r = c.at("textDocument/signatureHelp", &uri, 46, 4);
    assert!(r.contains(r#""result":null"#), "{r}");
    c.finish();
}

#[test]
fn incremental_changes_are_applied_and_diagnostics_are_debounced() {
    let mut c = Client::start();
    let (uri, text) = open_rich(&mut c);
    let before = c.diags_seen;
    // five quick edits, each a ranged insertion in the same place: `fn extra5() {}` is built up
    // character by character below the last line
    let last = text.split('\n').count() - 1; // the empty line after the final newline
    let mut col = 0;
    for chunk in ["fn ", "extra", "() ", "{ ", "}"] {
        c.notify(
            "textDocument/didChange",
            &format!(
                r#"{{"textDocument":{{"uri":"{}","version":3}},"contentChanges":[{{"range":{{"start":{{"line":{l},"character":{c}}},"end":{{"line":{l},"character":{c}}}}},"text":"{t}"}}]}}"#,
                uri,
                l = last,
                c = col,
                t = chunk
            ),
        );
        col += chunk.len();
    }
    // exactly one publication for the five edits, and it already reflects the final text
    let d = c.diagnostics();
    assert!(d.contains(r#""diagnostics":[]"#), "{d}");
    assert_eq!(c.diags_seen, before + 1);
    let r = c.request("textDocument/documentSymbol", &format!(r#"{{"textDocument":{{"uri":"{}"}}}}"#, uri));
    assert!(r.contains(r#""name":"extra""#), "ranged edits were applied: {r}");
    std::thread::sleep(std::time::Duration::from_millis(250));
    c.request("textDocument/documentSymbol", &format!(r#"{{"textDocument":{{"uri":"{}"}}}}"#, uri));
    assert_eq!(c.diags_seen, before + 1, "no extra publication after the pause");
    // a ranged edit that deletes text and one that spans lines
    c.notify(
        "textDocument/didChange",
        &format!(
            r#"{{"textDocument":{{"uri":"{}","version":4}},"contentChanges":[{{"range":{{"start":{{"line":{l},"character":0}},"end":{{"line":{l},"character":{e}}}}},"text":""}},{{"range":{{"start":{{"line":0,"character":0}},"end":{{"line":2,"character":0}}}},"text":"import std.fs\n\n"}}]}}"#,
            uri,
            l = last,
            e = col
        ),
    );
    let d = c.diagnostics();
    assert!(d.contains("helper"), "the import of helper was cut out by the second change: {d}");
    let r = c.request("textDocument/documentSymbol", &format!(r#"{{"textDocument":{{"uri":"{}"}}}}"#, uri));
    assert!(!r.contains(r#""name":"extra""#), "{r}");
    c.finish();
}

#[test]
fn incremental_edit_offsets_count_utf16_units() {
    let mut c = Client::start();
    c.request("initialize", "{}");
    let uri = "file:///tmp/yerux_lsp_test_utf16.yx";
    c.open(uri, "fn main() {\n    let s = \"é😀\"\n    print(s)\n}\n");
    // replace `print` (after the 4-unit string literal contents) by `println`: `é` is 1 unit, `😀` is 2
    c.notify(
        "textDocument/didChange",
        &format!(
            r#"{{"textDocument":{{"uri":"{}","version":2}},"contentChanges":[{{"range":{{"start":{{"line":1,"character":17}},"end":{{"line":1,"character":17}}}},"text":"\n    let t = 1"}}]}}"#,
            uri
        ),
    );
    let d = c.diagnostics();
    assert!(d.contains(r#""diagnostics":[]"#), "the literal must stay intact: {d}");
    let r = c.request("textDocument/hover", &format!(r#"{{"textDocument":{{"uri":"{}"}},"position":{{"line":2,"character":8}}}}"#, uri));
    assert!(r.contains("let t: Int"), "{r}");
    c.finish();
}

#[test]
fn semantic_tokens_classify_names() {
    let mut c = Client::start();
    let (uri, _) = open_rich(&mut c);
    let r = c.request("textDocument/semanticTokens/full", &format!(r#"{{"textDocument":{{"uri":"{}"}}}}"#, uri));
    let data: Vec<usize> = r
        .split(r#""data":["#)
        .nth(1)
        .unwrap()
        .split(']')
        .next()
        .unwrap()
        .split(',')
        .map(|x| x.parse().unwrap())
        .collect();
    assert_eq!(data.len() % 5, 0);
    // decode into (line, col, len, type, modifiers)
    let (mut l, mut ch) = (0, 0);
    let mut toks = vec![];
    for t in data.chunks(5) {
        l += t[0];
        ch = if t[0] == 0 { ch + t[1] } else { t[1] };
        toks.push((l, ch, t[2], t[3], t[4]));
    }
    let types = ["namespace", "type", "struct", "enum", "enumMember", "function", "method", "parameter", "variable", "property", "keyword", "interface"];
    let kind = |line: usize, col: usize| toks.iter().find(|t| t.0 == line && t.1 == col).map(|t| (types[t.3], t.4));
    assert_eq!(kind(0, 0), Some(("keyword", 0)), "import");
    assert_eq!(kind(0, 7), Some(("namespace", 0)), "fs in the import");
    assert_eq!(kind(3, 7), Some(("struct", 1)), "struct declaration");
    assert_eq!(kind(4, 4), Some(("property", 1)), "field declaration");
    assert_eq!(kind(8, 5), Some(("enum", 1)));
    assert_eq!(kind(9, 4), Some(("enumMember", 1)));
    assert_eq!(kind(15, 7), Some(("method", 1)), "method declaration");
    assert_eq!(kind(20, 3), Some(("function", 1)), "function declaration");
    assert_eq!(kind(20, 9), Some(("parameter", 1)), "parameter declaration");
    assert_eq!(kind(21, 11), Some(("parameter", 0)), "parameter use");
    assert_eq!(kind(26, 15), Some(("variable", 1)), "match binding");
    assert_eq!(kind(41, 8), Some(("variable", 1)), "let");
    assert_eq!(kind(44, 13), Some(("namespace", 0)), "fs.exists: the alias");
    assert_eq!(kind(44, 16), Some(("function", 2)), "fs.exists: a standard-library function");
    assert_eq!(kind(45, 18), Some(("method", 0)), "p.sum");
    assert_eq!(kind(42, 25), Some(("variable", 0)), "p");
    assert_eq!(kind(42, 27), Some(("property", 0)), "p.x");
    assert_eq!(kind(43, 18), Some(("function", 0)), "apply(..)");
    assert_eq!(kind(20, 15), Some(("type", 2)), "Int is a library type");
    assert_eq!(kind(25, 4), Some(("keyword", 0)), "match");
    c.finish();
}
