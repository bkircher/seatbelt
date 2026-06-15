use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    os::unix::{ffi::OsStrExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

use eyre::{Context, Result, bail, eyre};
use serde::Deserialize;

mod cli;
mod env_name;

use env_name::EnvName;

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

struct InvocationConfig {
    allow_env: Vec<EnvName>,
    profile: SandboxProfile,
    allow_read_paths: Vec<AllowPath>,
    allow_write_paths: Vec<AllowPath>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum AllowPath {
    File(PathBuf),
    Directory(PathBuf),
}

impl AllowPath {
    fn path(&self) -> &Path {
        match self {
            Self::File(path) | Self::Directory(path) => path,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum AllowAccess {
    Read,
    Write,
}

impl AllowAccess {
    fn option_name(self) -> &'static str {
        match self {
            Self::Read => "--allow-read",
            Self::Write => "--allow-write",
        }
    }

    fn source_label(self) -> &'static str {
        match self {
            Self::Read => "allow.read/--allow-read",
            Self::Write => "allow.write/--allow-write",
        }
    }

    fn comment(self) -> &'static str {
        match self {
            Self::Read => "Additional read-only paths from allow.read/--allow-read",
            Self::Write => "Additional read/write paths from allow.write/--allow-write",
        }
    }

    fn sbpl_permissions(self) -> &'static str {
        match self {
            Self::Read => "file-read*",
            Self::Write => "file-read* file-write*",
        }
    }
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

fn main() -> Result<()> {
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
        cli.allow_write,
    )?;

    match cli.command {
        cli::Command::PrintProfile => {
            print_profile(&invocation.profile).wrap_err("print-profile command failed")
        }
        cli::Command::Run(run_args) => run(RunConfig {
            invocation,
            dry_run: run_args.dry_run,
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

    for warning in project_dir_redundancy_warnings(
        AllowAccess::Read,
        &config.invocation.allow_read_paths,
        &project_dir,
    ) {
        eprintln!("{warning}");
    }
    for warning in project_dir_redundancy_warnings(
        AllowAccess::Write,
        &config.invocation.allow_write_paths,
        &project_dir,
    ) {
        eprintln!("{warning}");
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

fn resolve_allow_paths(
    home: &Path,
    paths: &[PathBuf],
    access: AllowAccess,
) -> Result<Vec<AllowPath>> {
    let option_name = access.option_name();
    let mut resolved_paths = Vec::with_capacity(paths.len());
    for path in paths {
        let expanded_path = expand_home_path(home, path);
        let resolved_path = fs::canonicalize(&expanded_path).wrap_err_with(|| {
            format!(
                "failed to resolve {option_name} path: {}",
                expanded_path.display()
            )
        })?;
        let metadata = fs::metadata(&resolved_path).wrap_err_with(|| {
            format!(
                "failed to inspect {option_name} path: {}",
                resolved_path.display()
            )
        })?;
        let allow_path = if metadata.is_file() {
            AllowPath::File(resolved_path)
        } else if metadata.is_dir() {
            AllowPath::Directory(resolved_path)
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
    paths: &[AllowPath],
    access: AllowAccess,
) -> Result<()> {
    let option_name = access.option_name();
    let broad_directories = broad_directory_paths(home)?;

    for path in paths {
        if let AllowPath::Directory(path) = path
            && is_overly_broad_directory(path, &broad_directories)
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

fn project_dir_redundancy_warnings(
    access: AllowAccess,
    paths: &[AllowPath],
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

    let imports = profiles
        .iter()
        .map(|profile_fragment| resolve_profile_fragment(profile_root, profile_fragment))
        .collect::<Result<Vec<_>>>()?;

    compose_import_profile(&imports, &[], &[])
}

fn compose_import_profile(
    imports: &[PathBuf],
    allow_read_paths: &[AllowPath],
    allow_write_paths: &[AllowPath],
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

fn append_allow_paths(
    profile: &mut String,
    allow_paths: &[AllowPath],
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

        if matches!(path, AllowPath::Directory(_)) {
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
    allow_env: &[EnvName],
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
        env_pair_path("_USERS_DIR", sandbox_context.resolved_users_dir),
        OsString::from("-D"),
        env_pair_path("_HOME", sandbox_context.resolved_home),
        OsString::from("-D"),
        env_pair_path("_PROJECT_DIR", sandbox_context.project_dir),
        OsString::from("-D"),
        env_pair_path("_TMPDIR", sandbox_context.resolved_tmpdir),
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
        let value = env_source
            .var_os(env_name.as_str())
            .ok_or_else(|| eyre!("environment variable is not set: {env_name}"))?;
        final_command.push(env_pair(env_name.as_str(), value));
    }

    final_command.extend(command.iter().cloned());

    Ok(final_command)
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
        return bytes.iter().map(|byte| char::from(*byte)).collect();
    }

    if let Some(text) = arg.to_str() {
        return shell_single_quote(text);
    }

    shell_ansi_c_quote(bytes)
}

fn shell_single_quote(text: &str) -> String {
    let mut quoted = String::from("'");
    for character in text.chars() {
        if character == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');
    quoted
}

fn shell_ansi_c_quote(bytes: &[u8]) -> String {
    let mut quoted = String::from("$'");
    for byte in bytes {
        match *byte {
            b'\'' => quoted.push_str("\\'"),
            b'\\' => quoted.push_str("\\\\"),
            b'\n' => quoted.push_str("\\n"),
            b'\r' => quoted.push_str("\\r"),
            b'\t' => quoted.push_str("\\t"),
            0x20..=0x7e => quoted.push(char::from(*byte)),
            _ => push_shell_octal_escape(&mut quoted, *byte),
        }
    }
    quoted.push('\'');
    quoted
}

fn push_shell_octal_escape(quoted: &mut String, byte: u8) {
    quoted.push('\\');
    quoted.push(char::from(b'0' + (byte >> 6)));
    quoted.push(char::from(b'0' + ((byte >> 3) & 0o7)));
    quoted.push(char::from(b'0' + (byte & 0o7)));
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
mod tests {
    use std::{collections::BTreeMap, fs, os::unix::ffi::OsStringExt};

    use tempfile::TempDir;

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

    #[track_caller]
    fn must<T, E: std::fmt::Debug>(result: std::result::Result<T, E>) -> T {
        result.expect("fallible test setup failed")
    }

    fn env_name(value: &str) -> EnvName {
        must(EnvName::try_from(value.to_owned()))
    }

    fn temp_dir() -> TempDir {
        must(TempDir::new())
    }

    fn create_dir_all(path: &Path) {
        must(
            fs::create_dir_all(path)
                .wrap_err_with(|| format!("failed to create test directory: {}", path.display())),
        );
    }

    fn write_file(path: impl AsRef<Path>, contents: &str) {
        let path = path.as_ref();
        must(
            fs::write(path, contents)
                .wrap_err_with(|| format!("failed to write test file: {}", path.display())),
        );
    }

    fn canonicalized(path: impl AsRef<Path>, context: &'static str) -> PathBuf {
        must(canonicalize(path, context))
    }

    fn profile_text_contains(profile: &SandboxProfile, needle: &str) -> bool {
        matches!(profile, SandboxProfile::Text(actual) if actual.contains(needle))
    }

    fn profile_text_excludes(profile: &SandboxProfile, needle: &str) -> bool {
        matches!(profile, SandboxProfile::Text(actual) if !actual.contains(needle))
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
    fn build_final_command_assembles_sandbox_env_and_command() {
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

        let actual = must(build_final_command(
            &context,
            &env_source,
            &[env_name("EXTRA_TOKEN")],
            &[os("echo"), os("hello world")],
        ));

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
    }

    #[test]
    fn build_final_command_uses_defaults_for_missing_base_env() {
        let env_source = empty_env();
        let profile = file_profile();
        let context = sandbox_context(&profile);

        let actual = must(build_final_command(
            &context,
            &env_source,
            &[],
            &[os("true")],
        ));

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
    }

    #[test]
    fn build_final_command_uses_generated_profile_text() {
        let env_source = empty_env();
        let profile =
            SandboxProfile::Text("(version 1)\n(import \"/profiles/base.sb\")\n".to_owned());
        let context = sandbox_context(&profile);

        let actual = must(build_final_command(
            &context,
            &env_source,
            &[],
            &[os("true")],
        ));

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
                AllowPath::Directory(expected_config_read_dir.clone()),
                AllowPath::File(expected_cli_read_file.clone())
            ]
        );
        assert_eq!(invocation.allow_write_paths, Vec::<AllowPath>::new());
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
        assert_eq!(invocation.allow_read_paths, Vec::<AllowPath>::new());
        assert_eq!(
            invocation.allow_write_paths,
            vec![
                AllowPath::Directory(expected_config_write_dir.clone()),
                AllowPath::File(expected_cli_write_file.clone())
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
            &[AllowPath::Directory(PathBuf::from("/Users/alice/project"))],
            project_dir,
        );
        let write_warnings = project_dir_redundancy_warnings(
            AllowAccess::Write,
            &[AllowPath::File(PathBuf::from(
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

        assert_eq!(actual, vec![AllowPath::Directory(expected)]);
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

        assert_eq!(actual, vec![AllowPath::File(expected)]);
    }

    #[test]
    fn compose_import_profile_appends_allow_read_paths() {
        let actual = must(compose_import_profile(
            &[PathBuf::from("/profiles/raw.sb")],
            &[
                AllowPath::Directory(PathBuf::from("/Users/alice/docs")),
                AllowPath::File(PathBuf::from("/Users/alice/.zshrc")),
                AllowPath::Directory(PathBuf::from("/Volumes/Shared Stuff")),
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
                AllowPath::Directory(PathBuf::from("/Users/alice/dist")),
                AllowPath::File(PathBuf::from("/Users/alice/output.log")),
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
            &[AllowPath::File(PathBuf::from("/Users/alice/quoted\"file"))],
            &[],
        ));

        assert_eq!(
            actual,
            "(version 1)\n\n(import \"/profiles/raw.sb\")\n\n; Additional read-only paths from allow.read/--allow-read\n(allow file-read*\n    (literal \"/Users/alice/quoted\\\"file\")\n)\n"
        );
    }

    #[test]
    fn shell_words_preserves_non_ascii_arguments() {
        let command = [os("echo"), os("café")];

        let actual = shell_words(&command);

        assert_eq!(actual, "echo 'café'");
    }

    #[test]
    fn shell_words_escapes_non_utf8_arguments_without_replacement() {
        let command = [
            os("echo"),
            OsString::from_vec(vec![b'f', b'o', b'o', 0xff, b'b', b'a', b'r']),
        ];

        let actual = shell_words(&command);

        assert_eq!(actual, "echo $'foo\\377bar'");
    }

    #[test]
    fn build_final_command_rejects_unset_allow_env_name() {
        let env_source = empty_env();

        let profile = file_profile();
        let context = sandbox_context(&profile);

        let result =
            build_final_command(&context, &env_source, &[env_name("TOKEN")], &[os("true")]);

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some("environment variable is not set: TOKEN".to_owned())
        );
    }
}
