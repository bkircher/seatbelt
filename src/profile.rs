use std::{
    fs,
    path::{Path, PathBuf},
};

use eyre::{Context, Result, bail};

use crate::{
    allow_paths::{AllowAccess, ResolvedAllowPath},
    paths::canonicalize_existing_file,
};

pub(crate) enum SandboxProfile {
    File(PathBuf),
    Text(String),
}

pub(crate) fn compose_profile(profile_root: &Path, profiles: &[PathBuf]) -> Result<String> {
    if profiles.is_empty() {
        bail!("config must contain at least one profile");
    }

    let imports = profiles
        .iter()
        .map(|profile_fragment| resolve_profile_fragment(profile_root, profile_fragment))
        .collect::<Result<Vec<_>>>()?;

    compose_import_profile(&imports, &[], &[])
}

pub(crate) fn compose_import_profile(
    imports: &[PathBuf],
    allow_read_paths: &[ResolvedAllowPath],
    allow_write_paths: &[ResolvedAllowPath],
) -> Result<String> {
    let mut profile = String::from("(version 1)\n\n");
    for import in imports {
        profile.push_str("(import ");
        profile.push_str(&sbpl_string_literal(import)?);
        profile.push_str(")\n");
    }
    append_allow_paths(&mut profile, allow_read_paths, AllowAccess::Read)?;
    append_allow_paths(&mut profile, allow_write_paths, AllowAccess::Write)?;

    Ok(profile)
}

pub(crate) fn append_allow_paths(
    profile: &mut String,
    allow_paths: &[ResolvedAllowPath],
    access: AllowAccess,
) -> Result<()> {
    if allow_paths.is_empty() {
        return Ok(());
    }

    profile.push_str("\n; ");
    profile.push_str(access.comment());
    profile.push_str("\n(allow ");
    profile.push_str(access.sbpl_permissions());
    profile.push('\n');
    for path in allow_paths {
        let literal = sbpl_string_literal(path.path())?;
        profile.push_str("    (literal ");
        profile.push_str(&literal);
        profile.push_str(")\n");

        if matches!(path, ResolvedAllowPath::Directory(_)) {
            profile.push_str("    (subpath ");
            profile.push_str(&literal);
            profile.push_str(")\n");
        }
    }
    profile.push_str(")\n");

    Ok(())
}

fn resolve_profile_fragment(profile_root: &Path, profile_fragment: &Path) -> Result<PathBuf> {
    if profile_fragment.is_absolute() {
        bail!(
            "profile entries must be relative to {}: {}",
            profile_root.display(),
            profile_fragment.display()
        );
    }

    canonicalize_existing_file(
        &profile_root.join(profile_fragment),
        "profile fragment not found",
    )
}

fn sbpl_string_literal(path: &Path) -> Result<String> {
    let Some(path) = path.to_str() else {
        bail!("profile path is not valid UTF-8: {}", path.display());
    };

    if path.chars().any(char::is_control) {
        bail!("profile path contains control characters: {path}");
    }

    Ok(format!(
        "\"{}\"",
        path.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

pub(crate) fn print_profile(profile: &SandboxProfile) -> Result<()> {
    match profile {
        SandboxProfile::File(path) => {
            let contents = fs::read_to_string(path)
                .wrap_err_with(|| format!("failed to read profile file: {}", path.display()))?;
            print!("{contents}");
        }
        SandboxProfile::Text(contents) => {
            print!("{contents}");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{allow_paths::ResolvedAllowPath, test_support::*};

    #[test]
    fn rejects_absolute_profile_entries_in_config() {
        let result = resolve_profile_fragment(
            Path::new("/Users/alice/.config/seatbelt/profiles"),
            Path::new("/tmp/profile.sb"),
        );

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some(
                "profile entries must be relative to /Users/alice/.config/seatbelt/profiles: /tmp/profile.sb"
                    .to_owned()
            )
        );
    }

    #[test]
    fn compose_import_profile_appends_allow_read_paths() {
        let actual = must(compose_import_profile(
            &[PathBuf::from("/profiles/raw.sb")],
            &[
                ResolvedAllowPath::Directory(canonical_path("/Users/alice/docs")),
                ResolvedAllowPath::File(canonical_path("/Users/alice/.zshrc")),
                ResolvedAllowPath::Directory(canonical_path("/Volumes/Shared Stuff")),
            ],
            &[],
        ));

        assert_eq!(
            actual,
            "(version 1)\n\n(import \"/profiles/raw.sb\")\n\n; Additional read-only paths from allow.read/--allow-read\n(allow file-read*\n    (literal \"/Users/alice/docs\")\n    (subpath \"/Users/alice/docs\")\n    (literal \"/Users/alice/.zshrc\")\n    (literal \"/Volumes/Shared Stuff\")\n    (subpath \"/Volumes/Shared Stuff\")\n)\n"
        );
    }

    #[test]
    fn compose_import_profile_appends_allow_write_paths() {
        let actual = must(compose_import_profile(
            &[PathBuf::from("/profiles/raw.sb")],
            &[],
            &[
                ResolvedAllowPath::Directory(canonical_path("/Users/alice/dist")),
                ResolvedAllowPath::File(canonical_path("/Users/alice/output.log")),
            ],
        ));

        assert_eq!(
            actual,
            "(version 1)\n\n(import \"/profiles/raw.sb\")\n\n; Additional read/write paths from allow.write/--allow-write\n(allow file-read* file-write*\n    (literal \"/Users/alice/dist\")\n    (subpath \"/Users/alice/dist\")\n    (literal \"/Users/alice/output.log\")\n)\n"
        );
    }

    #[test]
    fn compose_import_profile_escapes_allow_read_paths() {
        let actual = must(compose_import_profile(
            &[PathBuf::from("/profiles/raw.sb")],
            &[ResolvedAllowPath::File(canonical_path(
                "/Users/alice/quoted\"file",
            ))],
            &[],
        ));

        assert_eq!(
            actual,
            "(version 1)\n\n(import \"/profiles/raw.sb\")\n\n; Additional read-only paths from allow.read/--allow-read\n(allow file-read*\n    (literal \"/Users/alice/quoted\\\"file\")\n)\n"
        );
    }
}
