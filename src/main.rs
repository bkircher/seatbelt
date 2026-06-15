use eyre::{Context, Result};

mod allow_paths;
mod app;
mod cli;
mod config;
mod env_name;
mod paths;
mod profile;
mod sandbox_exec;
mod shell_quote;

#[cfg(test)]
mod test_support;

fn main() -> Result<()> {
    use clap::Parser;

    color_eyre::install().wrap_err("failed to install color-eyre error reports")?;

    let cli = cli::Cli::parse();
    app::run_cli(cli)
}
