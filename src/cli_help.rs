//! `yerux --help`, `yerux help [COMMAND]` and `yerux COMMAND --help`: one table describes every subcommand, and
//! docs/CLI.md is written from the same words. Help goes to stdout and exits 0; a wrong command line prints the usage
//! to stderr and exits 2.

/// (command, one-line summary, usage lines, details)
const COMMANDS: &[(&str, &str, &str, &str)] = &[
    ("check", "type- and ownership-check a program; no output file", "yerux check FILE.yx", "Prints `ok: no type or ownership errors` or the diagnostics (exit 1). Follows `import`s relative to FILE."),
    ("c", "print the generated C", "yerux c FILE.yx", "The C backend's translation unit on stdout (needs no C compiler)."),
    ("ir", "print the optimized IR of the native backend", "yerux ir FILE.yx", "A textual dump of the IR the native x86-64/AArch64/Windows backends compile."),
    (
        "build",
        "compile a program (or the project in the current directory) to an executable",
        "yerux build FILE.yx [-o OUT] [--native] [--target linux|aarch64|windows] [--debug|--release] [--dwarf] [--no-opt] [--native-debug-alloc] [--obj MOD.yxo] [--separate]\n       yerux build [--locked]",
        "Without --native the C backend is used (the C compiler is $YERUX_CC, else the first of cc, gcc, clang that runs; without one the native backend is used). --native emits a static ELF/PE directly. --target aarch64 builds a Linux AArch64 ELF, --target windows a PE32+ .exe. --debug is a native build with DWARF and no optimization, --release the default. --native-debug-alloc reports allocations on stderr at exit. With no FILE and a yerux.toml in the directory the project is built (--locked: fail when yerux.lock is out of date).",
    ),
    (
        "run",
        "compile and run a program (or the project in the current directory)",
        "yerux run FILE.yx [build flags] [-- ARGS...]\n       yerux run [--locked]",
        "Same flags as `build`. Everything after `--` is passed to the program; the program's exit status is yerux's.",
    ),
    (
        "test",
        "run the `test \"name\" { ... }` blocks of files, directories or the current project",
        "yerux test [FILE.yx|DIR ...] [--native] [--no-opt] [--filter TEXT] [--locked]",
        "Exit 0 when every test passes. --filter keeps the tests whose name contains TEXT.",
    ),
    ("fmt", "format source files in place (the only style)", "yerux fmt [--check|--stdout] FILE.yx|DIR ...", "--check lists the files that would change and exits 1; --stdout prints the formatted text of one file. `target/` and hidden directories are skipped. Formatting twice gives the same text and never changes what a program compiles to."),
    ("doc", "generate API documentation from `///` comments", "yerux doc [--html] [-o DIR] [DIR|FILE.yx ...]", "One page per module plus an index, Markdown by default (HTML with --html), for the `pub` items and the methods of public types; `//!` lines or the leading comment block describe the module. Default output directory: target/doc."),
    ("lsp", "language server on stdin/stdout", "yerux lsp", "Diagnostics, hover, definition, references, rename, completion, signature help, document symbols, formatting and semantic tokens (docs/LSP.md)."),
    ("bench", "time the `bench \"name\" { ... }` blocks of programs", "yerux bench [FILE.yx|DIR ...] [--native] [--no-opt] [--filter TEXT] [--runs N]", "Runs each program once as a warm-up (a failing exit status is a failure) and then N timed runs (default 5); prints min, median and mean wall time."),
    ("new", "create a project", "yerux new NAME", "Creates NAME/ with yerux.toml, src/main.yx and a tests/ directory."),
    ("add", "add a dependency to yerux.toml", "yerux add NAME --path DIR [--version REQ]\n       yerux add NAME --git URL [--tag TAG | --version REQ]", "Updates yerux.lock."),
    ("remove", "remove a dependency", "yerux remove NAME", "Updates yerux.toml and yerux.lock."),
    ("lock", "write or verify yerux.lock", "yerux lock [--locked | --update]", "--locked fails if the lock file would change; --update re-resolves every dependency."),
    ("obj", "separately compiled module objects", "yerux obj build MOD.yx [-o OUT.yxo] | obj info MOD.yxo | obj check MOD.yxo [--root DIR]", "Packs a module that type-checks into a .yxo; `build --obj` links against it (docs/MODULE_OBJECT.md)."),
    ("version", "print the version", "yerux --version | -V | version", ""),
    ("doctor", "check the C compiler, the backends and the paths", "yerux doctor", "Exit 0 when a backend is usable."),
    ("tokens", "debug: dump the lexer's tokens", "yerux tokens FILE.yx | tokens-positioned FILE.yx", "Also: ast-file, ast-expr, ast-stmt, resolve-file, locals-file and types-file FILE.yx dump the front end's internal views (used by the self-hosting tests)."),
];

fn find(name: &str) -> Option<&'static (&'static str, &'static str, &'static str, &'static str)> {
    let name = match name {
        "-V" | "--version" => "version",
        "tokens-positioned" | "ast-file" | "ast-expr" | "ast-stmt" | "resolve-file" | "locals-file" | "types-file" => "tokens",
        n => n,
    };
    COMMANDS.iter().find(|c| c.0 == name)
}

/// The overview printed by `yerux --help`.
pub fn overview() -> String {
    let mut s = String::from("yerux: the Yerux compiler and tools\n\nusage: yerux COMMAND [ARGS]\n       yerux help [COMMAND]\n       yerux COMMAND --help\n\ncommands:\n");
    for (name, summary, _, _) in COMMANDS {
        s.push_str(&format!("  {:<8} {}\n", name, summary));
    }
    s.push_str("\nexit status: 0 success, 1 the program or the check failed (diagnostics on stderr), 2 a bad command line.\nenvironment: YERUX_CC (C compiler), YERUX_LOCS / YERUX_BACKTRACE (source locations in panics), YERUX_LSP_DEBOUNCE_MS.\n`yerux --selfhost COMMAND ...` runs the self-hosted front end (docs/SELFHOST.md).\n");
    s
}

/// The help of one command, or None when there is no such command.
pub fn command(name: &str) -> Option<String> {
    let (n, summary, usage, details) = find(name)?;
    let mut s = format!("yerux {}: {}\n\nusage: {}\n", n, summary, usage);
    if !details.is_empty() {
        s.push_str(&format!("\n{}\n", details));
    }
    Some(s)
}

/// `usage: ...` lines of one command (for error messages).
pub fn usage_of(name: &str) -> Option<String> {
    find(name).map(|c| format!("usage: {}", c.2))
}

/// The usage for a wrong command line (stderr).
pub fn usage() -> String {
    "usage: yerux COMMAND [ARGS]   (yerux --help lists the commands, yerux COMMAND --help describes one)".to_string()
}

/// Handles `--help`, `-h`, `help [COMMAND]` and `COMMAND --help|-h`; Some(exit status) when it did.
pub fn dispatch(args: &[String]) -> Option<i32> {
    let first = args.first()?.as_str();
    if matches!(first, "--help" | "-h") || (first == "help" && args.len() == 1) {
        print!("{}", overview());
        return Some(0);
    }
    if first == "help" {
        return Some(match command(&args[1]) {
            Some(text) => {
                print!("{}", text);
                0
            }
            None => {
                eprintln!("yerux: no command '{}'\n{}", args[1], usage());
                2
            }
        });
    }
    // `yerux run prog.yx -- --help` is the program's flag
    let own = args[1..].iter().take_while(|a| a.as_str() != "--");
    if find(first).is_some() && own.clone().any(|a| a == "--help" || a == "-h") {
        print!("{}", command(first)?);
        return Some(0);
    }
    None
}
