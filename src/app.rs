use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

use eyre::{Context, Result, bail, eyre};

use crate::{
    allow_paths::{AllowAccess, project_dir_redundancy_warnings},
    cli,
    config::{InvocationConfig, load_invocation_config},
    paths::CanonicalPathBuf,
    profile::print_profile,
    sandbox_exec::{ProcessEnv, SandboxContext, build_final_command, exec_command},
    shell_quote::shell_words,
};

const REQUIRED_TMPDIR_PREFIX: &str = "/private/var/folders";

struct RunConfig {
    invocation: InvocationConfig,
    dry_run: bool,
    command: Vec<OsString>,
}

pub(crate) fn run_cli(cli: cli::Cli) -> Result<()> {
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
    let resolved_home = CanonicalPathBuf::new(&home, "failed to resolve HOME")?;
    let resolved_users_dir = CanonicalPathBuf::new(
        resolved_home
            .as_path()
            .parent()
            .ok_or_else(|| eyre!("resolved HOME has no parent: {}", resolved_home.display()))?,
        "failed to resolve users directory",
    )?;
    let mut project_dir = CanonicalPathBuf::new(
        env::current_dir().wrap_err("failed to read current directory")?,
        "failed to resolve current directory",
    )?;

    if let Some(git_root) = git_root(project_dir.as_path())? {
        project_dir = CanonicalPathBuf::new(git_root, "failed to resolve Git root")?;
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
        project_dir.as_path(),
    ) {
        eprintln!("{warning}");
    }
    for warning in project_dir_redundancy_warnings(
        AllowAccess::Write,
        &config.invocation.allow_write_paths,
        project_dir.as_path(),
    ) {
        eprintln!("{warning}");
    }

    let tmpdir = required_env_path("TMPDIR")?;
    let resolved_tmpdir = CanonicalPathBuf::new(tmpdir, "failed to resolve TMPDIR")?;
    validate_tmpdir(resolved_tmpdir.as_path())?;

    let sandbox_context = SandboxContext {
        profile: &config.invocation.profile,
        resolved_users_dir,
        resolved_home,
        project_dir,
        resolved_tmpdir,
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

pub(crate) fn required_env_path(name: &str) -> Result<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
