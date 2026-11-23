use super::resolve::{Kind, Pin};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

pub(super) fn is_revision(rev: &str) -> bool {
    matches!(rev.len(), 40 | 64) && rev.bytes().all(|c| c.is_ascii_hexdigit())
}

pub(super) struct Refs {
    pub head: Option<String>,
    pub tags: BTreeMap<String, String>,
}

impl Refs {
    pub(super) fn read(url: &str) -> Result<Self, String> {
        let mut cmd = Command::new("git");
        cmd.args(["ls-remote", "--", url, "HEAD", "refs/tags/*"]);
        let text = git(cmd)?;
        let mut refs = Self {
            head: None,
            tags: BTreeMap::new(),
        };
        for (rev, name) in text.lines().filter_map(|row| row.split_once('\t')) {
            if !is_revision(rev) {
                continue;
            }
            if name == "HEAD" {
                refs.head = Some(rev.into());
            } else if let Some(tag) = name.strip_prefix("refs/tags/") {
                if let Some(tag) = tag.strip_suffix("^{}") {
                    refs.tags.insert(tag.into(), rev.into());
                } else {
                    refs.tags.entry(tag.into()).or_insert_with(|| rev.into());
                }
            }
        }
        Ok(refs)
    }
}

pub(super) fn git_url(root: &Path, pin: &Pin) -> String {
    if pin.kind == Kind::GitLocal {
        root.join(&pin.source).to_string_lossy().into_owned()
    } else {
        pin.source.clone()
    }
}

fn git(mut cmd: Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| format!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {}: {}",
            cmd.get_args().next().unwrap_or_default().to_string_lossy(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("invalid git output: {e}"))
}

fn at(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir);
    cmd
}

pub(super) fn checkout(root: &Path, pin: &Pin) -> Result<PathBuf, String> {
    let rev = pin
        .rev
        .as_deref()
        .filter(|rev| is_revision(rev))
        .ok_or("invalid git revision")?;
    // A stable cache path also keeps in-repository path dependencies portable.
    let hash = pin
        .source
        .bytes()
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
    let parent = root
        .join(".yerux/git")
        .join(if pin.kind == Kind::Git {
            "remote"
        } else {
            "local"
        })
        .join(format!("{hash:016x}"));
    let dir = parent.join(rev);
    if dir.is_dir() {
        let mut cmd = at(&dir);
        cmd.args(["status", "--porcelain"]);
        let clean = git(cmd)?.is_empty();
        let mut cmd = at(&dir);
        cmd.args(["rev-parse", "HEAD"]);
        if !clean || git(cmd)?.trim() != rev {
            return Err(format!("modified package checkout at {}", dir.display()));
        }
        return canonical(&dir);
    }
    let stage = Temp::directory(&parent)?;
    let mut cmd = at(&stage.0);
    cmd.args(["init", "--quiet"]);
    if rev.len() == 64 {
        cmd.arg("--object-format=sha256");
    }
    git(cmd)?;
    let url = git_url(root, pin);
    let fetch = |reference: &str| {
        let mut cmd = at(&stage.0);
        cmd.args([
            "fetch",
            "--quiet",
            "--depth=1",
            "--no-tags",
            "--",
            &url,
            reference,
        ]);
        git(cmd)
    };
    if fetch(rev).is_err() {
        let reference = pin
            .tag
            .as_ref()
            .map(|tag| format!("refs/tags/{tag}"))
            .unwrap_or_else(|| "HEAD".into());
        fetch(&reference)?;
    }
    let mut cmd = at(&stage.0);
    cmd.args(["rev-parse", "FETCH_HEAD^{commit}"]);
    if git(cmd)?.trim() != rev {
        return Err(format!(
            "git source {} no longer provides locked revision {rev}",
            pin.source
        ));
    }
    let mut cmd = at(&stage.0);
    cmd.args(["checkout", "--quiet", "--detach", rev, "--"]);
    git(cmd)?;
    fs::rename(&stage.0, &dir).map_err(|e| format!("cannot cache {}: {e}", dir.display()))?;
    canonical(&dir)
}

pub(super) fn canonical(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))
}

pub(super) fn relative(base: &Path, path: &Path) -> Result<String, String> {
    use std::path::Component;
    let base: Vec<_> = base.components().collect();
    let mut path_parts = Vec::new();
    for part in path.components() {
        if part == Component::ParentDir && matches!(path_parts.last(), Some(Component::Normal(_))) {
            path_parts.pop();
        } else if part != Component::CurDir {
            path_parts.push(part);
        }
    }
    let prefix = base
        .iter()
        .zip(&path_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let path = if prefix == 0 {
        path_parts.into_iter().collect::<PathBuf>()
    } else {
        base[prefix..]
            .iter()
            .map(|_| Component::ParentDir)
            .chain(path_parts[prefix..].iter().copied())
            .collect()
    };
    path.to_str()
        .map(|s| {
            if s.is_empty() {
                ".".into()
            } else {
                s.replace(std::path::MAIN_SEPARATOR, "/")
            }
        })
        .ok_or_else(|| "package path must be UTF-8".into())
}

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
struct Temp(PathBuf);

impl Temp {
    fn directory(parent: &Path) -> Result<Self, String> {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        loop {
            let path = parent.join(format!(
                ".stage-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("cannot create {}: {e}", path.display())),
            }
        }
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn atomic_write(path: &Path, contents: &str) -> Result<(), String> {
    let dir = Temp::directory(path.parent().unwrap())?;
    let file = dir.0.join("new");
    let write = || -> io::Result<()> {
        let mut tmp = fs::File::create(&file)?;
        tmp.write_all(contents.as_bytes())?;
        if let Ok(metadata) = fs::metadata(path) {
            tmp.set_permissions(metadata.permissions())?;
        }
        tmp.sync_all()?;
        drop(tmp);
        fs::rename(&file, path)
    };
    write().map_err(|e| format!("cannot write {}: {e}", path.display()))
}
