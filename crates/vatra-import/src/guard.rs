use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Files that hold credentials or login state, by name. Matched without regard to case, as Windows does.
const FORBIDDEN_NAMES: [&str; 4] = [
    ".credentials.json",
    "auth.json",
    "mcp-needs-auth-cache.json",
    "mcp-oauth-locks",
];

/// The config folders of Claude Code and Codex by their default names. A name with `credential` or `token` that sits
/// directly inside one of them is refused.
const CONFIG_FOLDERS: [&str; 2] = [".claude", ".codex"];

/// Why a path could not be read through the guard.
#[derive(Debug, thiserror::Error)]
pub enum GuardError {
    #[error("{} holds credentials and is not read", path.display())]
    Forbidden { path: PathBuf },
    #[error("cannot read {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl GuardError {
    /// True when the path does not exist, which callers treat as "nothing there" rather than a problem.
    pub fn is_missing(&self) -> bool {
        matches!(self, GuardError::Io { source, .. } if source.kind() == io::ErrorKind::NotFound)
    }
}

/// The files of a folder walk: those read, and those left out because the guard refuses them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Walk {
    pub files: Vec<PathBuf>,
    pub skipped: Vec<PathBuf>,
}

/// Whether a path names a credential file: an exact forbidden name anywhere in the path, or a name with `credential`
/// or `token` directly inside a config folder. The check reads the path as written, so `..` cannot hide a name.
pub fn is_forbidden(path: &Path) -> bool {
    let mut after_config = false;
    for part in path.components() {
        let Component::Normal(name) = part else {
            continue;
        };
        let name = name.to_string_lossy().to_lowercase();
        if FORBIDDEN_NAMES.contains(&name.as_str()) {
            return true;
        }
        if after_config && is_secret_name(&name) {
            return true;
        }
        after_config = CONFIG_FOLDERS.contains(&name.as_str());
    }
    false
}

fn is_secret_name(lowercase: &str) -> bool {
    lowercase.contains("credential") || lowercase.contains("token")
}

/// The only way `vatra-import` reads the file system. Every path is checked as written and again after links are
/// followed, so a link into a forbidden file is refused too.
#[derive(Clone, Debug, Default)]
pub struct GuardedFs {
    config_roots: Vec<PathBuf>,
}

impl GuardedFs {
    pub fn new() -> GuardedFs {
        GuardedFs::default()
    }

    /// The same guard, also refusing a `credential` or `token` name directly inside each of `roots`. Used for the
    /// config folders a scan is given, which need not be called `.claude` or `.codex`.
    pub fn with_config_roots(&self, roots: &[PathBuf]) -> GuardedFs {
        let mut guard = self.clone();
        guard.config_roots.extend(roots.iter().cloned());
        guard
    }

    pub fn read_to_string(&self, path: &Path) -> Result<String, GuardError> {
        self.check(path)?;
        fs::read_to_string(path).map_err(|source| io_error(path, source))
    }

    /// Opens a guarded stream for large conversation logs; credential checks are identical to text reads.
    pub fn open(&self, path: &Path) -> Result<fs::File, GuardError> {
        self.check(path)?;
        fs::File::open(path).map_err(|source| io_error(path, source))
    }

    /// The entries of a folder, sorted. Entries are returned even when forbidden; reading them is refused.
    pub fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>, GuardError> {
        self.check(path)?;
        let entries = fs::read_dir(path).map_err(|source| io_error(path, source))?;
        let mut children = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| io_error(path, source))?;
            children.push(entry.path());
        }
        children.sort();
        Ok(children)
    }

    pub fn metadata(&self, path: &Path) -> Result<fs::Metadata, GuardError> {
        self.check(path)?;
        fs::metadata(path).map_err(|source| io_error(path, source))
    }

    /// Every file under a folder, not following links into folders. Forbidden entries are left out and listed in
    /// `skipped`; the walk does not fail for them. A folder that cannot be read fails the walk.
    pub fn walk(&self, root: &Path) -> Result<Walk, GuardError> {
        self.check(root)?;
        let mut walked = Walk::default();
        self.collect(root, &mut walked)?;
        walked.files.sort();
        walked.skipped.sort();
        Ok(walked)
    }

    fn collect(&self, dir: &Path, walked: &mut Walk) -> Result<(), GuardError> {
        for child in self.read_dir(dir)? {
            if self.check(&child).is_err() {
                walked.skipped.push(child);
                continue;
            }
            let kind = fs::symlink_metadata(&child).map_err(|source| io_error(&child, source))?;
            if kind.is_dir() {
                self.collect(&child, walked)?;
            } else {
                walked.files.push(child);
            }
        }
        Ok(())
    }

    /// Refuses a path that is forbidden as written, or that leads to a forbidden path once links are followed.
    fn check(&self, path: &Path) -> Result<(), GuardError> {
        if is_forbidden(path) || self.in_config_root(path) {
            return Err(GuardError::Forbidden {
                path: path.to_path_buf(),
            });
        }
        if let Ok(real) = fs::canonicalize(path)
            && is_forbidden(&real)
        {
            return Err(GuardError::Forbidden {
                path: path.to_path_buf(),
            });
        }
        Ok(())
    }
}

impl GuardedFs {
    /// Whether `path` is a `credential` or `token` name directly inside one of the config roots.
    fn in_config_root(&self, path: &Path) -> bool {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return false;
        };
        let name = name.to_string_lossy().to_lowercase();
        is_secret_name(&name)
            && self
                .config_roots
                .iter()
                .any(|root| root.as_path() == parent)
    }
}

fn io_error(path: &Path, source: io::Error) -> GuardError {
    GuardError::Io {
        path: path.to_path_buf(),
        source,
    }
}
