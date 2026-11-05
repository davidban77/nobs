use std::ffi::OsString;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use nobs_core::compose::{network_args, Compose, NetworkAction};
use nobs_core::containerlab::{env_names, Containerlab};
use nobs_core::env::load_env;
use nobs_core::locate::{find_workshops, locate, Kind, LocateError, Located};
use nobs_core::runner::Runner;
use nobs_core::sonda::{
    build_info, check_runnable, pretty, scenario_rows, Reply, ScenarioRow, Sonda,
};

const SERVICES_HELP: &str = "Services to act on; all of them when none is given";

#[derive(Debug, Parser)]
#[command(name = "nobs", version)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = "Lab or workshop folder to use instead of the one nobs finds"
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
    #[command(about = "List the containers of the workshop, stopped ones included")]
    Ps {
        #[arg(help = SERVICES_HELP)]
        services: Vec<String>,
    },
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
        remove_blank_sonda_variables();
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

fn list_workshops(given: Option<&Path>) -> i32 {
    let here = match current_folder() {
        Ok(here) => here,
        Err(message) => {
            eprintln!("nobs: {message}");
            return 1;
        }
    };
    let mut found = find_workshops(&here);
    match locate(&here, given) {
        Ok(Located {
            dir,
            kind: Kind::Workshop(workshop),
        }) => found.push((workshop.name, workshop.title, here.join(dir))),
        Ok(_) | Err(LocateError::NotFound) => {}
        Err(error) => {
            eprintln!("nobs: {error}");
            return 1;
        }
    }
    found.sort();
    found.dedup_by(|a, b| a.2 == b.2);
    for (name, title, folder) in found {
        println!("{name}  {title}  {}", folder.display());
    }
    0
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
    let mut known = Vec::new();
    match locate(&here, given) {
        Ok(located) => {
            if let Kind::Workshop(workshop) = &located.kind {
                if workshop.name == name {
                    return with_env(&located.dir, || {
                        run_workshop_command(&located, rest, runner)
                    });
                }
                known.push(workshop.name.clone());
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
                run_workshop_command(located, rest, runner)
            });
        }
        known.push(found);
    }
    known.sort();
    known.dedup();
    eprintln!("nobs: no workshop named {name}");
    if !known.is_empty() {
        eprintln!("known: {}", known.join(", "));
    }
    2
}

fn run_workshop_command(located: &Located, args: &[OsString], runner: &dyn Runner) -> i32 {
    let cli = match WorkshopCli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            return error.exit_code();
        }
    };
    match cli.command {
        WorkshopCommand::Ps { services } => {
            run_compose_file(compose_in(located, None), runner, |compose| {
                vec![compose.ps_args(&services)]
            })
        }
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

fn remove_blank_sonda_variables() {
    for name in ["SONDA_SERVER_URL", "SONDA_API_KEY"] {
        if std::env::var_os(name).is_some_and(|value| value.to_string_lossy().trim().is_empty()) {
            std::env::remove_var(name);
        }
    }
}

fn given(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
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
    let widths: [usize; 5] = std::array::from_fn(|i| {
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
