use clap::Parser;
use nobs_cli::{run, Cli};
use nobs_core::runner::RealRunner;

fn main() {
    std::process::exit(run(Cli::parse(), &RealRunner));
}
