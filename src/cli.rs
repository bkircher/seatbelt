use std::{ffi::OsString, path::PathBuf};

use clap::{Args, Parser, Subcommand};

use crate::env_name::EnvName;

#[derive(Debug, PartialEq, Eq, Parser)]
#[command(
    name = "seatbelt",
    about = "Run commands inside a macOS sandbox-exec jail",
    subcommand_required = true,
    arg_required_else_help = true
)]
pub struct Cli {
    #[arg(
        long = "allow-env",
        value_name = "NAME",
        help = "Pass through one additional environment variable"
    )]
    pub allow_env: Vec<EnvName>,

    #[arg(
        long = "allow-read",
        value_name = "PATH",
        help = "Allow read-only access to an additional file or directory"
    )]
    pub allow_read: Vec<PathBuf>,

    #[arg(
        long = "allow-write",
        value_name = "PATH",
        help = "Allow read/write access to an additional file or directory"
    )]
    pub allow_write: Vec<PathBuf>,

    #[arg(
        long,
        value_name = "NAME_OR_PATH",
        help = "Use a YAML config by name or path"
    )]
    pub config: Option<PathBuf>,

    #[arg(
        long,
        value_name = "PATH",
        help = "Use an explicit raw sandbox profile path"
    )]
    pub profile: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Print the sandbox profile that would be loaded.
    PrintProfile,

    /// Run a command inside the sandbox.
    Run(RunArgs),
}

#[derive(Debug, PartialEq, Eq, Args)]
pub struct RunArgs {
    #[arg(
        long,
        help = "Print the final sandbox-exec command without executing it"
    )]
    pub dry_run: bool,

    #[arg(
        required = true,
        value_name = "COMMAND",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub command: Vec<OsString>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repeated_allow_env_options() {
        let actual = Cli::parse_from([
            "seatbelt",
            "--allow-env",
            "TOKEN",
            "--allow-env",
            "_TOKEN_1",
            "run",
            "true",
        ]);

        let actual_names = actual
            .allow_env
            .iter()
            .map(EnvName::as_str)
            .collect::<Vec<_>>();
        assert_eq!(actual_names, vec!["TOKEN", "_TOKEN_1"]);
    }

    #[test]
    fn rejects_invalid_allow_env_options() {
        let result = Cli::try_parse_from(["seatbelt", "--allow-env", "1TOKEN", "run", "true"]);

        assert_eq!(
            result.err().map(|error| error.kind()),
            Some(clap::error::ErrorKind::ValueValidation)
        );
    }

    #[test]
    fn parses_repeated_allow_read_options() {
        let actual = Cli::parse_from([
            "seatbelt",
            "--allow-read",
            "docs",
            "--allow-read",
            "/opt/shared",
            "run",
            "true",
        ]);

        assert_eq!(
            actual.allow_read,
            vec![PathBuf::from("docs"), PathBuf::from("/opt/shared")]
        );
    }

    #[test]
    fn parses_repeated_allow_write_options() {
        let actual = Cli::parse_from([
            "seatbelt",
            "--allow-write",
            "dist",
            "--allow-write",
            "/opt/output",
            "run",
            "true",
        ]);

        assert_eq!(
            actual.allow_write,
            vec![PathBuf::from("dist"), PathBuf::from("/opt/output")]
        );
    }

    #[test]
    fn parses_dry_run_on_run_subcommand() {
        let actual = Cli::parse_from(["seatbelt", "run", "--dry-run", "true"]);

        assert_eq!(
            actual.command,
            Command::Run(RunArgs {
                dry_run: true,
                command: vec![OsString::from("true")]
            })
        );
    }

    #[test]
    fn rejects_dry_run_on_print_profile_command() {
        let result = Cli::try_parse_from(["seatbelt", "--dry-run", "print-profile"]);

        assert_eq!(
            result.err().map(|error| error.kind()),
            Some(clap::error::ErrorKind::UnknownArgument)
        );
    }
}
