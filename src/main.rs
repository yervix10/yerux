// yerux v0.2 — the Yerux compiler driver
// Pipeline: lexer -> parser -> checker (types + ownership) -> C backend -> cc
mod a64;
mod bench;
mod doc;
mod cgen;
mod check;
mod check_trace;
mod comptime;
mod dropscc;
mod elfdyn;
mod fmt;
mod inline;
mod dwarf;
mod ir;
mod json;
mod layout;
mod loopopt;
mod lsp;
mod memopt;
mod modobj;
mod modules;
mod numeric;
mod opt;
mod project;
mod selfhost;
mod regalloc;
mod sepc;
mod sroa;
mod stdlib;
mod syntax;
mod x64;

use std::process::{exit, Command};
use std::time::Instant;
use std::{env, fs};
use syntax::Parser;

pub(crate) fn profile_stage(stage: &str, start: Instant) {
    if env::var_os("YERUX_PROFILE").is_some() {
        eprintln!(
            "yerux profile: {stage}: {:.3} ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
    }
}

pub(crate) fn profile_result(stage: &str, accepted: bool) {
    if env::var_os("YERUX_PROFILE").is_some() {
        eprintln!(
            "yerux profile: {stage} result: {}",
            if accepted { "accepted" } else { "rejected" }
        );
    }
}

fn checked_frontend(
    file: &str,
    mode: modules::Mode,
) -> Result<(syntax::Program, check::Sigs, modules::Loaded), Vec<String>> {
    let start = Instant::now();
    let loaded_result = modules::load(file, mode);
    profile_stage("module load", start);
    profile_result("module load", loaded_result.is_ok());
    let mut loaded = loaded_result?;
    let start = Instant::now();
    let mut ps = Parser::new(std::mem::take(&mut loaded.toks));
    let parse_result = ps.program();
    profile_stage("parse", start);
    profile_result("parse", parse_result.is_ok());
    let mut prog = parse_result.map_err(|d| vec![loaded.render(&d)])?;
    let start = Instant::now();
    let checked = check::check_program(&mut prog, &mut ps);
    profile_stage("type and ownership check", start);
    profile_result("type and ownership check", checked.is_ok());
    let sigs = checked.map_err(|ds| ds.iter().map(|d| loaded.render(d)).collect::<Vec<_>>())?;
    Ok((prog, sigs, loaded))
}

fn frontend(
    file: &str,
    mode: modules::Mode,
) -> Result<(syntax::Program, check::Sigs, Vec<String>), Vec<String>> {
    let (mut prog, sigs, loaded) = checked_frontend(file, mode)?;
    let start = Instant::now();
    comptime::run(&mut prog)
        .map_err(|ds| ds.iter().map(|d| loaded.render(d)).collect::<Vec<_>>())?;
    profile_stage("compile-time evaluation", start);
    let files = loaded.files.iter().map(|f| f.path.clone()).collect();
    Ok((prog, sigs, files))
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

/// The C compiler used for the C backend: `$YERUX_CC`, else the first of cc, gcc, clang that runs
/// (on Windows also MinGW's `x86_64-w64-mingw32-gcc`). MSVC's `cl` is not supported: the generated C uses
/// POSIX threads and sockets, which MinGW-w64 and clang provide.
fn find_c_compiler() -> Option<String> {
    static CC: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CC.get_or_init(|| {
        let works = |name: &str| {
            Command::new(name)
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        if let Ok(cc) = env::var("YERUX_CC") {
            if !cc.is_empty() {
                return works(&cc).then_some(cc);
            }
        }
        let mut names = vec!["cc", "gcc", "clang"];
        if cfg!(windows) {
            names.push("x86_64-w64-mingw32-gcc");
        }
        names.into_iter().find(|n| works(n)).map(|n| n.to_string())
    })
    .clone()
}

const NO_CC: &str = "no C compiler found (looked for $YERUX_CC, cc, gcc, clang). Install gcc/clang (on Windows: MinGW-w64 or LLVM), set YERUX_CC, or use the native backend: `yerux build prog.yx --native`";

/// Compiler flags shared by every C build (one translation unit or many).
fn cc_flags(debug: bool) -> Vec<String> {
    let mut v: Vec<String> = [
        if debug { "-O0" } else { "-O2" },
        if debug { "-g" } else { "-DNDEBUG" },
        "-w",
        "-fno-math-errno",
        "-ffp-contract=off",
        "-pthread",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // with YERUX_BACKTRACE set at build time every Yerux function keeps its own frame, so the panic stack is complete
    let bt = env::var("YERUX_BACKTRACE").map(|v| !v.is_empty() && v != "0").unwrap_or(false);
    if bt {
        v.extend(["-fno-inline".to_string(), "-fno-optimize-sibling-calls".to_string()]);
    }
    if bt || debug {
        v.push("-DNX_BACKTRACE".to_string());
    }
    if let Ok(extra) = env::var("YERUX_CC_FLAGS") {
        v.extend(extra.split_whitespace().map(|s| s.to_string()));
    }
    v
}

fn link_flags(links: &[String]) -> Vec<String> {
    links
        .iter()
        .map(|lib| {
            if lib.starts_with("lib") && lib.contains(".so") {
                format!("-Wl,-l:{}", lib)
            } else {
                format!("-l{}", lib)
            }
        })
        .collect()
}

/// `--separate`: compile the generated C as one unit per user module, reusing cached objects (src/sepc.rs).
/// Falls back to the single-unit build when the C has a shape the splitter does not know.
fn c_compile_separate(c: &str, out: &str, links: &[String], debug: bool, source_files: &[String]) -> Result<(), String> {
    let cc = find_c_compiler().ok_or_else(|| NO_CC.to_string())?;
    let mut mods = std::collections::HashSet::new();
    for f in source_files {
        if f.starts_with("<std>") {
            continue;
        }
        let p = std::path::Path::new(f);
        if let Some(st) = p.file_stem().and_then(|s| s.to_str()) {
            mods.insert(st.to_string());
        }
    }
    mods.remove("main");
    let entry_stem = source_files.first().and_then(|f| std::path::Path::new(f).file_stem()).map(|s| s.to_string_lossy().into_owned());
    if let Some(e) = entry_stem {
        mods.remove(&e);
    }
    let Some(split) = sepc::split(c, &mods) else {
        eprintln!("separate: generated C not splittable, compiling as one unit");
        return c_compile_mode(c, out, links, debug);
    };
    let cache = match env::var("YERUX_CACHE") {
        Ok(d) if !d.is_empty() => std::path::PathBuf::from(d),
        _ => std::path::PathBuf::from(format!("{out}.cache")),
    };
    match sepc::build(&split, &cc, &cc_flags(debug), &link_flags(links), out, &cache) {
        Ok((n, hit)) => {
            eprintln!("separate: {n} unit(s), {hit} reused from {}", cache.display());
            Ok(())
        }
        Err(e) => {
            eprintln!("separate: {e}; compiling as one unit");
            c_compile_mode(c, out, links, debug)
        }
    }
}

/// Compile generated C with the system C compiler.
fn c_compile_mode(c: &str, out: &str, links: &[String], debug: bool) -> Result<(), String> {
    let cc = find_c_compiler().ok_or_else(|| NO_CC.to_string())?;
    let cfile = tmp_path("c");
    fs::write(&cfile, c).map_err(|_| "could not write the temporary C file".to_string())?;
    let mut cmd = Command::new(&cc);
    cmd.args(cc_flags(debug)).arg("-o").arg(out).arg(&cfile).arg("-lm");
    cmd.args(link_flags(links));
    let st = cmd
        .status()
        .map_err(|_| format!("C compiler ({}) could not be started", cc))?;
    let _ = fs::remove_file(&cfile);
    if st.success() {
        Ok(())
    } else {
        Err("the C compiler failed".into())
    }
}

fn c_compile(c: &str, out: &str, links: &[String]) -> Result<(), String> {
    c_compile_mode(c, out, links, false)
}

fn native_build(
    prog: &syntax::Program,
    out: &str,
    debug_alloc: bool,
    optimize: bool,
    target: &str,
    source_files: &[String],
) {
    if !prog.externs.is_empty() && (target == "aarch64" || target == "windows") {
        eprintln!(
            "error: native backend: extern C is not supported on the {} target yet",
            target
        );
        exit(1);
    }
    let start = Instant::now();
    let dwarf = matches!(target, "" | "linux")
        && prog.externs.is_empty()
        && x64::DWARF.load(std::sync::atomic::Ordering::Relaxed);
    let mut ir = ir::lower_with_debug(prog, dwarf).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        exit(1)
    });
    profile_stage("IR lowering", start);
    ir.files = source_files.to_vec();
    if optimize {
        let start = Instant::now();
        opt::run(&mut ir);
        profile_stage("native optimization", start);
    }
    if !target.is_empty() && target != "linux" {
        let aarch64 = target == "aarch64";
        if aarch64 {
            a64::discard_unused_functions(&mut ir);
        }
        let mut unsupported = ir::os_intrinsics_error(&ir, aarch64);
        if aarch64 && unsupported.is_none() {
            unsupported = a64::translate_syscalls(&mut ir).err();
        }
        if let Some(what) = unsupported {
            eprintln!("error: native backend: {} is not supported on the {} target yet (available on x86-64 Linux)", what, target);
            exit(1);
        }
    }
    // --target aarch64 selects the AArch64 Linux backend, --target windows the x86-64 Windows (PE32+)
    // one (default: x86-64 Linux)
    let start = Instant::now();
    let exe = match target {
        "aarch64" => a64::compile(&ir, debug_alloc),
        "windows" => x64::compile_os(&ir, debug_alloc, x64::Os::Windows),
        _ => x64::compile(&ir, debug_alloc),
    };
    profile_stage("native code generation", start);
    fs::write(out, &exe).expect("could not write the executable");
    #[cfg(unix)]
    if target != "windows" {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(out, fs::Permissions::from_mode(0o755)).expect("chmod failed");
    }
}

const USAGE: &str = "usage: yerux <check|c|ir|build|run|tokens|tokens-positioned|ast-file|resolve-file|locals-file|types-file|ast-expr|ast-stmt> file.yx [-o output] [--native] [--target linux|aarch64|windows] [--debug|--release] [--dwarf] [--obj mod.yxo] [--separate] [--no-opt] [--native-debug-alloc]
       yerux --selfhost <check|c|build|run|--stage2> ...
       yerux --version | doctor       (version; check cc, backends and paths)
       yerux lsp                      (language server on stdin/stdout)
       yerux fmt [--check|--stdout] file.yx|dir ...
       yerux test [file.yx|dir ...] [--native] [--no-opt] [--filter text]
       (run: `yerux run file.yx [flags] -- ARGS...` passes ARGS to the program)
       yerux add NAME --path DIR | --git URL [--tag T | --version REQ]   (dependency in yerux.toml; updates yerux.lock)
       yerux remove NAME
       yerux lock [--locked | --update]
       yerux build|run|test [--locked]   (current project)
       yerux doc [--html] [-o DIR] [dir|file.yx ...]
       yerux bench [file.yx|dir ...] [--native] [--no-opt] [--filter text] [--runs N]";

fn hex(bytes: &[u8]) -> String {
    let mut out = String::new();
    for byte in bytes {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

fn dump_tokens(file: &str, positioned: bool) {
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
        if positioned {
            println!("{}:{}", token.pos.line, row);
        } else {
            println!("{}", row);
        }
    }
}

fn dump_ast(file: &str, parse: fn(&str) -> Result<String, syntax::Diag>) {
    let src = fs::read_to_string(file).unwrap_or_else(|e| {
        eprintln!("could not read {}: {}", file, e);
        exit(2)
    });
    match parse(&src) {
        Ok(ast) => println!("{}", ast),
        Err(d) => {
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

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `yerux doctor`: what this installation can do. Returns the exit code (1 when nothing can build programs).
fn doctor() -> i32 {
    println!("yerux {}", VERSION);
    match env::current_exe() {
        Ok(p) => println!("  binary:      {}", p.display()),
        Err(e) => println!("  binary:      unknown ({})", e),
    }
    println!("  std library: built in (the prelude and std.* modules are part of the binary; nothing to locate)");
    let tool = |name: &str, arg: &str| -> Option<String> {
        let o = Command::new(name).arg(arg).output().ok()?;
        let text = String::from_utf8_lossy(if o.stdout.is_empty() {
            &o.stderr
        } else {
            &o.stdout
        })
        .to_string();
        Some(text.lines().next().unwrap_or("").to_string())
    };
    let cc = find_c_compiler().map(|name| {
        let v = tool(&name, "--version").unwrap_or_default();
        (name, v)
    });
    match &cc {
        Some((n, v)) => println!("  C compiler:  ok  {} ({})", n, v),
        None => println!("  C compiler:  MISSING  none of $YERUX_CC, cc, gcc, clang runs: the C backend needs one (install gcc/clang; on Windows MinGW-w64 or LLVM); the native backend works without it"),
    }
    println!(
        "  native:      writes {} executables itself (no linker needed){}",
        if cfg!(windows) {
            "Windows PE"
        } else {
            "x86-64 Linux ELF"
        },
        if cfg!(target_arch = "x86_64") {
            ""
        } else {
            "; this build is not x86-64, use the C backend"
        }
    );
    println!("  platform:    {} / {}", env::consts::OS, env::consts::ARCH);
    match tool("wine", "--version") {
        Some(v) => println!("  wine:        found ({}), used to run Windows targets", v),
        None => println!(
            "  wine:        not found (optional: only for running --target windows output)"
        ),
    }
    let dir = env::temp_dir();
    let writable = fs::write(
        dir.join(format!("yerux_doctor_{}", std::process::id())),
        b"x",
    )
    .map(|_| {
        let _ = fs::remove_file(dir.join(format!("yerux_doctor_{}", std::process::id())));
    })
    .is_ok();
    println!(
        "  temp dir:    {} ({})",
        dir.display(),
        if writable { "writable" } else { "NOT writable" }
    );
    // end-to-end check: compile and run a tiny program with each backend
    let src = dir.join(format!("yerux_doctor_{}.yx", std::process::id()));
    let _ = fs::write(&src, "fn main() {\n  print(6 * 7)\n}\n");
    let exe = env::current_exe().ok();
    let mut ok_any = false;
    for (label, flag) in [("C backend", None), ("native backend", Some("--native"))] {
        if label == "C backend" && cc.is_none() {
            continue;
        }
        let out = dir.join(format!(
            "yerux_doctor_{}_{}",
            std::process::id(),
            if flag.is_some() { "n" } else { "c" }
        ));
        let mut c = Command::new(exe.clone().unwrap_or_else(|| "yerux".into()));
        c.arg("build").arg(&src).arg("-o").arg(&out);
        if let Some(f) = flag {
            c.arg(f);
        }
        let built = c.output().map(|o| o.status.success()).unwrap_or(false);
        let ran = built
            && Command::new(&out)
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "42")
                .unwrap_or(false);
        println!(
            "  self-test:   {} {}",
            label,
            if ran {
                "ok (compiled and ran a program)"
            } else {
                "FAILED"
            }
        );
        ok_any |= ran;
        let _ = fs::remove_file(&out);
    }
    let _ = fs::remove_file(&src);
    if ok_any {
        0
    } else {
        println!("  result:      no backend can build programs here");
        1
    }
}

fn real_main() {
    let args: Vec<String> = env::args().collect();
    if let Some(code) = selfhost::dispatch(&args[1..]) {
        exit(code);
    }
    if args.len() == 2 && matches!(args[1].as_str(), "--version" | "-V" | "version") {
        println!("yerux {}", VERSION);
        return;
    }
    if args.len() == 2 && args[1] == "doctor" {
        exit(doctor());
    }
    if args.len() == 3 {
        match args[1].as_str() {
            "tokens" | "tokens-positioned" => {
                return dump_tokens(&args[2], args[1] == "tokens-positioned");
            }
            "resolve-file" => {
                let (rows, ok) = modules::resolution_trace(&args[2]);
                for row in rows {
                    println!("{row}");
                }
                if !ok {
                    exit(1);
                }
                return;
            }
            "types-file" => {
                match checked_frontend(&args[2], modules::Mode::Normal) {
                    Ok((program, _, loaded)) => {
                        for row in check_trace::binding_types(&program, &loaded.files[0].src) {
                            println!("{row}");
                        }
                    }
                    Err(errors) => report_errors(&errors),
                }
                return;
            }
            "locals-file" => return dump_ast(&args[2], syntax::dump_locals),
            "ast-file" => return dump_ast(&args[2], syntax::dump_file_source),
            "ast-expr" => return dump_ast(&args[2], syntax::dump_expr_source),
            "ast-stmt" => return dump_ast(&args[2], syntax::dump_stmt_source),
            _ => {}
        }
    }
    if args.len() == 2 && args[1] == "lsp" {
        exit(lsp::run());
    }
    if args.len() >= 2 && args[1] == "fmt" {
        exit(fmt::run_cli(&args[2..]));
    }
    if args.len() == 3 && args[1] == "new" {
        match project::new_project(&args[2]) {
            Ok(()) => println!("created project: {}", args[2]),
            Err(e) => {
                eprintln!("{e}");
                exit(1);
            }
        }
        return;
    }
    if args.get(1).is_some_and(|cmd| matches!(cmd.as_str(), "build" | "run" | "test"))
        && (args.len() == 2 || (args.len() == 3 && args[2] == "--locked"))
    {
        match project::run(&args[1], args.len() == 3) {
            Ok(code) => exit(code),
            Err(e) => {
                eprintln!("{e}");
                exit(1);
            }
        }
    }
    if args.get(1).is_some_and(|cmd| cmd == "lock") {
        match project::lock(&args[2..]) {
            Ok(()) => return,
            Err(e) => {
                eprintln!("{e}");
                exit(if e.starts_with("usage") { 2 } else { 1 });
            }
        }
    }
    if args.len() >= 2 && matches!(args[1].as_str(), "add" | "remove") {
        let r = if args[1] == "add" { project::add(&args[2..]) } else { project::remove(&args[2..]) };
        match r {
            Ok(msg) => println!("{msg}"),
            Err(e) => {
                eprintln!("{e}");
                exit(if e.starts_with("usage") { 2 } else { 1 });
            }
        }
        return;
    }
    if args.len() >= 2 && args[1] == "doc" {
        exit(doc::run_cli(&args[2..]));
    }
    if args.len() >= 2 && args[1] == "obj" {
        if args.len() >= 4 && args[2] == "build" {
            // a module is only packed once it type-checks
            // (a library module has no `main`: that is the only error that does not count)
            if let Err(msgs) = checked_frontend(&args[3], modules::Mode::Normal) {
                let real: Vec<String> = msgs.into_iter().filter(|m| !m.starts_with("error: missing `fn main()`")).collect();
                if !real.is_empty() {
                    report_errors(&real);
                }
            }
        }
        exit(modobj::run_cli(&args[2..]));
    }
    if args.len() >= 2 && args[1] == "bench" {
        exit(bench::run_cli(&args[2..]));
    }
    if args.len() >= 2 && args[1] == "test" {
        exit(run_tests(&args[2..]));
    }
    compile_file(&args);
}

fn compile_file(args: &[String]) {
    if args.len() < 3 {
        eprintln!("{}", USAGE);
        exit(2);
    }
    let (cmd, file) = (args[1].as_str(), &args[2]);
    if let Err(e) = fs::metadata(file) {
        eprintln!("could not read {}: {}", file, e);
        exit(2);
    }
    let objs: Vec<String> = args.windows(2).filter(|w| w[0] == "--obj").map(|w| w[1].clone()).collect();
    if let Err(e) = modobj::install(&objs, file) {
        eprintln!("error: {e}");
        exit(1);
    }
    let (prog, sigs, source_files) = match frontend(file, modules::Mode::Normal) {
        Ok(x) => x,
        Err(msgs) => report_errors(&msgs),
    };
    // --native-debug-alloc: native build that reports allocations / frees / live blocks on stderr at exit
    let debug_alloc = args.iter().any(|a| a == "--native-debug-alloc");
    let debug = args.iter().any(|a| a == "--debug");
    let release = args.iter().any(|a| a == "--release");
    if debug && release {
        eprintln!("error: choose either --debug or --release");
        exit(2);
    }
    let no_opt = args.iter().any(|a| a == "--no-opt") || debug;
    if debug || args.iter().any(|a| a == "--dwarf") {
        // native Linux: symbols + DWARF source, frame and variable information for gdb (docs/PANIC_DIAGNOSTICS.md)
        ir::ALL_LOCS.store(true, std::sync::atomic::Ordering::Relaxed);
        x64::DWARF.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if debug || std::env::var_os("YERUX_BACKTRACE").is_some() || std::env::var_os("YERUX_LOCS").is_some() {
        cgen::FULL_LOCS.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if args.iter().any(|a| a == "--check-ranges") {
        ir::CHECK_RANGES.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let explicit_target = args
        .iter()
        .position(|a| a == "--target")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("");
    let want_native = debug_alloc || args.iter().any(|a| a == "--native");
    // On a Windows host the native backend produces Windows executables, and without any C compiler the
    // native backend is used instead of failing (`yerux doctor` explains what is missing).
    let host_windows = cfg!(windows) && matches!(explicit_target, "" | "windows");
    let auto_native = cmd != "c"
        && cmd != "check"
        && cmd != "ir"
        && !want_native
        && explicit_target.is_empty()
        && find_c_compiler().is_none();
    if auto_native {
        eprintln!(
            "note: no C compiler found, using the native backend ({})",
            if cfg!(windows) {
                "Windows .exe"
            } else {
                "x86-64 Linux"
            }
        );
    }
    let target = if explicit_target.is_empty() && (want_native || auto_native) && host_windows {
        "windows"
    } else {
        explicit_target
    };
    if !matches!(target, "" | "linux" | "aarch64" | "windows") {
        eprintln!(
            "unknown target: {} (expected linux, aarch64 or windows)",
            target
        );
        exit(2);
    }
    // Explicit cross targets select the corresponding native backend.
    let native = want_native || auto_native || matches!(target, "aarch64" | "windows");
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
    let start = Instant::now();
    let c = if native {
        String::new()
    } else if cmd == "c" {
        cgen::generate(&prog, sigs)
    } else {
        cgen::generate_with_files(&prog, sigs, &source_files)
    };
    if !native {
        profile_stage("C generation", start);
    }
    let build = |out: &str| {
        let start = Instant::now();
        let separate = args.iter().any(|a| a == "--separate");
        let r = if separate {
            c_compile_separate(&c, out, &prog.links, debug, &source_files)
        } else {
            c_compile_mode(&c, out, &prog.links, debug)
        };
        if let Err(e) = r {
            eprintln!("{}", e);
            exit(1);
        }
        profile_stage("C compiler", start);
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
                    if target == "windows" || cfg!(windows) {
                        format!("{}.exe", stem)
                    } else {
                        stem.to_string()
                    }
                });
            if native {
                native_build(&prog, &out, debug_alloc, !no_opt, target, &source_files);
            } else {
                build(&out);
            }
            println!("built: {}", out);
        }
        "run" => {
            let ext = if target == "windows" || cfg!(windows) {
                "exe"
            } else {
                "bin"
            };
            let exe = tmp_path(ext).to_string_lossy().to_string();
            if native {
                native_build(&prog, &exe, debug_alloc, !no_opt, target, &source_files);
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
            // `yerux run file.yx [flags] -- a b c`: what follows `--` is the program's own command line
            if let Some(dash) = args.iter().position(|a| a == "--") {
                cmd.args(&args[dash + 1..]);
            }
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
/// Build the executable of one `test`/`bench` block (the program whose `main` is that block's body).
fn build_block_exe(
    file: &str,
    mode: modules::Mode,
    native: bool,
    optimize: bool,
) -> Result<String, Vec<String>> {
    let (prog, sigs, _) = frontend(file, mode)?;
    let exe = tmp_path("bin").to_string_lossy().to_string();
    if native {
        let mut ir = ir::lower(&prog).map_err(|e| vec![format!("error: {}", e)])?;
        if optimize {
            opt::run(&mut ir);
        }
        let bytes = x64::compile_os(
            &ir,
            false,
            if cfg!(windows) {
                x64::Os::Windows
            } else {
                x64::Os::Linux
            },
        );
        fs::write(&exe, &bytes).map_err(|e| vec![format!("error: {}", e)])?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&exe, fs::Permissions::from_mode(0o755));
        }
    } else {
        c_compile(&cgen::generate(&prog, sigs), &exe, &prog.links)
            .map_err(|e| vec![format!("error: {}", e)])?;
    }
    Ok(exe)
}

fn run_one_test(
    file: &str,
    idx: usize,
    native: bool,
    optimize: bool,
) -> Result<Option<String>, Vec<String>> {
    let exe = build_block_exe(file, modules::Mode::Test(idx), native, optimize)?;
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
