use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    os::unix::{ffi::OsStrExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

use eyre::{Context, ErrReport, Result, bail, eyre};
use serde::Deserialize;

pub mod cli;

const DEFAULT_CONFIG_SUFFIX: &str = ".config/seatbelt/default.yaml";
const CONFIGS_SUFFIX: &str = ".config/seatbelt";
const PROFILES_SUFFIX: &str = ".config/seatbelt/profiles";
const REQUIRED_TMPDIR_PREFIX: &str = "/private/var/folders";
const SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeatbeltConfig {
    #[serde(default)]
    profiles: Vec<PathBuf>,

    #[serde(default)]
    allowed_envs: Vec<String>,

    #[serde(default)]
    allow_read: Vec<PathBuf>,
}

struct InvocationConfig {
    allow_env: Vec<String>,
    profile: SandboxProfile,
}

struct RunConfig {
    invocation: InvocationConfig,
    dry_run: bool,
    command: Vec<OsString>,
}

enum SandboxProfile {
    File(PathBuf),
    Text(String),
}

struct SandboxContext<'a> {
    profile: &'a SandboxProfile,
    resolved_users_dir: &'a Path,
    resolved_home: &'a Path,
    project_dir: &'a Path,
    resolved_tmpdir: &'a Path,
}

trait EnvSource {
    fn var_os(&self, name: &str) -> Option<OsString>;
}

struct ProcessEnv;

impl EnvSource for ProcessEnv {
    fn var_os(&self, name: &str) -> Option<OsString> {
        env::var_os(name)
    }
}

fn main() -> Result<(), ErrReport> {
    use clap::Parser;

    color_eyre::install().wrap_err("failed to install color-eyre error reports")?;

    let cli = cli::Cli::parse();
    let home = required_env_path("HOME")?;
    let invocation = load_invocation_config(
        &home,
        cli.config,
        cli.profile,
        cli.allow_env,
        cli.allow_read,
    )?;

    match cli.command {
        cli::Command::PrintProfile => {
            print_profile(&invocation.profile).wrap_err("print-profile command failed")
        }
        cli::Command::Run(run_args) => run(RunConfig {
            invocation,
            dry_run: cli.dry_run,
            command: run_args.command,
        })
        .wrap_err("run command failed"),
    }
}

fn run(config: RunConfig) -> Result<()> {
    let home = required_env_path("HOME")?;
    let resolved_home = canonicalize(&home, "failed to resolve HOME")?;
    let resolved_users_dir = canonicalize(
        resolved_home
            .parent()
            .ok_or_else(|| eyre!("resolved HOME has no parent: {}", resolved_home.display()))?,
        "failed to resolve users directory",
    )?;
    let mut project_dir = canonicalize(
        env::current_dir().wrap_err("failed to read current directory")?,
        "failed to resolve current directory",
    )?;

    if let Some(git_root) = git_root(&project_dir)? {
        project_dir = canonicalize(git_root, "failed to resolve Git root")?;
    }

    if project_dir == resolved_home {
        bail!(
            "refusing to run from $HOME ({})\n\nRunning from $HOME would grant write access to your entire home directory, defeating the purpose of the sandbox.\n\nInstead, cd into a project directory first:\n  cd ~/my-project && seatbelt run <command>",
            resolved_home.display()
        );
    }

    let tmpdir = required_env_path("TMPDIR")?;
    let resolved_tmpdir = canonicalize(tmpdir, "failed to resolve TMPDIR")?;
    validate_tmpdir(&resolved_tmpdir)?;

    let sandbox_context = SandboxContext {
        profile: &config.invocation.profile,
        resolved_users_dir: &resolved_users_dir,
        resolved_home: &resolved_home,
        project_dir: &project_dir,
        resolved_tmpdir: &resolved_tmpdir,
    };
    let final_command = build_final_command(
        &sandbox_context,
        &ProcessEnv,
        &config.invocation.allow_env,
        &config.command,
    )?;

    if config.dry_run {
        println!("{}", shell_words(&final_command));
        return Ok(());
    }

    exec_command(&final_command)
}

fn load_invocation_config(
    home: &Path,
    config_arg: Option<PathBuf>,
    profile_arg: Option<PathBuf>,
    cli_allow_env: Vec<String>,
    cli_allow_read: Vec<PathBuf>,
) -> Result<InvocationConfig> {
    if config_arg.is_some() && profile_arg.is_some() {
        bail!("--profile cannot be used together with --config");
    }

    if let Some(profile) = profile_arg {
        validate_allowed_env_names(&cli_allow_env)?;
        let profile = canonicalize_existing_file(&profile, "sandbox profile not found")?;
        let allow_read_dirs = resolve_allow_read_dirs(home, &cli_allow_read)?;
        let profile = if allow_read_dirs.is_empty() {
            SandboxProfile::File(profile)
        } else {
            SandboxProfile::Text(compose_import_profile(&[profile], &allow_read_dirs)?)
        };
        return Ok(InvocationConfig {
            allow_env: cli_allow_env,
            profile,
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

    let mut allow_env = seatbelt_config.allowed_envs;
    allow_env.extend(cli_allow_env);
    validate_allowed_env_names(&allow_env)?;

    let mut allow_read_paths = seatbelt_config.allow_read;
    allow_read_paths.extend(cli_allow_read);
    let allow_read_dirs = resolve_allow_read_dirs(home, &allow_read_paths)?;

    let profile_root = home.join(PROFILES_SUFFIX);
    let mut profile_text = compose_profile(&profile_root, &seatbelt_config.profiles)?;
    append_allow_read_dirs(&mut profile_text, &allow_read_dirs)?;

    Ok(InvocationConfig {
        allow_env,
        profile: SandboxProfile::Text(profile_text),
    })
}

fn resolve_allow_read_dirs(home: &Path, paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut resolved_paths = Vec::with_capacity(paths.len());
    for path in paths {
        let expanded_path = expand_home_path(home, path);
        let resolved_path = canonicalize(&expanded_path, "failed to resolve --allow-read path")?;
        if !resolved_path.is_dir() {
            bail!(
                "--allow-read path must be a directory: {}",
                resolved_path.display()
            );
        }
        resolved_paths.push(resolved_path);
    }

    Ok(resolved_paths)
}

fn expand_home_path(home: &Path, path: &Path) -> PathBuf {
    if path == Path::new("~") {
        return home.to_path_buf();
    }

    if let Ok(path_from_home) = path.strip_prefix("~") {
        return home.join(path_from_home);
    }

    path.to_path_buf()
}

fn resolve_config_path(home: &Path, config_arg: &Path) -> Result<PathBuf> {
    if config_arg.is_file() {
        return canonicalize(config_arg, "failed to resolve config path");
    }

    if config_arg.is_absolute() {
        bail!("config file not found: {}", config_arg.display());
    }

    let config_dir = home.join(CONFIGS_SUFFIX);
    let candidates = config_path_candidates(&config_dir, config_arg);
    for candidate in &candidates {
        if candidate.is_file() {
            return canonicalize(candidate, "failed to resolve config path");
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

fn canonicalize_existing_file(path: &Path, context: &'static str) -> Result<PathBuf> {
    if !path.is_file() {
        bail!("{context}: {}", path.display());
    }

    canonicalize(path, "failed to resolve file path")
}

fn read_seatbelt_config(path: &Path) -> Result<SeatbeltConfig> {
    let contents = fs::read_to_string(path)
        .wrap_err_with(|| format!("failed to read config file: {}", path.display()))?;
    yaml_serde::from_str(&contents)
        .wrap_err_with(|| format!("failed to parse config file: {}", path.display()))
}

fn compose_profile(profile_root: &Path, profiles: &[PathBuf]) -> Result<String> {
    if profiles.is_empty() {
        bail!("config must contain at least one profile");
    }

    let mut imports = Vec::with_capacity(profiles.len());
    for profile_fragment in profiles {
        imports.push(resolve_profile_fragment(profile_root, profile_fragment)?);
    }

    compose_import_profile(&imports, &[])
}

fn compose_import_profile(imports: &[PathBuf], allow_read_dirs: &[PathBuf]) -> Result<String> {
    let mut profile = String::from("(version 1)\n\n");
    for import in imports {
        profile.push_str("(import ");
        profile.push_str(&sbpl_string_literal(import)?);
        profile.push_str(")\n");
    }
    append_allow_read_dirs(&mut profile, allow_read_dirs)?;

    Ok(profile)
}

fn append_allow_read_dirs(profile: &mut String, allow_read_dirs: &[PathBuf]) -> Result<()> {
    if allow_read_dirs.is_empty() {
        return Ok(());
    }

    profile.push_str("\n; Additional read-only directories from --allow-read\n");
    profile.push_str("(allow file-read*\n");
    for directory in allow_read_dirs {
        let directory = sbpl_string_literal(directory)?;
        profile.push_str("    (literal ");
        profile.push_str(&directory);
        profile.push_str(")\n    (subpath ");
        profile.push_str(&directory);
        profile.push_str(")\n");
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

fn print_profile(profile: &SandboxProfile) -> Result<()> {
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

fn build_final_command(
    sandbox_context: &SandboxContext<'_>,
    env_source: &impl EnvSource,
    allow_env: &[String],
    command: &[OsString],
) -> Result<Vec<OsString>> {
    let mut final_command = vec![OsString::from(SANDBOX_EXEC_PATH)];

    match sandbox_context.profile {
        SandboxProfile::File(profile) => {
            final_command.push(OsString::from("-f"));
            final_command.push(profile.as_os_str().to_os_string());
        }
        SandboxProfile::Text(profile) => {
            final_command.push(OsString::from("-p"));
            final_command.push(OsString::from(profile));
        }
    }

    final_command.extend([
        OsString::from("-D"),
        define_path("_USERS_DIR", sandbox_context.resolved_users_dir),
        OsString::from("-D"),
        define_path("_HOME", sandbox_context.resolved_home),
        OsString::from("-D"),
        define_path("_PROJECT_DIR", sandbox_context.project_dir),
        OsString::from("-D"),
        define_path("_TMPDIR", sandbox_context.resolved_tmpdir),
        OsString::from("/usr/bin/env"),
        OsString::from("-i"),
        env_pair_path("HOME", sandbox_context.resolved_home),
        env_pair("USER", env_source.var_os("USER").unwrap_or_default()),
        env_pair(
            "SHELL",
            env_source
                .var_os("SHELL")
                .unwrap_or_else(|| OsString::from("/bin/zsh")),
        ),
        env_pair(
            "TERM",
            env_source
                .var_os("TERM")
                .unwrap_or_else(|| OsString::from("xterm-256color")),
        ),
        env_pair(
            "LANG",
            env_source
                .var_os("LANG")
                .unwrap_or_else(|| OsString::from("en_US.UTF-8")),
        ),
        env_pair("PATH", env_source.var_os("PATH").unwrap_or_default()),
        env_pair_path("TMPDIR", sandbox_context.resolved_tmpdir),
    ]);

    append_if_set(&mut final_command, env_source, "SSH_AUTH_SOCK");
    append_if_set(&mut final_command, env_source, "EDITOR");
    append_if_set(&mut final_command, env_source, "VISUAL");
    append_if_set(&mut final_command, env_source, "XDG_CONFIG_HOME");
    append_if_set(&mut final_command, env_source, "XDG_DATA_HOME");
    append_if_set(&mut final_command, env_source, "XDG_CACHE_HOME");
    append_if_set(&mut final_command, env_source, "XDG_RUNTIME_DIR");

    for env_name in allow_env {
        if !is_valid_env_name(env_name) {
            bail!("invalid environment variable name: {env_name}");
        }

        let value = env_source
            .var_os(env_name)
            .ok_or_else(|| eyre!("environment variable is not set: {env_name}"))?;
        final_command.push(env_pair(env_name, value));
    }

    final_command.extend(command.iter().cloned());

    Ok(final_command)
}

fn validate_allowed_env_names(allow_env: &[String]) -> Result<()> {
    for env_name in allow_env {
        if !is_valid_env_name(env_name) {
            bail!("invalid environment variable name: {env_name}");
        }
    }

    Ok(())
}

fn required_env_path(name: &str) -> Result<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| eyre!("required environment variable is not set: {name}"))
}

fn validate_tmpdir(path: &Path) -> Result<()> {
    if !path.starts_with(REQUIRED_TMPDIR_PREFIX) {
        bail!(
            "TMPDIR must resolve under {REQUIRED_TMPDIR_PREFIX}: {}",
            path.display()
        );
    }

    Ok(())
}

fn canonicalize(path: impl AsRef<Path>, context: &'static str) -> Result<PathBuf> {
    fs::canonicalize(path.as_ref())
        .wrap_err_with(|| format!("{context}: {}", path.as_ref().display()))
}

fn git_root(project_dir: &Path) -> Result<Option<PathBuf>> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(project_dir)
        .args(["rev-parse", "--show-toplevel"])
        .output();

    let Ok(output) = output else {
        return Ok(None);
    };

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8(output.stdout).wrap_err("git root path was not valid UTF-8")?;
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    Ok(Some(PathBuf::from(trimmed)))
}

fn define_path(name: &str, value: &Path) -> OsString {
    env_pair_path(name, value)
}

fn env_pair_path(name: &str, value: &Path) -> OsString {
    env_pair(name, value.as_os_str())
}

fn env_pair(name: &str, value: impl AsRef<OsStr>) -> OsString {
    let mut pair = OsString::from(name);
    pair.push("=");
    pair.push(value.as_ref());
    pair
}

fn append_if_set(command: &mut Vec<OsString>, env_source: &impl EnvSource, name: &str) {
    if let Some(value) = env_source.var_os(name) {
        command.push(env_pair(name, value));
    }
}

fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();

    let Some(first) = chars.next() else {
        return false;
    };

    if !is_ascii_alpha_or_underscore(first) {
        return false;
    }

    chars.all(is_ascii_alnum_or_underscore)
}

fn is_ascii_alpha_or_underscore(character: char) -> bool {
    character == '_' || character.is_ascii_alphabetic()
}

fn is_ascii_alnum_or_underscore(character: char) -> bool {
    character == '_' || character.is_ascii_alphanumeric()
}

fn shell_words(args: &[OsString]) -> String {
    args.iter()
        .map(|arg| shell_quote(arg.as_os_str()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(arg: &OsStr) -> String {
    let bytes = arg.as_bytes();

    if bytes.is_empty() {
        return "''".to_owned();
    }

    if bytes.iter().all(|byte| is_shell_safe_byte(*byte)) {
        return arg.to_string_lossy().into_owned();
    }

    let mut quoted = String::from("'");
    for byte in bytes {
        if *byte == b'\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(char::from(*byte));
        }
    }
    quoted.push('\'');
    quoted
}

fn is_shell_safe_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'a'..=b'z'
            | b'A'..=b'Z'
            | b'0'..=b'9'
            | b'_'
            | b'@'
            | b'%'
            | b'+'
            | b'='
            | b':'
            | b','
            | b'.'
            | b'/'
            | b'-'
    )
}

fn exec_command(final_command: &[OsString]) -> Result<()> {
    let (program, args) = final_command
        .split_first()
        .ok_or_else(|| eyre!("internal error: final command is empty"))?;

    let error = ProcessCommand::new(program).args(args).exec();
    Err(error).wrap_err_with(|| format!("failed to execute {}", program.to_string_lossy()))
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests use ? for fallible command construction while assertions still panic"
)]
mod tests {
    use std::{collections::BTreeMap, fs};

    use super::*;

    struct TestEnv {
        values: BTreeMap<String, OsString>,
    }

    impl TestEnv {
        fn new(values: BTreeMap<String, OsString>) -> Self {
            Self { values }
        }
    }

    impl EnvSource for TestEnv {
        fn var_os(&self, name: &str) -> Option<OsString> {
            self.values.get(name).cloned()
        }
    }

    fn empty_env() -> TestEnv {
        TestEnv::new(BTreeMap::new())
    }

    fn os(value: &str) -> OsString {
        OsString::from(value)
    }

    struct TestTempDir {
        path: PathBuf,
    }

    impl TestTempDir {
        fn new(name: &str) -> Result<Self> {
            let base = env::var_os("TMPDIR")
                .map(PathBuf::from)
                .ok_or_else(|| eyre!("TMPDIR is not set for test"))?;
            let path = base.join(format!("seatbelt-{name}-{}", std::process::id()));

            if path.exists() {
                fs::remove_dir_all(&path).wrap_err_with(|| {
                    format!("failed to clean test directory: {}", path.display())
                })?;
            }
            fs::create_dir_all(&path)
                .wrap_err_with(|| format!("failed to create test directory: {}", path.display()))?;

            Ok(Self { path })
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestTempDir {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.path) {
                eprintln!(
                    "failed to clean test directory {}: {error}",
                    self.path.display()
                );
            }
        }
    }

    fn file_profile() -> SandboxProfile {
        SandboxProfile::File(PathBuf::from("/profiles/default.sb"))
    }

    fn sandbox_context(profile: &SandboxProfile) -> SandboxContext<'_> {
        SandboxContext {
            profile,
            resolved_users_dir: Path::new("/Users"),
            resolved_home: Path::new("/Users/alice"),
            project_dir: Path::new("/Users/alice/project"),
            resolved_tmpdir: Path::new("/tmp/alice"),
        }
    }

    #[test]
    fn build_final_command_assembles_sandbox_env_and_command() -> Result<()> {
        let env_source = TestEnv::new(BTreeMap::from([
            ("USER".to_owned(), os("alice")),
            ("SHELL".to_owned(), os("/bin/fish")),
            ("TERM".to_owned(), os("ansi")),
            ("LANG".to_owned(), os("C.UTF-8")),
            ("PATH".to_owned(), os("/usr/bin:/bin")),
            ("SSH_AUTH_SOCK".to_owned(), os("/tmp/ssh.sock")),
            ("EDITOR".to_owned(), os("vim")),
            ("VISUAL".to_owned(), os("nvim")),
            ("XDG_CONFIG_HOME".to_owned(), os("/Users/alice/.config")),
            ("XDG_DATA_HOME".to_owned(), os("/Users/alice/.local/share")),
            ("XDG_CACHE_HOME".to_owned(), os("/Users/alice/.cache")),
            ("XDG_RUNTIME_DIR".to_owned(), os("/tmp/runtime")),
            ("EXTRA_TOKEN".to_owned(), os("secret")),
        ]));

        let profile = file_profile();
        let context = sandbox_context(&profile);

        let actual = build_final_command(
            &context,
            &env_source,
            &["EXTRA_TOKEN".to_owned()],
            &[os("echo"), os("hello world")],
        )?;

        assert_eq!(
            actual,
            vec![
                os(SANDBOX_EXEC_PATH),
                os("-f"),
                os("/profiles/default.sb"),
                os("-D"),
                os("_USERS_DIR=/Users"),
                os("-D"),
                os("_HOME=/Users/alice"),
                os("-D"),
                os("_PROJECT_DIR=/Users/alice/project"),
                os("-D"),
                os("_TMPDIR=/tmp/alice"),
                os("/usr/bin/env"),
                os("-i"),
                os("HOME=/Users/alice"),
                os("USER=alice"),
                os("SHELL=/bin/fish"),
                os("TERM=ansi"),
                os("LANG=C.UTF-8"),
                os("PATH=/usr/bin:/bin"),
                os("TMPDIR=/tmp/alice"),
                os("SSH_AUTH_SOCK=/tmp/ssh.sock"),
                os("EDITOR=vim"),
                os("VISUAL=nvim"),
                os("XDG_CONFIG_HOME=/Users/alice/.config"),
                os("XDG_DATA_HOME=/Users/alice/.local/share"),
                os("XDG_CACHE_HOME=/Users/alice/.cache"),
                os("XDG_RUNTIME_DIR=/tmp/runtime"),
                os("EXTRA_TOKEN=secret"),
                os("echo"),
                os("hello world"),
            ]
        );
        Ok(())
    }

    #[test]
    fn build_final_command_uses_defaults_for_missing_base_env() -> Result<()> {
        let env_source = empty_env();
        let profile = file_profile();
        let context = sandbox_context(&profile);

        let actual = build_final_command(&context, &env_source, &[], &[os("true")])?;

        assert_eq!(
            actual,
            vec![
                os(SANDBOX_EXEC_PATH),
                os("-f"),
                os("/profiles/default.sb"),
                os("-D"),
                os("_USERS_DIR=/Users"),
                os("-D"),
                os("_HOME=/Users/alice"),
                os("-D"),
                os("_PROJECT_DIR=/Users/alice/project"),
                os("-D"),
                os("_TMPDIR=/tmp/alice"),
                os("/usr/bin/env"),
                os("-i"),
                os("HOME=/Users/alice"),
                os("USER="),
                os("SHELL=/bin/zsh"),
                os("TERM=xterm-256color"),
                os("LANG=en_US.UTF-8"),
                os("PATH="),
                os("TMPDIR=/tmp/alice"),
                os("true"),
            ]
        );
        Ok(())
    }

    #[test]
    fn build_final_command_uses_generated_profile_text() -> Result<()> {
        let env_source = empty_env();
        let profile =
            SandboxProfile::Text("(version 1)\n(import \"/profiles/base.sb\")\n".to_owned());
        let context = sandbox_context(&profile);

        let actual = build_final_command(&context, &env_source, &[], &[os("true")])?;

        assert_eq!(
            actual,
            vec![
                os(SANDBOX_EXEC_PATH),
                os("-p"),
                os("(version 1)\n(import \"/profiles/base.sb\")\n"),
                os("-D"),
                os("_USERS_DIR=/Users"),
                os("-D"),
                os("_HOME=/Users/alice"),
                os("-D"),
                os("_PROJECT_DIR=/Users/alice/project"),
                os("-D"),
                os("_TMPDIR=/tmp/alice"),
                os("/usr/bin/env"),
                os("-i"),
                os("HOME=/Users/alice"),
                os("USER="),
                os("SHELL=/bin/zsh"),
                os("TERM=xterm-256color"),
                os("LANG=en_US.UTF-8"),
                os("PATH="),
                os("TMPDIR=/tmp/alice"),
                os("true"),
            ]
        );
        Ok(())
    }

    #[test]
    fn accepts_system_per_user_tmpdir() {
        let result = validate_tmpdir(Path::new(
            "/private/var/folders/zz/zyxvpxvq6csfxvn_n0000000000000/T",
        ));

        assert_eq!(result.map_err(|error| error.to_string()), Ok(()));
    }

    #[test]
    fn rejects_global_tmpdir() {
        let result = validate_tmpdir(Path::new("/private/tmp"));

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some("TMPDIR must resolve under /private/var/folders: /private/tmp".to_owned())
        );
    }

    #[test]
    fn parses_seatbelt_config_yaml() -> Result<()> {
        let config: SeatbeltConfig = yaml_serde::from_str(
            r#"
profiles:
  - base.sb
  - agents/pi.sb
allowed_envs:
  - ATLASSIAN_API_TOKEN
allow_read:
  - ~/src/pi
  - docs
"#,
        )?;

        assert_eq!(
            config.profiles,
            vec![PathBuf::from("base.sb"), PathBuf::from("agents/pi.sb")]
        );
        assert_eq!(config.allowed_envs, vec!["ATLASSIAN_API_TOKEN"]);
        assert_eq!(
            config.allow_read,
            vec![PathBuf::from("~/src/pi"), PathBuf::from("docs")]
        );
        Ok(())
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
    fn rejects_config_and_profile_together() {
        let result = load_invocation_config(
            Path::new("/Users/alice"),
            Some(PathBuf::from("acme")),
            Some(PathBuf::from("raw.sb")),
            vec![],
            vec![],
        );

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some("--profile cannot be used together with --config".to_owned())
        );
    }

    #[test]
    fn load_invocation_config_combines_config_and_cli_allow_read_dirs() -> Result<()> {
        let temp = TestTempDir::new("config-allow-read")?;
        let home = temp.path().join("home");
        let profile_dir = home.join(PROFILES_SUFFIX);
        let config_read_dir = home.join("src/pi");
        let cli_read_dir = temp.path().join("obscura");
        fs::create_dir_all(&profile_dir).wrap_err_with(|| {
            format!(
                "failed to create profile directory: {}",
                profile_dir.display()
            )
        })?;
        fs::create_dir_all(&config_read_dir).wrap_err_with(|| {
            format!(
                "failed to create config allow-read directory: {}",
                config_read_dir.display()
            )
        })?;
        fs::create_dir_all(&cli_read_dir).wrap_err_with(|| {
            format!(
                "failed to create CLI allow-read directory: {}",
                cli_read_dir.display()
            )
        })?;
        fs::write(profile_dir.join("base.sb"), "; base profile\n")
            .wrap_err("failed to write test profile")?;
        fs::write(
            home.join(CONFIGS_SUFFIX).join("pi.yaml"),
            "profiles:\n  - base.sb\nallow_read:\n  - ~/src/pi\n",
        )
        .wrap_err("failed to write test config")?;

        let invocation = load_invocation_config(
            &home,
            Some(PathBuf::from("pi")),
            None,
            vec![],
            vec![cli_read_dir.clone()],
        )?;
        let expected_config_read_dir = canonicalize(
            config_read_dir,
            "failed to resolve expected config allow-read directory",
        )?;
        let expected_cli_read_dir = canonicalize(
            cli_read_dir,
            "failed to resolve expected CLI allow-read directory",
        )?;
        let SandboxProfile::Text(actual) = invocation.profile else {
            bail!("expected generated profile text")
        };

        assert_eq!(invocation.allow_env, Vec::<String>::new());
        assert!(actual.contains(&format!(
            "(literal \"{}\")",
            expected_config_read_dir.display()
        )));
        assert!(actual.contains(&format!(
            "(subpath \"{}\")",
            expected_config_read_dir.display()
        )));
        assert!(actual.contains(&format!(
            "(literal \"{}\")",
            expected_cli_read_dir.display()
        )));
        assert!(actual.contains(&format!(
            "(subpath \"{}\")",
            expected_cli_read_dir.display()
        )));
        Ok(())
    }

    #[test]
    fn resolve_allow_read_dirs_resolves_relative_directories() -> Result<()> {
        let expected = canonicalize(Path::new("src"), "failed to resolve test directory")?;

        let actual = resolve_allow_read_dirs(Path::new("/Users/alice"), &[PathBuf::from("src")])?;

        assert_eq!(actual, vec![expected]);
        Ok(())
    }

    #[test]
    fn resolve_allow_read_dirs_rejects_files() -> Result<()> {
        let resolved_file = canonicalize(Path::new("Cargo.toml"), "failed to resolve test file")?;

        let result =
            resolve_allow_read_dirs(Path::new("/Users/alice"), &[PathBuf::from("Cargo.toml")]);

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some(format!(
                "--allow-read path must be a directory: {}",
                resolved_file.display()
            ))
        );
        Ok(())
    }

    #[test]
    fn compose_import_profile_appends_allow_read_dirs() -> Result<()> {
        let actual = compose_import_profile(
            &[PathBuf::from("/profiles/raw.sb")],
            &[
                PathBuf::from("/Users/alice/docs"),
                PathBuf::from("/Volumes/Shared Stuff"),
            ],
        )?;

        assert_eq!(
            actual,
            "(version 1)\n\n(import \"/profiles/raw.sb\")\n\n; Additional read-only directories from --allow-read\n(allow file-read*\n    (literal \"/Users/alice/docs\")\n    (subpath \"/Users/alice/docs\")\n    (literal \"/Volumes/Shared Stuff\")\n    (subpath \"/Volumes/Shared Stuff\")\n)\n"
        );
        Ok(())
    }

    #[test]
    fn compose_import_profile_escapes_allow_read_dirs() -> Result<()> {
        let actual = compose_import_profile(
            &[PathBuf::from("/profiles/raw.sb")],
            &[PathBuf::from("/Users/alice/quoted\"directory")],
        )?;

        assert_eq!(
            actual,
            "(version 1)\n\n(import \"/profiles/raw.sb\")\n\n; Additional read-only directories from --allow-read\n(allow file-read*\n    (literal \"/Users/alice/quoted\\\"directory\")\n    (subpath \"/Users/alice/quoted\\\"directory\")\n)\n"
        );
        Ok(())
    }

    #[test]
    fn validates_environment_variable_names() {
        assert!(is_valid_env_name("TOKEN"));
        assert!(is_valid_env_name("_TOKEN_1"));
        assert!(!is_valid_env_name(""));
        assert!(!is_valid_env_name("1TOKEN"));
        assert!(!is_valid_env_name("BAD-NAME"));
        assert!(!is_valid_env_name("BAD.NAME"));
    }

    #[test]
    fn build_final_command_rejects_invalid_allow_env_name() {
        let env_source = empty_env();

        let profile = file_profile();
        let context = sandbox_context(&profile);

        let result =
            build_final_command(&context, &env_source, &["1TOKEN".to_owned()], &[os("true")]);

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some("invalid environment variable name: 1TOKEN".to_owned())
        );
    }

    #[test]
    fn build_final_command_rejects_unset_allow_env_name() {
        let env_source = empty_env();

        let profile = file_profile();
        let context = sandbox_context(&profile);

        let result =
            build_final_command(&context, &env_source, &["TOKEN".to_owned()], &[os("true")]);

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some("environment variable is not set: TOKEN".to_owned())
        );
    }
}
