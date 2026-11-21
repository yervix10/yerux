// yerux v0.2 — the Yerux compiler driver
// Pipeline: lexer -> parser -> checker (types + ownership) -> C backend -> cc
mod a64;
mod cgen;
mod check;
mod comptime;
mod dropscc;
mod elfdyn;
mod fmt;
mod inline;
mod ir;
mod layout;
mod loopopt;
mod memopt;
mod modules;
mod numeric;
mod opt;
mod regalloc;
mod sroa;
mod stdlib;
mod syntax;
mod x64;

use std::process::{exit, Command};
use std::{env, fs};
use syntax::Parser;

fn frontend(
    file: &str,
    mode: modules::Mode,
) -> Result<(syntax::Program, check::Sigs), Vec<String>> {
    let loaded = modules::load(file, mode)?;
    let mut ps = Parser::new(loaded.toks.clone());
    let mut prog = ps.program().map_err(|d| vec![loaded.render(&d)])?;
    let sigs = check::check_program(&mut prog, &mut ps)
        .map_err(|ds| ds.iter().map(|d| loaded.render(d)).collect::<Vec<_>>())?;
    comptime::run(&mut prog)
        .map_err(|ds| ds.iter().map(|d| loaded.render(d)).collect::<Vec<_>>())?;
    Ok((prog, sigs))
}

fn report_errors(msgs: &[String]) -> ! {
    for m in msgs {
        eprintln!("{}", m);
    }
    eprintln!("{} error(s). Nothing was generated.", msgs.len());
    exit(1);
}

static TMP_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn tmp_path(ext: &str) -> std::path::PathBuf {
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    env::temp_dir().join(format!("yerux_{}_{}.{}", std::process::id(), n, ext))
}

/// Compile generated C with the system C compiler.
fn c_compile(c: &str, out: &str, links: &[String]) -> Result<(), String> {
    let cfile = tmp_path("c");
    fs::write(&cfile, c).map_err(|_| "could not write the temporary C file".to_string())?;
    let mut cmd = Command::new("cc");
    cmd.args([
        "-O2",
        "-w",
        "-fno-math-errno",
        "-ffp-contract=off",
        "-pthread",
        "-o",
        out,
    ])
    .arg(&cfile)
    .arg("-lm");
    for lib in links {
        if lib.starts_with("lib") && lib.contains(".so") {
            cmd.arg(format!("-Wl,-l:{}", lib));
        } else {
            cmd.arg(format!("-l{}", lib));
        }
    }
    let st = cmd
        .status()
        .map_err(|_| "C compiler (cc) not found".to_string())?;
    let _ = fs::remove_file(&cfile);
    if st.success() {
        Ok(())
    } else {
        Err("the C compiler failed".into())
    }
}

fn native_build(
    prog: &syntax::Program,
    out: &str,
    debug_alloc: bool,
    optimize: bool,
    target: &str,
) {
    if !prog.externs.is_empty() && (target == "aarch64" || target == "windows") {
        eprintln!(
            "error: native backend: extern C is not supported on the {} target yet",
            target
        );
        exit(1);
    }
    let mut ir = ir::lower(prog).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        exit(1)
    });
    if optimize {
        opt::run(&mut ir);
    }
    if !target.is_empty() && target != "linux" {
        if let Some(what) = ir::os_intrinsics_error(&ir) {
            eprintln!("error: native backend: {} is not supported on the {} target yet (available on x86-64 Linux)", what, target);
            exit(1);
        }
    }
    // --target aarch64 selects the AArch64 Linux backend, --target windows the x86-64 Windows (PE32+)
    // one (default: x86-64 Linux)
    let exe = match target {
        "aarch64" => a64::compile(&ir, debug_alloc),
        "windows" => x64::compile_os(&ir, debug_alloc, x64::Os::Windows),
        _ => x64::compile(&ir, debug_alloc),
    };
    fs::write(out, &exe).expect("could not write the executable");
    if target != "windows" {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(out, fs::Permissions::from_mode(0o755)).expect("chmod failed");
    }
}

const USAGE: &str = "usage: yerux <check|c|ir|build|run|tokens|ast-expr|ast-stmt> file.yx [-o output] [--native] [--target linux|aarch64|windows] [--no-opt] [--native-debug-alloc]
       yerux fmt [--check|--stdout] file.yx|dir ...
       yerux test [file.yx|dir ...] [--native] [--no-opt] [--filter text]";

fn hex(bytes: &[u8]) -> String {
    let mut out = String::new();
    for byte in bytes {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

fn dump_tokens(file: &str) {
    let src = fs::read_to_string(file).unwrap_or_else(|e| {
        eprintln!("could not read {}: {}", file, e);
        exit(2)
    });
    let tokens = syntax::lex(&src).unwrap_or_else(|d| {
        println!(
            "Error:{}:{}:{}:{}",
            hex(d.msg.as_bytes()),
            d.pos.line,
            d.pos.col,
            d.pos.len
        );
        for note in d.notes {
            println!("Note:{}", hex(note.as_bytes()));
        }
        exit(1)
    });
    for token in tokens {
        let row = match token.t {
            syntax::Tok::Id(s) => format!("Id:{}", hex(s.as_bytes())),
            syntax::Tok::Num(n) => format!("Num:{}", n),
            syntax::Tok::NumT(n, suffix) => format!("NumT:{}:{}", n, suffix),
            syntax::Tok::Float(s, suffix) => format!("Float:{}:{}", hex(s.as_bytes()), suffix),
            syntax::Tok::Str(s) => format!("Str:{}", hex(s.as_bytes())),
            syntax::Tok::Char(n) => format!("Char:{}", n),
            syntax::Tok::Sym(s) => format!("Sym:{}", hex(s.as_bytes())),
            syntax::Tok::Eof => "Eof".to_string(),
        };
        println!("{}", row);
    }
}

fn dump_ast_expr(file: &str) {
    let src = fs::read_to_string(file).unwrap_or_else(|e| {
        eprintln!("could not read {}: {}", file, e);
        exit(2)
    });
    match syntax::dump_expr_source(&src) {
        Ok(ast) => println!("{}", ast),
        Err(d) => {
            println!("Error:{}:{}:{}:{}", hex(d.msg.as_bytes()), d.pos.line, d.pos.col, d.pos.len);
            for note in d.notes {
                println!("Note:{}", hex(note.as_bytes()));
            }
            exit(1)
        }
    }
}

fn dump_ast_stmt(file: &str) {
    let src = fs::read_to_string(file).unwrap_or_else(|e| {
        eprintln!("could not read {}: {}", file, e);
        exit(2)
    });
    match syntax::dump_stmt_source(&src) {
        Ok(ast) => println!("{}", ast),
        Err(d) => {
            println!("Error:{}:{}:{}:{}", hex(d.msg.as_bytes()), d.pos.line, d.pos.col, d.pos.len);
            for note in d.notes {
                println!("Note:{}", hex(note.as_bytes()));
            }
            exit(1)
        }
    }
}

fn main() {
    // the front end and the IR passes are recursive: run on a thread with a large (lazily committed) stack
    let h = std::thread::Builder::new()
        .stack_size(2 << 30)
        .spawn(real_main);
    match h {
        Ok(h) => {
            if h.join().is_err() {
                exit(101)
            }
        }
        Err(_) => real_main(),
    }
}

fn real_main() {
    let args: Vec<String> = env::args().collect();
    if args.len() == 3 && args[1] == "tokens" {
        dump_tokens(&args[2]);
        return;
    }
    if args.len() == 3 && args[1] == "ast-expr" {
        dump_ast_expr(&args[2]);
        return;
    }
    if args.len() == 3 && args[1] == "ast-stmt" {
        dump_ast_stmt(&args[2]);
        return;
    }
    if args.len() >= 2 && args[1] == "fmt" {
        exit(fmt::run_cli(&args[2..]));
    }
    if args.len() >= 2 && args[1] == "test" {
        exit(run_tests(&args[2..]));
    }
    if args.len() < 3 {
        eprintln!("{}", USAGE);
        exit(2);
    }
    let (cmd, file) = (args[1].as_str(), &args[2]);
    if let Err(e) = fs::metadata(file) {
        eprintln!("could not read {}: {}", file, e);
        exit(2);
    }
    let (prog, sigs) = match frontend(file, modules::Mode::Normal) {
        Ok(x) => x,
        Err(msgs) => report_errors(&msgs),
    };
    // --native-debug-alloc: native build that reports allocations / frees / live blocks on stderr at exit
    let debug_alloc = args.iter().any(|a| a == "--native-debug-alloc");
    let no_opt = args.iter().any(|a| a == "--no-opt");
    if args.iter().any(|a| a == "--check-ranges") {
        ir::CHECK_RANGES.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let target = args
        .iter()
        .position(|a| a == "--target")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("");
    if !matches!(target, "" | "linux" | "aarch64" | "windows") {
        eprintln!(
            "unknown target: {} (expected linux, aarch64 or windows)",
            target
        );
        exit(2);
    }
    // Explicit cross targets select the corresponding native backend.
    let native = debug_alloc
        || matches!(target, "aarch64" | "windows")
        || args.iter().any(|a| a == "--native");
    if cmd == "ir" {
        match ir::lower(&prog) {
            Ok(mut p) => {
                if !no_opt {
                    opt::run(&mut p);
                }
                print!("{}", ir::dump(&p))
            }
            Err(e) => {
                eprintln!("error: {}", e);
                exit(1)
            }
        }
        return;
    }
    let c = if native {
        String::new()
    } else {
        cgen::generate(&prog, sigs)
    };
    let build = |out: &str| {
        if let Err(e) = c_compile(&c, out, &prog.links) {
            eprintln!("{}", e);
            exit(1);
        }
    };
    match cmd {
        "check" => println!("ok: no type or ownership errors"),
        "c" => print!("{}", c),
        "build" => {
            let out = args
                .iter()
                .position(|a| a == "-o")
                .and_then(|i| args.get(i + 1).cloned())
                .unwrap_or_else(|| {
                    let stem = file.trim_end_matches(".yx");
                    if target == "windows" {
                        format!("{}.exe", stem)
                    } else {
                        stem.to_string()
                    }
                });
            if native {
                native_build(&prog, &out, debug_alloc, !no_opt, target);
            } else {
                build(&out);
            }
            println!("built: {}", out);
        }
        "run" => {
            let ext = if target == "windows" { "exe" } else { "bin" };
            let exe = tmp_path(ext).to_string_lossy().to_string();
            if native {
                native_build(&prog, &exe, debug_alloc, !no_opt, target);
            } else {
                build(&exe);
            }
            // a Windows executable can only run here through wine
            let mut cmd = if target == "windows" && !cfg!(windows) {
                let mut c = Command::new("wine");
                c.arg(&exe);
                c
            } else {
                Command::new(&exe)
            };
            let st = cmd.status().expect(
                "could not run the program (for --target windows on this OS, wine is needed)",
            );
            let _ = fs::remove_file(&exe);
            exit(st.code().unwrap_or(1));
        }
        _ => {
            eprintln!("unknown command: {}", cmd);
            exit(2);
        }
    }
}

// ───────────────────────── yerux test ─────────────────────────

/// `.yx` files under `dir` (recursively, sorted) whose text mentions a `test "` block.
fn collect_test_files(dir: &std::path::Path, out: &mut Vec<String>) {
    let mut entries: Vec<_> = match fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
        Err(_) => return,
    };
    entries.sort();
    for p in entries {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if p.is_dir() {
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                collect_test_files(&p, out);
            }
        } else if name.ends_with(".yx")
            && fs::read_to_string(&p).is_ok_and(|s| s.contains("test \""))
        {
            out.push(p.to_string_lossy().trim_start_matches("./").to_string());
        }
    }
}

/// Build the program for one test and run it. Ok(None) = passed, Ok(Some(why)) = failed.
fn run_one_test(
    file: &str,
    idx: usize,
    native: bool,
    optimize: bool,
) -> Result<Option<String>, Vec<String>> {
    let (prog, sigs) = frontend(file, modules::Mode::Test(idx))?;
    let exe = tmp_path("bin").to_string_lossy().to_string();
    if native {
        let mut ir = ir::lower(&prog).map_err(|e| vec![format!("error: {}", e)])?;
        if optimize {
            opt::run(&mut ir);
        }
        let bytes = x64::compile(&ir, false);
        fs::write(&exe, &bytes).map_err(|e| vec![format!("error: {}", e)])?;
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&exe, fs::Permissions::from_mode(0o755));
    } else {
        c_compile(&cgen::generate(&prog, sigs), &exe, &prog.links)
            .map_err(|e| vec![format!("error: {}", e)])?;
    }
    let (so, se) = (tmp_path("out"), tmp_path("err"));
    let child = Command::new(&exe)
        .stdout(fs::File::create(&so).map_err(|e| vec![e.to_string()])?)
        .stderr(fs::File::create(&se).map_err(|e| vec![e.to_string()])?)
        .spawn();
    let mut child = child.map_err(|e| vec![format!("error: could not run the test: {}", e)])?;
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if start.elapsed().as_secs() >= 30 => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(5)),
            Err(_) => break None,
        }
    };
    let stdout = fs::read_to_string(&so).unwrap_or_default();
    let stderr = fs::read_to_string(&se).unwrap_or_default();
    for p in [
        &exe,
        &so.to_string_lossy().to_string(),
        &se.to_string_lossy().to_string(),
    ] {
        let _ = fs::remove_file(p);
    }
    let mut detail = String::new();
    for l in stdout.lines() {
        detail += &format!("| {}\n", l);
    }
    for l in stderr.lines() {
        detail += &format!("! {}\n", l);
    }
    match status {
        Some(st) if st.success() => Ok(None),
        Some(st) => Ok(Some(format!(
            "exit code {}\n{}",
            st.code().unwrap_or(-1),
            detail
        ))),
        None => Ok(Some(format!("timed out after 30 s\n{}", detail))),
    }
}

fn run_tests(args: &[String]) -> i32 {
    let mut paths: Vec<String> = vec![];
    let (mut native, mut optimize, mut filter) = (false, true, None::<String>);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--native" => native = true,
            "--no-opt" => optimize = false,
            "--filter" => {
                i += 1;
                filter = args.get(i).cloned();
            }
            a if a.starts_with("--") => {
                eprintln!("unknown option {}\n{}", a, USAGE);
                return 2;
            }
            a => paths.push(a.to_string()),
        }
        i += 1;
    }
    if paths.is_empty() {
        paths.push(".".into());
    }
    let mut files: Vec<String> = vec![];
    for p in &paths {
        let pa = std::path::Path::new(p);
        if pa.is_dir() {
            collect_test_files(pa, &mut files);
        } else if pa.is_file() {
            files.push(p.clone());
        } else {
            eprintln!("could not read {}", p);
            return 2;
        }
    }
    let (mut passed, mut failed) = (0usize, 0usize);
    for f in &files {
        let names = match modules::load(f, modules::Mode::Normal) {
            Ok(l) => l.tests,
            Err(msgs) => {
                for m in &msgs {
                    eprintln!("{}", m);
                }
                failed += 1;
                continue;
            }
        };
        let wanted: Vec<(usize, &String)> = names
            .iter()
            .enumerate()
            .filter(|(_, n)| filter.as_ref().is_none_or(|x| n.contains(x.as_str())))
            .collect();
        if wanted.is_empty() {
            continue;
        }
        println!("{}", f);
        for (idx, name) in wanted {
            match run_one_test(f, idx, native, optimize) {
                Ok(None) => {
                    passed += 1;
                    println!("  ok    {}", name);
                }
                Ok(Some(why)) => {
                    failed += 1;
                    println!("  FAIL  {}", name);
                    for l in why.lines() {
                        println!("        {}", l);
                    }
                }
                Err(msgs) => {
                    failed += 1;
                    println!("  FAIL  {}", name);
                    println!("        does not compile:");
                    for m in &msgs {
                        for l in m.lines() {
                            println!("        {}", l);
                        }
                    }
                }
            }
        }
    }
    if passed + failed == 0 {
        println!("no tests found (a test is `test \"name\" {{ assert(...) }}` at the top level of a .yx file)");
        return 0;
    }
    println!();
    println!(
        "test result: {}. {} passed; {} failed",
        if failed == 0 { "ok" } else { "FAILED" },
        passed,
        failed
    );
    if failed == 0 {
        0
    } else {
        1
    }
}
