use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use nobs_core::compose::{network_args, Compose, NetworkAction};
use nobs_core::env::load_env;
use nobs_core::runner::Runner;

const SERVICES_HELP: &str = "Services to act on; all of them when none is given";

#[derive(Debug, Parser)]
#[command(name = "nobs", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(about = "Run the Docker Compose stack of a lab")]
    Docker {
        #[command(subcommand)]
        command: DockerCommand,
    },
}

#[derive(Debug, Args)]
pub struct ComposeFlags {
    #[arg(
        short,
        long,
        value_name = "FILE",
        help = "Compose file to use; ./docker-compose.yml when none is given"
    )]
    compose: Option<PathBuf>,
    #[arg(
        short,
        long,
        value_name = "NAME",
        conflicts_with = "compose",
        help = "Scenario to use; its file is ./chapters/NAME/docker-compose.yml"
    )]
    scenario: Option<String>,
    #[arg(
        short,
        long,
        value_name = "NAME",
        help = "Compose profile to enable; repeat the flag for several"
    )]
    profile: Vec<String>,
    #[arg(long, help = "Make Docker Compose print more")]
    verbose: bool,
}

#[derive(Debug, Subcommand)]
pub enum DockerCommand {
    #[command(about = "Build the images of the services")]
    Build {
        #[command(flatten)]
        flags: ComposeFlags,
        #[arg(help = SERVICES_HELP)]
        services: Vec<String>,
    },
    #[command(about = "Pull the images of the services")]
    Pull {
        #[command(flatten)]
        flags: ComposeFlags,
        #[arg(help = SERVICES_HELP)]
        services: Vec<String>,
    },
    #[command(about = "Run a command inside a running service")]
    Exec {
        #[command(flatten)]
        flags: ComposeFlags,
        #[arg(help = "Service to run the command in")]
        service: String,
        #[arg(
            default_value = "bash",
            help = "Command to run; put words that start with a dash after --"
        )]
        command: Vec<OsString>,
    },
    #[command(about = "Start the services in the foreground, with their output")]
    Debug {
        #[command(flatten)]
        flags: ComposeFlags,
        #[arg(help = SERVICES_HELP)]
        services: Vec<String>,
    },
    #[command(about = "Start the services in the background")]
    Start {
        #[command(flatten)]
        flags: ComposeFlags,
        #[arg(help = SERVICES_HELP)]
        services: Vec<String>,
    },
    #[command(about = "Stop the services without removing them")]
    Stop {
        #[command(flatten)]
        flags: ComposeFlags,
        #[arg(help = SERVICES_HELP)]
        services: Vec<String>,
    },
    #[command(about = "Restart the services")]
    Restart {
        #[command(flatten)]
        flags: ComposeFlags,
        #[arg(help = SERVICES_HELP)]
        services: Vec<String>,
    },
    #[command(about = "Show the logs of the services")]
    Logs {
        #[command(flatten)]
        flags: ComposeFlags,
        #[arg(short, long, help = "Keep showing new lines as they arrive")]
        follow: bool,
        #[arg(
            short,
            long,
            value_name = "N",
            help = "Show only the last N lines of each service"
        )]
        tail: Option<u32>,
        #[arg(help = SERVICES_HELP)]
        services: Vec<String>,
    },
    #[command(about = "List the containers of the services, stopped ones included")]
    Ps {
        #[command(flatten)]
        flags: ComposeFlags,
        #[arg(help = SERVICES_HELP)]
        services: Vec<String>,
    },
    #[command(about = "Stop and remove the containers and networks of the lab")]
    Destroy {
        #[command(flatten)]
        flags: ComposeFlags,
        #[arg(long, help = "Remove the named volumes too")]
        volumes: bool,
        #[arg(help = SERVICES_HELP)]
        services: Vec<String>,
    },
    #[command(about = "Stop and remove the containers of the services")]
    Rm {
        #[command(flatten)]
        flags: ComposeFlags,
        #[arg(long, help = "Remove the anonymous volumes of the containers too")]
        volumes: bool,
        #[arg(long, help = "Do not ask for confirmation")]
        force: bool,
        #[arg(help = SERVICES_HELP)]
        services: Vec<String>,
    },
    #[command(about = "Pull the images of the services and recreate their containers")]
    Update {
        #[command(flatten)]
        flags: ComposeFlags,
        #[arg(long, help = "Give the recreated containers fresh anonymous volumes")]
        volumes: bool,
        #[arg(help = SERVICES_HELP)]
        services: Vec<String>,
    },
    #[command(about = "Manage Docker networks")]
    Network {
        #[arg(
            value_parser = parse_network_action,
            help = "One of connect, create, disconnect, inspect, ls, prune, rm"
        )]
        action: NetworkAction,
        #[arg(short, long, help = "Name of the network")]
        name: Option<String>,
        #[arg(long, help = "Driver of a network to create")]
        driver: Option<String>,
        #[arg(long, help = "Subnet of a network to create")]
        subnet: Option<String>,
    },
}

pub fn run(cli: Cli, runner: &dyn Runner) -> i32 {
    if let Err(message) = load_env(Path::new(".")) {
        eprintln!("nobs: {message}");
        return 1;
    }
    let Command::Docker { command } = cli.command;
    match command {
        DockerCommand::Build { flags, services } => {
            run_compose(flags, runner, |compose| vec![compose.build_args(&services)])
        }
        DockerCommand::Pull { flags, services } => {
            run_compose(flags, runner, |compose| vec![compose.pull_args(&services)])
        }
        DockerCommand::Exec {
            flags,
            service,
            command,
        } => {
            let tty = std::io::stdin().is_terminal();
            run_compose(flags, runner, |compose| {
                vec![compose.exec_args(&service, &command, tty)]
            })
        }
        DockerCommand::Debug { flags, services } => {
            run_compose(flags, runner, |compose| vec![compose.debug_args(&services)])
        }
        DockerCommand::Start { flags, services } => {
            run_compose(flags, runner, |compose| vec![compose.start_args(&services)])
        }
        DockerCommand::Stop { flags, services } => {
            run_compose(flags, runner, |compose| vec![compose.stop_args(&services)])
        }
        DockerCommand::Restart { flags, services } => run_compose(flags, runner, |compose| {
            vec![compose.restart_args(&services)]
        }),
        DockerCommand::Logs {
            flags,
            follow,
            tail,
            services,
        } => run_compose(flags, runner, |compose| {
            vec![compose.logs_args(&services, follow, tail)]
        }),
        DockerCommand::Ps { flags, services } => {
            run_compose(flags, runner, |compose| vec![compose.ps_args(&services)])
        }
        DockerCommand::Destroy {
            flags,
            volumes,
            services,
        } => run_compose(flags, runner, |compose| {
            vec![compose.destroy_args(&services, volumes)]
        }),
        DockerCommand::Rm {
            flags,
            volumes,
            force,
            services,
        } => run_compose(flags, runner, |compose| {
            vec![compose.rm_args(&services, volumes, force)]
        }),
        DockerCommand::Update {
            flags,
            volumes,
            services,
        } => run_compose(flags, runner, |compose| {
            Vec::from(compose.update_args(&services, volumes))
        }),
        DockerCommand::Network {
            action,
            name,
            driver,
            subnet,
        } => {
            let needs_name = !matches!(action, NetworkAction::Ls | NetworkAction::Prune);
            if needs_name && name.is_none() {
                eprintln!("nobs: network {} needs --name", network_word(action));
                return 2;
            }
            run_docker(
                runner,
                &network_args(
                    action,
                    name.as_deref(),
                    driver.as_deref(),
                    subnet.as_deref(),
                ),
            )
        }
    }
}

impl ComposeFlags {
    fn compose(self) -> Compose {
        let file = match (self.compose, self.scenario) {
            (Some(file), _) => file,
            (None, Some(name)) => Path::new("./chapters")
                .join(name)
                .join("docker-compose.yml"),
            (None, None) => PathBuf::from("./docker-compose.yml"),
        };
        self.profile.iter().fold(
            Compose::new(file).verbose(self.verbose),
            |compose, profile| compose.with_profile(profile),
        )
    }
}

fn run_compose(
    flags: ComposeFlags,
    runner: &dyn Runner,
    lists: impl FnOnce(&Compose) -> Vec<Vec<OsString>>,
) -> i32 {
    let compose = flags.compose();
    if !compose.file().is_file() {
        eprintln!("nobs: compose file not found: {}", compose.file().display());
        return 1;
    }
    for args in lists(&compose) {
        let code = run_docker(runner, &args);
        if code != 0 {
            return code;
        }
    }
    0
}

fn run_docker(runner: &dyn Runner, args: &[OsString]) -> i32 {
    match runner.run(Compose::PROGRAM, args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("nobs: cannot run {}: {error}", Compose::PROGRAM);
            1
        }
    }
}

const NETWORK_ACTIONS: [(&str, NetworkAction); 7] = [
    ("connect", NetworkAction::Connect),
    ("create", NetworkAction::Create),
    ("disconnect", NetworkAction::Disconnect),
    ("inspect", NetworkAction::Inspect),
    ("ls", NetworkAction::Ls),
    ("prune", NetworkAction::Prune),
    ("rm", NetworkAction::Rm),
];

fn parse_network_action(word: &str) -> Result<NetworkAction, String> {
    NETWORK_ACTIONS
        .iter()
        .find(|(known, _)| *known == word)
        .map(|(_, action)| *action)
        .ok_or_else(|| format!("unknown network action: {word}"))
}

fn network_word(action: NetworkAction) -> &'static str {
    NETWORK_ACTIONS
        .iter()
        .find(|(_, known)| *known == action)
        .map_or("", |(word, _)| word)
}
