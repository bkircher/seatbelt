use std::{ffi::OsString, path::PathBuf};

use clap::{Args, Parser, Subcommand};

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
    pub allow_env: Vec<String>,

    #[arg(
        long = "allow-read",
        value_name = "DIR",
        help = "Allow read-only access to an additional directory"
    )]
    pub allow_read: Vec<PathBuf>,

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

    #[arg(
        long,
        help = "Print the final sandbox-exec command without executing it"
    )]
    pub dry_run: bool,

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
}
