//! The representation of the installed toolchain and its components.
//! `Components` and `DirectoryPackage` are the two sides of the
//! installation / uninstallation process.

use std::borrow::Cow;
use std::convert::Infallible;
use std::fmt;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Result, bail};

use crate::dist::component::package::{INSTALLER_VERSION, VERSION_FILE};
use crate::dist::component::transaction::Transaction;
use crate::dist::prefix::InstallPrefix;
use crate::errors::RustupError;
use crate::process::Process;
use crate::utils;

const COMPONENTS_FILE: &str = "components";

#[derive(Clone, Debug)]
pub struct Components {
    prefix: InstallPrefix,
}

impl Components {
    pub fn open(prefix: InstallPrefix) -> Result<Self> {
        let c = Self { prefix };

        // Validate that the metadata uses a format we know
        if let Some(v) = c.read_version()?
            && v != INSTALLER_VERSION
        {
            bail!(
                "unsupported metadata version in existing installation: {}",
                v
            );
        }

        Ok(c)
    }
    fn rel_components_file(&self) -> PathBuf {
        self.prefix.rel_manifest_file(COMPONENTS_FILE)
    }
    fn rel_component_manifest(&self, name: &str) -> PathBuf {
        self.prefix.rel_manifest_file(&format!("manifest-{name}"))
    }
    fn read_version(&self) -> Result<Option<String>> {
        let p = self.prefix.manifest_file(VERSION_FILE);
        if utils::is_file(&p) {
            Ok(Some(utils::read_file(VERSION_FILE, &p)?.trim().to_string()))
        } else {
            Ok(None)
        }
    }
    fn write_version(&self, tx: &mut Transaction<'_>) -> Result<()> {
        tx.modify_file(self.prefix.rel_manifest_file(VERSION_FILE))?;
        utils::write_file(
            VERSION_FILE,
            &self.prefix.manifest_file(VERSION_FILE),
            INSTALLER_VERSION,
        )?;

        Ok(())
    }
    pub fn list(&self) -> Result<Vec<Component>> {
        let path = self.prefix.abs_path(self.rel_components_file());
        if !utils::is_file(&path) {
            return Ok(Vec::new());
        }
        let content = utils::read_file("components", &path)?;
        Ok(content
            .lines()
            .map(|s| Component {
                components: self.clone(),
                name: s.to_owned(),
            })
            .collect())
    }
    pub(crate) fn add<'a>(&self, name: &str, tx: Transaction<'a>) -> ComponentBuilder<'a> {
        ComponentBuilder {
            components: self.clone(),
            name: name.to_owned(),
            parts: Vec::new(),
            tx,
        }
    }
    pub fn find(&self, name: &str) -> Result<Option<Component>> {
        let result = self.list()?;
        Ok(result.into_iter().find(|c| (c.name() == name)))
    }
    pub(crate) fn prefix(&self) -> InstallPrefix {
        self.prefix.clone()
    }
}

pub(crate) struct ComponentBuilder<'a> {
    components: Components,
    name: String,
    parts: Vec<ComponentPart>,
    tx: Transaction<'a>,
}

impl<'a> ComponentBuilder<'a> {
    pub(crate) fn copy_file(&mut self, path: PathBuf, src: &Path) -> Result<()> {
        self.parts.push(ComponentPart {
            kind: ComponentPartKind::File,
            path: path.clone(),
        });
        self.tx.copy_file(&self.name, path, src)
    }
    pub(crate) fn copy_dir(&mut self, path: PathBuf, src: &Path) -> Result<()> {
        self.parts.push(ComponentPart {
            kind: ComponentPartKind::Dir,
            path: path.clone(),
        });
        self.tx.copy_dir(&self.name, path, src)
    }
    pub(crate) fn move_file(&mut self, path: PathBuf, src: &Path) -> Result<()> {
        self.parts.push(ComponentPart {
            kind: ComponentPartKind::File,
            path: path.clone(),
        });
        self.tx.move_file(&self.name, path, src)
    }
    pub(crate) fn move_dir(&mut self, path: PathBuf, src: &Path) -> Result<()> {
        self.parts.push(ComponentPart {
            kind: ComponentPartKind::Dir,
            path: path.clone(),
        });
        self.tx.move_dir(&self.name, path, src)
    }
    pub(crate) fn finish(mut self) -> Result<Transaction<'a>> {
        // Write component manifest
        let path = self.components.rel_component_manifest(&self.name);
        let abs_path = self.components.prefix.abs_path(&path);
        let mut file = BufWriter::new(self.tx.add_file(&self.name, path)?);
        for part in self.parts {
            // FIXME: This writes relative paths to the component manifest,
            // but rust-installer writes absolute paths.
            utils::write_line("component", &mut file, &abs_path, &part.encode())?;
        }

        // Add component to components file
        let path = self.components.rel_components_file();
        let abs_path = self.components.prefix.abs_path(&path);
        self.tx.modify_file(path)?;
        utils::append_file("components", &abs_path, &self.name)?;

        // Drop in the version file for future use
        self.components.write_version(&mut self.tx)?;

        Ok(self.tx)
    }
}

#[derive(Debug)]
pub struct ComponentPart {
    /// Kind of the [`ComponentPart`], such as `"file"` or `"dir"`.
    pub kind: ComponentPartKind,
    /// Relative path of the [`ComponentPart`],
    /// with components separated by the system's main path separator.
    pub path: PathBuf,
}

#[derive(Debug, PartialEq)]
pub enum ComponentPartKind {
    File,
    Dir,
    Unknown(String),
}

impl fmt::Display for ComponentPartKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File => write!(f, "file"),
            Self::Dir => write!(f, "dir"),
            Self::Unknown(s) => write!(f, "{s}"),
        }
    }
}

impl FromStr for ComponentPartKind {
    type Err = Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "file" => Ok(Self::File),
            "dir" => Ok(Self::Dir),
            s => Ok(Self::Unknown(s.to_owned())),
        }
    }
}

impl ComponentPart {
    const PATH_SEP_MANIFEST: &str = "/";
    const PATH_SEP_MAIN: &str = std::path::MAIN_SEPARATOR_STR;

    pub(crate) fn encode(&self) -> String {
        let mut buf = self.kind.to_string();
        buf.push(':');
        // Lossy conversion is safe here because we assume that `path` comes from
        // `ComponentPart::decode()`, i.e. from calling `Path::from()` on a `&str`.
        let mut path = self.path.to_string_lossy();
        if Self::PATH_SEP_MAIN != Self::PATH_SEP_MANIFEST {
            path = Cow::Owned(path.replace(Self::PATH_SEP_MAIN, Self::PATH_SEP_MANIFEST));
        };
        buf.push_str(&path);
        buf
    }

    pub(crate) fn decode(line: &str) -> Option<Self> {
        let pos = line.find(':')?;
        let mut path_str = Cow::Borrowed(&line[(pos + 1)..]);
        if Self::PATH_SEP_MANIFEST != Self::PATH_SEP_MAIN {
            path_str = Cow::Owned(path_str.replace(Self::PATH_SEP_MANIFEST, Self::PATH_SEP_MAIN));
        };
        Some(Self {
            // FIXME: Use `.into_ok()` when it's available.
            kind: line[0..pos].parse().unwrap(),
            path: PathBuf::from(path_str.as_ref()),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Component {
    components: Components,
    name: String,
}

impl Component {
    pub(crate) fn manifest_name(&self) -> String {
        format!("manifest-{}", &self.name)
    }
    pub(crate) fn manifest_file(&self) -> PathBuf {
        self.components.prefix.manifest_file(&self.manifest_name())
    }
    pub(crate) fn rel_manifest_file(&self) -> PathBuf {
        self.components
            .prefix
            .rel_manifest_file(&self.manifest_name())
    }
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn parts(&self) -> Result<Vec<ComponentPart>> {
        let mut result = Vec::new();
        for line in utils::read_file("component", &self.manifest_file())?.lines() {
            result.push(
                ComponentPart::decode(line)
                    .ok_or_else(|| RustupError::CorruptComponent(self.name.clone()))?,
            );
        }
        Ok(result)
    }
    pub fn uninstall<'a>(
        &self,
        mut tx: Transaction<'a>,
        process: &Process,
    ) -> Result<Transaction<'a>> {
        // Update components file
        let path = self.components.rel_components_file();
        let abs_path = self.components.prefix.abs_path(&path);
        let temp = tx.temp().new_file()?;
        utils::filter_file("components", &abs_path, &temp, |l| l != self.name)?;
        tx.modify_file(path)?;
        utils::rename("components", &temp, &abs_path, tx.notify_handler(), process)?;

        // TODO: If this is the last component remove the components file
        // and the version file.

        // Track visited directories
        use std::collections::HashSet;
        use std::collections::hash_set::IntoIter;
        use std::fs::read_dir;

        // dirs will contain the set of longest disjoint directory paths seen
        // ancestors help in filtering seen paths and constructing dirs
        // All seen paths must be relative to avoid surprises
        struct PruneSet {
            dirs: HashSet<PathBuf>,
            ancestors: HashSet<PathBuf>,
            prefix: PathBuf,
        }

        impl PruneSet {
            fn seen(&mut self, mut path: PathBuf) {
                if !path.is_relative() || !path.pop() {
                    return;
                }
                if self.dirs.contains(&path) || self.ancestors.contains(&path) {
                    return;
                }
                self.dirs.insert(path.clone());
                while path.pop() {
                    if path.file_name().is_none() {
                        break;
                    }
                    if self.dirs.contains(&path) {
                        self.dirs.remove(&path);
                    }
                    if self.ancestors.contains(&path) {
                        break;
                    }
                    self.ancestors.insert(path.clone());
                }
            }
        }

        struct PruneIter {
            iter: IntoIter<PathBuf>,
            path_buf: Option<PathBuf>,
            prefix: PathBuf,
        }

        impl IntoIterator for PruneSet {
            type Item = PathBuf;
            type IntoIter = PruneIter;

            fn into_iter(self) -> Self::IntoIter {
                PruneIter {
                    iter: self.dirs.into_iter(),
                    path_buf: None,
                    prefix: self.prefix,
                }
            }
        }

        // Returns only empty directories
        impl Iterator for PruneIter {
            type Item = PathBuf;

            fn next(&mut self) -> Option<Self::Item> {
                self.path_buf = match self.path_buf {
                    None => self.iter.next(),
                    Some(_) => {
                        let mut path_buf = self.path_buf.take().unwrap();
                        match path_buf.file_name() {
                            Some(_) => {
                                if path_buf.pop() {
                                    Some(path_buf)
                                } else {
                                    None
                                }
                            }
                            None => self.iter.next(),
                        }
                    }
                };
                self.path_buf.as_ref()?;
                let full_path = self.prefix.join(self.path_buf.as_ref().unwrap());
                let empty = match read_dir(full_path) {
                    Ok(dir) => dir.count() == 0,
                    Err(_) => false,
                };
                if empty {
                    self.path_buf.clone()
                } else {
                    // No dir above can be empty, go to next path in dirs
                    self.path_buf = None;
                    self.next()
                }
            }
        }

        // Remove parts
        let mut pset = PruneSet {
            dirs: HashSet::new(),
            ancestors: HashSet::new(),
            prefix: self.components.prefix.abs_path(""),
        };
        for part in self.parts()?.into_iter().rev() {
            match part.kind {
                ComponentPartKind::File => tx.remove_file(&self.name, part.path.clone())?,
                ComponentPartKind::Dir => tx.remove_dir(&self.name, part.path.clone())?,
                _ => return Err(RustupError::CorruptComponent(self.name.clone()).into()),
            }
            pset.seen(part.path);
        }
        for empty_dir in pset {
            tx.remove_dir(&self.name, empty_dir)?;
        }

        // Remove component manifest
        tx.remove_file(&self.name, self.rel_manifest_file())?;

        Ok(tx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_component_part() {
        let part = ComponentPart::decode("dir:share/doc/rust/html").unwrap();
        assert_eq!(part.kind, ComponentPartKind::Dir);
        assert_eq!(
            part.path,
            Path::new(&"share/doc/rust/html".replace("/", ComponentPart::PATH_SEP_MAIN))
        );
    }

    #[test]
    fn encode_component_part() {
        let part = ComponentPart {
            kind: ComponentPartKind::Dir,
            path: ["share", "doc", "rust", "html"].into_iter().collect(),
        };
        assert_eq!(part.encode(), "dir:share/doc/rust/html");
    }
}
