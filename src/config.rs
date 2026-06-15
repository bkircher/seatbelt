use std::{
    fs,
    path::{Path, PathBuf},
};

use eyre::{Context, Result, bail};
use serde::Deserialize;

use crate::{
    allow_paths::{AllowAccess, ResolvedAllowPath, resolve_allow_paths},
    env_name::EnvName,
    paths::canonicalize_existing_file,
    profile::{SandboxProfile, append_allow_paths, compose_import_profile, compose_profile},
};

const DEFAULT_CONFIG_SUFFIX: &str = ".config/seatbelt/default.yaml";
const CONFIGS_SUFFIX: &str = ".config/seatbelt";
const PROFILES_SUFFIX: &str = ".config/seatbelt/profiles";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeatbeltConfig {
    #[serde(default)]
    profiles: Vec<PathBuf>,

    #[serde(default)]
    allow: AllowConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AllowConfig {
    #[serde(default)]
    env: Vec<EnvName>,

    #[serde(default)]
    read: Vec<PathBuf>,

    #[serde(default)]
    write: Vec<PathBuf>,
}

pub(crate) struct InvocationConfig {
    pub(crate) allow_env: Vec<EnvName>,
    pub(crate) profile: SandboxProfile,
    pub(crate) allow_read_paths: Vec<ResolvedAllowPath>,
    pub(crate) allow_write_paths: Vec<ResolvedAllowPath>,
}

pub(crate) fn load_invocation_config(
    home: &Path,
    config_arg: Option<PathBuf>,
    profile_arg: Option<PathBuf>,
    cli_allow_env: Vec<EnvName>,
    cli_allow_read: Vec<PathBuf>,
    cli_allow_write: Vec<PathBuf>,
) -> Result<InvocationConfig> {
    if config_arg.is_some() && profile_arg.is_some() {
        bail!("--profile cannot be used together with --config");
    }

    if let Some(profile) = profile_arg {
        let profile = canonicalize_existing_file(&profile, "sandbox profile not found")?;
        let allow_read_paths = resolve_allow_paths(home, &cli_allow_read, AllowAccess::Read)?;
        let allow_write_paths = resolve_allow_paths(home, &cli_allow_write, AllowAccess::Write)?;
        let profile = if allow_read_paths.is_empty() && allow_write_paths.is_empty() {
            SandboxProfile::File(profile)
        } else {
            SandboxProfile::Text(compose_import_profile(
                &[profile],
                &allow_read_paths,
                &allow_write_paths,
            )?)
        };
        return Ok(InvocationConfig {
            allow_env: cli_allow_env,
            profile,
            allow_read_paths,
            allow_write_paths,
        });
    }

    let config_path = match config_arg {
        Some(config_arg) => resolve_config_path(home, &config_arg)?,
        None => canonicalize_existing_file(
            &home.join(DEFAULT_CONFIG_SUFFIX),
            "default config file not found",
        )?,
    };
    let seatbelt_config = read_seatbelt_config(&config_path)?;
    let SeatbeltConfig { profiles, allow } = seatbelt_config;

    let mut allow_env = allow.env;
    allow_env.extend(cli_allow_env);

    let mut allow_read_paths = allow.read;
    allow_read_paths.extend(cli_allow_read);
    let allow_read_paths = resolve_allow_paths(home, &allow_read_paths, AllowAccess::Read)?;

    let mut allow_write_paths = allow.write;
    allow_write_paths.extend(cli_allow_write);
    let allow_write_paths = resolve_allow_paths(home, &allow_write_paths, AllowAccess::Write)?;

    let profile_root = home.join(PROFILES_SUFFIX);
    let mut profile_text = compose_profile(&profile_root, &profiles)?;
    append_allow_paths(&mut profile_text, &allow_read_paths, AllowAccess::Read)?;
    append_allow_paths(&mut profile_text, &allow_write_paths, AllowAccess::Write)?;

    Ok(InvocationConfig {
        allow_env,
        profile: SandboxProfile::Text(profile_text),
        allow_read_paths,
        allow_write_paths,
    })
}

fn resolve_config_path(home: &Path, config_arg: &Path) -> Result<PathBuf> {
    if config_arg.is_file() {
        return crate::paths::canonicalize(config_arg, "failed to resolve config path");
    }

    if config_arg.is_absolute() {
        bail!("config file not found: {}", config_arg.display());
    }

    let config_dir = home.join(CONFIGS_SUFFIX);
    let candidates = config_path_candidates(&config_dir, config_arg);
    for candidate in &candidates {
        if candidate.is_file() {
            return crate::paths::canonicalize(candidate, "failed to resolve config path");
        }
    }

    let checked = candidates
        .iter()
        .map(|candidate| candidate.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    bail!(
        "config file not found: {} (checked: {checked})",
        config_arg.display()
    );
}

fn config_path_candidates(config_dir: &Path, config_arg: &Path) -> Vec<PathBuf> {
    if config_arg.extension().is_some() {
        return vec![config_dir.join(config_arg)];
    }

    vec![
        config_dir.join(config_arg).with_extension("yaml"),
        config_dir.join(config_arg).with_extension("yml"),
    ]
}

fn read_seatbelt_config(path: &Path) -> Result<SeatbeltConfig> {
    let contents = fs::read_to_string(path)
        .wrap_err_with(|| format!("failed to read config file: {}", path.display()))?;
    yaml_serde::from_str(&contents)
        .wrap_err_with(|| format!("failed to parse config file: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;

    #[test]
    fn parses_seatbelt_config_yaml() {
        let config: SeatbeltConfig = must(yaml_serde::from_str(
            r#"
profiles:
  - base.sb
  - agents/pi.sb
allow:
  env:
    - ATLASSIAN_API_TOKEN
  read:
    - ~/src/pi
    - docs
  write:
    - dist
    - ~/tmp/output
"#,
        ));

        assert_eq!(
            config.profiles,
            vec![PathBuf::from("base.sb"), PathBuf::from("agents/pi.sb")]
        );
        assert_eq!(config.allow.env, vec![env_name("ATLASSIAN_API_TOKEN")]);
        assert_eq!(
            config.allow.read,
            vec![PathBuf::from("~/src/pi"), PathBuf::from("docs")]
        );
        assert_eq!(
            config.allow.write,
            vec![PathBuf::from("dist"), PathBuf::from("~/tmp/output")]
        );
    }

    #[test]
    fn config_path_candidates_support_yaml_and_yml_names() {
        let actual = config_path_candidates(
            Path::new("/Users/alice/.config/seatbelt"),
            Path::new("acme"),
        );

        assert_eq!(
            actual,
            vec![
                PathBuf::from("/Users/alice/.config/seatbelt/acme.yaml"),
                PathBuf::from("/Users/alice/.config/seatbelt/acme.yml"),
            ]
        );
    }

    #[test]
    fn rejects_config_and_profile_together() {
        let result = load_invocation_config(
            Path::new("/Users/alice"),
            Some(PathBuf::from("acme")),
            Some(PathBuf::from("raw.sb")),
            vec![],
            vec![],
            vec![],
        );

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some("--profile cannot be used together with --config".to_owned())
        );
    }

    #[test]
    fn load_invocation_config_combines_config_and_cli_allow_read_paths() {
        let temp = temp_dir();
        let home = temp.path().join("home");
        let profile_dir = home.join(PROFILES_SUFFIX);
        let config_read_dir = home.join("src/pi");
        let cli_read_file = temp.path().join("obscura.json");
        create_dir_all(&profile_dir);
        create_dir_all(&config_read_dir);
        write_file(&cli_read_file, "{}\n");
        write_file(profile_dir.join("base.sb"), "; base profile\n");
        write_file(
            home.join(CONFIGS_SUFFIX).join("pi.yaml"),
            "profiles:\n  - base.sb\nallow:\n  read:\n    - ~/src/pi\n",
        );

        let invocation = must(load_invocation_config(
            &home,
            Some(PathBuf::from("pi")),
            None,
            vec![],
            vec![cli_read_file.clone()],
            vec![],
        ));
        let expected_config_read_dir = canonicalized(
            config_read_dir,
            "failed to resolve expected config allow-read directory",
        );
        let expected_cli_read_file = canonicalized(
            cli_read_file,
            "failed to resolve expected CLI allow-read file",
        );

        assert_eq!(invocation.allow_env, Vec::<EnvName>::new());
        assert_eq!(
            invocation.allow_read_paths,
            vec![
                ResolvedAllowPath::Directory(expected_config_read_dir.clone()),
                ResolvedAllowPath::File(expected_cli_read_file.clone())
            ]
        );
        assert_eq!(
            invocation.allow_write_paths,
            Vec::<ResolvedAllowPath>::new()
        );
        assert!(profile_text_contains(
            &invocation.profile,
            &format!("(literal \"{}\")", expected_config_read_dir.display())
        ));
        assert!(profile_text_contains(
            &invocation.profile,
            &format!("(subpath \"{}\")", expected_config_read_dir.display())
        ));
        assert!(profile_text_contains(
            &invocation.profile,
            &format!("(literal \"{}\")", expected_cli_read_file.display())
        ));
        assert!(profile_text_excludes(
            &invocation.profile,
            &format!("(subpath \"{}\")", expected_cli_read_file.display())
        ));
    }

    #[test]
    fn load_invocation_config_combines_config_and_cli_allow_write_paths() {
        let temp = temp_dir();
        let home = temp.path().join("home");
        let profile_dir = home.join(PROFILES_SUFFIX);
        let config_write_dir = home.join("dist");
        let cli_write_file = temp.path().join("output.log");
        create_dir_all(&profile_dir);
        create_dir_all(&config_write_dir);
        write_file(&cli_write_file, "existing\n");
        write_file(profile_dir.join("base.sb"), "; base profile\n");
        write_file(
            home.join(CONFIGS_SUFFIX).join("build.yaml"),
            "profiles:\n  - base.sb\nallow:\n  write:\n    - ~/dist\n",
        );

        let invocation = must(load_invocation_config(
            &home,
            Some(PathBuf::from("build")),
            None,
            vec![],
            vec![],
            vec![cli_write_file.clone()],
        ));
        let expected_config_write_dir = canonicalized(
            config_write_dir,
            "failed to resolve expected config allow-write directory",
        );
        let expected_cli_write_file = canonicalized(
            cli_write_file,
            "failed to resolve expected CLI allow-write file",
        );

        assert_eq!(invocation.allow_env, Vec::<EnvName>::new());
        assert_eq!(invocation.allow_read_paths, Vec::<ResolvedAllowPath>::new());
        assert_eq!(
            invocation.allow_write_paths,
            vec![
                ResolvedAllowPath::Directory(expected_config_write_dir.clone()),
                ResolvedAllowPath::File(expected_cli_write_file.clone())
            ]
        );
        assert!(profile_text_contains(
            &invocation.profile,
            "; Additional read/write paths from allow.write/--allow-write"
        ));
        assert!(profile_text_contains(
            &invocation.profile,
            "(allow file-read* file-write*"
        ));
        assert!(profile_text_contains(
            &invocation.profile,
            &format!("(literal \"{}\")", expected_config_write_dir.display())
        ));
        assert!(profile_text_contains(
            &invocation.profile,
            &format!("(subpath \"{}\")", expected_config_write_dir.display())
        ));
        assert!(profile_text_contains(
            &invocation.profile,
            &format!("(literal \"{}\")", expected_cli_write_file.display())
        ));
        assert!(profile_text_excludes(
            &invocation.profile,
            &format!("(subpath \"{}\")", expected_cli_write_file.display())
        ));
    }
}
