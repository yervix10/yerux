mod manifest;
mod resolve;
mod store;

use manifest::Manifest;
pub use resolve::Graph;
use resolve::Mode;
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};
use store::{atomic_write, canonical};
use toml_edit::{InlineTable, Value};

fn guard(root: &Path) -> Result<File, String> {
    let cache = root.join(".yerux");
    fs::create_dir_all(&cache).map_err(|e| format!("cannot create {}: {e}", cache.display()))?;
    let lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(cache.join("resolve.lock"))
        .map_err(|e| format!("cannot open package lock: {e}"))?;
    lock.lock()
        .map_err(|e| format!("cannot lock project: {e}"))?;
    Ok(lock)
}

fn root() -> Result<PathBuf, String> {
    canonical(&std::env::current_dir().map_err(|e| e.to_string())?)
}

fn load(root: &Path, mode: Mode) -> Result<Graph, String> {
    let _guard = guard(root)?;
    let (graph, lock) = resolve::resolve(root, Manifest::read(root)?, mode)?;
    if let Some(lock) = lock {
        atomic_write(&root.join("yerux.lock"), &lock)?;
    }
    Ok(graph)
}

pub fn new_project(name: &str) -> Result<(), String> {
    let root = Path::new(name);
    let package_name = root.file_name().and_then(|s| s.to_str()).unwrap_or(name);
    if !manifest::valid_package(package_name) {
        return Err(format!("invalid package name {package_name:?}"));
    }
    fs::create_dir_all(root.join("src")).map_err(|e| e.to_string())?;
    fs::create_dir_all(root.join("tests")).map_err(|e| e.to_string())?;
    fs::write(
        root.join("yerux.toml"),
        format!("[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\n\n[dependencies]\n"),
    )
    .map_err(|e| e.to_string())?;
    fs::write(
        root.join("src/main.yx"),
        "fn main() {\n    print(\"Hello from Yerux!\")\n}\n",
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn lock(args: &[String]) -> Result<(), String> {
    let mode = match args {
        [] => Mode::Reuse,
        [flag] if flag == "--locked" => Mode::Locked,
        [flag] if flag == "--update" => Mode::Update,
        _ => return Err("usage: yerux lock [--locked | --update]".into()),
    };
    load(&root()?, mode)?;
    Ok(())
}

pub fn run(command: &str, locked: bool) -> Result<i32, String> {
    let root = root()?;
    let graph = load(&root, if locked { Mode::Locked } else { Mode::Reuse })?;
    let name = graph.manifest.name.clone();
    if command == "test" {
        let dir = root.join("tests");
        let mut files: Vec<PathBuf> = fs::read_dir(&dir)
            .map_err(|e| format!("cannot read tests/: {e}"))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "yx"))
            .collect();
        files.sort();
        if files.is_empty() {
            println!("no tests found");
            return Ok(0);
        }
        let args = files
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        return Ok(crate::run_tests(&args));
    }
    let entry = root.join("src/main.yx");
    if !entry.is_file() {
        return Err("missing src/main.yx".into());
    }
    let mut args = vec![
        "yerux".into(),
        command.into(),
        entry.to_string_lossy().into_owned(),
    ];
    if command == "build" {
        let bin = root.join("target").join(&name);
        fs::create_dir_all(bin.parent().unwrap()).map_err(|e| e.to_string())?;
        args.extend(["-o".into(), bin.to_string_lossy().into_owned()]);
    }
    crate::compile_file(&args);
    Ok(0)
}

fn edit(root: &Path, old: &str, new: &str) -> Result<Graph, String> {
    let (graph, lock) = resolve::resolve(root, Manifest::parse(new)?, Mode::Reuse)?;
    let manifest = root.join("yerux.toml");
    atomic_write(&manifest, new)?;
    if let Some(lock) = lock {
        if let Err(error) = atomic_write(&root.join("yerux.lock"), &lock) {
            if let Err(restore) = atomic_write(&manifest, old) {
                return Err(format!("{error}; restoring yerux.toml failed: {restore}"));
            }
            return Err(error);
        }
    }
    Ok(graph)
}

/// Add a path or git dependency and resolve the edited manifest before publishing it.
pub fn add(args: &[String]) -> Result<String, String> {
    let usage = "usage: yerux add NAME --path DIR [--version REQ]\n       yerux add NAME --git URL [--tag TAG | --version REQ]";
    let mut name = None;
    let (mut path, mut git, mut tag, mut version) = (None, None, None, None);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let slot = match arg.as_str() {
            "--path" => &mut path,
            "--git" => &mut git,
            "--tag" => &mut tag,
            "--version" => &mut version,
            a if a.starts_with("--") => return Err(format!("usage: unknown option {a}\n{usage}")),
            _ if name.is_none() => {
                name = Some(arg.as_str());
                continue;
            }
            _ => return Err(usage.into()),
        };
        let val = args
            .next()
            .filter(|value| !value.starts_with("--"))
            .ok_or_else(|| format!("usage: {arg} needs a value\n{usage}"))?;
        if slot.replace(val.as_str()).is_some() {
            return Err(format!("usage: repeated {arg}\n{usage}"));
        }
    }
    let name = name.ok_or(usage)?;
    if !manifest::valid_dependency(name) {
        return Err(format!("invalid dependency name {name:?}"));
    }
    if path.is_some() == git.is_some() {
        return Err(format!(
            "usage: give exactly one of --path and --git\n{usage}"
        ));
    }
    if tag.is_some() && (version.is_some() || path.is_some()) {
        return Err("usage: --tag applies to --git without --version".into());
    }
    if let Some(version) = version {
        manifest::requirement(version)?;
    }
    let root = root()?;
    let _guard = guard(&root)?;
    let text = fs::read_to_string(root.join("yerux.toml"))
        .map_err(|e| format!("cannot read yerux.toml: {e}"))?;
    Manifest::parse(&text)?;
    let mut entry = InlineTable::new();
    let default_tag;
    if let Some(path) = path {
        entry.insert("path", Value::from(path));
    } else if let Some(url) = git {
        entry.insert("git", Value::from(url));
        if tag.is_none() && version.is_none() {
            default_tag = resolve::latest_tag(&root, url)?;
            tag = default_tag.as_deref();
        }
        if let Some(tag) = tag {
            entry.insert("tag", Value::from(tag));
        }
    }
    if let Some(version) = version {
        entry.insert("version", Value::from(version));
    }
    entry.fmt();
    let new = manifest::edit(&text, name, Some(entry))?;
    let graph = edit(&root, &text, &new)?;
    if let Some(path) = path {
        Ok(format!(
            "added {name} {} (path {path})",
            graph.version(name).unwrap()
        ))
    } else {
        let selected = graph
            .tag(name)
            .map(|tag| format!("tag {tag}"))
            .unwrap_or_else(|| "HEAD".into());
        Ok(format!("added {name} (git {}, {selected})", git.unwrap()))
    }
}

pub fn remove(args: &[String]) -> Result<String, String> {
    let [name] = args else {
        return Err("usage: yerux remove NAME".into());
    };
    let root = root()?;
    let _guard = guard(&root)?;
    let text = fs::read_to_string(root.join("yerux.toml"))
        .map_err(|e| format!("cannot read yerux.toml: {e}"))?;
    let new = manifest::edit(&text, name, None)?;
    edit(&root, &text, &new)?;
    Ok(format!("removed {name}"))
}
