use std::{
    fs,
    path::{Path, PathBuf},
};

use eyre::{Context, Result, bail, eyre};

use crate::paths::{CanonicalPathBuf, canonicalize, expand_home_path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedAllowPath {
    File(CanonicalPathBuf),
    Directory(CanonicalPathBuf),
}

impl ResolvedAllowPath {
    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::File(path) | Self::Directory(path) => path.as_path(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum AllowAccess {
    Read,
    Write,
}

impl AllowAccess {
    pub(crate) fn option_name(self) -> &'static str {
        match self {
            Self::Read => "--allow-read",
            Self::Write => "--allow-write",
        }
    }

    pub(crate) fn source_label(self) -> &'static str {
        match self {
            Self::Read => "allow.read/--allow-read",
            Self::Write => "allow.write/--allow-write",
        }
    }

    pub(crate) fn comment(self) -> &'static str {
        match self {
            Self::Read => "Additional read-only paths from allow.read/--allow-read",
            Self::Write => "Additional read/write paths from allow.write/--allow-write",
        }
    }

    pub(crate) fn sbpl_permissions(self) -> &'static str {
        match self {
            Self::Read => "file-read*",
            Self::Write => "file-read* file-write*",
        }
    }
}

pub(crate) fn resolve_allow_paths(
    home: &Path,
    paths: &[PathBuf],
    access: AllowAccess,
) -> Result<Vec<ResolvedAllowPath>> {
    let option_name = access.option_name();
    let mut resolved_paths = Vec::with_capacity(paths.len());
    for path in paths {
        let expanded_path = expand_home_path(home, path);
        let context = format!("failed to resolve {option_name} path");
        let resolved_path = CanonicalPathBuf::new(&expanded_path, &context)?;
        let metadata = fs::metadata(resolved_path.as_path()).wrap_err_with(|| {
            format!(
                "failed to inspect {option_name} path: {}",
                resolved_path.display()
            )
        })?;
        let allow_path = if metadata.is_file() {
            ResolvedAllowPath::File(resolved_path)
        } else if metadata.is_dir() {
            ResolvedAllowPath::Directory(resolved_path)
        } else {
            bail!(
                "{option_name} path must be a file or directory: {}",
                resolved_path.display()
            );
        };
        resolved_paths.push(allow_path);
    }

    if !resolved_paths.is_empty() {
        reject_overly_broad_directories(home, &resolved_paths, access)?;
    }

    Ok(resolved_paths)
}

fn reject_overly_broad_directories(
    home: &Path,
    paths: &[ResolvedAllowPath],
    access: AllowAccess,
) -> Result<()> {
    let option_name = access.option_name();
    let broad_directories = broad_directory_paths(home)?;

    for path in paths {
        if let ResolvedAllowPath::Directory(path) = path
            && is_overly_broad_directory(path.as_path(), &broad_directories)
        {
            bail!("{option_name} directory is too broad: {}", path.display());
        }
    }

    Ok(())
}

fn broad_directory_paths(home: &Path) -> Result<Vec<PathBuf>> {
    let resolved_home = canonicalize(home, "failed to resolve HOME")?;
    let resolved_users_dir = canonicalize(
        resolved_home
            .parent()
            .ok_or_else(|| eyre!("resolved HOME has no parent: {}", resolved_home.display()))?,
        "failed to resolve users directory",
    )?;
    let mut paths = vec![
        PathBuf::from("/"),
        resolved_users_dir,
        resolved_home.clone(),
    ];
    push_existing_broad_directory(&mut paths, resolved_home.join("Documents"))?;
    push_existing_broad_directory(&mut paths, resolved_home.join("src"))?;

    Ok(paths)
}

fn push_existing_broad_directory(paths: &mut Vec<PathBuf>, path: PathBuf) -> Result<()> {
    if path.is_dir() {
        paths.push(canonicalize(path, "failed to resolve broad directory")?);
    }

    Ok(())
}

fn is_overly_broad_directory(path: &Path, broad_directories: &[PathBuf]) -> bool {
    broad_directories
        .iter()
        .any(|broad_directory| path == broad_directory)
}

pub(crate) fn project_dir_redundancy_warnings(
    access: AllowAccess,
    paths: &[ResolvedAllowPath],
    project_dir: &Path,
) -> Vec<String> {
    let source_label = access.source_label();

    paths
        .iter()
        .filter(|path| path.path().starts_with(project_dir))
        .map(|path| {
            format!(
                "warning: {source_label} {} is already covered by $PROJECT_DIR",
                path.path().display()
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::{app::required_env_path, test_support::*};

    #[test]
    fn resolve_allow_paths_rejects_write_nonexistent_paths() {
        let temp = temp_dir();
        let home = temp.path().join("home");
        create_dir_all(&home);
        let missing = home.join("missing");

        let result = resolve_allow_paths(&home, std::slice::from_ref(&missing), AllowAccess::Write);

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some(format!(
                "failed to resolve --allow-write path: {}",
                missing.display()
            ))
        );
    }

    #[test]
    fn resolve_allow_paths_rejects_write_root_directory() {
        let temp = temp_dir();
        let home = temp.path().join("home");
        create_dir_all(&home);

        let result = resolve_allow_paths(&home, &[PathBuf::from("/")], AllowAccess::Write);

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some("--allow-write directory is too broad: /".to_owned())
        );
    }

    #[test]
    fn resolve_allow_paths_rejects_write_home_directory() {
        let temp = temp_dir();
        let home = temp.path().join("home");
        create_dir_all(&home);
        let expected_home = canonicalized(&home, "failed to resolve expected home");

        let result = resolve_allow_paths(&home, &[PathBuf::from("~")], AllowAccess::Write);

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some(format!(
                "--allow-write directory is too broad: {}",
                expected_home.display()
            ))
        );
    }

    #[test]
    fn resolve_allow_paths_rejects_write_home_documents_directory() {
        let temp = temp_dir();
        let home = temp.path().join("home");
        let documents = home.join("Documents");
        create_dir_all(&documents);
        let expected_documents = canonicalized(&documents, "failed to resolve expected Documents");

        let result =
            resolve_allow_paths(&home, &[PathBuf::from("~/Documents")], AllowAccess::Write);

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some(format!(
                "--allow-write directory is too broad: {}",
                expected_documents.display()
            ))
        );
    }

    #[test]
    fn resolve_allow_paths_rejects_write_home_src_directory() {
        let temp = temp_dir();
        let home = temp.path().join("home");
        let src = home.join("src");
        create_dir_all(&src);
        let expected_src = canonicalized(&src, "failed to resolve expected src");

        let result = resolve_allow_paths(&home, &[PathBuf::from("~/src")], AllowAccess::Write);

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some(format!(
                "--allow-write directory is too broad: {}",
                expected_src.display()
            ))
        );
    }

    #[test]
    fn resolve_allow_paths_rejects_write_users_directory() {
        let temp = temp_dir();
        let home = temp.path().join("home");
        create_dir_all(&home);
        let expected_users_dir = canonicalized(temp.path(), "failed to resolve expected users dir");

        let result = resolve_allow_paths(&home, &[temp.path().to_path_buf()], AllowAccess::Write);

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some(format!(
                "--allow-write directory is too broad: {}",
                expected_users_dir.display()
            ))
        );
    }

    #[test]
    fn project_dir_redundancy_warnings_report_read_and_write_paths() {
        let project_dir = Path::new("/Users/alice/project");

        let read_warnings = project_dir_redundancy_warnings(
            AllowAccess::Read,
            &[ResolvedAllowPath::Directory(canonical_path(
                "/Users/alice/project",
            ))],
            project_dir,
        );
        let write_warnings = project_dir_redundancy_warnings(
            AllowAccess::Write,
            &[ResolvedAllowPath::File(canonical_path(
                "/Users/alice/project/output.log",
            ))],
            project_dir,
        );

        assert_eq!(
            read_warnings,
            vec![
                "warning: allow.read/--allow-read /Users/alice/project is already covered by $PROJECT_DIR"
                    .to_owned()
            ]
        );
        assert_eq!(
            write_warnings,
            vec![
                "warning: allow.write/--allow-write /Users/alice/project/output.log is already covered by $PROJECT_DIR"
                    .to_owned()
            ]
        );
    }

    #[test]
    fn resolve_allow_paths_rejects_read_nonexistent_paths() {
        let temp = temp_dir();
        let home = temp.path().join("home");
        create_dir_all(&home);
        let missing = home.join("missing");

        let result = resolve_allow_paths(&home, std::slice::from_ref(&missing), AllowAccess::Read);

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some(format!(
                "failed to resolve --allow-read path: {}",
                missing.display()
            ))
        );
    }

    #[test]
    fn resolve_allow_paths_rejects_read_root_directory() {
        let temp = temp_dir();
        let home = temp.path().join("home");
        create_dir_all(&home);

        let result = resolve_allow_paths(&home, &[PathBuf::from("/")], AllowAccess::Read);

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some("--allow-read directory is too broad: /".to_owned())
        );
    }

    #[test]
    fn resolve_allow_paths_rejects_read_home_directory() {
        let temp = temp_dir();
        let home = temp.path().join("home");
        create_dir_all(&home);
        let expected_home = canonicalized(&home, "failed to resolve expected home");

        let result = resolve_allow_paths(&home, &[PathBuf::from("~")], AllowAccess::Read);

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some(format!(
                "--allow-read directory is too broad: {}",
                expected_home.display()
            ))
        );
    }

    #[test]
    fn resolve_allow_paths_rejects_read_home_documents_directory() {
        let temp = temp_dir();
        let home = temp.path().join("home");
        let documents = home.join("Documents");
        create_dir_all(&documents);
        let expected_documents = canonicalized(&documents, "failed to resolve expected Documents");

        let result = resolve_allow_paths(&home, &[PathBuf::from("~/Documents")], AllowAccess::Read);

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some(format!(
                "--allow-read directory is too broad: {}",
                expected_documents.display()
            ))
        );
    }

    #[test]
    fn resolve_allow_paths_rejects_read_home_src_directory() {
        let temp = temp_dir();
        let home = temp.path().join("home");
        let src = home.join("src");
        create_dir_all(&src);
        let expected_src = canonicalized(&src, "failed to resolve expected src");

        let result = resolve_allow_paths(&home, &[PathBuf::from("~/src")], AllowAccess::Read);

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some(format!(
                "--allow-read directory is too broad: {}",
                expected_src.display()
            ))
        );
    }

    #[test]
    fn resolve_allow_paths_rejects_read_users_directory() {
        let temp = temp_dir();
        let home = temp.path().join("home");
        create_dir_all(&home);
        let expected_users_dir = canonicalized(temp.path(), "failed to resolve expected users dir");

        let result = resolve_allow_paths(&home, &[temp.path().to_path_buf()], AllowAccess::Read);

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some(format!(
                "--allow-read directory is too broad: {}",
                expected_users_dir.display()
            ))
        );
    }

    #[test]
    fn resolve_allow_paths_resolves_read_relative_directories() {
        let home = must(required_env_path("HOME"));
        let expected = canonicalized(Path::new("src"), "failed to resolve test directory");

        let actual = must(resolve_allow_paths(
            &home,
            &[PathBuf::from("src")],
            AllowAccess::Read,
        ));

        assert_eq!(actual, vec![ResolvedAllowPath::Directory(expected)]);
    }

    #[test]
    fn resolve_allow_paths_resolves_read_files() {
        let home = must(required_env_path("HOME"));
        let expected = canonicalized(Path::new("Cargo.toml"), "failed to resolve test file");

        let actual = must(resolve_allow_paths(
            &home,
            &[PathBuf::from("Cargo.toml")],
            AllowAccess::Read,
        ));

        assert_eq!(actual, vec![ResolvedAllowPath::File(expected)]);
    }
}
