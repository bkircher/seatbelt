use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use eyre::Context;
use tempfile::TempDir;

use crate::{env_name::EnvName, paths::CanonicalPathBuf, profile::SandboxProfile};

pub fn os(value: &str) -> OsString {
    OsString::from(value)
}

#[track_caller]
pub fn must<T, E: std::fmt::Debug>(result: std::result::Result<T, E>) -> T {
    result.expect("fallible test setup failed")
}

pub fn env_name(value: &str) -> EnvName {
    must(EnvName::try_from(value.to_owned()))
}

pub fn temp_dir() -> TempDir {
    must(TempDir::new())
}

pub fn create_dir_all(path: &Path) {
    must(
        fs::create_dir_all(path)
            .wrap_err_with(|| format!("failed to create test directory: {}", path.display())),
    );
}

pub fn write_file(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    must(
        fs::write(path, contents)
            .wrap_err_with(|| format!("failed to write test file: {}", path.display())),
    );
}

pub fn canonicalized(path: impl AsRef<Path>, context: &str) -> CanonicalPathBuf {
    must(CanonicalPathBuf::new(path, context))
}

pub fn canonical_path(path: &str) -> CanonicalPathBuf {
    CanonicalPathBuf::assume_canonical_for_test(PathBuf::from(path))
}

pub fn profile_text_contains(profile: &SandboxProfile, needle: &str) -> bool {
    matches!(profile, SandboxProfile::Text(actual) if actual.contains(needle))
}

pub fn profile_text_excludes(profile: &SandboxProfile, needle: &str) -> bool {
    matches!(profile, SandboxProfile::Text(actual) if !actual.contains(needle))
}
