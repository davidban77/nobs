use std::path::PathBuf;

use clap::{Parser, Subcommand};
use nobs_core::compose::Compose;
use nobs_core::runner::Runner;

#[derive(Debug, Parser)]
#[command(name = "nobs", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Docker {
        #[command(subcommand)]
        command: DockerCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum DockerCommand {
    Ps {
        #[arg(long, default_value = "./docker-compose.yml")]
        compose: PathBuf,
    },
}

pub fn run(cli: Cli, runner: &dyn Runner) -> i32 {
    match cli.command {
        Command::Docker {
            command: DockerCommand::Ps { compose },
        } => docker_ps(Compose::new(compose), runner),
    }
}

fn docker_ps(compose: Compose, runner: &dyn Runner) -> i32 {
    if !compose.file().is_file() {
        eprintln!("nobs: compose file not found: {}", compose.file().display());
        return 1;
    }
    match runner.run(Compose::PROGRAM, &compose.ps_args()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("nobs: cannot run {}: {error}", Compose::PROGRAM);
            1
        }
    }
}
