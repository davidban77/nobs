use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Args, Parser, Subcommand};
use nobs_core::alertmanager::{Alert, Alertmanager, Silence};
use nobs_core::compose::{network_args, Compose, NetworkAction};
use nobs_core::containerlab::{env_names, Containerlab};
use nobs_core::cycle::{self, decision_record, Group};
use nobs_core::env::load_env;
use nobs_core::evidence::{self, decode_admin, decode_oper, rca_record, value_text, Metrics};
use nobs_core::infrahub::{
    annotation, host_address, read_schema, DeviceIntent, Error as InfrahubError, Infrahub,
    IntentSession, Seed, DEFAULT_ADDRESS, KINDS,
};
use nobs_core::lab_vars;
use nobs_core::locate::{find_workshops, locate, Kind, LocateError, Located};
use nobs_core::loki::{self, Loki};
use nobs_core::prefect::{self, FlowRun, Prefect};
use nobs_core::preflight::{preflight, Check, Status};
use nobs_core::prometheus::{self, Prometheus};
use nobs_core::runner::Runner;
use nobs_core::sonda::{
    build_info, check_runnable, pretty, scenario_rows, Reply, ScenarioRow, Sonda,
};
use nobs_core::status::{probe_all, Probe, State, Urls};
use nobs_core::time;
use nobs_core::webhook;
use nobs_core::workshop::{bootstrap, foreign_containers, Bootstrap, URLS};

const SERVICES_HELP: &str = "Services to act on; all of them when none is given";

const DASH: &str = "—";

const LOKI_SINK: &str = "http://loki:3001";

const LOKI_ADDRESS: (&str, &str) = ("LOKI_URL", loki::DEFAULT_URL);

const PROMETHEUS_ADDRESS: (&str, &str) = ("PROMETHEUS_URL", prometheus::DEFAULT_URL);

const PREFECT_ADDRESS: (&str, &str) = ("PREFECT_API_URL", prefect::DEFAULT_URL);

const WEBHOOK_ADDRESS: (&str, &str) = ("WEBHOOK_URL", webhook::DEFAULT_URL);

const ALERTNAME: &str = "BgpSessionNotUp";

const FLOW_RUN_LIMIT: u64 = 100;

const TRIGGER_WINDOW_MINUTES: u64 = 120;

const TRIGGER_TRIES: u32 = 15;

const TRIGGER_SECONDS: i64 = 30;

const TRIGGER_PAUSE: Duration = Duration::from_secs(2);

const SILENCES_SHOWN: usize = 5;

const GROUPS_SHOWN: usize = 5;

const TOKEN_REQUIRED: &str = "INFRAHUB_API_TOKEN is required (set it in .env or pass --token).";

const KIND_WAIT: Duration = Duration::from_secs(30);

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
    #[command(about = "Apply the schema to Infrahub, then load the lab variables into it")]
    LoadInfrahub {
        #[command(flatten)]
        flags: LoadInfrahubFlags,
    },
    #[command(about = "Manage the Infrahub schema")]
    Schema {
        #[command(subcommand)]
        command: SchemaCommand,
    },
    #[command(about = "Set or clear the maintenance flag of a device in Infrahub")]
    Maintenance {
        #[command(flatten)]
        flags: MaintenanceFlags,
    },
    #[command(about = "Print the most recent AI RCA narratives from Loki")]
    Rca {
        #[command(flatten)]
        flags: RcaFlags,
    },
    #[command(
        about = "Print the source-of-truth gate, BGP metrics snapshot, recent log lines, and a policy hint for one device and peer"
    )]
    Evidence {
        #[command(flatten)]
        flags: EvidenceFlags,
    },
    #[command(
        about = "Capture the workflow's full cycle state for one device and peer; optionally re-drive it"
    )]
    Cycle {
        #[command(flatten)]
        flags: CycleFlags,
    },
    #[command(about = "Render a fake on-call page from a live alert")]
    Page {
        #[command(flatten)]
        flags: PageFlags,
    },
}

#[derive(Debug, Args)]
pub struct CycleFlags {
    #[arg(help = "Device name (e.g. srl1).")]
    device: String,
    #[arg(help = "Peer IP (e.g. 10.1.99.2).")]
    peer: String,
    #[arg(
        long,
        help = "Post a fresh alert payload through the webhook before rendering."
    )]
    trigger: bool,
    #[arg(
        long,
        value_name = "STATUS",
        default_value = "firing",
        value_parser = ["firing", "resolved"],
        hide_default_value = true,
        help = "Alert status to post with --trigger: 'firing' (default) or 'resolved'."
    )]
    status: String,
    #[arg(
        long,
        value_name = "N",
        default_value_t = 30,
        help = "Lookback window for recent panels."
    )]
    minutes: u64,
    #[arg(
        long,
        value_name = "URL",
        help = "Address of Alertmanager; nobs looks at --am-url, then ALERTMANAGER_URL, then http://localhost:9093"
    )]
    am_url: Option<String>,
    #[arg(
        long,
        value_name = "URL",
        help = "Address of Loki; nobs looks at --loki-url, then LOKI_URL, then http://localhost:3001"
    )]
    loki_url: Option<String>,
    #[arg(
        long,
        value_name = "URL",
        help = "Address of Prefect; nobs looks at --prefect-url, then PREFECT_API_URL, then http://localhost:4200"
    )]
    prefect_url: Option<String>,
    #[arg(
        long,
        value_name = "URL",
        help = "Address of the webhook; nobs looks at --webhook-url, then WEBHOOK_URL, then http://localhost:9997/v1/api/webhook"
    )]
    webhook_url: Option<String>,
}

#[derive(Debug, Args)]
pub struct PageFlags {
    #[arg(
        short,
        long,
        value_name = "ALERT",
        default_value = ALERTNAME,
        hide_default_value = true,
        help = "Alertname to filter by; the default is BgpSessionNotUp"
    )]
    alert: String,
    #[arg(
        short,
        long,
        value_name = "DEVICE",
        default_value = "",
        hide_default_value = true,
        help = "Filter to alerts on this device."
    )]
    device: String,
    #[arg(
        short,
        long,
        value_name = "PEER",
        default_value = "",
        hide_default_value = true,
        help = "Filter to alerts on this peer_address."
    )]
    peer: String,
    #[arg(
        long,
        value_name = "HH:MM",
        help = "Override the wall-clock printed in the page (HH:MM). Default: real time."
    )]
    now: Option<String>,
    #[arg(
        long,
        value_name = "URL",
        help = "Address of Alertmanager; nobs looks at --am-url, then ALERTMANAGER_URL, then http://localhost:9093"
    )]
    am_url: Option<String>,
}

#[derive(Debug, Args)]
pub struct RcaFlags {
    #[arg(help = "Device name (e.g. srl1). Optional — defaults to any device.")]
    device: Option<String>,
    #[arg(help = "Peer IP (e.g. 10.1.99.2). Optional — defaults to any peer.")]
    peer: Option<String>,
    #[arg(
        long,
        value_name = "N",
        default_value_t = 1,
        help = "Number of most-recent records to show."
    )]
    last: u64,
    #[arg(
        long,
        value_name = "N",
        default_value_t = 60,
        help = "How far back to look."
    )]
    minutes: u64,
    #[arg(
        long,
        value_name = "URL",
        help = "Address of Loki; nobs looks at --loki-url, then LOKI_URL, then http://localhost:3001"
    )]
    loki_url: Option<String>,
}

#[derive(Debug, Args)]
pub struct EvidenceFlags {
    #[arg(help = "Device name (e.g. srl1).")]
    device: String,
    #[arg(help = "Peer IP address (e.g. 10.1.99.2).")]
    peer: String,
    #[arg(
        long,
        value_name = "NAME",
        default_value = "ipv4-unicast",
        help = "Address family of the BGP session"
    )]
    afi_safi: String,
    #[arg(
        long,
        value_name = "NAME",
        default_value = "default",
        help = "Name of the network instance of the BGP session"
    )]
    instance: String,
    #[arg(
        long,
        value_name = "N",
        default_value_t = 10,
        help = "How many minutes back to look for log lines"
    )]
    log_minutes: u64,
    #[arg(
        long,
        value_name = "N",
        default_value_t = 20,
        help = "How many log lines to show at most"
    )]
    log_limit: u64,
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
        help = "Address of Infrahub; nobs looks at --infrahub-url, then INFRAHUB_ADDRESS, then http://localhost:8000"
    )]
    infrahub_url: Option<String>,
    #[arg(
        long,
        value_name = "TOKEN",
        help = "Token for Infrahub; nobs looks at --token, then INFRAHUB_API_TOKEN"
    )]
    token: Option<String>,
}

#[derive(Debug, Args)]
pub struct InfrahubFlags {
    #[arg(
        long,
        value_name = "URL",
        help = "Address of Infrahub; nobs looks at --address, then INFRAHUB_ADDRESS, then http://localhost:8000"
    )]
    address: Option<String>,
    #[arg(
        long,
        value_name = "TOKEN",
        help = "Token for Infrahub; nobs looks at --token, then INFRAHUB_API_TOKEN"
    )]
    token: Option<String>,
}

#[derive(Debug, Args)]
pub struct LoadInfrahubFlags {
    #[arg(
        long,
        value_name = "PATH",
        help = "Schema file; the schema_file setting of nobs.toml when none is given; a relative path is taken from the workshop folder"
    )]
    schema: Option<PathBuf>,
    #[arg(
        long,
        value_name = "PATH",
        help = "Lab variables file; lab_vars.yml when none is given; a relative path is taken from the workshop folder"
    )]
    lab_vars: Option<PathBuf>,
    #[arg(long, help = "Do not apply the schema; only load the lab variables")]
    skip_schema: bool,
    #[command(flatten)]
    infrahub: InfrahubFlags,
}

#[derive(Debug, Args)]
pub struct MaintenanceFlags {
    #[arg(
        short,
        long,
        value_name = "NAME",
        help = "Name of the device in Infrahub"
    )]
    device: String,
    #[arg(
        long,
        overrides_with = "clear",
        help = "Set the maintenance flag to true; this is the default"
    )]
    state: bool,
    #[arg(
        long,
        overrides_with = "state",
        help = "Set the maintenance flag to false"
    )]
    clear: bool,
    #[arg(
        long,
        value_name = "KIND",
        default_value = "WorkshopDevice",
        help = "Kind of the Infrahub node to update"
    )]
    kind: String,
    #[command(flatten)]
    infrahub: InfrahubFlags,
}

#[derive(Debug, Subcommand)]
pub enum SchemaCommand {
    #[command(about = "Apply a schema file to Infrahub")]
    Load {
        #[arg(help = "Schema file; a relative path is taken from the workshop folder")]
        path: PathBuf,
        #[command(flatten)]
        infrahub: InfrahubFlags,
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
        help = "Show only alerts that have this label; repeat it for more labels, and an alert must have them all"
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
            "PREFECT_API_URL",
            "WEBHOOK_URL",
            "INFRAHUB_ADDRESS",
            "INFRAHUB_API_TOKEN",
            "SONDA_SERVER_URL",
            "SONDA_LOKI_SINK_URL",
            "SONDA_API_KEY",
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
        WorkshopCommand::LoadInfrahub { flags } => run_load_infrahub(flags, located),
        WorkshopCommand::Schema {
            command: SchemaCommand::Load { path, infrahub },
        } => run_schema_load(&located.dir.join(path), infrahub),
        WorkshopCommand::Maintenance { flags } => run_maintenance(flags),
        WorkshopCommand::Rca { flags } => run_rca(flags, located),
        WorkshopCommand::Evidence { flags } => run_evidence(flags, located),
        WorkshopCommand::Cycle { flags } => run_cycle(flags, located),
        WorkshopCommand::Page { flags } => run_page(flags, located),
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

fn seconds_of(moment: SystemTime) -> i64 {
    moment
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

fn now_seconds() -> i64 {
    seconds_of(SystemTime::now())
}

fn run_alerts(flags: AlertsFlags) -> i32 {
    let filters: Result<BTreeMap<_, _>, _> =
        flags.label.iter().map(|text| label_filter(text)).collect();
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

fn infrahub_settings(
    flags: InfrahubFlags,
    lookup: impl Fn(&str) -> Option<String>,
) -> (String, Option<String>) {
    let address = given(flags.address)
        .or_else(|| given(lookup("INFRAHUB_ADDRESS")))
        .unwrap_or_else(|| DEFAULT_ADDRESS.to_string());
    let token = given(flags.token).or_else(|| given(lookup("INFRAHUB_API_TOKEN")));
    (address, token)
}

fn require_token(token: Option<String>) -> Option<String> {
    if token.is_none() {
        eprintln!("nobs: {TOKEN_REQUIRED}");
    }
    token
}

fn announce_rewrite(address: &str) {
    let host = host_address(address);
    let host = host.trim_end_matches('/');
    if host != address.trim_end_matches('/') {
        println!("INFRAHUB_ADDRESS rewritten to host-reachable {host}");
    }
}

fn infrahub_failed(error: impl std::fmt::Display) -> i32 {
    eprintln!("nobs: {error}");
    1
}

fn is_missing(file: &Path, label: &str) -> bool {
    let missing = !file.is_file();
    if missing {
        eprintln!("nobs: {label} file not found: {}", file.display());
    }
    missing
}

fn read_input<T>(file: &Path, label: &str, parse: fn(&str) -> Result<T, String>) -> Option<T> {
    if is_missing(file, label) {
        return None;
    }
    let read = fs::read_to_string(file)
        .map_err(|error| error.to_string())
        .and_then(|text| parse(&text));
    match read {
        Ok(value) => Some(value),
        Err(reason) => {
            eprintln!("nobs: cannot read {}: {reason}", file.display());
            None
        }
    }
}

fn apply_schema(file: &Path, load: impl FnOnce() -> Result<bool, InfrahubError>) -> i32 {
    println!("Applying schema {}", file.display());
    match load() {
        Ok(_) => {
            println!("Schema applied");
            0
        }
        Err(error) => infrahub_failed(error),
    }
}

fn run_load_infrahub(flags: LoadInfrahubFlags, located: &Located) -> i32 {
    let Kind::Workshop(workshop) = &located.kind else {
        unreachable!("a workshop command runs in a workshop");
    };
    let (address, token) = infrahub_settings(flags.infrahub, |name| std::env::var(name).ok());
    announce_rewrite(&address);
    let Some(token) = require_token(token) else {
        return 1;
    };
    let infrahub = Infrahub::new(&address, &token);
    let schema_file = located
        .dir
        .join(flags.schema.as_ref().unwrap_or(&workshop.schema_file));
    let lab_file = located.dir.join(
        flags
            .lab_vars
            .unwrap_or_else(|| PathBuf::from("lab_vars.yml")),
    );
    let schema = if flags.skip_schema {
        None
    } else {
        let Some(schema) = read_input(&schema_file, "schema", read_schema) else {
            return 1;
        };
        Some(schema)
    };
    let Some(lab) = read_input(&lab_file, "lab_vars", lab_vars::read) else {
        return 1;
    };
    if let Some(schema) = &schema {
        let code = apply_schema(&schema_file, || infrahub.load_schema(schema));
        if code != 0 {
            return code;
        }
    }
    println!(
        "Loading {} into Infrahub at {}",
        lab_file.display(),
        infrahub.address()
    );
    if let Err(error) = infrahub.wait_for_kinds(&KINDS, KIND_WAIT) {
        return infrahub_failed(error);
    }
    let seed = infrahub.seed(&lab, &mut |name, created| {
        let done = if created { "created" } else { "updated" };
        println!("device {name} {done}");
    });
    let seed = match seed {
        Ok(seed) => seed,
        Err(error) => return infrahub_failed(error),
    };
    println!("{}", seed_table(&seed));
    let nodes: u64 = seed
        .rows
        .iter()
        .map(|(_, created, updated)| u64::from(*created) + u64::from(*updated))
        .sum();
    println!(
        "Done: {nodes} nodes across {} types. Open Infrahub at {}.",
        seed.rows.len(),
        infrahub.address()
    );
    if lab.devices.is_empty() {
        eprintln!(
            "nobs: no devices found in {}: is --lab-vars the right file?",
            lab_file.display()
        );
    }
    0
}

fn seed_table(seed: &Seed) -> String {
    let header = ["Node type", "Created", "Updated", "Total"].map(String::from);
    let mut lines = vec![header];
    lines.extend(seed.rows.iter().map(|(kind, created, updated)| {
        [
            kind.clone(),
            created.to_string(),
            updated.to_string(),
            (u64::from(*created) + u64::from(*updated)).to_string(),
        ]
    }));
    pad_lines(&lines)
}

fn run_schema_load(file: &Path, flags: InfrahubFlags) -> i32 {
    let (address, token) = infrahub_settings(flags, |name| std::env::var(name).ok());
    announce_rewrite(&address);
    if is_missing(file, "schema") {
        return 1;
    }
    let Some(token) = require_token(token) else {
        return 1;
    };
    let infrahub = Infrahub::new(&address, &token);
    let Some(schema) = read_input(file, "schema", read_schema) else {
        return 1;
    };
    apply_schema(file, || infrahub.load_schema(&schema))
}

fn event_addresses(lookup: impl Fn(&str) -> Option<String>) -> (String, String) {
    let server = given(lookup("SONDA_SERVER_URL")).unwrap_or_else(|| Urls::defaults().sonda);
    let sink = given(lookup("SONDA_LOKI_SINK_URL")).unwrap_or_else(|| LOKI_SINK.to_string());
    (server, sink)
}

fn run_maintenance(flags: MaintenanceFlags) -> i32 {
    let state = flags.state || !flags.clear;
    let (address, token) = infrahub_settings(flags.infrahub, |name| std::env::var(name).ok());
    announce_rewrite(&address);
    let Some(token) = require_token(token) else {
        return 1;
    };
    let infrahub = Infrahub::new(&address, &token);
    let saved = match infrahub.maintenance(&flags.kind, &flags.device, state) {
        Ok(saved) => saved,
        Err(error) => return infrahub_failed(error),
    };
    println!("{}.maintenance: {} → {state}", flags.device, saved.previous);
    let (server, sink) = event_addresses(|name| std::env::var(name).ok());
    let key = given(std::env::var("SONDA_API_KEY").ok());
    let body = annotation(&flags.device, state, &sink);
    let posted = match Sonda::new(server).api_key(key).event(body.as_bytes()) {
        Ok(reply) if reply.status >= 400 => {
            Err(format!("sonda answered {}: {}", reply.status, reply.body))
        }
        other => other.map(|_| ()),
    };
    if let Err(error) = posted {
        eprintln!("nobs: sonda /events post failed: {error}");
        return 1;
    }
    if state {
        println!("The next alert for this device will be SKIPPED by the policy.");
    } else {
        println!("The next alert for this device will be evaluated normally.");
    }
    0
}

fn service_address(
    flag: Option<String>,
    (variable, default): (&str, &str),
    lookup: impl Fn(&str) -> Option<String>,
) -> String {
    given(flag)
        .or_else(|| given(lookup(variable)))
        .unwrap_or_else(|| default.to_string())
}

fn run_rca(flags: RcaFlags, located: &Located) -> i32 {
    let Kind::Workshop(workshop) = &located.kind else {
        unreachable!("a workshop command runs in a workshop");
    };
    let device = given(flags.device);
    let peer = given(flags.peer);
    let mut query = String::from(r#"{source="prefect", ai_rca="true""#);
    if let Some(device) = &device {
        query.push_str(&format!(r#", device="{device}""#));
    }
    if let Some(peer) = &peer {
        query.push_str(&format!(r#", peer_address="{peer}""#));
    }
    query.push('}');
    let client = Loki::new(service_address(flags.loki_url, LOKI_ADDRESS, |name| {
        std::env::var(name).ok()
    }));
    let lines = match client.query_range(&query, flags.minutes, flags.last, SystemTime::now()) {
        Ok(lines) => lines,
        Err(error) => {
            let reason = match error {
                loki::Error::Reach { reason, .. } => reason,
                other => other.to_string(),
            };
            eprintln!("nobs: Failed to query Loki at {}: {reason}", client.url());
            return 1;
        }
    };
    if lines.is_empty() {
        let scope = match (&device, &peer) {
            (Some(device), Some(peer)) => format!("device={device} peer={peer}"),
            (Some(device), None) => format!("device={device}"),
            (None, Some(peer)) => format!("peer={peer}"),
            (None, None) => "any device/peer".to_string(),
        };
        println!(
            "No AI RCA records found in the last {}m for {scope}. If you haven't enabled it yet, set ENABLE_AI_RCA=true in {}/.env and re-run `nobs {} up`.",
            flags.minutes,
            located.dir.display(),
            workshop.name
        );
        return 0;
    }
    println!();
    for (number, line) in (1..).zip(&lines) {
        match rca_record(&line.text) {
            Some(record) => println!(
                "{}  device={}  peer={}\n{}\n",
                record.timestamp, record.device, record.peer, record.message
            ),
            None => println!("(skipping unparseable record #{number})"),
        }
    }
    0
}

fn read_metrics(
    client: &Prometheus,
    device: &str,
    peer: &str,
    afi_safi: &str,
    instance: &str,
) -> Metrics {
    let labels = format!(
        r#"{{device="{device}",peer_address="{peer}",afi_safi_name="{afi_safi}",name="{instance}"}}"#
    );
    let ask = |metric: &str| client.instant(&format!("{metric}{labels}")).ok().flatten();
    Metrics {
        admin: ask("bgp_admin_state"),
        oper: ask("bgp_oper_state"),
        received: ask("bgp_received_routes"),
        sent: ask("bgp_sent_routes"),
        active: ask("bgp_active_routes"),
        suppressed: ask("bgp_suppressed_routes"),
    }
}

fn source_found(intent: &DeviceIntent, session: Option<&IntentSession>) -> String {
    let unknown = |text: &Option<String>| text.clone().unwrap_or_else(|| "?".to_string());
    let row = |label: &str, value: &str| format!("{label:<16}{value}");
    let mut lines = vec![
        "Source of truth (Infrahub)".to_string(),
        format!(
            "{:<16}{}   site={}  role={}",
            "device",
            intent.name,
            unknown(&intent.site),
            unknown(&intent.role)
        ),
        row("maintenance", &intent.maintenance.to_string()),
        row(
            "intended peer",
            if session.is_some() { "yes" } else { "no" },
        ),
    ];
    if let Some(session) = session {
        let expected = session
            .expected_state
            .as_deref()
            .filter(|text| !text.is_empty());
        lines.push(row("expected state", expected.unwrap_or(DASH)));
        if let Some(reason) = session.reason.as_deref().filter(|text| !text.is_empty()) {
            lines.push(row("reason", reason));
        }
        if let Some(remote_as) = session.remote_as.filter(|number| *number != 0) {
            lines.push(row("remote_as", &remote_as.to_string()));
        }
    }
    lines.join("\n")
}

fn metrics_block(metrics: &Metrics) -> String {
    let rows = [
        (
            "admin_state",
            value_text(metrics.admin, "-1"),
            decode_admin(metrics.admin),
        ),
        (
            "oper_state",
            value_text(metrics.oper, "-1"),
            decode_oper(metrics.oper),
        ),
        (
            "received_routes",
            value_text(metrics.received, "0"),
            DASH.to_string(),
        ),
        (
            "sent_routes",
            value_text(metrics.sent, "0"),
            DASH.to_string(),
        ),
        (
            "active_routes",
            value_text(metrics.active, "0"),
            DASH.to_string(),
        ),
        (
            "suppressed_routes",
            value_text(metrics.suppressed, "0"),
            DASH.to_string(),
        ),
    ];
    let width = rows
        .iter()
        .map(|(_, value, _)| value.chars().count())
        .fold("Value".len(), usize::max);
    let mut lines = vec![[
        "Metric".to_string(),
        format!("{:>width$}", "Value"),
        "Decoded".to_string(),
    ]];
    lines.extend(
        rows.into_iter()
            .map(|(name, value, decoded)| [name.to_string(), format!("{value:>width$}"), decoded]),
    );
    format!("BGP metrics snapshot (Prometheus)\n{}", pad_lines(&lines))
}

fn run_evidence(flags: EvidenceFlags, located: &Located) -> i32 {
    let Kind::Workshop(workshop) = &located.kind else {
        unreachable!("a workshop command runs in a workshop");
    };
    let (infrahub_address, token) = infrahub_settings(
        InfrahubFlags {
            address: flags.infrahub_url,
            token: flags.token,
        },
        |name| std::env::var(name).ok(),
    );
    announce_rewrite(&infrahub_address);
    let Some(token) = require_token(token) else {
        return 1;
    };
    let intent = Infrahub::new(&infrahub_address, &token).device_intent(&flags.device);
    let prometheus = Prometheus::new(service_address(
        flags.prom_url,
        PROMETHEUS_ADDRESS,
        |name| std::env::var(name).ok(),
    ));
    let metrics = read_metrics(
        &prometheus,
        &flags.device,
        &flags.peer,
        &flags.afi_safi,
        &flags.instance,
    );
    let query = evidence::fill(&workshop.evidence_query, &flags.device, &flags.peer);
    let lines = Loki::new(service_address(flags.loki_url, LOKI_ADDRESS, |name| {
        std::env::var(name).ok()
    }))
    .query_range(
        &query,
        flags.log_minutes,
        flags.log_limit,
        SystemTime::now(),
    )
    .unwrap_or_default();
    let found = intent.as_ref().ok().and_then(Option::as_ref);
    let session = found.and_then(|found| evidence::session(found, &flags.peer, &flags.afi_safi));
    let missing = match &intent {
        Ok(_) => "device not found in Infrahub".to_string(),
        Err(error) => format!("Infrahub query failed: {error}"),
    };
    let source = match (found, &intent) {
        (Some(found), _) => source_found(found, session),
        (None, Ok(_)) => format!("Source of truth\n{missing}."),
        (None, Err(_)) => format!("Source of truth\n{missing}"),
    };
    let logs = if lines.is_empty() {
        "Loki\nNo relevant log lines in the window.".to_string()
    } else {
        let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
        format!(
            "Loki - last {} relevant line(s)\n{}",
            lines.len(),
            texts.join("\n")
        )
    };
    let (verdict, reason) = evidence::decision(found, session, &metrics, &missing);
    let blocks = [
        source,
        metrics_block(&metrics),
        logs,
        format!("Policy hint\ndecision: {verdict}\nreason  : {reason}"),
    ];
    println!("\n{}", blocks.join("\n\n"));
    0
}

fn recent_runs(prefect: &Prefect, device: &str, peer: &str, now: i64) -> Vec<FlowRun> {
    prefect
        .flow_runs(FLOW_RUN_LIMIT)
        .map(|runs| cycle::select(runs, device, peer, TRIGGER_WINDOW_MINUTES, now))
        .unwrap_or_default()
}

fn poll_until<T>(
    mut clock: impl FnMut() -> i64,
    mut attempt: impl FnMut(i64) -> Option<T>,
    mut pause: impl FnMut(),
) -> Option<T> {
    let mut first = None;
    for _ in 0..TRIGGER_TRIES {
        let now = clock();
        if now - *first.get_or_insert(now) >= TRIGGER_SECONDS {
            return None;
        }
        if let Some(found) = attempt(now) {
            return Some(found);
        }
        pause();
    }
    None
}

fn drive_cycle(
    prefect: &Prefect,
    webhook_url: &str,
    device: &str,
    peer: &str,
    status: &str,
) -> Option<i32> {
    let now = SystemTime::now();
    let seconds = seconds_of(now);
    let baseline = recent_runs(prefect, device, peer, seconds)
        .first()
        .and_then(|run| time::parse_rfc3339(&run.start_time))
        .unwrap_or(seconds);
    if let Err(error) = webhook::post(webhook_url, &webhook::payload(device, peer, status, now)) {
        eprintln!("nobs: could not POST to webhook: {error}");
        return Some(1);
    }
    println!("posted {status} payload for {device} → {peer}; waiting for new flow run …");
    let started = poll_until(
        now_seconds,
        |seconds| {
            let runs = recent_runs(prefect, device, peer, seconds);
            cycle::newer_than(&runs, baseline).map(|run| time::clock(&run.start_time))
        },
        || std::thread::sleep(TRIGGER_PAUSE),
    );
    match started {
        Some(clock) => println!("new flow run started at {clock}"),
        None => println!("no new flow run appeared within 30s; rendering current state anyway."),
    }
    std::thread::sleep(TRIGGER_PAUSE);
    None
}

fn short_id(id: &str) -> String {
    format!("{}…", id.chars().take(12).collect::<String>())
}

fn cycle_alert_lines(alert: &Alert, now: i64) -> String {
    let severity = alert.labels.get("severity").map_or("?", String::as_str);
    let state = match alert.state.as_str() {
        "active" => "firing",
        "unprocessed" => "pending",
        "" => "?",
        other => other,
    };
    let age = if time::parse_rfc3339(&alert.starts_at).is_some() {
        time::age(&alert.starts_at, now)
    } else {
        "?".to_string()
    };
    let silenced = match alert.silenced_by.split_first() {
        None => "(none)".to_string(),
        Some((first, [])) => short_id(first),
        Some((first, more)) => format!("{} (+{} more)", short_id(first), more.len()),
    };
    format!("{ALERTNAME} · {severity} · state={state} · age {age}\nsilencedBy: {silenced}")
}

fn alert_panel(alertmanager: &Alertmanager, device: &str, peer: &str, now: i64) -> String {
    let body = match alertmanager.alerts(true, true) {
        Err(error) => format!("Alertmanager query failed: {error}"),
        Ok(alerts) => {
            let lines: Vec<String> = alerts
                .iter()
                .filter(|alert| {
                    let label = |name: &str| alert.labels.get(name).map(String::as_str);
                    label("device") == Some(device)
                        && label("peer_address") == Some(peer)
                        && label("alertname") == Some(ALERTNAME)
                })
                .map(|alert| cycle_alert_lines(alert, now))
                .collect();
            if lines.is_empty() {
                format!("No {ALERTNAME} alert active for {device} → {peer}.")
            } else {
                lines.join("\n")
            }
        }
    };
    format!("\nAlert\n{body}")
}

fn scoped_to(silence: &Silence, device: &str, peer: &str) -> bool {
    let matchers: BTreeMap<&str, &str> = silence
        .matchers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    matchers.get("device") == Some(&device) && matchers.get("peer_address") == Some(&peer)
}

fn peer_silences_table(silences: &[Silence], now: i64) -> String {
    let header = ["ID", "State", "Starts", "Ends", "Remaining"].map(String::from);
    let mut lines = vec![header];
    lines.extend(silences.iter().map(|silence| {
        [
            if silence.id.is_empty() {
                "?".to_string()
            } else {
                short_id(&silence.id)
            },
            if silence.state.is_empty() {
                "?".to_string()
            } else {
                silence.state.clone()
            },
            time::clock(&silence.starts_at),
            time::clock(&silence.ends_at),
            time::remaining(&silence.ends_at, now),
        ]
    }));
    pad_lines(&lines)
}

fn silences_panel(alertmanager: &Alertmanager, device: &str, peer: &str, now: i64) -> String {
    let body = match alertmanager.silences() {
        Err(error) => format!("silences query failed: {error}"),
        Ok(silences) => {
            let mut scoped: Vec<Silence> = silences
                .into_iter()
                .filter(|silence| scoped_to(silence, device, peer))
                .collect();
            scoped.sort_by(|first, second| second.starts_at.cmp(&first.starts_at));
            scoped.truncate(SILENCES_SHOWN);
            if scoped.is_empty() {
                format!("No silences scoped to {device} → {peer}.")
            } else {
                peer_silences_table(&scoped, now)
            }
        }
    };
    format!("\nSilences (this peer)\n{body}")
}

fn flow_runs_table(groups: &[Group]) -> String {
    let name = |run: &FlowRun| {
        if run.name.is_empty() {
            "?".to_string()
        } else {
            run.name.clone()
        }
    };
    let header = ["Started", "State", "Flow"].map(String::from);
    let mut lines = vec![header];
    for group in groups {
        lines.push([
            time::clock(&group.run.start_time),
            group.run.state.clone(),
            name(&group.run),
        ]);
        let last = group.children.len().saturating_sub(1);
        for (index, child) in group.children.iter().enumerate() {
            let branch = if index == last { "└─" } else { "├─" };
            lines.push([
                time::clock(&child.start_time),
                child.state.clone(),
                format!("  {branch} {}", name(child)),
            ]);
        }
    }
    pad_lines(&lines)
}

fn flow_runs_panel(prefect: &Prefect, device: &str, peer: &str, minutes: u64, now: i64) -> String {
    let body = match prefect.flow_runs(FLOW_RUN_LIMIT) {
        Err(error) => format!("Prefect query failed: {error}"),
        Ok(runs) => {
            let selected = cycle::select(runs, device, peer, minutes, now);
            if selected.is_empty() {
                format!("No quarantine_bgp / resolved_bgp runs for {device}:{peer} in the last {minutes}m.")
            } else {
                let mut parents: Vec<String> = Vec::new();
                for parent in selected
                    .iter()
                    .filter_map(|run| run.parent_task_run_id.clone())
                {
                    if !parents.contains(&parent) {
                        parents.push(parent);
                    }
                }
                let task_runs = if parents.is_empty() {
                    Vec::new()
                } else {
                    prefect.task_runs(&parents).unwrap_or_default()
                };
                let mut groups = cycle::group(selected, &task_runs);
                groups.truncate(GROUPS_SHOWN);
                flow_runs_table(&groups)
            }
        }
    };
    format!("\nPrefect flow runs (last {minutes}m)\n{body}")
}

fn decision_panel(
    loki: &Loki,
    workflow_label: &str,
    device: &str,
    peer: &str,
    minutes: u64,
    now: SystemTime,
) -> String {
    let query = format!(
        r#"{{source="prefect", workflow="{workflow_label}", device="{device}", peer_address="{peer}", decision=~".+"}} | json"#
    );
    let body = match loki.query_range(&query, minutes, 1, now) {
        Err(error) => format!("Loki query failed: {error}"),
        Ok(lines) => match lines.first() {
            None => {
                format!("No decision audit records for {device} → {peer} in the last {minutes}m.")
            }
            Some(line) => match decision_record(&line.text) {
                None => "Could not parse latest decision JSON.".to_string(),
                Some(record) => format!(
                    "{}  decision={}\nreason: {}",
                    time::clock(&record.timestamp),
                    record.decision,
                    record.message
                ),
            },
        },
    };
    format!("\nMost recent decision\n{body}")
}

fn run_cycle(flags: CycleFlags, located: &Located) -> i32 {
    let Kind::Workshop(workshop) = &located.kind else {
        unreachable!("a workshop command runs in a workshop");
    };
    let CycleFlags {
        device,
        peer,
        trigger,
        status,
        minutes,
        am_url,
        loki_url,
        prefect_url,
        webhook_url,
    } = flags;
    let lookup = |name: &str| std::env::var(name).ok();
    let prefect = Prefect::new(service_address(prefect_url, PREFECT_ADDRESS, lookup));
    if trigger {
        let webhook_url = service_address(webhook_url, WEBHOOK_ADDRESS, lookup);
        if let Some(code) = drive_cycle(&prefect, &webhook_url, &device, &peer, &status) {
            return code;
        }
    }
    let alertmanager = Alertmanager::new(alertmanager_address(am_url, lookup));
    let loki = Loki::new(service_address(loki_url, LOKI_ADDRESS, lookup));
    let now = SystemTime::now();
    let seconds = seconds_of(now);
    println!("{}", alert_panel(&alertmanager, &device, &peer, seconds));
    println!("{}", silences_panel(&alertmanager, &device, &peer, seconds));
    println!(
        "{}",
        flow_runs_panel(&prefect, &device, &peer, minutes, seconds)
    );
    println!(
        "{}",
        decision_panel(
            &loki,
            &workshop.workflow_label,
            &device,
            &peer,
            minutes,
            now
        )
    );
    if !trigger {
        println!(
            "\n  To drive a fresh cycle (bypassing Alertmanager's repeat_interval):\n    nobs {} cycle {device} {peer} --trigger",
            workshop.name
        );
    }
    0
}

fn default_summary(labels: &BTreeMap<String, String>) -> String {
    let label = |name: &str| labels.get(name).map_or("?", String::as_str);
    match labels.get("alertname").map_or("", String::as_str) {
        ALERTNAME => format!(
            "BGP peer {} not reaching Established",
            label("peer_address")
        ),
        "PeerInterfaceFlapping" => {
            format!("interface {} flapping above threshold", label("interface"))
        }
        name => format!("alert {name} firing"),
    }
}

fn utc_clock() -> String {
    let seconds = now_seconds().rem_euclid(86400);
    format!("{:02}:{:02}", seconds / 3600, seconds % 3600 / 60)
}

fn run_page(flags: PageFlags, located: &Located) -> i32 {
    let Kind::Workshop(workshop) = &located.kind else {
        unreachable!("a workshop command runs in a workshop");
    };
    let alertmanager = Alertmanager::new(alertmanager_address(flags.am_url, |name| {
        std::env::var(name).ok()
    }));
    let alerts = match alertmanager.alerts(true, true) {
        Ok(alerts) => alerts,
        Err(error) => {
            eprintln!("nobs: Alertmanager fetch failed: {error}");
            return 1;
        }
    };
    let filters = [
        ("alertname", &flags.alert),
        ("device", &flags.device),
        ("peer_address", &flags.peer),
    ];
    let chosen = alerts.iter().find(|alert| {
        alert.state == "active"
            && filters
                .iter()
                .all(|(name, wanted)| wanted.is_empty() || alert.labels.get(*name) == Some(*wanted))
    });
    let Some(alert) = chosen else {
        eprintln!(
            "nobs: no firing alert matched the filters. Run `nobs {} alerts` to see what's currently firing.",
            workshop.name
        );
        return 1;
    };
    let label = |name: &str| alert.labels.get(name).map(String::as_str);
    let mut title = label("alertname").unwrap_or("unknown alert").to_string();
    if let Some(device) = label("device").filter(|text| !text.is_empty()) {
        title.push_str(&format!(" on {device}"));
    }
    if let Some(peer) = label("peer_address").filter(|text| !text.is_empty()) {
        title.push_str(&format!(" — peer {peer}"));
    }
    let summary = alert
        .annotations
        .get("summary")
        .filter(|text| !text.is_empty())
        .cloned()
        .unwrap_or_else(|| default_summary(&alert.labels));
    let clock = flags
        .now
        .filter(|text| !text.is_empty())
        .unwrap_or_else(utc_clock);
    println!(
        "on-call\nPAGED {clock}\n{title}\n{summary}\nfiring for: {}\nseverity: {}\n\nYou're awake. The dashboard is your only friend.\n\n  Confirm the alert: nobs {} alerts\n  Open the runbook:  {}/guides/",
        time::age_rough(&alert.starts_at, now_seconds()),
        label("severity").unwrap_or("unknown"),
        workshop.name,
        located.dir.display()
    );
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

    #[test]
    fn the_loki_and_prometheus_addresses_are_the_flag_then_the_variable_then_the_default() {
        let blank = |_: &str| Some("  ".to_string());
        let cases = [
            (LOKI_ADDRESS, "LOKI_URL", "http://localhost:3001"),
            (
                PROMETHEUS_ADDRESS,
                "PROMETHEUS_URL",
                "http://localhost:9090",
            ),
        ];

        for (address, name, default) in cases {
            let variable = move |asked: &str| (asked == name).then(|| format!("http://var-{name}"));

            assert_eq!(
                service_address(Some("http://flag".to_string()), address, variable),
                "http://flag"
            );
            assert_eq!(
                service_address(Some(" ".to_string()), address, variable),
                format!("http://var-{name}")
            );
            assert_eq!(
                service_address(None, address, variable),
                format!("http://var-{name}")
            );
            assert_eq!(service_address(None, address, |_| None), default);
            assert_eq!(
                service_address(Some(String::new()), address, blank),
                default
            );
        }
    }

    #[test]
    fn the_prefect_and_webhook_addresses_are_the_flag_then_the_variable_then_the_default() {
        let blank = |_: &str| Some("  ".to_string());
        let cases = [
            (PREFECT_ADDRESS, "PREFECT_API_URL", "http://localhost:4200"),
            (
                WEBHOOK_ADDRESS,
                "WEBHOOK_URL",
                "http://localhost:9997/v1/api/webhook",
            ),
        ];

        for (address, name, default) in cases {
            let variable = move |asked: &str| (asked == name).then(|| format!("http://var-{name}"));

            assert_eq!(
                service_address(Some("http://flag".to_string()), address, variable),
                "http://flag"
            );
            assert_eq!(
                service_address(Some(" ".to_string()), address, variable),
                format!("http://var-{name}")
            );
            assert_eq!(service_address(None, address, |_| None), default);
            assert_eq!(
                service_address(Some(String::new()), address, blank),
                default
            );
        }
    }

    #[test]
    fn polling_stops_after_fifteen_tries_when_nothing_is_found() {
        let mut tries = 0;
        let mut pauses = 0;

        let found: Option<()> = poll_until(
            || 0,
            |_| {
                tries += 1;
                None
            },
            || pauses += 1,
        );

        assert_eq!(found, None);
        assert_eq!((tries, pauses), (15, 15));
    }

    #[test]
    fn polling_stops_once_thirty_seconds_have_passed_by_the_clock_it_reads_on_each_try() {
        let mut now = -10;
        let mut tries = 0;

        let found: Option<()> = poll_until(
            || {
                now += 10;
                now
            },
            |_| {
                tries += 1;
                None
            },
            || {},
        );

        assert_eq!(found, None);
        assert_eq!(tries, 3);
    }

    #[test]
    fn the_infrahub_settings_are_the_flag_then_the_variable_then_the_default() {
        let flags = |address: Option<&str>, token: Option<&str>| InfrahubFlags {
            address: address.map(String::from),
            token: token.map(String::from),
        };
        let variable = |name: &str| Some(format!("var-{name}"));

        assert_eq!(
            infrahub_settings(flags(Some("flag-address"), Some("flag-token")), variable),
            ("flag-address".to_string(), Some("flag-token".to_string()))
        );
        assert_eq!(
            infrahub_settings(flags(Some(" "), Some(" ")), variable),
            (
                "var-INFRAHUB_ADDRESS".to_string(),
                Some("var-INFRAHUB_API_TOKEN".to_string())
            )
        );
        assert_eq!(
            infrahub_settings(flags(None, None), |_| Some("  ".to_string())),
            ("http://localhost:8000".to_string(), None)
        );
    }

    #[test]
    fn the_event_addresses_are_the_variables_then_the_defaults() {
        assert_eq!(
            event_addresses(|_| None),
            (
                "http://localhost:8085".to_string(),
                "http://loki:3001".to_string()
            )
        );
        assert_eq!(
            event_addresses(|name| match name {
                "SONDA_SERVER_URL" => Some("http://var-sonda".to_string()),
                "SONDA_LOKI_SINK_URL" => Some("http://var-sink".to_string()),
                _ => None,
            }),
            (
                "http://var-sonda".to_string(),
                "http://var-sink".to_string()
            )
        );
        assert_eq!(
            event_addresses(|_| Some("  ".to_string())),
            (
                "http://localhost:8085".to_string(),
                "http://loki:3001".to_string()
            )
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
            silenced_by: Vec::new(),
            annotations: BTreeMap::new(),
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
