use std::ffi::OsString;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Args, Parser, Subcommand};
use nobs_core::alertmanager::{Alert, Alertmanager, Silence};
use nobs_core::compose::{network_args, Compose, NetworkAction};
use nobs_core::containerlab::{env_names, Containerlab};
use nobs_core::env::load_env;
use nobs_core::locate::{find_workshops, locate, Kind, LocateError, Located};
use nobs_core::preflight::{preflight, Check, Status};
use nobs_core::runner::Runner;
use nobs_core::sonda::{
    build_info, check_runnable, pretty, scenario_rows, Reply, ScenarioRow, Sonda,
};
use nobs_core::status::{probe_all, Probe, State, Urls};
use nobs_core::time;
use nobs_core::workshop::{bootstrap, foreign_containers, Bootstrap, URLS};

const SERVICES_HELP: &str = "Services to act on; all of them when none is given";

const DASH: &str = "—";

const HOSTS: [&str; 5] = [
    "ghcr.io",
    "registry-1.docker.io",
    "github.com",
    "registry.opsmill.io",
    "grafana.com",
];

#[derive(Debug, Parser)]
#[command(name = "nobs", version)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = "Lab or workshop folder to use instead of the one nobs finds; NOBS_DIR sets it too"
    )]
    pub dir: Option<PathBuf>,
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
    #[command(about = "Run the containerlab topology of a lab")]
    Containerlab {
        #[command(subcommand)]
        command: ContainerlabCommand,
    },
    #[command(about = "Talk to a running sonda-server over HTTP")]
    Sonda {
        #[command(flatten)]
        flags: SondaFlags,
        #[command(subcommand)]
        command: SondaCommand,
    },
    #[command(about = "List the workshops nobs can find")]
    Workshops,
    #[command(
        about = "Check this machine, then create the .env file of every workshop nobs can find"
    )]
    Setup,
    #[command(about = "Check that this machine can run a workshop")]
    Preflight,
    #[command(external_subcommand)]
    External(Vec<OsString>),
}

#[derive(Debug, Parser)]
#[command(bin_name = "nobs <workshop>", no_binary_name = true)]
pub struct WorkshopCli {
    #[command(subcommand)]
    pub command: WorkshopCommand,
}

#[derive(Debug, Subcommand)]
pub enum WorkshopCommand {
    #[command(about = "Create the .env file of the workshop from .env.example when it is missing")]
    Setup,
    #[command(about = "Start the workshop in the background and print its addresses")]
    Up {
        #[arg(
            long,
            help = "Rebuild the images built from the workshop's files before starting"
        )]
        build: bool,
        #[arg(help = SERVICES_HELP)]
        services: Vec<String>,
    },
    #[command(about = "Stop the workshop and remove its containers; the volumes stay")]
    Down,
    #[command(about = "Stop the workshop and remove its containers and its volumes")]
    Destroy,
    #[command(
        about = "Restart the services; with none given, take the stack down and up again with a rebuild"
    )]
    Restart {
        #[arg(help = SERVICES_HELP)]
        services: Vec<String>,
    },
    #[command(about = "List the containers of the workshop, stopped ones included")]
    Ps {
        #[arg(help = SERVICES_HELP)]
        services: Vec<String>,
    },
    #[command(about = "Show the logs of one service, or of all of them")]
    Logs {
        #[arg(help = "Service to show; all of them when none is given")]
        service: Option<String>,
        #[arg(
            long,
            value_name = "N",
            default_value_t = 200,
            help = "Show only the last N lines of each service"
        )]
        tail: u32,
        #[arg(
            short,
            long,
            overrides_with = "no_follow",
            help = "Keep showing new lines as they arrive; this is on by default"
        )]
        follow: bool,
        #[arg(
            long,
            overrides_with = "follow",
            help = "Show the lines there are and stop; follow is on by default"
        )]
        no_follow: bool,
    },
    #[command(about = "Run a command inside a running service")]
    Exec {
        #[arg(help = "Service to run the command in")]
        service: String,
        #[arg(
            default_value = "sh",
            help = "Command to run; put words that start with a dash after --"
        )]
        command: Vec<OsString>,
    },
    #[command(about = "Build the images of the services, then start them")]
    Build {
        #[arg(help = SERVICES_HELP)]
        services: Vec<String>,
    },
    #[command(about = "Ask each of the seven services if it is up, and print one table")]
    Status {
        #[command(flatten)]
        flags: StatusFlags,
    },
    #[command(about = "List the active alerts of Alertmanager as one table")]
    Alerts {
        #[command(flatten)]
        flags: AlertsFlags,
    },
    #[command(about = "List the silences of Alertmanager as one table")]
    Silences {
        #[command(flatten)]
        flags: SilencesFlags,
    },
}

#[derive(Debug, Args)]
pub struct AlertsFlags {
    #[arg(
        long,
        value_name = "URL",
        help = "Address of Alertmanager; nobs looks at --am-url, then ALERTMANAGER_URL, then http://localhost:9093"
    )]
    am_url: Option<String>,
    #[arg(
        short,
        long,
        value_name = "KEY=VALUE",
        help = "Show only alerts that have this label; repeat it to ask for more labels"
    )]
    label: Vec<String>,
    #[arg(
        long,
        overrides_with = "no_silenced",
        help = "Show silenced alerts; this is on by default"
    )]
    silenced: bool,
    #[arg(
        long,
        overrides_with = "silenced",
        help = "Leave out silenced alerts; they are shown by default"
    )]
    no_silenced: bool,
    #[arg(
        long,
        overrides_with = "no_inhibited",
        help = "Show inhibited alerts; this is on by default"
    )]
    inhibited: bool,
    #[arg(
        long,
        overrides_with = "inhibited",
        help = "Leave out inhibited alerts; they are shown by default"
    )]
    no_inhibited: bool,
}

#[derive(Debug, Args)]
pub struct SilencesFlags {
    #[arg(
        long,
        value_name = "URL",
        help = "Address of Alertmanager; nobs looks at --am-url, then ALERTMANAGER_URL, then http://localhost:9093"
    )]
    am_url: Option<String>,
    #[arg(long, help = "Show expired silences too")]
    show_expired: bool,
}

#[derive(Debug, Args)]
pub struct StatusFlags {
    #[arg(
        long,
        value_name = "URL",
        help = "Address of Prometheus; nobs looks at --prom-url, then PROMETHEUS_URL, then http://localhost:9090"
    )]
    prom_url: Option<String>,
    #[arg(
        long,
        value_name = "URL",
        help = "Address of Loki; nobs looks at --loki-url, then LOKI_URL, then http://localhost:3001"
    )]
    loki_url: Option<String>,
    #[arg(
        long,
        value_name = "URL",
        help = "Address of Alertmanager; nobs looks at --am-url, then ALERTMANAGER_URL, then http://localhost:9093"
    )]
    am_url: Option<String>,
    #[arg(
        long,
        value_name = "URL",
        help = "Address of Infrahub; nobs looks at --infrahub-url, then INFRAHUB_ADDRESS, then http://localhost:8000"
    )]
    infrahub_url: Option<String>,
    #[arg(
        long,
        value_name = "URL",
        help = "Address of Grafana; nobs looks at --grafana-url, then http://localhost:3000"
    )]
    grafana_url: Option<String>,
    #[arg(
        long,
        value_name = "URL",
        help = "Address of Prefect; nobs looks at --prefect-url, then http://localhost:4200"
    )]
    prefect_url: Option<String>,
    #[arg(
        long,
        value_name = "URL",
        help = "Address of Sonda; nobs looks at --sonda-url, then http://localhost:8085"
    )]
    sonda_url: Option<String>,
}

#[derive(Debug, Args)]
pub struct SondaFlags {
    #[arg(
        long,
        global = true,
        value_name = "URL",
        help = "Address of the server; nobs looks at --server, then SONDA_SERVER_URL, then the .env file of the current folder, then http://localhost:8080"
    )]
    server: Option<String>,
    #[arg(
        long,
        global = true,
        value_name = "KEY",
        help = "Key to send with each call except health and ready; nobs looks at --api-key, then SONDA_API_KEY, then the .env file of the current folder"
    )]
    api_key: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum SondaCommand {
    #[command(about = "Check that the server is alive")]
    Health,
    #[command(about = "Check that the server is ready")]
    Ready,
    #[command(about = "List the scenarios as a table")]
    List,
    #[command(about = "Print one scenario")]
    Show {
        #[arg(help = "Id of the scenario")]
        id: String,
    },
    #[command(about = "Print the numbers of one scenario")]
    Stats {
        #[arg(help = "Id of the scenario")]
        id: String,
    },
    #[command(about = "Stop and remove one scenario")]
    Delete {
        #[arg(help = "Id of the scenario")]
        id: String,
    },
    #[command(about = "Send a scenario file to the server to run it")]
    Load {
        #[arg(help = "Scenario file; it must have kind: runnable at the start of a line")]
        file: PathBuf,
    },
    #[command(about = "Send one event file to the server")]
    Event {
        #[arg(help = "File with the event as JSON")]
        file: PathBuf,
    },
    #[command(about = "Print the metrics of the server")]
    Metrics,
    #[command(about = "Print the version of the server")]
    Version,
}

#[derive(Debug, Args)]
pub struct ContainerlabFlags {
    #[arg(
        help = "Topology file; the one under containerlab/ in the lab folder when none is given"
    )]
    topology: Option<PathBuf>,
    #[arg(
        long,
        overrides_with = "no_sudo",
        help = "Run containerlab with sudo; this is on by default"
    )]
    sudo: bool,
    #[arg(
        long,
        overrides_with = "sudo",
        help = "Run containerlab without sudo; sudo is on by default"
    )]
    no_sudo: bool,
}

#[derive(Debug, Subcommand)]
pub enum ContainerlabCommand {
    #[command(about = "Create the lab from the topology file and start its nodes")]
    Deploy {
        #[command(flatten)]
        flags: ContainerlabFlags,
    },
    #[command(about = "Stop and remove the lab of the topology file")]
    Destroy {
        #[command(flatten)]
        flags: ContainerlabFlags,
    },
    #[command(about = "List the nodes of the lab of the topology file")]
    Inspect {
        #[command(flatten)]
        flags: ContainerlabFlags,
    },
}

#[derive(Debug, Args)]
pub struct ComposeFlags {
    #[arg(
        short,
        long,
        value_name = "FILE",
        help = "Compose file to use; docker-compose.yml of the lab folder when none is given"
    )]
    compose: Option<PathBuf>,
    #[arg(
        short,
        long,
        value_name = "NAME",
        conflicts_with = "compose",
        help = "Scenario to use; its file is chapters/NAME/docker-compose.yml in the lab folder"
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
    if matches!(cli.command, Command::Sonda { .. }) {
        remove_blank_variables(&["SONDA_SERVER_URL", "SONDA_API_KEY"]);
    }
    if matches!(cli.command, Command::External(_)) {
        remove_blank_variables(&[
            "PROMETHEUS_URL",
            "LOKI_URL",
            "ALERTMANAGER_URL",
            "INFRAHUB_ADDRESS",
        ]);
    }
    let given = cli.dir.or_else(|| {
        std::env::var_os("NOBS_DIR")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
    });
    let given = given.as_deref();
    let command = match cli.command {
        Command::Docker { command } => command,
        Command::Containerlab { command } => {
            return run_containerlab_command(command, given, runner)
        }
        Command::Sonda { flags, command } => {
            return with_env(Path::new("."), || run_sonda(flags, command))
        }
        Command::Workshops => return with_env(Path::new("."), || list_workshops(given)),
        Command::Setup => {
            return with_env(Path::new("."), || {
                in_current_folder(|here| run_setup(runner, here, given, &HOSTS))
            })
        }
        Command::Preflight => {
            return with_env(Path::new("."), || {
                in_current_folder(|here| run_preflight(runner, here, &HOSTS))
            })
        }
        Command::External(words) => return run_workshop(&words, given, runner),
    };
    match command {
        DockerCommand::Build { flags, services } => run_compose(flags, given, runner, |compose| {
            vec![compose.build_args(&services)]
        }),
        DockerCommand::Pull { flags, services } => run_compose(flags, given, runner, |compose| {
            vec![compose.pull_args(&services)]
        }),
        DockerCommand::Exec {
            flags,
            service,
            command,
        } => {
            let tty = std::io::stdin().is_terminal();
            run_compose(flags, given, runner, |compose| {
                vec![compose.exec_args(&service, &command, tty)]
            })
        }
        DockerCommand::Debug { flags, services } => run_compose(flags, given, runner, |compose| {
            vec![compose.debug_args(&services)]
        }),
        DockerCommand::Start { flags, services } => run_compose(flags, given, runner, |compose| {
            vec![compose.start_args(&services)]
        }),
        DockerCommand::Stop { flags, services } => run_compose(flags, given, runner, |compose| {
            vec![compose.stop_args(&services)]
        }),
        DockerCommand::Restart { flags, services } => {
            run_compose(flags, given, runner, |compose| {
                vec![compose.restart_args(&services)]
            })
        }
        DockerCommand::Logs {
            flags,
            follow,
            tail,
            services,
        } => run_compose(flags, given, runner, |compose| {
            vec![compose.logs_args(&services, follow, tail)]
        }),
        DockerCommand::Ps { flags, services } => run_compose(flags, given, runner, |compose| {
            vec![compose.ps_args(&services)]
        }),
        DockerCommand::Destroy {
            flags,
            volumes,
            services,
        } => run_compose(flags, given, runner, |compose| {
            vec![compose.destroy_args(&services, volumes)]
        }),
        DockerCommand::Rm {
            flags,
            volumes,
            force,
            services,
        } => run_compose(flags, given, runner, |compose| {
            vec![compose.rm_args(&services, volumes, force)]
        }),
        DockerCommand::Update {
            flags,
            volumes,
            services,
        } => run_compose(flags, given, runner, |compose| {
            Vec::from(compose.update_args(&services, volumes))
        }),
        DockerCommand::Network {
            action,
            name,
            driver,
            subnet,
        } => with_env(Path::new("."), || {
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
        }),
    }
}

fn with_env(dir: &Path, arm: impl FnOnce() -> i32) -> i32 {
    if let Err(message) = load_env(dir) {
        eprintln!("nobs: {message}");
        return 1;
    }
    arm()
}

fn in_lab(given: Option<&Path>, arm: impl FnOnce(&Located) -> i32) -> i32 {
    match current_folder().and_then(|here| locate(&here, given).map_err(|error| error.to_string()))
    {
        Ok(located) => with_env(&located.dir, || arm(&located)),
        Err(message) => {
            eprintln!("nobs: {message}");
            1
        }
    }
}

fn current_folder() -> Result<PathBuf, String> {
    std::env::current_dir().map_err(|error| format!("cannot read the current folder: {error}"))
}

fn bad_settings_upward(here: &Path) -> Option<LocateError> {
    match locate(here, None) {
        Err(error @ LocateError::Settings { .. }) => Some(error),
        _ => None,
    }
}

fn in_current_folder(arm: impl FnOnce(&Path) -> i32) -> i32 {
    match current_folder() {
        Ok(here) => arm(&here),
        Err(message) => {
            eprintln!("nobs: {message}");
            1
        }
    }
}

fn workshop_list(
    here: &Path,
    given: Option<&Path>,
) -> Result<Vec<(String, String, PathBuf)>, String> {
    if let Some(error) = bad_settings_upward(here) {
        return Err(error.to_string());
    }
    let mut found = find_workshops(here);
    match locate(here, given) {
        Ok(Located {
            dir,
            kind: Kind::Workshop(workshop),
        }) => found.push((workshop.name, workshop.title, here.join(dir))),
        Ok(_) | Err(LocateError::NotFound) => {}
        Err(error) => return Err(error.to_string()),
    }
    found.sort();
    found.dedup_by(|a, b| a.2 == b.2);
    Ok(found)
}

fn list_workshops(given: Option<&Path>) -> i32 {
    in_current_folder(|here| match workshop_list(here, given) {
        Ok(found) => {
            for (name, title, folder) in found {
                println!("{name}  {title}  {}", folder.display());
            }
            0
        }
        Err(message) => {
            eprintln!("nobs: {message}");
            1
        }
    })
}

fn check_lines(checks: &[Check]) -> Vec<String> {
    checks
        .iter()
        .map(|check| {
            let status = match check.status {
                Status::Ok => "ok",
                Status::Warn => "warn",
                Status::Fail => "fail",
            };
            format!("{:<7}  {status:<4}  {}", check.name, check.detail)
                .trim_end()
                .to_string()
        })
        .collect()
}

fn print_preflight(runner: &dyn Runner, dir: &Path, hosts: &[&str]) -> bool {
    let checks = preflight(runner, dir, hosts);
    for line in check_lines(&checks) {
        println!("{line}");
    }
    checks.iter().all(|check| check.status != Status::Fail)
}

fn run_preflight(runner: &dyn Runner, dir: &Path, hosts: &[&str]) -> i32 {
    i32::from(!print_preflight(runner, dir, hosts))
}

fn bootstrap_message(dir: &Path, outcome: Bootstrap) -> String {
    match outcome {
        Bootstrap::Copied => format!("created {}/.env from .env.example", dir.display()),
        Bootstrap::Kept => format!("kept {}/.env", dir.display()),
        Bootstrap::NoExample => format!("no .env.example in {}", dir.display()),
    }
}

fn run_setup(runner: &dyn Runner, here: &Path, given: Option<&Path>, hosts: &[&str]) -> i32 {
    if !print_preflight(runner, here, hosts) {
        return 1;
    }
    let workshops = match workshop_list(here, given) {
        Ok(workshops) => workshops,
        Err(message) => {
            eprintln!("nobs: {message}");
            return 1;
        }
    };
    for (name, _, dir) in workshops {
        match bootstrap(&dir) {
            Ok(outcome) => println!("{name}: {}", bootstrap_message(&dir, outcome)),
            Err(error) => {
                eprintln!("nobs: {error}");
                return 1;
            }
        }
    }
    0
}

fn bootstrap_and_say(dir: &Path, say_kept: bool) -> Option<i32> {
    match bootstrap(dir) {
        Ok(Bootstrap::Kept) if !say_kept => None,
        Ok(outcome) => {
            println!("{}", bootstrap_message(dir, outcome));
            None
        }
        Err(error) => {
            eprintln!("nobs: {error}");
            Some(1)
        }
    }
}

fn guard(located: &Located, here: &Path, given: Option<&Path>, runner: &dyn Runner) -> Option<i32> {
    let workshops = match workshop_list(here, given) {
        Ok(workshops) => workshops,
        Err(message) => {
            eprintln!("nobs: {message}");
            return Some(1);
        }
    };
    for (name, title, dir) in workshops {
        let Some(project) = locate(here, Some(&dir))
            .ok()
            .and_then(|other| other.project().map(str::to_string))
        else {
            continue;
        };
        if Some(project.as_str()) == located.project() {
            continue;
        }
        match foreign_containers(runner, &project) {
            Ok(names) if names.is_empty() => {}
            Ok(mut names) => {
                names.sort();
                eprintln!(
                    "nobs: containers of the {title} stack are present and share names and ports with this one: {}; run nobs {name} down first",
                    names.join(", ")
                );
                return Some(1);
            }
            Err(message) => {
                eprintln!("nobs: {message}");
                return Some(1);
            }
        }
    }
    None
}

fn print_urls() {
    for (name, url) in URLS {
        println!("{name:<14}{url}");
    }
}

fn run_workshop(words: &[OsString], given: Option<&Path>, runner: &dyn Runner) -> i32 {
    let name = words[0].to_string_lossy();
    let rest = &words[1..];
    let here = match current_folder() {
        Ok(here) => here,
        Err(message) => {
            eprintln!("nobs: {message}");
            return 1;
        }
    };
    if let Some(error) = bad_settings_upward(&here) {
        eprintln!("nobs: {error}");
        return 1;
    }
    let mut known = Vec::new();
    let mut current = None;
    match locate(&here, given) {
        Ok(located) => {
            if let Kind::Workshop(workshop) = &located.kind {
                if workshop.name == name {
                    return with_env(&located.dir, || {
                        run_workshop_command(&located, rest, &here, given, runner)
                    });
                }
                current = Some(located);
            }
        }
        Err(LocateError::NotFound) => {}
        Err(error) => {
            eprintln!("nobs: {error}");
            return 1;
        }
    }
    for (found, _, dir) in find_workshops(&here) {
        if found == name {
            return in_lab(Some(&dir), |located| {
                run_workshop_command(located, rest, &here, given, runner)
            });
        }
        known.push(found);
    }
    if let Some(located) = current {
        return with_env(&located.dir, || {
            run_workshop_command(&located, words, &here, given, runner)
        });
    }
    known.sort();
    known.dedup();
    eprintln!("nobs: no workshop named {name}");
    if !known.is_empty() {
        eprintln!("known: {}", known.join(", "));
    }
    2
}

fn run_workshop_command(
    located: &Located,
    args: &[OsString],
    here: &Path,
    given: Option<&Path>,
    runner: &dyn Runner,
) -> i32 {
    let cli = match WorkshopCli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            return error.exit_code();
        }
    };
    let stack = || compose_in(located, None);
    match cli.command {
        WorkshopCommand::Setup => bootstrap_and_say(&located.dir, true).unwrap_or(0),
        WorkshopCommand::Up { build, services } => {
            if let Some(code) = guard(located, here, given, runner) {
                return code;
            }
            if let Some(code) = bootstrap_and_say(&located.dir, false) {
                return code;
            }
            let code = run_compose_file(stack(), runner, |compose| {
                vec![compose.up_args(&services, build)]
            });
            if code == 0 {
                print_urls();
            }
            code
        }
        WorkshopCommand::Down => {
            run_compose_file(stack(), runner, |compose| vec![compose.down_args()])
        }
        WorkshopCommand::Destroy => run_compose_file(stack(), runner, |compose| {
            vec![compose.destroy_args(&[], true)]
        }),
        WorkshopCommand::Restart { services } => {
            if let Some(code) = guard(located, here, given, runner) {
                return code;
            }
            if !services.is_empty() {
                return run_compose_file(stack(), runner, |compose| {
                    vec![compose.restart_args(&services)]
                });
            }
            let code = run_compose_file(stack(), runner, |compose| {
                vec![compose.down_args(), compose.up_args(&[], true)]
            });
            if code == 0 {
                print_urls();
            }
            code
        }
        WorkshopCommand::Ps { services } => {
            run_compose_file(stack(), runner, |compose| vec![compose.ps_args(&services)])
        }
        WorkshopCommand::Logs {
            service,
            tail,
            follow,
            no_follow,
        } => run_compose_file(stack(), runner, |compose| {
            vec![compose.logs_args(service.as_slice(), follow || !no_follow, Some(tail))]
        }),
        WorkshopCommand::Exec { service, command } => {
            let tty = std::io::stdin().is_terminal();
            run_compose_file(stack(), runner, |compose| {
                vec![compose.exec_args(&service, &command, tty)]
            })
        }
        WorkshopCommand::Build { services } => {
            if let Some(code) = guard(located, here, given, runner) {
                return code;
            }
            run_compose_file(stack(), runner, |compose| {
                vec![
                    compose.build_args(&services),
                    compose.up_args(&services, false),
                ]
            })
        }
        WorkshopCommand::Status { flags } => {
            let Kind::Workshop(workshop) = &located.kind else {
                unreachable!("a workshop command runs in a workshop");
            };
            run_status(flags, &workshop.name)
        }
        WorkshopCommand::Alerts { flags } => run_alerts(flags),
        WorkshopCommand::Silences { flags } => run_silences(flags),
    }
}

fn compose_in(located: &Located, scenario: Option<&str>) -> Compose {
    let compose = Compose::new(located.compose(scenario));
    match located.project() {
        Some(name) => compose.project(name),
        None => compose,
    }
}

impl ComposeFlags {
    fn options(&self, compose: Compose) -> Compose {
        self.profile
            .iter()
            .fold(compose.verbose(self.verbose), |compose, profile| {
                compose.with_profile(profile)
            })
    }
}

fn run_compose(
    flags: ComposeFlags,
    given: Option<&Path>,
    runner: &dyn Runner,
    lists: impl FnOnce(&Compose) -> Vec<Vec<OsString>>,
) -> i32 {
    match &flags.compose {
        Some(file) => with_env(Path::new("."), || {
            run_compose_file(flags.options(Compose::new(file)), runner, lists)
        }),
        None => in_lab(given, |located| {
            let compose = compose_in(located, flags.scenario.as_deref());
            run_compose_file(flags.options(compose), runner, lists)
        }),
    }
}

fn run_compose_file(
    compose: Compose,
    runner: &dyn Runner,
    lists: impl FnOnce(&Compose) -> Vec<Vec<OsString>>,
) -> i32 {
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

fn run_containerlab_command(
    command: ContainerlabCommand,
    given: Option<&Path>,
    runner: &dyn Runner,
) -> i32 {
    match command {
        ContainerlabCommand::Deploy { flags } => {
            run_containerlab(flags, given, runner, Containerlab::deploy_args)
        }
        ContainerlabCommand::Destroy { flags } => {
            run_containerlab(flags, given, runner, Containerlab::destroy_args)
        }
        ContainerlabCommand::Inspect { flags } => {
            run_containerlab(flags, given, runner, Containerlab::inspect_args)
        }
    }
}

fn run_containerlab(
    flags: ContainerlabFlags,
    given: Option<&Path>,
    runner: &dyn Runner,
    args: impl FnOnce(&Containerlab) -> Vec<OsString>,
) -> i32 {
    let sudo = flags.sudo || !flags.no_sudo;
    match flags.topology {
        Some(topology) => with_env(Path::new("."), || {
            run_topology(topology, sudo, runner, args)
        }),
        None => in_lab(given, |located| {
            run_topology(located.topology(), sudo, runner, args)
        }),
    }
}

fn run_topology(
    topology: PathBuf,
    sudo: bool,
    runner: &dyn Runner,
    args: impl FnOnce(&Containerlab) -> Vec<OsString>,
) -> i32 {
    if !topology.is_file() {
        eprintln!("nobs: topology file not found: {}", topology.display());
        return 1;
    }
    let text = match fs::read_to_string(&topology) {
        Ok(text) => text,
        Err(error) => {
            eprintln!(
                "nobs: cannot read topology file: {}: {error}",
                topology.display()
            );
            return 1;
        }
    };
    let containerlab = Containerlab::new(topology)
        .sudo(sudo)
        .preserve_env(env_names(&text));
    let program = containerlab.program();
    match runner.run(program, &args(&containerlab)) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("nobs: cannot run {program}: {error}");
            1
        }
    }
}

fn remove_blank_variables(names: &[&str]) {
    for name in names {
        if std::env::var_os(name).is_some_and(|value| value.to_string_lossy().trim().is_empty()) {
            std::env::remove_var(name);
        }
    }
}

fn given(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
}

fn urls(flags: StatusFlags, lookup: impl Fn(&str) -> Option<String>) -> Urls {
    let defaults = Urls::defaults();
    let address = |flag: Option<String>, variable: Option<&str>, default: String| {
        given(flag)
            .or_else(|| variable.and_then(|name| given(lookup(name))))
            .unwrap_or(default)
    };
    Urls {
        grafana: address(flags.grafana_url, None, defaults.grafana),
        prometheus: address(flags.prom_url, Some("PROMETHEUS_URL"), defaults.prometheus),
        loki: address(flags.loki_url, Some("LOKI_URL"), defaults.loki),
        alertmanager: address(
            flags.am_url,
            Some("ALERTMANAGER_URL"),
            defaults.alertmanager,
        ),
        infrahub: address(
            flags.infrahub_url,
            Some("INFRAHUB_ADDRESS"),
            defaults.infrahub,
        ),
        prefect: address(flags.prefect_url, None, defaults.prefect),
        sonda: address(flags.sonda_url, None, defaults.sonda),
    }
}

fn alertmanager_address(flag: Option<String>, lookup: impl Fn(&str) -> Option<String>) -> String {
    given(flag)
        .or_else(|| given(lookup("ALERTMANAGER_URL")))
        .unwrap_or_else(|| Urls::defaults().alertmanager)
}

fn label_filter(text: &str) -> Result<(String, String), String> {
    text.split_once('=')
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .ok_or_else(|| format!("--label must be key=value (got '{text}')"))
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

fn run_alerts(flags: AlertsFlags) -> i32 {
    let filters: Result<Vec<_>, _> = flags.label.iter().map(|text| label_filter(text)).collect();
    let filters = match filters {
        Ok(filters) => filters,
        Err(message) => {
            eprintln!("nobs: {message}");
            return 2;
        }
    };
    let client = Alertmanager::new(alertmanager_address(flags.am_url, |name| {
        std::env::var(name).ok()
    }));
    let alerts = match client.alerts(
        flags.silenced || !flags.no_silenced,
        flags.inhibited || !flags.no_inhibited,
    ) {
        Ok(alerts) => alerts,
        Err(error) => {
            eprintln!("nobs: {error}");
            return 1;
        }
    };
    let kept: Vec<Alert> = alerts
        .into_iter()
        .filter(|alert| {
            filters
                .iter()
                .all(|(key, value)| alert.labels.get(key) == Some(value))
        })
        .collect();
    let table = alerts_table(&kept, now_seconds());
    if kept.is_empty() {
        println!("{table}\nNo alerts match.");
    } else {
        println!("{table}");
    }
    0
}

fn run_silences(flags: SilencesFlags) -> i32 {
    let client = Alertmanager::new(alertmanager_address(flags.am_url, |name| {
        std::env::var(name).ok()
    }));
    let silences = match client.silences() {
        Ok(silences) => silences,
        Err(error) => {
            eprintln!("nobs: {error}");
            return 1;
        }
    };
    let shown: Vec<Silence> = silences
        .into_iter()
        .filter(|silence| flags.show_expired || silence.state != "expired")
        .collect();
    if shown.is_empty() {
        let states = if flags.show_expired {
            "(any state)"
        } else {
            "active or pending"
        };
        println!("No silences {states} in Alertmanager at {}.", client.url());
    } else {
        println!("{}", silences_table(&shown, now_seconds()));
    }
    0
}

fn run_status(flags: StatusFlags, workshop: &str) -> i32 {
    let urls = urls(flags, |name| std::env::var(name).ok());
    let probes = probe_all(&urls);
    println!("{}", status_table(&probes));
    if probes.iter().all(|probe| probe.state == State::Ok) {
        return 0;
    }
    println!("Some services are not ready yet: give them a minute, or check nobs {workshop} logs.");
    1
}

fn run_sonda(flags: SondaFlags, command: SondaCommand) -> i32 {
    let server = given(flags.server)
        .or_else(|| given(std::env::var("SONDA_SERVER_URL").ok()))
        .unwrap_or_else(|| Sonda::DEFAULT_URL.to_string());
    let key = given(flags.api_key).or_else(|| given(std::env::var("SONDA_API_KEY").ok()));
    let sonda = Sonda::new(server).api_key(key);
    match command {
        SondaCommand::Health => answer(sonda.health(), print_body),
        SondaCommand::Ready => answer(sonda.ready(), print_body),
        SondaCommand::List => answer(sonda.list(), print_table),
        SondaCommand::Show { id } => answer(sonda.show(&id), print_body),
        SondaCommand::Stats { id } => answer(sonda.stats(&id), print_body),
        SondaCommand::Delete { id } => answer(sonda.delete(&id), print_body),
        SondaCommand::Load { file } => {
            let Some(text) = read_file(&file, |path| fs::read_to_string(path)) else {
                return 1;
            };
            if let Err(reason) = check_runnable(&text) {
                eprintln!("nobs: {}: {reason}", file.display());
                return 1;
            }
            answer(sonda.load(text.as_bytes()), print_body)
        }
        SondaCommand::Event { file } => {
            let Some(bytes) = read_file(&file, |path| fs::read(path)) else {
                return 1;
            };
            answer(sonda.event(&bytes), print_body)
        }
        SondaCommand::Metrics => answer(sonda.metrics(), print_body),
        SondaCommand::Version => answer(sonda.metrics(), print_version),
    }
}

fn read_file<T>(path: &Path, read: impl FnOnce(&Path) -> std::io::Result<T>) -> Option<T> {
    read(path)
        .map_err(|error| eprintln!("nobs: {}: {error}", path.display()))
        .ok()
}

fn answer(result: Result<Reply, String>, print: impl FnOnce(&str) -> i32) -> i32 {
    match result {
        Err(text) => {
            eprintln!("nobs: {text}");
            1
        }
        Ok(reply) if reply.status >= 400 => {
            eprintln!("nobs: sonda answered {}: {}", reply.status, reply.body);
            1
        }
        Ok(reply) => print(&reply.body),
    }
}

fn print_body(body: &str) -> i32 {
    println!("{}", pretty(body).trim_end_matches('\n'));
    0
}

fn print_table(body: &str) -> i32 {
    match scenario_rows(body) {
        Ok(rows) => {
            println!("{}", table(&rows));
            0
        }
        Err(reason) => {
            eprintln!("nobs: sonda answered a list nobs cannot read: {reason}");
            1
        }
    }
}

fn print_version(metrics: &str) -> i32 {
    match build_info(metrics) {
        Some(info) => {
            println!("sonda-server {} {}", info.version, info.git_sha);
            0
        }
        None => {
            eprintln!("nobs: sonda answered without sonda_server_build_info");
            1
        }
    }
}

fn table(rows: &[ScenarioRow]) -> String {
    let header = ["ID", "NAME", "STATE", "ELAPSED", "DEGRADED"].map(String::from);
    let mut lines = vec![header];
    lines.extend(rows.iter().map(|row| {
        [
            row.id.clone(),
            row.name.clone(),
            row.state.clone(),
            row.elapsed_secs.clone(),
            row.degraded.clone(),
        ]
    }));
    pad_lines(&lines)
}

fn status_table(probes: &[Probe]) -> String {
    let header = ["Service", "URL", "State"].map(String::from);
    let mut lines = vec![header];
    lines.extend(probes.iter().map(|probe| {
        [
            probe.target.name.to_string(),
            probe.target.base.clone(),
            state_text(&probe.state),
        ]
    }));
    pad_lines(&lines)
}

fn alerts_table(alerts: &[Alert], now: i64) -> String {
    let header = ["Alertname", "Severity", "Device / target", "State", "Age"].map(String::from);
    let mut lines = vec![header];
    lines.extend(alerts.iter().map(|alert| alert_row(alert, now)));
    pad_lines(&lines)
}

fn alert_row(alert: &Alert, now: i64) -> [String; 5] {
    let label = |name: &str| alert.labels.get(name).map_or("", String::as_str);
    let or_dash = |text: &str| String::from(if text.is_empty() { DASH } else { text });
    let device = or_dash(label("device"));
    let target = match ["peer_address", "interface"]
        .into_iter()
        .map(label)
        .find(|text| !text.is_empty())
    {
        Some(peer) => format!("{device} → {peer}"),
        None => device,
    };
    let state = if alert.silenced {
        "silenced".to_string()
    } else if alert.inhibited {
        "inhibited".to_string()
    } else {
        match alert.state.to_lowercase().as_str() {
            "active" => "firing".to_string(),
            other => or_dash(other),
        }
    };
    [
        alert
            .labels
            .get("alertname")
            .cloned()
            .unwrap_or_else(|| DASH.to_string()),
        or_dash(label("severity")),
        target,
        state,
        time::age(&alert.starts_at, now),
    ]
}

fn silences_table(silences: &[Silence], now: i64) -> String {
    let header = [
        "ID",
        "State",
        "Starts (UTC)",
        "Ends (UTC)",
        "Remaining",
        "Matchers",
        "Creator",
    ]
    .map(String::from);
    let mut lines = vec![header];
    lines.extend(silences.iter().map(|silence| silence_row(silence, now)));
    pad_lines(&lines)
}

fn silence_row(silence: &Silence, now: i64) -> [String; 7] {
    let first = |text: &str, count: usize| text.chars().take(count).collect::<String>();
    [
        format!("{}…", first(&silence.id, 12)),
        if silence.state.is_empty() {
            "?".to_string()
        } else {
            silence.state.clone()
        },
        time::clock(&silence.starts_at),
        time::clock(&silence.ends_at),
        time::remaining(&silence.ends_at, now),
        silence
            .matchers
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(", "),
        first(&silence.created_by, 14),
    ]
}

fn state_text(state: &State) -> String {
    match state {
        State::Ok => "ok".to_string(),
        State::Http(code) => format!("http {code}"),
        State::Down(reason) => format!("down ({reason})"),
    }
}

fn pad_lines<const N: usize>(lines: &[[String; N]]) -> String {
    let widths: [usize; N] = std::array::from_fn(|i| {
        lines
            .iter()
            .map(|l| l[i].chars().count())
            .max()
            .unwrap_or(0)
    });
    lines
        .iter()
        .map(|line| {
            let cells: Vec<String> = line
                .iter()
                .zip(widths)
                .map(|(cell, width)| format!("{cell:<width$}"))
                .collect();
            cells.join("  ").trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
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

#[cfg(test)]
mod tests {
    use super::*;
    use nobs_core::runner::FakeRunner;
    use std::net::TcpListener;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("nobs-cli-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn closed_host() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("127.0.0.1:{}", listener.local_addr().unwrap().port())
    }

    fn passing() -> FakeRunner {
        FakeRunner::answering_for(&[
            ("docker", "Docker version 1\n"),
            ("sysctl", "38654705664\n"),
            (
                "df",
                "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk3s5 971350180 1000 209715200 2% /\n",
            ),
        ])
    }

    fn two_workshops(name: &str) -> Scratch {
        let scratch = Scratch::new(name);
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures");
        for (folder, source) in [
            ("one", "workshop-one"),
            ("two", "workshops-repo/workshops/nobs-t-two"),
        ] {
            let dir = scratch.0.join("workshops").join(folder);
            fs::create_dir_all(&dir).unwrap();
            fs::copy(
                fixtures.join(source).join("nobs.toml"),
                dir.join("nobs.toml"),
            )
            .unwrap();
            fs::write(dir.join(".env.example"), format!("FROM={folder}\n")).unwrap();
        }
        scratch
    }

    fn check(name: &str, status: Status, detail: &str) -> Check {
        Check {
            name: name.to_string(),
            status,
            detail: detail.to_string(),
        }
    }

    fn flags_all(text: Option<&str>) -> StatusFlags {
        let flag = || text.map(String::from);
        StatusFlags {
            prom_url: flag(),
            loki_url: flag(),
            am_url: flag(),
            infrahub_url: flag(),
            grafana_url: flag(),
            prefect_url: flag(),
            sonda_url: flag(),
        }
    }

    fn variables(name: &str) -> Option<String> {
        match name {
            "PROMETHEUS_URL" => Some("http://var-prometheus".to_string()),
            "LOKI_URL" => Some("http://var-loki".to_string()),
            "ALERTMANAGER_URL" => Some("http://var-alertmanager".to_string()),
            "INFRAHUB_ADDRESS" => Some("http://var-infrahub".to_string()),
            _ => None,
        }
    }

    fn with_variables_only() -> Urls {
        Urls {
            prometheus: "http://var-prometheus".to_string(),
            loki: "http://var-loki".to_string(),
            alertmanager: "http://var-alertmanager".to_string(),
            infrahub: "http://var-infrahub".to_string(),
            ..Urls::defaults()
        }
    }

    #[test]
    fn with_no_flag_and_no_variable_each_address_is_its_own_default() {
        assert_eq!(urls(flags_all(None), |_| None), Urls::defaults());
    }

    #[test]
    fn each_variable_gives_its_own_address_when_there_is_no_flag() {
        assert_eq!(urls(flags_all(None), variables), with_variables_only());
    }

    #[test]
    fn each_flag_gives_its_own_address_and_beats_the_variable() {
        let flags = StatusFlags {
            prom_url: Some("http://flag-prometheus".to_string()),
            loki_url: Some("http://flag-loki".to_string()),
            am_url: Some("http://flag-alertmanager".to_string()),
            infrahub_url: Some("http://flag-infrahub".to_string()),
            grafana_url: Some("http://flag-grafana".to_string()),
            prefect_url: Some("http://flag-prefect".to_string()),
            sonda_url: Some("http://flag-sonda".to_string()),
        };

        assert_eq!(
            urls(flags, variables),
            Urls {
                grafana: "http://flag-grafana".to_string(),
                prometheus: "http://flag-prometheus".to_string(),
                loki: "http://flag-loki".to_string(),
                alertmanager: "http://flag-alertmanager".to_string(),
                infrahub: "http://flag-infrahub".to_string(),
                prefect: "http://flag-prefect".to_string(),
                sonda: "http://flag-sonda".to_string(),
            }
        );
    }

    #[test]
    fn a_blank_flag_counts_as_not_given() {
        assert_eq!(urls(flags_all(Some(" ")), variables), with_variables_only());
        assert_eq!(urls(flags_all(Some("")), |_| None), Urls::defaults());
    }

    #[test]
    fn a_blank_variable_counts_as_not_given() {
        assert_eq!(
            urls(flags_all(None), |_| Some("  ".to_string())),
            Urls::defaults()
        );
    }

    #[test]
    fn the_alertmanager_address_is_the_flag_then_the_variable_then_the_default() {
        let variable = |_: &str| Some("http://var-alertmanager".to_string());

        assert_eq!(
            alertmanager_address(Some("http://flag".to_string()), variable),
            "http://flag"
        );
        assert_eq!(
            alertmanager_address(Some(" ".to_string()), variable),
            "http://var-alertmanager"
        );
        assert_eq!(
            alertmanager_address(None, variable),
            "http://var-alertmanager"
        );
        assert_eq!(
            alertmanager_address(Some(String::new()), |_| Some("  ".to_string())),
            "http://localhost:9093"
        );
    }

    const NOW: i64 = 1_793_960_130 + 42;

    fn alert(labels: &[(&str, &str)], state: &str, silenced: bool, inhibited: bool) -> Alert {
        Alert {
            labels: labels
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            state: state.to_string(),
            silenced,
            inhibited,
            starts_at: "2026-11-06T10:15:30Z".to_string(),
        }
    }

    #[test]
    fn an_alert_row_follows_the_label_and_state_rules() {
        let row = |labels: &[(&str, &str)], state: &str, silenced, inhibited| {
            alert_row(&alert(labels, state, silenced, inhibited), NOW)
        };

        assert_eq!(row(&[], "", false, false), ["—", "—", "—", "—", "42s"]);
        assert_eq!(
            row(
                &[
                    ("alertname", "A"),
                    ("severity", "critical"),
                    ("device", "srl1"),
                    ("interface", "ethernet-1/1"),
                ],
                "active",
                false,
                false
            ),
            ["A", "critical", "srl1 → ethernet-1/1", "firing", "42s"]
        );
        assert_eq!(
            row(
                &[
                    ("severity", ""),
                    ("peer_address", "10.0.0.2"),
                    ("interface", "ethernet-1/1")
                ],
                "active",
                false,
                true
            ),
            ["—", "—", "— → 10.0.0.2", "inhibited", "42s"]
        );
        assert_eq!(
            row(
                &[("device", "srl1"), ("peer_address", "")],
                "Suppressed",
                false,
                false
            )[2..4],
            ["srl1", "suppressed"]
        );
        assert_eq!(row(&[], "ACTIVE", true, true)[3], "silenced");
    }

    #[test]
    fn a_silence_row_cuts_the_id_and_creator_by_character_and_marks_a_missing_state() {
        let silence = Silence {
            id: "é".repeat(13),
            state: String::new(),
            starts_at: "2026-11-06T10:00:00.000Z".to_string(),
            ends_at: "2026-11-06T10:17:35.000Z".to_string(),
            matchers: vec![
                ("alertname".to_string(), "A".to_string()),
                ("device".to_string(), "srl1".to_string()),
            ],
            created_by: "ü".repeat(15),
        };

        assert_eq!(
            silence_row(&silence, NOW - 42),
            [
                format!("{}…", "é".repeat(12)),
                "?".to_string(),
                "10:00:00".to_string(),
                "10:17:35".to_string(),
                "2m5s".to_string(),
                "alertname=A, device=srl1".to_string(),
                "ü".repeat(14),
            ]
        );
    }

    #[test]
    fn the_checks_are_lines_with_padded_name_and_status_and_no_trailing_spaces() {
        let checks = [
            check("docker", Status::Ok, "Docker version 27.0.1"),
            check("compose", Status::Ok, ""),
            check("memory", Status::Warn, "7 GiB"),
            check("disk", Status::Fail, "4 GiB free"),
            check("network", Status::Warn, "no answer from github.com"),
        ];

        assert_eq!(
            check_lines(&checks),
            [
                "docker   ok    Docker version 27.0.1",
                "compose  ok",
                "memory   warn  7 GiB",
                "disk     fail  4 GiB free",
                "network  warn  no answer from github.com",
            ]
        );
    }

    #[test]
    fn preflight_exits_1_when_a_check_fails_and_0_when_none_does() {
        let scratch = Scratch::new("preflight");
        let host = closed_host();

        assert_eq!(
            run_preflight(&FakeRunner::failing(), &scratch.0, &[&host]),
            1
        );
        assert_eq!(run_preflight(&passing(), &scratch.0, &[&host]), 0);
    }

    #[test]
    fn setup_creates_the_env_file_of_every_workshop_after_a_passing_preflight() {
        let scratch = two_workshops("setup-passes");
        let host = closed_host();

        let code = run_setup(&passing(), &scratch.0, None, &[&host]);

        assert_eq!(code, 0);
        for folder in ["one", "two"] {
            let env = scratch.0.join("workshops").join(folder).join(".env");
            assert_eq!(
                fs::read_to_string(&env).unwrap(),
                format!("FROM={folder}\n"),
                "{folder}"
            );
        }
    }

    #[test]
    fn setup_creates_nothing_after_a_failed_preflight() {
        let scratch = two_workshops("setup-fails");
        let host = closed_host();

        let code = run_setup(&FakeRunner::failing(), &scratch.0, None, &[&host]);

        assert_eq!(code, 1);
        for folder in ["one", "two"] {
            let env = scratch.0.join("workshops").join(folder).join(".env");
            assert!(!env.exists(), "{folder}");
        }
    }
}
