//! Select an installed Yerux driver before entering the seed compiler's pipeline.
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

pub fn dispatch(args: &[String]) -> Option<i32> {
    // The formatter already owns its subcommand's `--selfhost` switch.
    if args.first().is_some_and(|arg| arg == "fmt") {
        return None;
    }
    let end = args
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(args.len());
    let selection = args[..end].iter().position(|arg| arg == "--selfhost")?;
    let mut forwarded = args[..end].to_vec();
    forwarded.remove(selection);
    let program_args = args.get(end + 1..).unwrap_or_default();
    let command = forwarded.first().map(String::as_str).unwrap_or("--help");
    if !matches!(
        command,
        "build" | "check" | "c" | "run" | "--stage2" | "--help" | "-h"
    ) || (command != "run" && end < args.len())
        || (command == "run"
            && forwarded
                .iter()
                .any(|arg| matches!(arg.as_str(), "-o" | "--emit-ast" | "--keep-c" | "--stage2")))
    {
        eprintln!("usage: yerux --selfhost <build|check|c|run|--stage2> ...  (run: pass program arguments after --)");
        return Some(2);
    }
    if !cfg!(target_os = "linux") {
        eprintln!("error: --selfhost requires Linux");
        return Some(2);
    }
    if forwarded.is_empty() {
        forwarded.push("--help".into());
    }
    let help = forwarded
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"));
    let status = if forwarded[0] == "run" && !help {
        run(&forwarded, program_args)
    } else {
        invoke(&forwarded, None)
    };
    Some(match status {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    })
}

fn driver() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("YERUX_SELFHOST").filter(|path| !path.is_empty()) {
        return std::path::absolute(path);
    }
    let current = env::current_exe()?;
    if let Some(dir) = current.parent() {
        for candidate in [dir.join("yerux-self"), dir.join("selfhost/yerux-self")] {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        if dir
            .file_name()
            .is_some_and(|name| matches!(name.to_str(), Some("debug" | "release")))
        {
            if let Some(target) = dir.parent() {
                let candidate = target.join("selfhost/yerux-self");
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    Ok(PathBuf::from("yerux-self"))
}

fn invoke(args: &[String], output: Option<&Path>) -> Result<ExitStatus, String> {
    let driver = driver().map_err(|e| format!("cannot locate yerux-self: {e}"))?;
    let mut process = Command::new(&driver);
    if args[0] == "run" {
        process.arg("build").args(&args[1..]);
    } else {
        process.args(args);
    }
    if let Some(output) = output {
        process.arg("-o").arg(output);
    }
    process.status().map_err(|error| {
        format!(
            "cannot start {}: {error}. Install with bash selfhost/bootstrap.sh target/selfhost or set YERUX_SELFHOST.",
            driver.display()
        )
    })
}

struct RunDirectory(PathBuf);

impl Drop for RunDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[String], program_args: &[String]) -> Result<ExitStatus, String> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    let dir = loop {
        let path = crate::tmp_path("selfhost");
        match builder.create(&path) {
            Ok(()) => break RunDirectory(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("cannot create {}: {error}", path.display())),
        }
    };
    let executable = dir.0.join("program");
    let built = invoke(args, Some(&executable))?;
    if !built.success() {
        return Ok(built);
    }
    Command::new(executable)
        .args(program_args)
        .status()
        .map_err(|error| format!("cannot run the program: {error}"))
}
