use std::{
    fs,
    path::{Path, PathBuf},
};

use eyre::{Context, Result, bail};

/// Path resolved through `fs::canonicalize` when constructed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CanonicalPathBuf(PathBuf);

impl CanonicalPathBuf {
    pub(crate) fn new(path: impl AsRef<Path>, context: &str) -> Result<Self> {
        let path = path.as_ref();
        fs::canonicalize(path)
            .map(Self)
            .wrap_err_with(|| format!("{context}: {}", path.display()))
    }

    pub(crate) fn as_path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn into_path_buf(self) -> PathBuf {
        self.0
    }

    pub(crate) fn display(&self) -> std::path::Display<'_> {
        self.0.display()
    }

    #[cfg(test)]
    pub(crate) fn assume_canonical_for_test(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }
}

impl AsRef<Path> for CanonicalPathBuf {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

pub(crate) fn expand_home_path(home: &Path, path: &Path) -> PathBuf {
    if path == Path::new("~") {
        return home.to_path_buf();
    }

    if let Ok(path_from_home) = path.strip_prefix("~") {
        return home.join(path_from_home);
    }

    path.to_path_buf()
}

pub(crate) fn canonicalize_existing_file(path: &Path, context: &str) -> Result<PathBuf> {
    if !path.is_file() {
        bail!("{context}: {}", path.display());
    }

    canonicalize(path, "failed to resolve file path")
}

pub(crate) fn canonicalize(path: impl AsRef<Path>, context: &str) -> Result<PathBuf> {
    CanonicalPathBuf::new(path, context).map(CanonicalPathBuf::into_path_buf)
}
