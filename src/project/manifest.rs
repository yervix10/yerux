use semver::{Version, VersionReq};
use serde::Deserialize;
use std::{collections::BTreeMap, fs, path::Path};
use toml_edit::{DocumentMut, InlineTable, Item, Table, Value};

pub fn version(text: &str) -> Result<Version, semver::Error> {
    Version::parse(text.strip_prefix('v').unwrap_or(text))
}

pub fn requirement(text: &str) -> Result<VersionReq, String> {
    // Keep accepting v-prefixed versions in the original CLI syntax.
    let text = text
        .split(',')
        .map(|part| {
            if let Some((op, version)) = part.trim().split_once('v') {
                if op.trim().chars().all(|c| "<>=^~".contains(c)) {
                    return format!("{}{}", op.trim(), version);
                }
            }
            part.trim().to_owned()
        })
        .collect::<Vec<_>>()
        .join(",");
    VersionReq::parse(&text).map_err(|e| format!("invalid version requirement {text:?}: {e}"))
}

pub fn valid_package(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}

pub fn valid_dependency(name: &str) -> bool {
    name.as_bytes()
        .first()
        .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        && name != "std"
        && !crate::syntax::KW.contains(&name)
}

#[derive(Clone, Debug)]
pub struct Dependency {
    pub source: Location,
    pub tag: Option<String>,
    pub requirement: Option<VersionReq>,
}

#[derive(Clone, Debug)]
pub enum Location {
    Path(String),
    Git(String),
}

#[derive(Clone)]
pub struct Manifest {
    pub name: String,
    pub version: Version,
    pub dependencies: BTreeMap<String, Dependency>,
}

#[derive(Deserialize)]
struct File {
    package: Package,
    #[serde(default)]
    dependencies: BTreeMap<String, Entry>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    version: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Entry {
    Shorthand(String),
    Detailed(Options),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Options {
    path: Option<String>,
    git: Option<String>,
    tag: Option<String>,
    version: Option<String>,
}

impl Manifest {
    pub fn read(root: &Path) -> Result<Self, String> {
        let path = root.join("yerux.toml");
        let text = fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let file: File = toml_edit::de::from_str(text).map_err(|e| e.to_string())?;
        let name = file.package.name;
        if !valid_package(&name) {
            return Err(format!("invalid package name {name:?}"));
        }
        let version = version(&file.package.version)
            .map_err(|e| format!("invalid package version {:?}: {e}", file.package.version))?;
        let mut dependencies = BTreeMap::new();
        for (name, entry) in file.dependencies {
            if !valid_dependency(&name) {
                return Err(format!("invalid dependency name {name:?}"));
            }
            let (source, tag, req) = match entry {
                Entry::Shorthand(path) if !path.is_empty() => (Location::Path(path), None, None),
                Entry::Detailed(opts) => {
                    let source = match (opts.path, opts.git) {
                        (Some(path), None) if !path.is_empty() && opts.tag.is_none() => Location::Path(path),
                        (None, Some(url)) if !url.is_empty() && opts.tag.as_deref() != Some("") => Location::Git(url),
                        _ => return Err(format!("dependency {name} needs exactly one path or git source; tag applies only to git")),
                    };
                    let req = opts.version.as_deref().map(requirement).transpose()?;
                    (source, opts.tag, req)
                }
                _ => return Err(format!("dependency {name} needs a nonempty path")),
            };
            dependencies.insert(
                name,
                Dependency {
                    source,
                    tag,
                    requirement: req,
                },
            );
        }
        Ok(Self {
            name,
            version,
            dependencies,
        })
    }
}

pub fn edit(text: &str, name: &str, entry: Option<InlineTable>) -> Result<String, String> {
    let mut doc: DocumentMut = text.parse::<DocumentMut>().map_err(|err| err.to_string())?;
    if doc.get("dependencies").is_none() {
        doc["dependencies"] = Item::Table(Table::new());
    }
    let deps = doc["dependencies"]
        .as_table_like_mut()
        .ok_or("invalid [dependencies]")?;
    if let Some(entry) = entry {
        deps.insert(name, Item::Value(Value::InlineTable(entry)));
    } else if deps.remove(name).is_none() {
        return Err(format!("{name} is not a dependency"));
    }
    Ok(doc.to_string())
}
