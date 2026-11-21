// Integration tests of `yerux lsp`: JSON-RPC over the binary's stdin/stdout.
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Client {
    child: Child,
    to: ChildStdin,
    from: BufReader<ChildStdout>,
    next_id: i64,
}

// a tiny JSON reader is not needed: responses are searched as text and parsed by hand where required
impl Client {
    fn start() -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_yerux"))
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let to = child.stdin.take().unwrap();
        let from = BufReader::new(child.stdout.take().unwrap());
        Client { child, to, from, next_id: 1 }
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
fn exit_without_shutdown_is_a_failure_code() {
    let mut c = Client::start();
    c.request("initialize", "{}");
    c.notify("exit", "null");
    assert_eq!(c.child.wait().unwrap().code(), Some(1));
}
