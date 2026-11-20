use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::vec::Drain;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use nobs_core::alertmanager::Alert;
use nobs_core::device::{self, Password};
use nobs_core::env::read_env;
use nobs_core::lab;
use nobs_core::locate::{locate, Kind, Located};
use nobs_core::loki::Line;
use nobs_core::prefect::FlowRun;
use nobs_core::prometheus::Sample;
use nobs_core::sonda::Sonda;
use nobs_core::sot::{Device, Session, SourceOfTruth};
use nobs_core::status::{self, Urls, SERVICES};
use nobs_core::time;

use crate::actions::{flat, pieces, Action, Ask, Row, Target, ACTIONS};
use crate::book;
use crate::data::{self, Prompt};
use crate::devices;
use crate::run::{self, End};

const LOG_LINES: usize = 2000;
pub const WINDOW_MINUTES: u64 = 30;
const WINDOW_MIN: u64 = 1;
const WINDOW_MAX: u64 = 1440;
pub(crate) const SERVICES_TABS: [Tab; 3] = [Tab::Overview, Tab::Logs, Tab::Sonda];
pub(crate) const DEVICE_TABS: [Tab; 4] = [Tab::Overview, Tab::Interfaces, Tab::Timeline, Tab::Logs];
const LAB_DEVICE_TABS: [Tab; 5] = [
    Tab::Overview,
    Tab::Interfaces,
    Tab::Config,
    Tab::Timeline,
    Tab::Logs,
];
pub(crate) const OBSERVABILITY_TABS: [Tab; 3] = [Tab::Alerts, Tab::Logs, Tab::Metrics];
const MIN_SIZE: (u16, u16) = (60, 15);
const RUNNING: &str = "running";
pub(crate) const DOCKER_LOGS: &str = "docker logs";

pub type ShownDeviceLogs<'a> = (&'a str, Vec<(i64, &'a str)>, bool, usize);
pub type ShownLabLogs<'a> = (
    Vec<(i64, &'a str, &'a str)>,
    bool,
    Option<usize>,
    Option<usize>,
);
pub(crate) type Lookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
type Labels = BTreeMap<String, String>;

#[derive(Clone)]
pub struct Start {
    pub lab: Option<Located>,
    pub stacks: Vec<Stack>,
    pub sonda: SondaTarget,
    pub home: Workbench,
    pub chooser: bool,
    pub source: Option<Arc<dyn SourceOfTruth>>,
    pub source_error: Option<String>,
    pub source_service: Option<status::Service>,
    pub urls: Urls,
    pub program: PathBuf,
    pub login: Lookup,
}

impl fmt::Debug for Start {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Start")
            .field("lab", &self.lab)
            .field("stacks", &self.stacks)
            .field("sonda", &self.sonda)
            .field("home", &self.home)
            .field("chooser", &self.chooser)
            .field("source", &self.source.as_ref().map(|_| ".."))
            .field("source_error", &self.source_error)
            .field("source_service", &self.source_service)
            .field("urls", &self.urls)
            .field("program", &self.program)
            .field("login", &"..")
            .finish()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Login {
    pub user: Option<String>,
    pub password: Option<Password>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SondaTarget {
    pub server: Option<String>,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workbench {
    Devices,
    Services,
    Lab,
    Observability,
}

pub(crate) const WORKBENCHES: [Workbench; 4] = [
    Workbench::Devices,
    Workbench::Services,
    Workbench::Lab,
    Workbench::Observability,
];

impl Workbench {
    pub fn title(self, wide: bool) -> &'static str {
        match self {
            Workbench::Devices => "devices",
            Workbench::Services => "services",
            Workbench::Lab => "lab",
            Workbench::Observability if wide => "observability",
            Workbench::Observability => "obs",
        }
    }
}

fn workbench_place(code: KeyCode) -> Option<usize> {
    let KeyCode::Char(typed) = code else {
        return None;
    };
    let place = usize::try_from(typed.to_digit(10)?).ok()?.checked_sub(1)?;
    (place < WORKBENCHES.len()).then_some(place)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Tabs,
    Lines,
    Run,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Interfaces,
    Config,
    Timeline,
    Logs,
    Sonda,
    Alerts,
    Metrics,
}

impl Tab {
    pub fn title(self, wide: bool) -> &'static str {
        match self {
            Tab::Overview => "overview",
            Tab::Interfaces if wide => "interfaces",
            Tab::Interfaces => "ifaces",
            Tab::Config => "config",
            Tab::Timeline => "timeline",
            Tab::Logs => "logs",
            Tab::Sonda => "sonda",
            Tab::Alerts => "alerts",
            Tab::Metrics => "metrics",
        }
    }
}

fn step(tabs: &[Tab], tab: Tab, forward: bool) -> Tab {
    let at = tabs
        .iter()
        .position(|candidate| *candidate == tab)
        .unwrap_or(0);
    let next = if forward {
        (at + 1) % tabs.len()
    } else {
        (at + tabs.len() - 1) % tabs.len()
    };
    tabs[next]
}

fn kept_row<T>(shown: &[T], kept: impl Fn(&T) -> bool) -> Option<usize> {
    shown
        .iter()
        .position(kept)
        .or((!shown.is_empty()).then_some(0))
}

fn neighbour<T: Copy>(shown: &[T], row: usize, down: bool) -> Option<T> {
    let next = if down {
        Some(row + 1)
    } else {
        row.checked_sub(1)
    };
    shown.get(next?).copied()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    Device(usize),
    Alert(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackKind {
    Workshop,
    BookLab,
    Lab,
    Sonda,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stack {
    pub name: String,
    pub kind: StackKind,
    pub path: PathBuf,
    pub compose: PathBuf,
    pub project: Option<String>,
    pub env: Vec<(String, String)>,
    pub shows_sonda: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StackState {
    Unknown,
    Up { running: usize, total: usize },
    Down { total: usize },
    Failed,
}

impl StackState {
    pub fn text(&self) -> String {
        match self {
            StackState::Unknown | StackState::Failed => "?".to_string(),
            StackState::Up { running, total } => format!("up {running}/{total}"),
            StackState::Down { total } => format!("down 0/{total}"),
        }
    }

    pub fn total(&self) -> Option<usize> {
        match self {
            StackState::Up { total, .. } | StackState::Down { total } => Some(*total),
            StackState::Unknown | StackState::Failed => None,
        }
    }

    fn of(defined: &[String], listed: &[lab::Service]) -> StackState {
        let total = defined.len();
        let running = defined
            .iter()
            .filter(|name| {
                listed
                    .iter()
                    .any(|row| row.name == **name && row.state == RUNNING)
            })
            .count();
        if running > 0 {
            StackState::Up { running, total }
        } else {
            StackState::Down { total }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    Unknown,
    NotInLab,
    StackDown,
    Answers,
    NoAnswer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub state: String,
    pub health: Option<String>,
    pub port: Option<String>,
    pub age: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    pub service: String,
    pub state: String,
}

#[derive(Debug)]
pub enum Update {
    Services(
        usize,
        Result<Vec<String>, String>,
        Result<Vec<lab::Service>, String>,
    ),
    Health(usize, Vec<Health>),
    Logs(usize, String, Result<Vec<String>, String>),
    Sonda(data::Update),
    Note(data::Note),
    Stopped(String),
    Devices(Result<Vec<Device>, String>),
    Metrics(Result<Vec<Sample>, String>),
    Alerts(Result<Vec<Alert>, String>),
    DeviceLogs(String, Result<Vec<Line>, String>),
    Rcas(String, Result<Vec<Line>, String>),
    LabLogs(Result<Vec<(Labels, Line)>, String>),
    FlowRuns(Result<Vec<FlowRun>, String>),
    ActionLine(String),
    ActionError(String),
    ActionEnded(run::End),
    Nodes(Result<Vec<book::NodeRow>, String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Quit,
    Refresh,
    SelectStack(usize),
    OpenLogs {
        stack: usize,
        service: String,
    },
    Follow(bool),
    FollowDevice(bool),
    FollowLab(bool),
    CloseLogs,
    Sonda(data::Command),
    SelectDevice(String),
    OpenTab(Tab),
    Window(u64),
    Run {
        program: PathBuf,
        args: Vec<OsString>,
        login: Login,
    },
    Stop,
    Ask(status::Service, bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Running,
    Stopping,
    Ended(End),
}

#[derive(Debug)]
pub struct Run {
    pub words: String,
    pub command: String,
    pub target: Target,
    pub lines: Vec<String>,
    pub output: Option<Vec<String>>,
    pub started: Instant,
    pub ended: Option<Instant>,
    pub state: RunState,
    pub(crate) typed: bool,
}

struct RunView {
    scroll: usize,
    following: bool,
    overflow: usize,
    drawn: bool,
    cut: Vec<String>,
}

impl Default for RunView {
    fn default() -> RunView {
        RunView {
            scroll: 0,
            following: true,
            overflow: 0,
            drawn: false,
            cut: Vec::new(),
        }
    }
}

pub(crate) struct RunMeasure {
    pub overflow: usize,
    pub cut_rows: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub lines: Vec<String>,
    pub fetched: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Question {
    Pending {
        args: Vec<Vec<OsString>>,
        target: Target,
        saying: Option<&'static str>,
    },
    Quit,
}

impl Question {
    pub fn text(&self) -> String {
        match self {
            Question::Pending {
                args, saying: None, ..
            } => format!("run nobs {}? y/n", joined(args)),
            Question::Pending {
                args,
                saying: Some(saying),
                ..
            } => format!("run nobs {}? It {saying}. y/n", joined(args)),
            Question::Quit => "an action is running: stop it and quit? y/n".to_string(),
        }
    }
}

pub fn not_stopped(group: u32) -> String {
    format!(
        "the action did not stop: a program it started is still running in process group {group}"
    )
}

pub(crate) const NO_BREAK: char = '\u{a0}';

fn joined(pieces: &[Vec<OsString>]) -> String {
    pieces
        .iter()
        .map(|piece| {
            piece
                .iter()
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
                .replace(' ', &NO_BREAK.to_string())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn words(pieces: &[Vec<OsString>]) -> String {
    joined(pieces.get(1..).unwrap_or_default())
}

#[derive(Default)]
struct Filter {
    text: Option<String>,
    typing: bool,
}

impl Filter {
    fn start(&mut self) {
        self.typing = true;
        self.text.get_or_insert_with(String::new);
    }

    fn type_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => *self = Filter::default(),
            KeyCode::Enter => {
                self.typing = false;
                if self.text.as_deref() == Some("") {
                    self.text = None;
                }
            }
            KeyCode::Backspace => {
                if let Some(text) = &mut self.text {
                    text.pop();
                }
            }
            KeyCode::Char(typed) => self.text.get_or_insert_with(String::new).push(typed),
            _ => {}
        }
    }

    fn matches(&self, text: &str) -> bool {
        self.text
            .as_deref()
            .is_none_or(|filter| text.contains(filter))
    }

    fn matches_any_case(&self, text: &str) -> bool {
        self.text
            .as_deref()
            .is_none_or(|filter| text.to_lowercase().contains(&filter.to_lowercase()))
    }
}

#[derive(PartialEq, Eq)]
enum Step {
    User { then_password: bool },
    Password { user: String },
    Command,
}

struct Asked {
    args: Vec<Vec<OsString>>,
    target: Target,
    text: Filter,
    step: Step,
    action: &'static Action,
    typed: Login,
}

pub(crate) enum LoginEntry<'a> {
    User {
        text: &'a str,
        hint: &'static str,
    },
    Password {
        user: &'a str,
        dots: usize,
        hint: &'static str,
    },
}

struct DeviceLine {
    device: String,
    line: Line,
    shown: String,
}

struct LogStream {
    lines: Vec<DeviceLine>,
    following: bool,
    filter: Filter,
}

impl Default for LogStream {
    fn default() -> LogStream {
        LogStream {
            lines: Vec::new(),
            following: true,
            filter: Filter::default(),
        }
    }
}

impl LogStream {
    fn take(&mut self, lines: Vec<(String, Line)>) {
        for (device, line) in lines {
            if !self
                .lines
                .iter()
                .any(|held| held.device == device && held.line == line)
            {
                let shown = line.message().unwrap_or_else(|| line.text.clone());
                self.lines.push(DeviceLine {
                    device,
                    line,
                    shown,
                });
            }
        }
        self.lines
            .sort_by_key(|held| Reverse(held.line.nanoseconds));
        self.lines.truncate(LOG_LINES);
    }

    fn shown(&self) -> Vec<&DeviceLine> {
        self.lines
            .iter()
            .filter(|held| self.filter.matches(&held.shown))
            .collect()
    }

    fn toggle(&mut self) -> bool {
        self.following = !self.following;
        self.following
    }

    fn clear(&mut self) {
        self.lines.clear();
    }
}

struct LogsState {
    service: String,
    lines: Vec<String>,
    following: bool,
    filter: Filter,
    scroll: usize,
}

impl LogsState {
    fn shown(&self) -> usize {
        self.lines
            .iter()
            .filter(|line| self.filter.matches(line))
            .count()
    }
}

pub struct Model {
    lab: Option<Located>,
    stacks: Vec<Stack>,
    states: Vec<StackState>,
    defined: Vec<Option<Vec<String>>>,
    urls: Urls,
    asks: Vec<status::Service>,
    selected: usize,
    workbench: Workbench,
    focus: Focus,
    tab: Tab,
    chooser: bool,
    filter: Filter,
    services: Vec<Service>,
    services_at: Option<Instant>,
    health: Vec<Health>,
    service: usize,
    logs: Option<LogsState>,
    server: Option<String>,
    sonda: data::Model,
    errors: Vec<(String, String, Instant)>,
    help: bool,
    help_scroll: usize,
    help_overflow: usize,
    run_pane: bool,
    size: (u16, u16),
    now: Instant,
    wall: SystemTime,
    clock: String,
    quit: bool,
    exit_code: i32,
    source: bool,
    source_error: Option<String>,
    source_service: Option<status::Service>,
    devices: Vec<Device>,
    devices_at: Option<Instant>,
    device: usize,
    cursor: Cursor,
    device_filter: Filter,
    metrics: Option<Vec<Sample>>,
    alerts: Vec<Alert>,
    alerts_ok: bool,
    kept_alert: Option<BTreeMap<String, String>>,
    alert_filter: Filter,
    kept_device: Option<String>,
    metrics_filter: Filter,
    runs: Vec<FlowRun>,
    rcas: Vec<Line>,
    device_logs: LogStream,
    logs_scroll: usize,
    lab_logs: LogStream,
    lab_mark: Option<(String, Line)>,
    lab_asked: bool,
    view: Tab,
    config_scroll: usize,
    config_filter: Filter,
    window: u64,
    timeline_row: usize,
    popup: Option<devices::Entry>,
    popup_scroll: usize,
    popup_overflow: usize,
    overview_scroll: usize,
    overview_overflow: usize,
    lab_scroll: usize,
    lab_overflow: usize,
    program: PathBuf,
    run: Option<Run>,
    run_view: RunView,
    question: Option<Question>,
    asked: Option<Asked>,
    notice: Option<String>,
    failed: Vec<Target>,
    configs: BTreeMap<String, Config>,
    chapter: usize,
    nodes: Vec<book::NodeRow>,
    nodes_ok: bool,
    node_row: usize,
    interface_row: usize,
    quitting: bool,
    login: Login,
    lookup: Lookup,
}

impl Model {
    pub fn new(start: Start) -> Model {
        let Start {
            lab,
            stacks,
            sonda,
            home,
            chooser,
            source,
            source_error,
            source_service,
            urls,
            program,
            login,
        } = start;
        let chapter = stacks
            .iter()
            .position(|stack| stack.kind == StackKind::BookLab)
            .unwrap_or(0);
        let sonda_only_start = lab.is_none() && !chooser;
        let selected = match &lab {
            Some(lab) => stacks.iter().position(|stack| stack.path == lab.dir),
            None if sonda_only_start => stacks
                .iter()
                .position(|stack| stack.kind == StackKind::Sonda),
            None => None,
        }
        .unwrap_or(0);
        let url = sonda_url(sonda.server.as_deref(), stacks.get(selected));
        let now = Instant::now();
        let errors = source_error
            .clone()
            .map(|text| (error_source(status::Service::Sot).to_string(), text, now))
            .into_iter()
            .collect();
        Model {
            lab,
            states: vec![StackState::Unknown; stacks.len()],
            defined: vec![None; stacks.len()],
            urls,
            asks: Vec::new(),
            stacks,
            selected,
            workbench: home,
            focus: if sonda_only_start {
                Focus::Tabs
            } else {
                Focus::List
            },
            tab: if sonda_only_start {
                Tab::Sonda
            } else {
                Tab::Overview
            },
            chooser,
            filter: Filter::default(),
            services: Vec::new(),
            services_at: None,
            health: Vec::new(),
            service: 0,
            logs: None,
            server: sonda.server,
            sonda: data::Model::new(url),
            errors,
            help: false,
            help_scroll: 0,
            help_overflow: 0,
            run_pane: false,
            size: (80, 24),
            now,
            wall: UNIX_EPOCH,
            clock: String::new(),
            quit: false,
            exit_code: 0,
            source: source.is_some(),
            source_error,
            source_service,
            devices: Vec::new(),
            devices_at: None,
            device: 0,
            cursor: Cursor::Device(0),
            device_filter: Filter::default(),
            metrics: None,
            alerts: Vec::new(),
            alerts_ok: false,
            kept_alert: None,
            alert_filter: Filter::default(),
            kept_device: None,
            metrics_filter: Filter::default(),
            runs: Vec::new(),
            rcas: Vec::new(),
            device_logs: LogStream::default(),
            logs_scroll: 0,
            lab_logs: LogStream::default(),
            lab_mark: None,
            lab_asked: false,
            view: Tab::Alerts,
            config_scroll: 0,
            config_filter: Filter::default(),
            window: WINDOW_MINUTES,
            timeline_row: 0,
            popup: None,
            popup_scroll: 0,
            popup_overflow: 0,
            overview_scroll: 0,
            overview_overflow: 0,
            lab_scroll: 0,
            lab_overflow: 0,
            program,
            run: None,
            run_view: RunView::default(),
            question: None,
            asked: None,
            notice: None,
            failed: Vec::new(),
            configs: BTreeMap::new(),
            chapter,
            nodes: Vec::new(),
            nodes_ok: false,
            node_row: 0,
            interface_row: 0,
            quitting: false,
            login: Login::default(),
            lookup: login,
        }
    }

    pub fn tick(&mut self, now: Instant, wall: SystemTime, size: (u16, u16)) {
        self.now = now;
        self.wall = wall;
        self.clock = clock(wall);
        self.size = size;
    }

    pub fn too_small(&self) -> bool {
        self.size.0 < MIN_SIZE.0 || self.size.1 < MIN_SIZE.1
    }

    pub fn apply(&mut self, update: Update) -> Vec<Command> {
        if self
            .answer_of(&update)
            .is_some_and(|service| !self.asking().contains(&service))
        {
            return Vec::new();
        }
        let mut commands = self.take(update);
        commands.extend(self.ask_changes());
        commands
    }

    pub fn reach(&self, service: status::Service) -> Reach {
        let compose = status::row(service).compose;
        let of_the_lab = |index: &usize| self.lab.is_some() || *index == self.selected;
        let holders: Vec<&StackState> = (0..self.stacks.len())
            .filter(of_the_lab)
            .filter(|index| {
                self.defined[*index]
                    .as_ref()
                    .is_some_and(|names| names.iter().any(|name| name == compose))
            })
            .map(|index| &self.states[index])
            .collect();
        let up = holders
            .iter()
            .any(|state| matches!(state, StackState::Up { .. }));
        let unknown = (0..self.stacks.len())
            .filter(of_the_lab)
            .any(|index| watched(&self.stacks[index]) && self.defined[index].is_none())
            || holders
                .iter()
                .any(|state| !matches!(state, StackState::Up { .. } | StackState::Down { .. }));
        if !up && unknown {
            return Reach::Unknown;
        }
        if !up && !holders.is_empty() {
            return Reach::StackDown;
        }
        let set = match service {
            status::Service::Sonda => self
                .stacks
                .get(self.selected)
                .is_some_and(|stack| sonda_set(self.server.as_deref(), stack)),
            _ => self.urls.is_set(service),
        };
        if !up && !set {
            return Reach::NotInLab;
        }
        let failed = self
            .errors
            .iter()
            .any(|(source, ..)| source == error_source(service))
            || (service == status::Service::Sonda && self.sonda.error().is_some());
        if failed {
            Reach::NoAnswer
        } else {
            Reach::Answers
        }
    }

    pub(crate) fn asking(&self) -> Vec<status::Service> {
        SERVICES
            .iter()
            .map(|row| row.service)
            .filter(|service| matches!(self.reach(*service), Reach::Answers | Reach::NoAnswer))
            .collect()
    }

    pub(crate) fn ask_changes(&mut self) -> Vec<Command> {
        let asking = self.asking();
        let mut commands = Vec::new();
        for row in &SERVICES {
            let on = asking.contains(&row.service);
            if on == self.asks.contains(&row.service) {
                continue;
            }
            if !on {
                self.forget(row.service);
            }
            commands.push(Command::Ask(row.service, on));
        }
        self.asks = asking;
        let lab = self.lab_wanted();
        if lab != self.lab_asked {
            self.lab_asked = lab;
            commands.push(Command::FollowLab(lab));
        }
        self.keep_alert();
        self.keep_marked_device();
        self.keep_mark();
        commands
    }

    fn lab_wanted(&self) -> bool {
        self.workbench == Workbench::Observability
            && self.view == Tab::Logs
            && self.lab_logs.following
    }

    fn answer_of(&self, update: &Update) -> Option<status::Service> {
        match update {
            Update::Metrics(_) => Some(status::Service::Metrics),
            Update::Alerts(_) => Some(status::Service::Alerts),
            Update::DeviceLogs(..) | Update::Rcas(..) | Update::LabLogs(_) => {
                Some(status::Service::Logs)
            }
            Update::FlowRuns(_) => Some(status::Service::Flows),
            Update::Devices(_) => self.source_service,
            Update::Sonda(_) => Some(status::Service::Sonda),
            _ => None,
        }
    }

    fn forget(&mut self, service: status::Service) {
        let compose = status::row(service).compose;
        self.health.retain(|entry| entry.service != compose);
        self.rate_services();
        match service {
            status::Service::Metrics => self.metrics = None,
            status::Service::Logs => {
                self.device_logs.clear();
                self.lab_logs.clear();
                self.rcas.clear();
            }
            status::Service::Alerts => {
                self.alerts.clear();
                self.alerts_ok = false;
                if let Cursor::Alert(_) = self.cursor {
                    self.cursor = Cursor::Device(self.device);
                }
            }
            status::Service::Flows => self.runs.clear(),
            status::Service::Sonda => {
                self.sonda = data::Model::new(self.sonda.server().to_string());
            }
            status::Service::Sot if self.source_service == Some(service) => {
                self.devices.clear();
                self.devices_at = None;
                self.device = 0;
                if let Cursor::Device(_) = self.cursor {
                    self.cursor = Cursor::Device(0);
                }
            }
            status::Service::Sot | status::Service::Grafana => return,
        }
        self.clear_error(error_source(service));
    }

    fn rate_services(&mut self) {
        for service in &mut self.services {
            service.health = health_of(&self.health, &service.name, &service.state);
        }
    }

    fn take(&mut self, update: Update) -> Vec<Command> {
        match update {
            Update::Services(index, defined, listed) => self.apply_services(index, defined, listed),
            Update::Health(index, list) => {
                if index != self.selected {
                    return Vec::new();
                }
                self.health = list;
                self.rate_services();
            }
            Update::Logs(index, service, Ok(lines)) => {
                self.clear_error(DOCKER_LOGS);
                if index != self.selected {
                    return Vec::new();
                }
                let Some(logs) = self.logs.as_mut().filter(|logs| logs.service == service) else {
                    return Vec::new();
                };
                logs.lines
                    .extend(lines.iter().map(|line| untimed(line).to_string()));
                keep_last(&mut logs.lines);
            }
            Update::Logs(_, service, Err(text)) => {
                self.fail(DOCKER_LOGS, format!("{service}: {text}"));
            }
            Update::Sonda(update) => self.sonda.apply(update),
            Update::Note(note) => self.sonda.note(note.text, note.at),
            Update::Stopped(source) => {
                let service = SERVICES
                    .iter()
                    .find(|row| row.word == source)
                    .map(|row| row.service);
                match service {
                    Some(status::Service::Metrics) => self.metrics = None,
                    Some(status::Service::Alerts) => self.alerts_ok = false,
                    Some(status::Service::Logs) => {
                        self.device_logs.following = false;
                        self.lab_logs.following = false;
                    }
                    _ if source == "nodes" => self.nodes_ok = false,
                    _ => {}
                }
                self.fail(&source, "polling stopped".to_string());
            }
            Update::Devices(Ok(list)) => {
                self.devices = list;
                self.devices_at = Some(self.now);
                let last = self.devices.len().saturating_sub(1);
                self.device = self.device.min(last);
                if let Cursor::Device(at) = self.cursor {
                    self.cursor = Cursor::Device(at.min(last));
                }
                self.clear_error(error_source(status::Service::Sot));
                return self.show_cursor();
            }
            Update::Devices(Err(text)) => self.fail(error_source(status::Service::Sot), text),
            Update::Metrics(Ok(samples)) => {
                self.metrics = Some(samples);
                self.clear_error(error_source(status::Service::Metrics));
            }
            Update::Metrics(Err(text)) => {
                self.metrics = None;
                self.fail(error_source(status::Service::Metrics), text);
            }
            Update::Alerts(Ok(mut list)) => {
                list.sort_by_key(|alert| Reverse(time::parse_rfc3339(&alert.starts_at)));
                self.alerts = list;
                self.alerts_ok = true;
                self.clear_error(error_source(status::Service::Alerts));
                if let Cursor::Alert(at) = self.cursor {
                    return match self.alerts.len() {
                        0 => {
                            self.cursor = Cursor::Device(self.device);
                            self.show_cursor()
                        }
                        len => {
                            self.cursor = Cursor::Alert(at.min(len - 1));
                            Vec::new()
                        }
                    };
                }
            }
            Update::Alerts(Err(text)) => {
                self.alerts_ok = false;
                self.fail(error_source(status::Service::Alerts), text);
            }
            Update::DeviceLogs(device, Ok(lines)) => {
                self.clear_error(error_source(status::Service::Logs));
                if !self.is_selected(&device) {
                    return Vec::new();
                }
                self.device_logs.take(
                    lines
                        .into_iter()
                        .map(|line| (device.clone(), line))
                        .collect(),
                );
            }
            Update::DeviceLogs(_, Err(text)) => {
                self.fail(error_source(status::Service::Logs), text);
            }
            Update::Rcas(device, Ok(lines)) => {
                self.clear_error(error_source(status::Service::Logs));
                if self.is_selected(&device) {
                    self.rcas = lines;
                }
            }
            Update::Rcas(_, Err(text)) => self.fail(error_source(status::Service::Logs), text),
            Update::LabLogs(Ok(lines)) => {
                self.clear_error(error_source(status::Service::Logs));
                if self.lab_logs.following {
                    self.lab_logs.take(
                        lines
                            .into_iter()
                            .filter_map(|(mut labels, line)| {
                                labels.remove("device").map(|device| (device, line))
                            })
                            .collect(),
                    );
                }
            }
            Update::LabLogs(Err(text)) => self.fail(error_source(status::Service::Logs), text),
            Update::FlowRuns(Ok(runs)) => {
                self.runs = runs;
                self.clear_error(error_source(status::Service::Flows));
            }
            Update::FlowRuns(Err(text)) => self.fail(error_source(status::Service::Flows), text),
            Update::ActionLine(line) => {
                if let Some(run) = self.run.as_mut() {
                    if let Some(output) = run.output.as_mut() {
                        output.push(line.clone());
                    }
                    push_line(run, &mut self.run_view, line);
                }
            }
            Update::ActionError(line) => {
                if let Some(run) = self.run.as_mut() {
                    push_line(run, &mut self.run_view, line);
                }
            }
            Update::ActionEnded(end) => self.end_run(end),
            Update::Nodes(Ok(mut rows)) => {
                rows.sort_by(|one, other| one.name.cmp(&other.name));
                self.nodes = rows;
                self.node_row = self.node_row.min(self.nodes.len().saturating_sub(1));
                self.nodes_ok = true;
                self.clear_error("nodes");
            }
            Update::Nodes(Err(text)) => {
                self.nodes_ok = false;
                self.fail("nodes", text);
            }
        }
        Vec::new()
    }

    fn end_run(&mut self, end: End) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        run.state = RunState::Ended(end);
        run.ended = Some(self.now);
        if let End::NotStopped { group } = end {
            push_line(
                run,
                &mut self.run_view,
                format!("nobs: {}", not_stopped(group)),
            );
        }
        let output = run.output.take();
        if let (End::Exited(0), Some(lines), Target::Device(device)) = (end, output, &run.target) {
            let fetched = self.clock.clone();
            self.configs
                .insert(device.clone(), Config { lines, fetched });
        }
        if end == End::Exited(0) {
            self.failed.retain(|held| *held != run.target);
        } else if !self.failed.contains(&run.target) {
            self.failed.push(run.target.clone());
        }
        let refused = matches!(&run.target, Target::Device(name)
            if run.lines.last() == Some(&device::error_line(name, &device::Error::AuthenticationFailed)));
        if run.typed && end == End::Exited(1) && refused {
            self.login = Login::default();
        }
        if self.quitting {
            self.quit = true;
        }
    }

    fn active(&self) -> Option<&Run> {
        self.run
            .as_ref()
            .filter(|run| !matches!(run.state, RunState::Ended(_)))
    }

    fn stop(&mut self) -> Vec<Command> {
        self.question = None;
        if let Some(run) = self.run.as_mut() {
            run.state = RunState::Stopping;
        }
        vec![Command::Stop]
    }

    fn answer(&mut self, question: Question, key: KeyEvent) -> Vec<Command> {
        let yes = key.code == KeyCode::Char('y')
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match question {
            Question::Pending { args, target, .. } if yes => {
                self.start_run(args, target, false, Login::default())
            }
            Question::Pending { args, .. } => {
                self.notice = Some(format!("not run: {}", words(&args)));
                Vec::new()
            }
            Question::Quit if yes && self.active().is_some() => {
                self.quitting = true;
                self.stop()
            }
            Question::Quit if yes => {
                self.quit = true;
                vec![Command::Quit]
            }
            Question::Quit => Vec::new(),
        }
    }

    fn start_run(
        &mut self,
        args: Vec<Vec<OsString>>,
        target: Target,
        keeps: bool,
        login: Login,
    ) -> Vec<Command> {
        let typed = login != Login::default();
        if typed {
            self.login = login.clone();
        }
        self.run = Some(Run {
            words: words(&args),
            command: joined(&args),
            target,
            lines: Vec::new(),
            output: keeps.then(Vec::new),
            started: self.now,
            ended: None,
            state: RunState::Running,
            typed,
        });
        self.run_view = RunView {
            drawn: self.run_view.drawn,
            ..RunView::default()
        };
        self.run_pane = true;
        vec![Command::Run {
            program: self.program.clone(),
            args: flat(&args),
            login,
        }]
    }

    fn act(
        &mut self,
        action: &'static Action,
        args: Vec<Vec<OsString>>,
        target: Target,
        login: Login,
    ) -> Vec<Command> {
        let login = if action.login {
            login
        } else {
            Login::default()
        };
        let saying = match action.ask {
            Ask::No => {
                let mut commands = self.start_run(args, target, action.keeps, login);
                if action.keeps {
                    self.tab = Tab::Config;
                    commands.push(Command::OpenTab(Tab::Config));
                }
                return commands;
            }
            Ask::Yes => None,
            Ask::Saying(saying) => Some(saying),
            Ask::Text => {
                self.ask(action, args, target, Step::Command, login);
                return Vec::new();
            }
        };
        self.question = Some(Question::Pending {
            args,
            target,
            saying,
        });
        Vec::new()
    }

    fn ask(
        &mut self,
        action: &'static Action,
        args: Vec<Vec<OsString>>,
        target: Target,
        step: Step,
        typed: Login,
    ) {
        let mut text = Filter::default();
        text.start();
        self.asked = Some(Asked {
            args,
            target,
            text,
            step,
            action,
            typed,
        });
    }

    fn asked_step(&self, typed: &Login) -> Option<Step> {
        let found = |name: &str| (self.lookup)(name);
        let then_password = typed.password.is_none() && found(device::PASSWORD).is_none();
        match typed.user.clone().or_else(|| found(device::USER)) {
            None => Some(Step::User { then_password }),
            Some(user) => then_password.then_some(Step::Password { user }),
        }
    }

    pub(crate) fn action_target(&self, row: Row) -> Option<Target> {
        match row {
            Row::Stack
                if self.focus == Focus::List || matches!(self.tab, Tab::Overview | Tab::Logs) =>
            {
                Some(Target::Stack(self.selected))
            }
            Row::Stack => None,
            Row::Device | Row::Node => self
                .selected_device()
                .map(|device| Target::Device(device.name.clone())),
            Row::Chapter => self.selected_chapter().map(Target::Chapter),
            Row::Lab => self.book_lab().then_some(Target::Lab),
        }
    }

    fn action_key(&mut self, code: KeyCode) -> Option<Vec<Command>> {
        let KeyCode::Char(typed) = code else {
            return None;
        };
        let action = ACTIONS
            .iter()
            .find(|action| action.key == typed && action.workbench == self.workbench)?;
        let target = self.action_target(action.row);
        if action.row == Row::Stack && target.is_none() {
            return None;
        }
        let Some(target) = target else {
            return Some(Vec::new());
        };
        let running = self.active().map(|run| run.words.clone());
        match pieces(action, &target, self) {
            Some(_) if running.is_some() => {
                self.notice =
                    running.map(|words| format!("one action at a time: {words} is running"));
            }
            Some(args) => {
                self.notice = None;
                let login = self.login.clone();
                let step = if action.login {
                    self.asked_step(&login)
                } else {
                    None
                };
                match step {
                    Some(step) => self.ask(action, args, target, step, login),
                    None => return Some(self.act(action, args, target, login)),
                }
            }
            None if action.row == Row::Stack => {
                let mut notice = format!("{} {} is for a workshop", action.key, action.hint);
                let chapter = matches!(target, Target::Stack(index)
                    if self.stacks.get(index).is_some_and(|stack| stack.kind == StackKind::BookLab));
                if chapter {
                    notice.push_str("; a chapter has its actions in the lab workbench");
                }
                self.notice = Some(notice);
            }
            None => {}
        }
        Some(Vec::new())
    }

    fn lab_key(&mut self, code: KeyCode) -> Vec<Command> {
        if self.focus == Focus::Lines {
            match code {
                KeyCode::Char('j') | KeyCode::Down => {
                    self.lab_scroll = (self.lab_scroll + 1).min(self.lab_overflow);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.lab_scroll = self.lab_scroll.saturating_sub(1);
                }
                _ => {}
            }
            return Vec::new();
        }
        if self.focus == Focus::Tabs {
            let last = self.nodes.len().saturating_sub(1);
            match code {
                KeyCode::Char('j') | KeyCode::Down => self.node_row = (self.node_row + 1).min(last),
                KeyCode::Char('k') | KeyCode::Up => self.node_row = self.node_row.saturating_sub(1),
                _ => {}
            }
            return Vec::new();
        }
        let chapters = self.chapters();
        let next = match code {
            KeyCode::Char('j') | KeyCode::Down => {
                chapters.into_iter().find(|index| *index > self.chapter)
            }
            KeyCode::Char('k') | KeyCode::Up => chapters
                .into_iter()
                .rev()
                .find(|index| *index < self.chapter),
            _ => None,
        };
        if let Some(index) = next {
            self.chapter = index;
            self.lab_scroll = 0;
        }
        Vec::new()
    }

    fn fail(&mut self, source: &str, text: String) {
        let entry = (source.to_string(), text, self.now);
        match self.errors.iter_mut().find(|(held, _, _)| held == source) {
            Some(held) => *held = entry,
            None => self.errors.push(entry),
        }
    }

    fn clear_error(&mut self, source: &str) {
        self.errors.retain(|(held, _, _)| held != source);
    }

    fn answered<T>(&mut self, source: &str, answer: Result<T, String>) -> Option<T> {
        match answer {
            Ok(value) => {
                self.clear_error(source);
                Some(value)
            }
            Err(text) => {
                self.fail(source, text);
                None
            }
        }
    }

    fn dismiss_error(&mut self) {
        if !self.errors.is_empty() {
            self.errors.remove(0);
        }
    }

    fn is_selected(&self, device: &str) -> bool {
        self.selected_device()
            .is_some_and(|selected| selected.name == device)
    }

    pub fn chooser(&self) -> bool {
        self.chooser
    }

    pub fn workbench(&self) -> Workbench {
        self.workbench
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    pub fn tab(&self) -> Tab {
        if self.workbench == Workbench::Observability {
            self.view
        } else {
            self.tab
        }
    }

    pub fn stacks(&self) -> &[Stack] {
        &self.stacks
    }

    pub fn stack_state(&self, index: usize) -> &StackState {
        self.states.get(index).unwrap_or(&StackState::Unknown)
    }

    pub fn selected_stack(&self) -> usize {
        self.selected
    }

    pub fn visible_stacks(&self) -> Vec<usize> {
        (0..self.stacks.len())
            .filter(|index| self.filter.matches(&self.stacks[*index].name))
            .collect()
    }

    pub fn sonda_address(&self, index: usize) -> String {
        sonda_url(self.server.as_deref(), self.stacks.get(index))
    }

    pub fn typing(&self) -> bool {
        self.asked.is_some()
            || self.filter.typing
            || self.device_filter.typing
            || self.device_logs.filter.typing
            || self.lab_logs.filter.typing
            || self.config_filter.typing
            || self.alert_filter.typing
            || self.metrics_filter.typing
            || self.logs.as_ref().is_some_and(|logs| logs.filter.typing)
    }

    pub(crate) fn entry(&self) -> Option<(&str, &str)> {
        let asked = self
            .asked
            .as_ref()
            .filter(|asked| asked.step == Step::Command)?;
        let Target::Device(device) = &asked.target else {
            return None;
        };
        Some((device, asked.text.text.as_deref().unwrap_or_default()))
    }

    pub(crate) fn login_entry(&self) -> Option<LoginEntry<'_>> {
        let asked = self.asked.as_ref()?;
        let text = asked.text.text.as_deref().unwrap_or_default();
        let command_follows = asked.action.ask == Ask::Text;
        let hint = |next: bool| if next { "Enter next" } else { "Enter run" };
        match &asked.step {
            Step::User { then_password } => Some(LoginEntry::User {
                text,
                hint: hint(*then_password || command_follows),
            }),
            Step::Password { user } => Some(LoginEntry::Password {
                user,
                dots: text.chars().count(),
                hint: hint(command_follows),
            }),
            Step::Command => None,
        }
    }

    pub fn typed_filter(&self) -> Option<&str> {
        if self.filter.typing {
            return self.filter.text.as_deref();
        }
        if self.device_filter.typing {
            return self.device_filter.text.as_deref();
        }
        if self.device_logs.filter.typing {
            return self.device_logs.filter.text.as_deref();
        }
        if self.lab_logs.filter.typing {
            return self.lab_logs.filter.text.as_deref();
        }
        if self.config_filter.typing {
            return self.config_filter.text.as_deref();
        }
        if self.alert_filter.typing {
            return self.alert_filter.text.as_deref();
        }
        if self.metrics_filter.typing {
            return self.metrics_filter.text.as_deref();
        }
        self.logs
            .as_ref()
            .filter(|logs| logs.filter.typing)
            .and_then(|logs| logs.filter.text.as_deref())
    }

    pub fn services(&self) -> &[Service] {
        &self.services
    }

    pub fn selected_service(&self) -> usize {
        self.service
    }

    pub fn services_at(&self) -> Option<Instant> {
        self.services_at
    }

    pub fn logs(&self) -> Option<(&str, Vec<&str>, bool, usize)> {
        self.logs.as_ref().map(|logs| {
            let lines = logs
                .lines
                .iter()
                .filter(|line| logs.filter.matches(line))
                .map(String::as_str)
                .collect();
            (logs.service.as_str(), lines, logs.following, logs.scroll)
        })
    }

    pub fn sonda(&self) -> &data::Model {
        &self.sonda
    }

    pub fn error(&self) -> Option<(&str, &str, Instant)> {
        self.errors
            .first()
            .map(|(source, text, at)| (source.as_str(), text.as_str(), *at))
    }

    pub fn help(&self) -> bool {
        self.help
    }

    pub(crate) fn help_scroll(&self) -> usize {
        self.help_scroll
    }

    pub(crate) fn set_help_overflow(&mut self, rows: usize) {
        self.help_overflow = rows;
        self.help_scroll = self.help_scroll.min(rows);
    }

    pub(crate) fn overview_scroll(&self) -> usize {
        self.overview_scroll
    }

    pub(crate) fn set_overview_overflow(&mut self, lines: usize) {
        self.overview_overflow = lines;
        self.overview_scroll = self.overview_scroll.min(lines);
    }

    pub(crate) fn popup_scroll(&self) -> usize {
        self.popup_scroll
    }

    pub(crate) fn set_popup_overflow(&mut self, lines: usize) {
        self.popup_overflow = lines;
        self.popup_scroll = self.popup_scroll.min(lines);
    }

    pub(crate) fn lab_scroll(&self) -> usize {
        self.lab_scroll
    }

    pub(crate) fn set_lab_overflow(&mut self, lines: usize) {
        self.lab_overflow = lines;
        self.lab_scroll = self.lab_scroll.min(lines);
    }

    pub(crate) fn run_scroll(&self) -> usize {
        self.run_view.scroll
    }

    pub(crate) fn run_cut(&self) -> &[String] {
        &self.run_view.cut
    }

    pub(crate) fn run_drawn(&self) -> bool {
        self.run_view.drawn
    }

    pub(crate) fn run_overflow(&self) -> usize {
        self.run_view.overflow
    }

    pub(crate) fn set_run_view(&mut self, measure: Option<RunMeasure>) {
        let Some(measure) = measure else {
            self.run_view = RunView::default();
            if self.focus == Focus::Run {
                self.focus = Focus::List;
            }
            return;
        };
        let view = &mut self.run_view;
        view.cut.clear();
        view.drawn = true;
        view.overflow = measure.overflow;
        let scroll = view
            .scroll
            .saturating_sub(measure.cut_rows)
            .min(measure.overflow);
        view.following = view.following || scroll == measure.overflow;
        view.scroll = if view.following {
            measure.overflow
        } else {
            scroll
        };
    }

    pub(crate) fn set_details_overflow(&mut self, lines: usize) {
        self.sonda.set_details_overflow(lines);
    }

    pub fn run_pane(&self) -> bool {
        self.run_pane
    }

    pub fn now(&self) -> Instant {
        self.now
    }

    pub fn clock(&self) -> &str {
        &self.clock
    }

    pub fn wall(&self) -> SystemTime {
        self.wall
    }

    pub fn devices(&self) -> &[Device] {
        &self.devices
    }

    pub fn visible_devices(&self) -> Vec<usize> {
        self.devices_matching(&self.device_filter)
    }

    fn devices_matching(&self, filter: &Filter) -> Vec<usize> {
        (0..self.devices.len())
            .filter(|index| filter.matches_any_case(&self.devices[*index].name))
            .collect()
    }

    pub fn selected_device(&self) -> Option<&Device> {
        self.devices.get(self.device)
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    pub fn device_rows(&self) -> Vec<devices::ListRow> {
        let visible = self.visible_devices();
        devices::list_rows(
            &self.devices,
            self.metrics.as_deref(),
            self.alerts_ok.then_some(self.alerts.as_slice()),
        )
        .into_iter()
        .enumerate()
        .filter(|(index, _)| visible.contains(index))
        .map(|(_, row)| row)
        .collect()
    }

    pub fn source_note(&self) -> Option<&'static str> {
        if !self.devices.is_empty() {
            None
        } else if self.source_error.is_some() {
            Some("source of truth not opened")
        } else if !self.source {
            Some("no source of truth (sot = \"none\")")
        } else if self.devices_at.is_none() {
            Some("no answer yet")
        } else {
            Some("no devices")
        }
    }

    pub(crate) fn source_service(&self) -> Option<status::Service> {
        self.source_service
    }

    pub fn devices_at(&self) -> Option<Instant> {
        self.devices_at
    }

    pub fn alerts(&self) -> &[Alert] {
        &self.alerts
    }

    pub fn alerts_ok(&self) -> bool {
        self.alerts_ok
    }

    pub fn shown_alerts(&self) -> &[Alert] {
        match self.reach(status::Service::Alerts) {
            Reach::Answers => &self.alerts,
            Reach::Unknown | Reach::NotInLab | Reach::StackDown | Reach::NoAnswer => &[],
        }
    }

    pub fn visible_alerts(&self) -> Vec<usize> {
        self.shown_alerts()
            .iter()
            .enumerate()
            .filter(|(_, alert)| {
                let label = |name: &str| alert.labels.get(name).cloned().unwrap_or_default();
                [
                    label("alertname"),
                    label("device"),
                    label("severity"),
                    alert.state_word(),
                ]
                .iter()
                .any(|text| self.alert_filter.matches_any_case(text))
            })
            .map(|(index, _)| index)
            .collect()
    }

    pub(crate) fn selected_alert_row(&self) -> Option<usize> {
        kept_row(&self.visible_alerts(), |&index| {
            Some(&self.alerts[index].labels) == self.kept_alert.as_ref()
        })
    }

    pub fn selected_alert(&self) -> Option<&Alert> {
        let row = self.selected_alert_row()?;
        Some(&self.alerts[self.visible_alerts()[row]])
    }

    fn keep_alert(&mut self) {
        if self.workbench != Workbench::Observability {
            return;
        }
        if let Some(alert) = self.selected_alert() {
            self.kept_alert = Some(alert.labels.clone());
        }
    }

    pub fn metrics_rows(&self) -> Vec<devices::MetricsRow> {
        self.visible_metric_devices()
            .into_iter()
            .map(|index| devices::metrics_row(&self.devices[index], self.metrics.as_deref()))
            .collect()
    }

    pub(crate) fn metrics_listed(&self) -> bool {
        !self.devices.is_empty()
            && !matches!(
                self.reach(status::Service::Metrics),
                Reach::NotInLab | Reach::StackDown
            )
    }

    pub(crate) fn visible_metric_devices(&self) -> Vec<usize> {
        if !self.metrics_listed() {
            return Vec::new();
        }
        self.devices_matching(&self.metrics_filter)
    }

    pub(crate) fn marked_metrics_row(&self) -> Option<usize> {
        kept_row(&self.visible_metric_devices(), |&index| {
            Some(&self.devices[index].name) == self.kept_device.as_ref()
        })
    }

    pub(crate) fn marked_metrics_device(&self) -> Option<usize> {
        Some(self.visible_metric_devices()[self.marked_metrics_row()?])
    }

    fn keep_marked_device(&mut self) {
        if self.workbench != Workbench::Observability {
            return;
        }
        if let Some(index) = self.marked_metrics_device() {
            self.kept_device = Some(self.devices[index].name.clone());
        }
    }

    pub fn alert_device(&self, alert: &Alert) -> Option<usize> {
        self.device_index(alert.labels.get("device")?)
    }

    pub(crate) fn device_index(&self, name: &str) -> Option<usize> {
        self.devices.iter().position(|device| device.name == name)
    }

    pub fn lab_logs(&self) -> ShownLabLogs<'_> {
        let rows = self
            .lab_logs
            .shown()
            .into_iter()
            .map(|held| {
                (
                    held.line.nanoseconds,
                    held.device.as_str(),
                    held.shown.as_str(),
                )
            })
            .collect();
        (
            rows,
            self.lab_logs.following,
            self.marked_lab_row(),
            self.lab_logs
                .filter
                .text
                .is_some()
                .then_some(self.lab_logs.lines.len()),
        )
    }

    pub(crate) fn lab_lines_shown(&self) -> bool {
        self.reach(status::Service::Logs) == Reach::Answers
    }

    fn marked_lab_row(&self) -> Option<usize> {
        if self.lab_logs.following {
            return None;
        }
        kept_row(&self.lab_logs.shown(), |held| {
            self.lab_mark
                .as_ref()
                .is_some_and(|(device, line)| held.device == *device && held.line == *line)
        })
    }

    fn marked_lab_line(&self) -> Option<&DeviceLine> {
        self.lab_logs.shown().get(self.marked_lab_row()?).copied()
    }

    fn keep_mark(&mut self) {
        if self.lab_logs.following {
            self.lab_mark = None;
        } else if let Some(held) = self.marked_lab_line() {
            self.lab_mark = Some((held.device.clone(), held.line.clone()));
        }
    }

    pub(crate) fn lab_can_pause(&self) -> bool {
        self.asking().contains(&status::Service::Logs)
    }

    pub(crate) fn lab_can_filter(&self) -> bool {
        self.lab_lines_shown() && !self.lab_logs.lines.is_empty()
    }

    pub(crate) fn lab_can_move(&self) -> bool {
        self.lab_lines_shown() && self.marked_lab_row().is_some()
    }

    pub(crate) fn lab_enter_device(&self) -> Option<usize> {
        if !self.lab_lines_shown() {
            return None;
        }
        self.device_index(&self.marked_lab_line()?.device)
    }

    pub fn interfaces(&self) -> Vec<devices::Row> {
        self.selected_device().map_or_else(Vec::new, |device| {
            devices::join(
                &device.name,
                &device.interfaces,
                &device.sessions,
                self.metrics.as_deref(),
            )
        })
    }

    pub fn session_states(&self) -> Vec<(Session, String)> {
        let Some(device) = self.selected_device() else {
            return Vec::new();
        };
        devices::session_states(device, self.metrics.as_deref())
    }

    pub fn timeline(&self) -> Vec<devices::Entry> {
        let Some(device) = self.selected_device() else {
            return Vec::new();
        };
        let answers = |service| self.reach(service) == Reach::Answers;
        let (rcas, logs): (&[Line], Vec<(i64, &str)>) = if answers(status::Service::Logs) {
            (&self.rcas, self.held_device_logs().collect())
        } else {
            (&[], Vec::new())
        };
        devices::merge(
            &device.name,
            if answers(status::Service::Alerts) {
                &self.alerts
            } else {
                &[]
            },
            if answers(status::Service::Flows) {
                &self.runs
            } else {
                &[]
            },
            rcas,
            &logs,
            Duration::from_secs(self.window * 60),
            self.wall,
        )
    }

    pub fn timeline_row(&self) -> usize {
        self.timeline_row
            .min(self.timeline().len().saturating_sub(1))
    }

    pub(crate) fn interface_row(&self) -> usize {
        self.interface_row
            .min(self.interfaces().len().saturating_sub(1))
    }

    pub fn popup(&self) -> bool {
        self.popup.is_some()
    }

    pub(crate) fn popup_entry(&self) -> Option<&devices::Entry> {
        self.popup.as_ref()
    }

    pub fn device_logs(&self) -> Option<ShownDeviceLogs<'_>> {
        let device = self.selected_device()?;
        Some((
            device.name.as_str(),
            self.shown_device_logs(),
            self.device_logs.following,
            self.logs_scroll,
        ))
    }

    fn held_device_logs(&self) -> impl Iterator<Item = (i64, &str)> {
        self.device_logs
            .lines
            .iter()
            .map(|held| (held.line.nanoseconds, held.shown.as_str()))
    }

    fn shown_device_logs(&self) -> Vec<(i64, &str)> {
        self.device_logs
            .shown()
            .into_iter()
            .map(|held| (held.line.nanoseconds, held.shown.as_str()))
            .collect()
    }

    pub fn window(&self) -> u64 {
        self.window
    }

    pub fn sot(&self) -> String {
        match self.lab.as_ref().map(|lab| &lab.kind) {
            Some(Kind::Workshop(workshop)) => workshop.sot.clone(),
            Some(Kind::BookLab(_) | Kind::LabcliLab(_))
                if self.source || self.source_error.is_some() =>
            {
                "lab_vars".to_string()
            }
            Some(Kind::BookLab(_) | Kind::LabcliLab(_)) => "none".to_string(),
            None => {
                let infrahub = self
                    .stacks
                    .get(self.selected)
                    .and_then(|stack| {
                        env_value(&stack.env, status::row(status::Service::Sot).setting?)
                    })
                    .is_some();
                if infrahub { "infrahub" } else { "none" }.to_string()
            }
        }
    }

    pub fn header(&self) -> String {
        if self.chooser {
            return "nobs  no lab here".to_string();
        }
        let stack = self.stacks.get(self.selected);
        let (name, kind) = match (&self.lab, stack) {
            (Some(lab), _) => located_name(lab),
            (None, Some(stack)) if stack.kind != StackKind::Sonda => {
                (stack.name.clone(), kind_text(stack.kind))
            }
            (None, _) => return format!("nobs  sonda only  {}", self.sonda.server()),
        };
        format!("nobs  {name} {kind}  sot {}", self.sot())
    }

    pub fn quit(&self) -> bool {
        self.quit
    }

    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }

    pub fn run(&self) -> Option<&Run> {
        self.run.as_ref()
    }

    pub fn question(&self) -> Option<&Question> {
        self.question.as_ref()
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn failed(&self, target: &Target) -> bool {
        self.failed.contains(target)
    }

    pub fn chapters(&self) -> Vec<usize> {
        (0..self.stacks.len())
            .filter(|index| self.stacks[*index].kind == StackKind::BookLab)
            .collect()
    }

    pub fn selected_chapter(&self) -> Option<usize> {
        self.stacks
            .get(self.chapter)
            .filter(|stack| stack.kind == StackKind::BookLab)
            .map(|_| self.chapter)
    }

    pub fn nodes(&self) -> &[book::NodeRow] {
        &self.nodes
    }

    pub fn nodes_ok(&self) -> bool {
        self.nodes_ok
    }

    pub(crate) fn node_row(&self) -> usize {
        self.node_row
    }

    pub(crate) fn located(&self) -> Option<&Located> {
        self.lab.as_ref()
    }

    pub fn book_lab(&self) -> bool {
        matches!(
            self.lab.as_ref().map(|lab| &lab.kind),
            Some(Kind::BookLab(_))
        )
    }

    pub fn workbenches(&self) -> Vec<Workbench> {
        WORKBENCHES
            .into_iter()
            .filter(|&workbench| workbench != Workbench::Lab || self.book_lab())
            .collect()
    }

    pub fn lab_with_nodes(&self) -> bool {
        matches!(
            self.lab.as_ref().map(|lab| &lab.kind),
            Some(Kind::BookLab(_) | Kind::LabcliLab(_))
        )
    }

    pub(crate) fn device_tabs(&self) -> &'static [Tab] {
        if self.lab_with_nodes() {
            &LAB_DEVICE_TABS
        } else {
            &DEVICE_TABS
        }
    }

    pub fn config(&self) -> Option<(&str, Option<&Config>, Vec<&str>, usize)> {
        let device = self.selected_device()?;
        let kept = self.configs.get(&device.name);
        let shown = kept.map_or_else(Vec::new, |kept| {
            kept.lines
                .iter()
                .map(String::as_str)
                .filter(|line| self.config_filter.matches(line))
                .collect()
        });
        Some((device.name.as_str(), kept, shown, self.config_scroll))
    }

    #[cfg(test)]
    pub(crate) fn config_of(&self, device: &str) -> Option<&Config> {
        self.configs.get(device)
    }

    fn types_q(&self) -> bool {
        !self.too_small() && (self.typing() || self.prompting())
    }

    fn prompting(&self) -> bool {
        self.workbench == Workbench::Services
            && self.focus != Focus::List
            && self.tab == Tab::Sonda
            && self.sonda.prompt() != &Prompt::None
    }

    fn has_lines(&self) -> bool {
        self.workbench == Workbench::Lab
            || (self.workbench == Workbench::Services && self.tab == Tab::Sonda)
    }

    fn settle_focus(&mut self) {
        if self.focus == Focus::Lines && !self.has_lines() {
            self.focus = Focus::Tabs;
        }
    }

    fn apply_services(
        &mut self,
        index: usize,
        defined: Result<Vec<String>, String>,
        listed: Result<Vec<lab::Service>, String>,
    ) {
        let Some(name) = self.stacks.get(index).map(|stack| stack.name.clone()) else {
            return;
        };
        let listed = self.answered(&format!("ps {name}"), listed);
        let defined = self.answered(&format!("config {name}"), defined);
        self.defined[index] = defined.clone();
        self.states[index] = match (&defined, &listed) {
            (Some(defined), Some(listed)) => StackState::of(defined, listed),
            _ => StackState::Failed,
        };
        let Some(rows) = listed.filter(|_| index == self.selected) else {
            return;
        };
        let without_row: Vec<String> = defined
            .unwrap_or_default()
            .into_iter()
            .filter(|name| rows.iter().all(|row| row.name != *name))
            .collect();
        let mut services: Vec<Service> = rows
            .into_iter()
            .map(|row| Service {
                health: health_of(&self.health, &row.name, &row.state),
                name: row.name,
                state: row.state,
                port: row.port,
                age: row.age,
            })
            .collect();
        services.extend(without_row.into_iter().map(|name| Service {
            health: health_of(&self.health, &name, "—"),
            name,
            state: "—".to_string(),
            port: None,
            age: None,
        }));
        self.services = services;
        self.service = self.service.min(self.services.len().saturating_sub(1));
        self.services_at = Some(self.now);
    }

    fn select(&mut self, index: usize) {
        self.selected = index;
        self.sonda = data::Model::new(self.sonda_address(index));
        self.services.clear();
        self.services_at = None;
        self.health.clear();
        self.service = 0;
        self.logs = None;
        self.lab_logs.clear();
        self.lab_mark = None;
        if self.tab == Tab::Logs {
            self.tab = Tab::Overview;
        }
    }

    fn chooser_key(&mut self, code: KeyCode) -> Vec<Command> {
        match code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.selected = (self.selected + 1).min(self.stacks.len().saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Enter => {
                let index = self.selected;
                let Some(stack) = self.stacks.get(index) else {
                    return Vec::new();
                };
                let tab = if stack.kind == StackKind::Sonda {
                    Tab::Sonda
                } else {
                    Tab::Overview
                };
                self.chooser = false;
                self.select(index);
                self.tab = tab;
                if tab == Tab::Sonda {
                    self.focus = Focus::Tabs;
                }
                return vec![Command::SelectStack(index)];
            }
            _ => {}
        }
        Vec::new()
    }

    fn workbench_key(&mut self, key: KeyEvent) -> Vec<Command> {
        if let Some(mut asked) = self.asked.take() {
            asked.text.type_key(key.code);
            if asked.text.typing {
                self.asked = Some(asked);
                return Vec::new();
            }
            let Some(text) = asked.text.text.take() else {
                return Vec::new();
            };
            let name = match asked.step {
                Step::Command => {
                    asked.args.push(vec![OsString::from(text)]);
                    return self.start_run(asked.args, asked.target, false, asked.typed);
                }
                Step::User { .. } => device::USER,
                Step::Password { .. } => device::PASSWORD,
            };
            let Some(value) = device::credential(name, Some(text), &[]) else {
                return Vec::new();
            };
            if name == device::USER {
                asked.typed.user = Some(value);
            } else {
                asked.typed.password = Some(Password::new(value));
            }
            return match self.asked_step(&asked.typed) {
                Some(step) => {
                    self.ask(asked.action, asked.args, asked.target, step, asked.typed);
                    Vec::new()
                }
                None => self.act(asked.action, asked.args, asked.target, asked.typed),
            };
        }
        if self.filter.typing {
            self.filter.type_key(key.code);
            return self.show_cursor();
        }
        if self.device_filter.typing {
            self.device_filter.type_key(key.code);
            return self.show_cursor();
        }
        if self.device_logs.filter.typing {
            self.device_logs.filter.type_key(key.code);
            return Vec::new();
        }
        if self.lab_logs.filter.typing {
            self.lab_logs.filter.type_key(key.code);
            return Vec::new();
        }
        if self.config_filter.typing {
            self.config_filter.type_key(key.code);
            return Vec::new();
        }
        if self.alert_filter.typing {
            self.alert_filter.type_key(key.code);
            return Vec::new();
        }
        if self.metrics_filter.typing {
            self.metrics_filter.type_key(key.code);
            return Vec::new();
        }
        if let Some(logs) = self.logs.as_mut().filter(|logs| logs.filter.typing) {
            logs.filter.type_key(key.code);
            return Vec::new();
        }
        let sonda_keys = self.workbench == Workbench::Services
            && matches!(self.focus, Focus::Tabs | Focus::Lines)
            && self.tab == Tab::Sonda;
        if self.prompting() && key.code != KeyCode::Esc {
            return self.sonda_key(key);
        }
        if self.focus != Focus::Run {
            if let Some(commands) = self.action_key(key.code) {
                return commands;
            }
        }
        if let Some(place) = workbench_place(key.code) {
            return match self.workbenches().get(place) {
                Some(&workbench) => self.switch(workbench),
                None => Vec::new(),
            };
        }
        match key.code {
            KeyCode::Char('?') => {
                self.help = true;
                self.help_scroll = 0;
            }
            KeyCode::Char('x') => self.dismiss_error(),
            KeyCode::Char('o') if self.run_pane => self.fold_pane(),
            KeyCode::Char('o') => self.run_pane = true,
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::List if self.workbench != Workbench::Observability => Focus::Tabs,
                    Focus::Tabs if self.has_lines() => Focus::Lines,
                    Focus::List | Focus::Tabs | Focus::Lines if self.run_view.drawn => Focus::Run,
                    Focus::List | Focus::Tabs | Focus::Lines | Focus::Run => Focus::List,
                };
            }
            KeyCode::Esc => return self.escape(key),
            KeyCode::Char('r') if !sonda_keys => return vec![Command::Refresh],
            KeyCode::Char('j' | 'k' | 'f') | KeyCode::Up | KeyCode::Down
                if self.focus == Focus::Run =>
            {
                return self.run_key(key.code)
            }
            _ if self.focus == Focus::Run => {}
            _ if self.workbench == Workbench::Services => return self.services_key(key),
            _ if self.workbench == Workbench::Devices => return self.devices_key(key),
            _ if self.workbench == Workbench::Lab => return self.lab_key(key.code),
            _ if self.workbench == Workbench::Observability => {
                return self.observability_key(key.code)
            }
            _ => {}
        }
        Vec::new()
    }

    fn switch(&mut self, workbench: Workbench) -> Vec<Command> {
        let left_devices = self.workbench == Workbench::Devices && workbench != Workbench::Devices;
        self.workbench = workbench;
        if matches!(workbench, Workbench::Lab | Workbench::Observability) {
            self.focus = Focus::List;
        }
        let tabs: &[Tab] = match workbench {
            Workbench::Devices => self.device_tabs(),
            Workbench::Services => &SERVICES_TABS,
            Workbench::Lab | Workbench::Observability => &[],
        };
        if !tabs.is_empty() && !tabs.contains(&self.tab) {
            self.tab = tabs[0];
        }
        self.settle_focus();
        if left_devices {
            return vec![Command::OpenTab(Tab::Overview)];
        }
        match (workbench, self.selected_device()) {
            (Workbench::Devices, Some(device)) => vec![
                Command::SelectDevice(device.name.clone()),
                Command::OpenTab(self.tab),
            ],
            _ => Vec::new(),
        }
    }

    fn escape(&mut self, key: KeyEvent) -> Vec<Command> {
        if self.run_pane {
            self.fold_pane();
            return Vec::new();
        }
        if self.workbench == Workbench::Services {
            if self.tab == Tab::Sonda && self.sonda.prompt() != &Prompt::None {
                return self.sonda_key(key);
            }
            if self.tab == Tab::Logs {
                self.logs = None;
                self.tab = Tab::Overview;
                self.focus = Focus::List;
                return vec![Command::CloseLogs];
            }
        }
        if self.workbench == Workbench::Devices && self.popup.is_some() {
            self.popup = None;
            return Vec::new();
        }
        self.focus = Focus::List;
        Vec::new()
    }

    fn fold_pane(&mut self) {
        self.run_pane = false;
        self.run_view.drawn = false;
        if self.focus == Focus::Run {
            self.focus = Focus::List;
        }
    }

    fn run_key(&mut self, code: KeyCode) -> Vec<Command> {
        let view = &mut self.run_view;
        if code == KeyCode::Char('f') {
            view.scroll = view.overflow;
        } else {
            scroll_step(code, &mut view.scroll, view.overflow + 1);
        }
        view.following = view.scroll == view.overflow;
        Vec::new()
    }

    fn sonda_key(&mut self, key: KeyEvent) -> Vec<Command> {
        self.sonda
            .handle(key)
            .into_iter()
            .map(Command::Sonda)
            .collect()
    }

    fn services_key(&mut self, key: KeyEvent) -> Vec<Command> {
        if self.focus != Focus::List {
            match key.code {
                KeyCode::Char('[') => return self.go_tab(step(&SERVICES_TABS, self.tab, false)),
                KeyCode::Char(']') => return self.go_tab(step(&SERVICES_TABS, self.tab, true)),
                _ => {}
            }
        }
        match (self.focus, self.tab) {
            (Focus::List, _) => self.list_key(key.code),
            (Focus::Lines, _) => self.details_key(key),
            (
                Focus::Tabs,
                Tab::Overview
                | Tab::Interfaces
                | Tab::Config
                | Tab::Timeline
                | Tab::Alerts
                | Tab::Metrics,
            ) => self.overview_key(key.code),
            (Focus::Tabs, Tab::Logs) => self.logs_key(key.code),
            (Focus::Tabs, Tab::Sonda) => self.sonda_key(key),
            (Focus::Run, _) => Vec::new(),
        }
    }

    fn details_key(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.sonda.move_details(true),
            KeyCode::Char('k') | KeyCode::Up => self.sonda.move_details(false),
            _ => return self.sonda_key(key),
        }
        Vec::new()
    }

    fn devices_key(&mut self, key: KeyEvent) -> Vec<Command> {
        if self.focus == Focus::List {
            return self.device_list_key(key.code);
        }
        match key.code {
            KeyCode::Char('[') => return self.open_device_tab(false),
            KeyCode::Char(']') => return self.open_device_tab(true),
            _ => {}
        }
        match self.tab {
            Tab::Overview => self.device_overview_key(key.code),
            Tab::Interfaces => self.interfaces_key(key.code),
            Tab::Timeline => self.timeline_key(key.code),
            Tab::Logs => self.device_logs_key(key.code),
            Tab::Config => self.config_key(key.code),
            _ => Vec::new(),
        }
    }

    fn open_device_tab(&mut self, forward: bool) -> Vec<Command> {
        self.tab = step(self.device_tabs(), self.tab, forward);
        let mut commands: Vec<Command> = self
            .selected_device()
            .map(|device| Command::SelectDevice(device.name.clone()))
            .into_iter()
            .collect();
        commands.push(Command::OpenTab(self.tab));
        commands
    }

    fn device_list_key(&mut self, code: KeyCode) -> Vec<Command> {
        match code {
            KeyCode::Char('j') | KeyCode::Down => return self.step_cursor(true),
            KeyCode::Char('k') | KeyCode::Up => return self.step_cursor(false),
            KeyCode::Char('/') => self.device_filter.start(),
            KeyCode::Enter => return self.enter_cursor(),
            _ => {}
        }
        Vec::new()
    }

    fn step_cursor(&mut self, down: bool) -> Vec<Command> {
        let visible = self.visible_devices();
        let next = match (self.cursor, down) {
            (Cursor::Device(at), true) => visible
                .iter()
                .find(|index| **index > at)
                .map(|index| Cursor::Device(*index))
                .or_else(|| (!self.alerts.is_empty()).then_some(Cursor::Alert(0))),
            (Cursor::Device(at), false) => visible
                .iter()
                .rev()
                .find(|index| **index < at)
                .map(|index| Cursor::Device(*index)),
            (Cursor::Alert(at), true) => {
                (at + 1 < self.alerts.len()).then_some(Cursor::Alert(at + 1))
            }
            (Cursor::Alert(0), false) => visible.last().map(|index| Cursor::Device(*index)),
            (Cursor::Alert(at), false) => Some(Cursor::Alert(at - 1)),
        };
        let Some(next) = next else {
            return Vec::new();
        };
        if let Cursor::Device(index) = next {
            return self.go_device(index);
        }
        self.cursor = next;
        Vec::new()
    }

    fn go_device(&mut self, index: usize) -> Vec<Command> {
        self.cursor = Cursor::Device(index);
        self.select_device(index)
    }

    fn show_cursor(&mut self) -> Vec<Command> {
        let mut commands = Vec::new();
        let stacks = self.visible_stacks();
        if let Some(&first) = stacks.first().filter(|_| !stacks.contains(&self.selected)) {
            commands.extend(self.go_stack(first));
        }
        let devices = self.visible_devices();
        if let (Cursor::Device(at), Some(&first)) = (self.cursor, devices.first()) {
            if !devices.contains(&at) {
                commands.extend(self.go_device(first));
            }
        }
        commands
    }

    fn enter_cursor(&mut self) -> Vec<Command> {
        match self.cursor {
            Cursor::Device(index) if index < self.devices.len() => {
                let mut commands = self.select_device(index);
                self.focus = Focus::Tabs;
                self.tab = Tab::Overview;
                commands.push(Command::OpenTab(Tab::Overview));
                commands
            }
            Cursor::Device(_) => Vec::new(),
            Cursor::Alert(at) => {
                match self
                    .alerts
                    .get(at)
                    .and_then(|alert| self.alert_device(alert))
                {
                    Some(index) => self.open_device_on(index, Tab::Timeline),
                    None => Vec::new(),
                }
            }
        }
    }

    fn open_device_on(&mut self, index: usize, tab: Tab) -> Vec<Command> {
        let mut commands = self.select_device(index);
        self.focus = Focus::Tabs;
        self.tab = tab;
        commands.push(Command::OpenTab(tab));
        commands
    }

    fn observability_key(&mut self, code: KeyCode) -> Vec<Command> {
        match code {
            KeyCode::Char('[') => self.view = step(&OBSERVABILITY_TABS, self.view, false),
            KeyCode::Char(']') => self.view = step(&OBSERVABILITY_TABS, self.view, true),
            _ if self.view == Tab::Logs => return self.lab_logs_key(code),
            _ if self.view == Tab::Metrics => return self.metrics_key(code),
            KeyCode::Char('j') | KeyCode::Down => self.step_alert(true),
            KeyCode::Char('k') | KeyCode::Up => self.step_alert(false),
            KeyCode::Char('/') if !self.shown_alerts().is_empty() => self.alert_filter.start(),
            KeyCode::Enter => {
                let index = self
                    .selected_alert()
                    .and_then(|alert| self.alert_device(alert));
                return self.open_in_devices(index, Tab::Timeline);
            }
            _ => {}
        }
        Vec::new()
    }

    fn lab_logs_key(&mut self, code: KeyCode) -> Vec<Command> {
        match code {
            KeyCode::Char('f') if self.lab_can_pause() => {
                self.lab_logs.toggle();
            }
            KeyCode::Char('/') if self.lab_can_filter() => self.lab_logs.filter.start(),
            KeyCode::Char('j') | KeyCode::Down if self.lab_can_move() => self.step_mark(true),
            KeyCode::Char('k') | KeyCode::Up if self.lab_can_move() => self.step_mark(false),
            KeyCode::Enter => return self.open_in_devices(self.lab_enter_device(), Tab::Logs),
            _ => {}
        }
        Vec::new()
    }

    fn metrics_key(&mut self, code: KeyCode) -> Vec<Command> {
        let down = match code {
            KeyCode::Char('j') | KeyCode::Down => true,
            KeyCode::Char('k') | KeyCode::Up => false,
            KeyCode::Char('/') if self.metrics_listed() => {
                self.metrics_filter.start();
                return Vec::new();
            }
            KeyCode::Enter => {
                return self.open_in_devices(self.marked_metrics_device(), Tab::Interfaces)
            }
            _ => return Vec::new(),
        };
        let visible = self.visible_metric_devices();
        let next = self
            .marked_metrics_row()
            .and_then(|at| neighbour(&visible, at, down));
        if let Some(index) = next {
            self.kept_device = Some(self.devices[index].name.clone());
        }
        Vec::new()
    }

    fn step_mark(&mut self, down: bool) {
        let Some(at) = self.marked_lab_row() else {
            return;
        };
        if let Some(held) = neighbour(&self.lab_logs.shown(), at, down) {
            self.lab_mark = Some((held.device.clone(), held.line.clone()));
        }
    }

    fn step_alert(&mut self, down: bool) {
        let Some(at) = self.selected_alert_row() else {
            return;
        };
        if let Some(index) = neighbour(&self.visible_alerts(), at, down) {
            self.kept_alert = Some(self.alerts[index].labels.clone());
        }
    }

    fn open_in_devices(&mut self, index: Option<usize>, tab: Tab) -> Vec<Command> {
        let Some(index) = index else {
            return Vec::new();
        };
        self.workbench = Workbench::Devices;
        self.cursor = Cursor::Device(index);
        self.open_device_on(index, tab)
    }

    fn select_device(&mut self, index: usize) -> Vec<Command> {
        let Some(name) = self.devices.get(index).map(|device| device.name.clone()) else {
            return Vec::new();
        };
        if index != self.device {
            self.device = index;
            self.device_logs.clear();
            self.rcas.clear();
            self.logs_scroll = 0;
            self.config_scroll = 0;
            self.timeline_row = 0;
            self.interface_row = 0;
            self.overview_scroll = 0;
            self.popup = None;
        }
        vec![Command::SelectDevice(name)]
    }

    fn device_overview_key(&mut self, code: KeyCode) -> Vec<Command> {
        match code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.overview_scroll = (self.overview_scroll + 1).min(self.overview_overflow);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.overview_scroll = self.overview_scroll.saturating_sub(1);
            }
            _ => {}
        }
        Vec::new()
    }

    fn interfaces_key(&mut self, code: KeyCode) -> Vec<Command> {
        let last = self.interfaces().len().saturating_sub(1);
        let row = self.interface_row();
        match code {
            KeyCode::Char('j') | KeyCode::Down => self.interface_row = (row + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => self.interface_row = row.saturating_sub(1),
            _ => {}
        }
        Vec::new()
    }

    fn timeline_key(&mut self, code: KeyCode) -> Vec<Command> {
        let entries = self.timeline();
        let last = entries.len().saturating_sub(1);
        let row = self.timeline_row();
        match code {
            KeyCode::Char('j') | KeyCode::Down if self.popup.is_some() => {
                self.popup_scroll = (self.popup_scroll + 1).min(self.popup_overflow);
            }
            KeyCode::Char('k') | KeyCode::Up if self.popup.is_some() => {
                self.popup_scroll = self.popup_scroll.saturating_sub(1);
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.timeline_row = (row + 1).min(last);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.timeline_row = row.saturating_sub(1);
            }
            KeyCode::Enter if self.popup.is_none() => {
                self.popup = entries.get(row).cloned();
                self.popup_scroll = 0;
            }
            KeyCode::Char('-') => {
                self.window = (self.window / 2).max(WINDOW_MIN);
                return vec![Command::Window(self.window)];
            }
            KeyCode::Char('+') => {
                self.window = (self.window * 2).min(WINDOW_MAX);
                return vec![Command::Window(self.window)];
            }
            _ => {}
        }
        Vec::new()
    }

    fn device_logs_key(&mut self, code: KeyCode) -> Vec<Command> {
        match code {
            KeyCode::Char('f') => {
                let following = self.device_logs.toggle();
                if following {
                    self.logs_scroll = 0;
                }
                return vec![Command::FollowDevice(following)];
            }
            KeyCode::Char('/') => self.device_logs.filter.start(),
            _ if !self.device_logs.following => {
                let shown = self.shown_device_logs().len();
                scroll_step(code, &mut self.logs_scroll, shown);
            }
            _ => {}
        }
        Vec::new()
    }

    fn config_key(&mut self, code: KeyCode) -> Vec<Command> {
        if code == KeyCode::Char('/') {
            self.config_filter.start();
        } else {
            let shown = self.config().map_or(0, |(_, _, shown, _)| shown.len());
            scroll_step(code, &mut self.config_scroll, shown);
        }
        Vec::new()
    }

    fn list_key(&mut self, code: KeyCode) -> Vec<Command> {
        match code {
            KeyCode::Char('j') | KeyCode::Down => return self.step_stack(true),
            KeyCode::Char('k') | KeyCode::Up => return self.step_stack(false),
            KeyCode::Char('/') => self.filter.start(),
            KeyCode::Enter => self.focus = Focus::Tabs,
            KeyCode::Char('l') if !self.services.is_empty() => return self.open_logs(),
            _ => {}
        }
        Vec::new()
    }

    fn step_stack(&mut self, down: bool) -> Vec<Command> {
        let mut visible = self.visible_stacks().into_iter();
        let next = if down {
            visible.find(|index| *index > self.selected)
        } else {
            visible.rev().find(|index| *index < self.selected)
        };
        match next {
            Some(index) => self.go_stack(index),
            None => Vec::new(),
        }
    }

    fn go_stack(&mut self, index: usize) -> Vec<Command> {
        self.select(index);
        vec![Command::SelectStack(index)]
    }

    fn overview_key(&mut self, code: KeyCode) -> Vec<Command> {
        let last = self.services.len().saturating_sub(1);
        match code {
            KeyCode::Char('j') | KeyCode::Down => self.service = (self.service + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => self.service = self.service.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char('l') if !self.services.is_empty() => {
                return self.open_logs();
            }
            _ => {}
        }
        Vec::new()
    }

    fn logs_key(&mut self, code: KeyCode) -> Vec<Command> {
        let Some(logs) = self.logs.as_mut() else {
            return Vec::new();
        };
        match code {
            KeyCode::Char('f') => {
                logs.following = !logs.following;
                if logs.following {
                    logs.scroll = 0;
                }
                return vec![Command::Follow(logs.following)];
            }
            KeyCode::Char('/') => logs.filter.start(),
            KeyCode::Char('j') | KeyCode::Down if !logs.following => {
                logs.scroll = logs.scroll.saturating_sub(1);
            }
            KeyCode::Char('k') | KeyCode::Up if !logs.following => {
                logs.scroll = (logs.scroll + 1).min(logs.shown().saturating_sub(1));
            }
            _ => {}
        }
        Vec::new()
    }

    fn go_tab(&mut self, target: Tab) -> Vec<Command> {
        let mut commands = Vec::new();
        if self.tab == Tab::Logs {
            self.logs = None;
            commands.push(Command::CloseLogs);
        }
        if target == Tab::Logs {
            commands.extend(self.open_logs());
        } else {
            self.tab = target;
        }
        self.settle_focus();
        commands
    }

    fn open_logs(&mut self) -> Vec<Command> {
        let service = self.services.get(self.service).map(|row| row.name.clone());
        self.logs = Some(LogsState {
            service: service.clone().unwrap_or_default(),
            lines: Vec::new(),
            following: true,
            filter: Filter::default(),
            scroll: 0,
        });
        self.tab = Tab::Logs;
        self.focus = Focus::Tabs;
        service
            .map(|service| Command::OpenLogs {
                stack: self.selected,
                service,
            })
            .into_iter()
            .collect()
    }
}

pub fn handle(model: &mut Model, key: KeyEvent) -> Vec<Command> {
    let commands = take_key(model, key);
    model.keep_alert();
    model.keep_marked_device();
    model.keep_mark();
    commands
}

fn take_key(model: &mut Model, key: KeyEvent) -> Vec<Command> {
    if key.kind != KeyEventKind::Press {
        return Vec::new();
    }
    model.notice = None;
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let quit = match key.code {
        KeyCode::Char('c') => control,
        KeyCode::Char('q') => !control && !alt && !model.types_q(),
        _ => false,
    };
    let ctrl_c = quit && control;
    if ctrl_c && model.active().is_some() {
        return model.stop();
    }
    if !ctrl_c {
        if let Some(question) = model.question.take() {
            return model.answer(question, key);
        }
    }
    if quit && model.active().is_some() {
        model.question = Some(Question::Quit);
        return Vec::new();
    }
    if quit {
        model.quit = true;
        model.exit_code = i32::from(model.chooser);
        return vec![Command::Quit];
    }
    if control || alt || model.too_small() {
        return Vec::new();
    }
    if model.help {
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc => model.help = false,
            KeyCode::Char('j') => {
                model.help_scroll = (model.help_scroll + 1).min(model.help_overflow);
            }
            KeyCode::Char('k') => model.help_scroll = model.help_scroll.saturating_sub(1),
            _ => {}
        }
        return Vec::new();
    }
    if model.chooser {
        return model.chooser_key(key.code);
    }
    model.workbench_key(key)
}

pub fn stacks(
    lab: Option<&Located>,
    workshops: Vec<(String, String, PathBuf)>,
) -> Result<Vec<Stack>, String> {
    let workshops = workshops
        .into_iter()
        .map(|(name, _, dir)| workshop_stack(name, dir));
    match lab {
        Some(lab) if matches!(lab.kind, Kind::BookLab(_)) => {
            let mut stacks = book_stacks(lab)?;
            stacks.extend(workshops);
            Ok(stacks)
        }
        Some(lab) if matches!(lab.kind, Kind::LabcliLab(_)) => Ok(vec![lab_stack(lab)?]),
        _ => {
            if let Some(lab) = lab {
                read_env(&lab.dir)?;
            }
            Ok(workshops.collect())
        }
    }
}

pub fn sonda_only(here: &Path) -> Result<Stack, String> {
    Ok(Stack {
        name: "sonda only".to_string(),
        kind: StackKind::Sonda,
        path: here.to_path_buf(),
        compose: here.to_path_buf(),
        project: None,
        env: read_env(here)?,
        shows_sonda: true,
    })
}

fn workshop_stack(name: String, dir: PathBuf) -> Stack {
    let located = locate(&dir, Some(&dir))
        .ok()
        .filter(|located| matches!(located.kind, Kind::Workshop(_)));
    Stack {
        kind: StackKind::Workshop,
        compose: located.as_ref().map_or_else(
            || dir.join("docker-compose.yml"),
            |located| located.compose(None),
        ),
        project: located
            .as_ref()
            .and_then(Located::project)
            .map(str::to_string),
        env: read_env(&dir).unwrap_or_default(),
        name,
        path: dir,
        shows_sonda: false,
    }
}

fn book_stacks(lab: &Located) -> Result<Vec<Stack>, String> {
    let chapters = lab::scenarios(&lab.dir)?;
    let env = read_env(&lab.dir)?;
    Ok(chapters
        .into_iter()
        .map(|chapter| Stack {
            compose: lab.compose(Some(&chapter)),
            name: chapter,
            kind: StackKind::BookLab,
            path: lab.dir.clone(),
            project: lab.project().map(str::to_string),
            env: env.clone(),
            shows_sonda: false,
        })
        .collect())
}

fn lab_stack(lab: &Located) -> Result<Stack, String> {
    Ok(Stack {
        name: folder_name(&lab.dir),
        kind: StackKind::Lab,
        path: lab.dir.clone(),
        compose: lab.compose(None),
        project: lab.project().map(str::to_string),
        env: read_env(&lab.dir)?,
        shows_sonda: false,
    })
}

fn folder_name(dir: &Path) -> String {
    dir.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn located_name(lab: &Located) -> (String, &'static str) {
    match &lab.kind {
        Kind::Workshop(workshop) => (workshop.name.clone(), "workshop"),
        Kind::BookLab(_) => (folder_name(&lab.dir), "book lab"),
        Kind::LabcliLab(_) => (folder_name(&lab.dir), "lab"),
    }
}

pub fn kind_text(kind: StackKind) -> &'static str {
    match kind {
        StackKind::Workshop => "workshop",
        StackKind::BookLab => "book lab",
        StackKind::Lab => "lab",
        StackKind::Sonda => "sonda only",
    }
}

pub(crate) fn env_value<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
    env.iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.trim().is_empty())
}

fn sonda_address<'a>(flag: Option<&'a str>, stack: Option<&'a Stack>) -> Option<&'a str> {
    flag.or_else(|| {
        let setting = status::row(status::Service::Sonda).setting?;
        stack.and_then(|stack| env_value(&stack.env, setting))
    })
}

fn sonda_set(flag: Option<&str>, stack: &Stack) -> bool {
    stack.shows_sonda || sonda_address(flag, Some(stack)).is_some()
}

pub(crate) fn sonda_url(flag: Option<&str>, stack: Option<&Stack>) -> String {
    match sonda_address(flag, stack) {
        Some(url) => url.to_string(),
        None if stack.is_some_and(|stack| stack.kind != StackKind::Sonda) => {
            status::row(status::Service::Sonda).default.to_string()
        }
        None => Sonda::DEFAULT_URL.to_string(),
    }
}

pub(crate) fn watched(stack: &Stack) -> bool {
    stack.kind != StackKind::Sonda
}

fn error_source(service: status::Service) -> &'static str {
    status::row(service).word
}

fn health_of(health: &[Health], name: &str, state: &str) -> Option<String> {
    if state != RUNNING {
        return None;
    }
    health
        .iter()
        .find(|entry| entry.service == name)
        .map(|entry| entry.state.clone())
}

fn scroll_step(code: KeyCode, scroll: &mut usize, shown: usize) {
    match code {
        KeyCode::Char('j') | KeyCode::Down => *scroll = (*scroll + 1).min(shown.saturating_sub(1)),
        KeyCode::Char('k') | KeyCode::Up => *scroll = scroll.saturating_sub(1),
        _ => {}
    }
}

fn keep_last(lines: &mut Vec<String>) -> Drain<'_, String> {
    let extra = lines.len().saturating_sub(LOG_LINES);
    lines.drain(..extra)
}

fn push_line(run: &mut Run, view: &mut RunView, line: String) {
    run.lines.push(line);
    let left = keep_last(&mut run.lines);
    if !view.following {
        view.cut.extend(left);
    }
}

fn untimed(line: &str) -> &str {
    line.split_once(' ').map_or(line, |(_, text)| text)
}

fn clock(wall: SystemTime) -> String {
    let seconds = wall
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
        % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use nobs_core::lab_vars;
    use nobs_core::locate::locate;
    use nobs_core::sot::Fixed;

    use super::*;
    use crate::actions::{arguments, Action};
    use crate::screen::{help_entries, key_line};

    const TWO: &str = r#"{"scenarios":[{"id":"a1","name":"cpu","state":"running"},{"id":"b2","name":"mem","state":"paused"}]}"#;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn code(model: &mut Model, code: KeyCode) -> Vec<Command> {
        handle(model, key(code))
    }

    fn ch(model: &mut Model, typed: char) -> Vec<Command> {
        code(model, KeyCode::Char(typed))
    }

    fn typed(model: &mut Model, text: &str) {
        for letter in text.chars() {
            ch(model, letter);
        }
    }

    pub(crate) fn fixture(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures")
            .join(relative)
    }

    pub(crate) fn stack(name: &str, env: &[(&str, &str)]) -> Stack {
        let path = PathBuf::from("/ws").join(name);
        Stack {
            name: name.to_string(),
            kind: StackKind::Workshop,
            compose: path.join("docker-compose.yml"),
            path,
            project: Some(name.to_string()),
            env: env
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            shows_sonda: false,
        }
    }

    fn target(server: Option<&str>) -> SondaTarget {
        SondaTarget {
            server: server.map(str::to_string),
            api_key: None,
        }
    }

    pub(crate) const USER: &str = "nobs-t-x";
    pub(crate) const SECRET: &str = "nobs-t-not-a-secret";

    pub(crate) fn finding(user: Option<&'static str>, password: Option<&'static str>) -> Lookup {
        Arc::new(move |name| {
            match name {
                device::USER => user,
                device::PASSWORD => password,
                _ => None,
            }
            .map(str::to_string)
        })
    }

    pub(crate) fn start(stacks: Vec<Stack>) -> Start {
        Start {
            lab: None,
            stacks,
            sonda: target(None),
            home: Workbench::Services,
            chooser: true,
            source: None,
            source_error: None,
            source_service: None,
            urls: Urls::defaults(),
            program: PathBuf::from("/bin/nobs"),
            login: finding(Some(USER), Some(SECRET)),
        }
    }

    fn opened(names: &[&str]) -> Model {
        let stacks = names.iter().map(|name| stack(name, &[])).collect();
        let mut model = Model::new(start(stacks));
        code(&mut model, KeyCode::Enter);
        model
    }

    fn chooser(names: &[&str]) -> Model {
        let stacks = names.iter().map(|name| stack(name, &[])).collect();
        Model::new(start(stacks))
    }

    fn sonda_stack() -> Stack {
        sonda_only(Path::new("/nonexistent-sonda-folder")).unwrap()
    }

    fn sonda_tab() -> Model {
        Model::new(Start {
            chooser: false,
            urls: Urls {
                set: vec![status::Service::Sonda],
                ..Urls::defaults()
            },
            ..start(vec![sonda_stack()])
        })
    }

    pub(crate) fn row(name: &str, state: &str) -> lab::Service {
        lab::Service {
            name: name.to_string(),
            state: state.to_string(),
            health: String::new(),
            port: None,
            age: None,
        }
    }

    pub(crate) fn every_service_up(index: usize) -> Update {
        let names: Vec<&str> = SERVICES.iter().map(|row| row.compose).collect();
        let rows = names.iter().map(|name| row(name, RUNNING)).collect();
        Update::Services(index, defined(&names), Ok(rows))
    }

    pub(crate) fn defined(names: &[&str]) -> Result<Vec<String>, String> {
        Ok(names.iter().map(|name| name.to_string()).collect())
    }

    fn names_of(rows: &[lab::Service]) -> Vec<String> {
        let mut names: Vec<String> = rows.iter().map(|row| row.name.clone()).collect();
        names.sort();
        names.dedup();
        names
    }

    pub(crate) fn listing(index: usize, listed: Result<Vec<lab::Service>, String>) -> Update {
        let defined = Ok(listed.as_deref().map(names_of).unwrap_or_default());
        Update::Services(index, defined, listed)
    }

    fn with_services(model: &mut Model, names: &[&str]) {
        let rows = names.iter().map(|name| row(name, "running")).collect();
        model.apply(listing(0, Ok(rows)));
    }

    fn snapshot(model: &Model) -> String {
        format!(
            "{:?}",
            (
                model.chooser(),
                model.workbench(),
                model.focus(),
                model.tab(),
                model.selected_stack(),
                model.typed_filter(),
                model.typing(),
                model.selected_service(),
                model.help(),
                model.run_pane(),
                model.quit(),
                model.error().is_some()
            )
        )
    }

    fn small(model: &mut Model, columns: u16, rows: u16) {
        model.tick(Instant::now(), UNIX_EPOCH, (columns, rows));
    }

    pub(crate) fn sonda_rows(model: &mut Model) {
        model.apply(Update::Sonda(data::Update {
            at: Instant::now(),
            list: Ok(TWO.to_string()),
            selected: None,
        }));
    }

    fn every_state() -> Vec<(&'static str, Model)> {
        let mut states = vec![
            ("services list", opened(&["a", "b"])),
            ("chooser", chooser(&["a"])),
        ];
        let mut tabs = opened(&["a"]);
        code(&mut tabs, KeyCode::Enter);
        states.push(("services tabs", tabs));
        let mut devices = opened(&["a"]);
        ch(&mut devices, '1');
        states.push(("devices", devices));
        let mut help = opened(&["a"]);
        ch(&mut help, '?');
        states.push(("help", help));
        let mut tiny = opened(&["a"]);
        small(&mut tiny, 10, 5);
        states.push(("too small", tiny));
        let mut typing = opened(&["a"]);
        ch(&mut typing, '/');
        states.push(("typing", typing));
        let mut prompt = sonda_tab();
        ch(&mut prompt, 'l');
        states.push(("sonda prompt", prompt));
        states
    }

    #[test]
    fn q_and_ctrl_c_quit_in_every_state_and_the_chooser_exits_with_1() {
        for (name, mut model) in every_state() {
            if name == "typing" || name == "sonda prompt" {
                continue;
            }
            assert_eq!(ch(&mut model, 'q'), [Command::Quit], "{name}");
            assert!(model.quit(), "{name}");
            assert_eq!(model.exit_code(), i32::from(name == "chooser"), "{name}");
        }
        for (name, mut model) in every_state() {
            let ctrl_c = with(KeyCode::Char('c'), KeyModifiers::CONTROL);
            assert_eq!(handle(&mut model, ctrl_c), [Command::Quit], "{name}");
            assert!(model.quit(), "{name}");
            assert_eq!(model.exit_code(), i32::from(name == "chooser"), "{name}");
        }
    }

    fn with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn q_is_a_typed_letter_while_typing_and_in_a_sonda_question_and_ctrl_c_still_quits() {
        let mut model = opened(&["a", "b"]);
        ch(&mut model, '/');
        assert_eq!(ch(&mut model, 'q'), []);
        assert!(!model.quit());
        assert_eq!(model.typed_filter(), Some("q"));
        let ctrl_c = with(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(handle(&mut model, ctrl_c), [Command::Quit]);
        assert!(model.quit());

        let mut model = logs_open("grafana");
        model.apply(Update::Logs(
            0,
            "grafana".to_string(),
            Ok(timed(&["quiet", "loud"])),
        ));
        ch(&mut model, '/');
        assert_eq!(ch(&mut model, 'q'), []);
        assert!(!model.quit());
        assert_eq!(shown(&model), ["quiet"]);

        let mut model = sonda_tab();
        ch(&mut model, 'l');
        assert_eq!(ch(&mut model, 'q'), []);
        assert!(!model.quit());
        assert_eq!(model.sonda().prompt(), &Prompt::Path("q".to_string()));

        let mut model = sonda_tab();
        sonda_rows(&mut model);
        ch(&mut model, 'd');
        assert_eq!(ch(&mut model, 'q'), []);
        assert!(!model.quit());
        assert_eq!(model.sonda().prompt(), &Prompt::None);
        assert_eq!(ch(&mut model, 'q'), [Command::Quit]);

        let mut model = opened(&["a"]);
        ch(&mut model, '/');
        small(&mut model, 10, 5);
        assert_eq!(ch(&mut model, 'q'), [Command::Quit]);
    }

    #[test]
    fn esc_never_sets_quit() {
        for (name, mut model) in every_state() {
            for _ in 0..3 {
                code(&mut model, KeyCode::Esc);
                assert!(!model.quit(), "{name}");
            }
        }
    }

    #[test]
    fn a_key_with_an_unused_modifier_does_nothing_and_changes_nothing() {
        for (name, mut model) in every_state() {
            let before = snapshot(&model);
            for event in [
                with(KeyCode::Char('j'), KeyModifiers::CONTROL),
                with(KeyCode::Char('x'), KeyModifiers::ALT),
                with(KeyCode::Char('q'), KeyModifiers::ALT),
                with(KeyCode::Enter, KeyModifiers::CONTROL),
            ] {
                assert_eq!(handle(&mut model, event), [], "{name}");
                assert_eq!(snapshot(&model), before, "{name}");
            }
        }
    }

    #[test]
    fn only_a_press_counts() {
        let mut model = opened(&["a", "b"]);
        for kind in [KeyEventKind::Release, KeyEventKind::Repeat] {
            let mut event = key(KeyCode::Char('q'));
            event.kind = kind;
            assert_eq!(handle(&mut model, event), []);
            let mut event = key(KeyCode::Char('j'));
            event.kind = kind;
            assert_eq!(handle(&mut model, event), []);
        }
        assert!(!model.quit());
        assert_eq!(model.selected_stack(), 0);
    }

    #[test]
    fn the_workbench_keys_switch_open_help_clear_the_error_fold_the_pane_and_move_the_focus() {
        let mut model = opened(&["a"]);
        assert_eq!(model.workbench(), Workbench::Services);
        ch(&mut model, '1');
        assert_eq!(model.workbench(), Workbench::Devices);
        ch(&mut model, '2');
        assert_eq!(model.workbench(), Workbench::Services);
        let mut book = book_model();
        ch(&mut book, '1');
        ch(&mut book, '3');
        assert_eq!(book.workbench(), Workbench::Lab);

        assert_eq!(model.focus(), Focus::List);
        code(&mut model, KeyCode::Tab);
        assert_eq!(model.focus(), Focus::Tabs);
        code(&mut model, KeyCode::Tab);
        assert_eq!(model.focus(), Focus::List);

        assert!(!model.run_pane());
        ch(&mut model, 'o');
        assert!(model.run_pane());
        ch(&mut model, 'o');
        assert!(!model.run_pane());

        assert_eq!(ch(&mut model, 'r'), [Command::Refresh]);

        model.apply(Update::Stopped("ps".to_string()));
        ch(&mut model, 'x');
        assert_eq!(model.error(), None);

        assert_eq!(ch(&mut model, '?'), []);
        assert!(model.help());
    }

    #[test]
    fn with_the_help_open_only_question_mark_and_esc_close_it() {
        for closer in [KeyCode::Char('?'), KeyCode::Esc] {
            let mut model = opened(&["a", "b"]);
            ch(&mut model, '?');
            for ignored in ['j', '1', 'o', 'r', 'x', 'l', '/'] {
                assert_eq!(ch(&mut model, ignored), [], "{ignored}");
            }
            code(&mut model, KeyCode::Enter);
            code(&mut model, KeyCode::Tab);
            assert!(model.help());
            assert_eq!(model.selected_stack(), 0);
            assert_eq!(model.workbench(), Workbench::Services);
            assert!(!model.run_pane());
            assert_eq!(model.focus(), Focus::List);
            assert_eq!(code(&mut model, closer), []);
            assert!(!model.help());
        }
    }

    #[test]
    fn esc_closes_the_filter_then_the_pane_then_the_question_then_moves_to_the_list() {
        let mut model = opened(&["a"]);
        ch(&mut model, '/');
        ch(&mut model, 'z');
        assert!(model.visible_stacks().is_empty());
        code(&mut model, KeyCode::Esc);
        assert!(!model.typing());
        assert_eq!(model.visible_stacks(), [0]);

        code(&mut model, KeyCode::Tab);
        ch(&mut model, 'o');
        code(&mut model, KeyCode::Esc);
        assert!(!model.run_pane());
        assert_eq!(model.focus(), Focus::Tabs);
        code(&mut model, KeyCode::Esc);
        assert_eq!(model.focus(), Focus::List);

        let mut model = sonda_tab();
        ch(&mut model, 'l');
        assert_eq!(model.sonda().prompt(), &Prompt::Path(String::new()));
        assert_eq!(code(&mut model, KeyCode::Esc), []);
        assert_eq!(model.sonda().prompt(), &Prompt::None);
        assert_eq!(model.focus(), Focus::Tabs);
        code(&mut model, KeyCode::Esc);
        assert_eq!(model.focus(), Focus::List);
    }

    #[test]
    fn the_chooser_moves_over_the_stacks_and_enter_opens_the_row() {
        let mut model = chooser(&["a", "b"]);
        assert!(model.chooser());
        assert_eq!(ch(&mut model, 'k'), []);
        assert_eq!(model.selected_stack(), 0);
        ch(&mut model, 'j');
        code(&mut model, KeyCode::Down);
        code(&mut model, KeyCode::Down);
        assert_eq!(model.selected_stack(), 1);
        code(&mut model, KeyCode::Up);
        assert_eq!(model.selected_stack(), 0);
        ch(&mut model, 'j');
        assert_eq!(model.selected_stack(), 1);
        for ignored in ['1', '2', '3', '?', 'o', 'r', 'x', 'l'] {
            assert_eq!(ch(&mut model, ignored), [], "{ignored}");
        }
        assert_eq!(code(&mut model, KeyCode::Esc), []);
        assert_eq!(code(&mut model, KeyCode::Tab), []);
        assert!(model.chooser());
        assert!(!model.help());
        assert_eq!(model.selected_stack(), 1);

        assert_eq!(code(&mut model, KeyCode::Enter), [Command::SelectStack(1)]);
        assert!(!model.chooser());
        assert_eq!(model.selected_stack(), 1);
        assert_eq!(model.tab(), Tab::Overview);
        assert_eq!(model.stacks().len(), 2);
    }

    #[test]
    fn the_chooser_sonda_row_is_a_stack_of_the_list_and_opens_the_sonda_tab() {
        let sonda = Stack {
            env: vec![("SONDA_SERVER_URL".to_string(), "http://s:9".to_string())],
            ..sonda_stack()
        };
        let mut model = Model::new(start(vec![stack("a", &[]), sonda]));
        assert_eq!(model.sonda_address(1), "http://s:9");
        ch(&mut model, 'j');
        assert_eq!(model.selected_stack(), 1);
        ch(&mut model, 'j');
        assert_eq!(model.selected_stack(), 1);

        assert_eq!(code(&mut model, KeyCode::Enter), [Command::SelectStack(1)]);

        assert!(!model.chooser());
        assert_eq!(model.tab(), Tab::Sonda);
        assert_eq!(model.focus(), Focus::Tabs);
        assert_eq!(model.stacks().len(), 2);
        assert_eq!(model.stacks()[0].name, "a");
        assert_eq!(model.stacks()[1].kind, StackKind::Sonda);
        assert_eq!(model.sonda().server(), "http://s:9");
    }

    #[test]
    fn the_model_never_adds_a_stack_and_enter_in_an_empty_chooser_does_nothing() {
        let mut model = Model::new(start(Vec::new()));
        assert_eq!(code(&mut model, KeyCode::Enter), []);
        assert!(model.chooser());
        assert_eq!(model.stacks(), []);

        let model = Model::new(Start {
            chooser: false,
            ..start(Vec::new())
        });
        assert_eq!(model.stacks(), []);
    }

    #[test]
    fn the_lab_workbench_acts_on_its_own_keys_and_the_global_keys_only() {
        let mut model = book_model();
        let before = snapshot(&model);
        for ignored in ['l', '/', 'f', '[', ']', 'u', 'd', 'm'] {
            assert_eq!(ch(&mut model, ignored), [], "{ignored}");
        }
        assert_eq!(code(&mut model, KeyCode::Enter), []);
        assert_eq!(snapshot(&model), before);
        assert_eq!(model.selected_chapter(), Some(0));
        assert_eq!(model.question(), None);
        assert_eq!(model.notice(), None);
        assert_eq!(ch(&mut model, 'r'), [Command::Refresh]);
        ch(&mut model, 'o');
        assert!(model.run_pane());
        ch(&mut model, '2');
        assert_eq!(model.workbench(), Workbench::Services);
    }

    #[test]
    fn three_opens_the_lab_workbench_only_in_a_book_lab() {
        let mut model = opened(&["a"]);
        assert_eq!(ch(&mut model, '3'), []);
        assert_eq!(model.workbench(), Workbench::Observability);

        let mut model = devices_model(workshop_located("infrahub"));
        assert_eq!(ch(&mut model, '3'), [Command::OpenTab(Tab::Overview)]);
        assert_eq!(model.workbench(), Workbench::Observability);

        let mut model = book_model();
        ch(&mut model, '2');
        ch(&mut model, '3');
        assert_eq!(model.workbench(), Workbench::Lab);
    }

    fn workbenches_after(mut model: Model, keys: &str) -> Vec<Workbench> {
        keys.chars()
            .map(|key| {
                ch(&mut model, key);
                model.workbench()
            })
            .collect()
    }

    #[test]
    fn in_a_workshop_one_two_and_three_open_devices_services_and_observability_and_four_does_nothing(
    ) {
        let model = devices_model(workshop_located("infrahub"));
        assert_eq!(
            workbenches_after(model, "23143"),
            [
                Workbench::Services,
                Workbench::Observability,
                Workbench::Devices,
                Workbench::Devices,
                Workbench::Observability
            ]
        );
    }

    #[test]
    fn in_a_book_lab_one_to_four_open_devices_services_lab_and_observability() {
        assert_eq!(
            workbenches_after(book_model(), "123454"),
            [
                Workbench::Devices,
                Workbench::Services,
                Workbench::Lab,
                Workbench::Observability,
                Workbench::Observability,
                Workbench::Observability
            ]
        );
    }

    #[test]
    fn in_a_labcli_lab_the_keys_open_what_they_open_in_a_workshop() {
        let model = lab_model("project-two-node", "project-two-node/expected/lab_vars.yml");
        assert!(matches!(model.located().unwrap().kind, Kind::LabcliLab(_)));
        assert_eq!(
            workbenches_after(model, "23143"),
            [
                Workbench::Services,
                Workbench::Observability,
                Workbench::Devices,
                Workbench::Devices,
                Workbench::Observability
            ]
        );
    }

    fn observability_model(alerts: Vec<Alert>) -> Model {
        let mut model = devices_model(workshop_located("infrahub"));
        model.apply(Update::Alerts(Ok(alerts)));
        ch(&mut model, '3');
        model
    }

    fn three_alerts() -> Vec<Alert> {
        vec![
            alert("srl1", "BgpSessionNotUp", "2026-11-09T13:53:32Z"),
            alert("srl2", "BgpSessionNotUp", "2026-11-09T13:52:32Z"),
            alert("srl2", "InterfaceDown", "2026-11-09T13:51:32Z"),
        ]
    }

    #[test]
    fn the_alert_filter_keeps_the_alerts_whose_words_hold_the_typed_text_and_reads_q_as_a_letter() {
        let mut model = observability_model(three_alerts());

        ch(&mut model, '/');
        assert!(model.typing());
        assert_eq!(ch(&mut model, 'q'), []);
        code(&mut model, KeyCode::Backspace);
        typed(&mut model, "SRL2");
        assert_eq!(model.typed_filter(), Some("SRL2"));
        code(&mut model, KeyCode::Enter);

        assert!(!model.quit());
        assert!(!model.typing());
        assert_eq!(model.workbench(), Workbench::Observability);
        assert_eq!(model.visible_alerts(), [1, 2]);
    }

    #[test]
    fn the_selected_alert_stays_selected_across_a_failed_poll_and_a_newer_alert() {
        let mut model = observability_model(three_alerts());
        ch(&mut model, 'j');
        let chosen = model.selected_alert().unwrap().labels.clone();

        model.apply(Update::Alerts(Err("refused".to_string())));
        let mut list = three_alerts();
        list.push(alert("srl3", "InterfaceDown", "2026-11-09T13:54:32Z"));
        model.apply(Update::Alerts(Ok(list)));

        assert_eq!(model.selected_alert().unwrap().labels, chosen);
    }

    #[test]
    fn the_selected_alert_stays_on_the_shown_row_when_the_filter_hid_the_old_one_and_a_newer_alert_arrives(
    ) {
        let mut model = observability_model(three_alerts());
        ch(&mut model, 'k');
        assert_eq!(model.selected_alert().unwrap().labels["device"], "srl1");
        ch(&mut model, '/');
        typed(&mut model, "srl2");
        code(&mut model, KeyCode::Enter);
        let shown = model.selected_alert().unwrap().labels.clone();
        assert_eq!(shown["device"], "srl2");

        let mut list = three_alerts();
        list.push(alert("srl2", "LinkFlap", "2026-11-09T13:54:32Z"));
        model.apply(Update::Alerts(Ok(list)));

        assert_eq!(model.visible_alerts(), [0, 2, 3]);
        assert_eq!(model.selected_alert().unwrap().labels, shown);
    }

    fn selected_words(model: &Model) -> Option<[String; 2]> {
        model
            .selected_alert()
            .map(|alert| ["device", "alertname"].map(|name| alert.labels[name].clone()))
    }

    fn alert_words(device: &str, name: &str) -> Option<[String; 2]> {
        Some([device.to_string(), name.to_string()])
    }

    #[test]
    fn k_on_the_first_shown_alert_and_j_on_the_last_keep_the_selected_alert() {
        let mut model = observability_model(three_alerts());
        let labels = |model: &Model| model.selected_alert().map(|alert| alert.labels.clone());

        ch(&mut model, 'k');
        let first_row = model.selected_alert_row();
        let first = labels(&model);
        ch(&mut model, 'k');
        let after_k = labels(&model);
        ch(&mut model, 'j');
        ch(&mut model, 'j');
        let last_row = model.selected_alert_row();
        let last = labels(&model);
        ch(&mut model, 'j');
        let after_j = labels(&model);

        assert_eq!(first_row, Some(0));
        assert_eq!(after_k, first);
        assert_eq!(last_row, Some(2));
        assert_eq!(after_j, last);
    }

    #[test]
    fn when_the_selected_alert_leaves_the_list_the_first_shown_alert_is_selected() {
        let mut model = observability_model(three_alerts());
        ch(&mut model, 'j');
        ch(&mut model, 'j');
        assert_eq!(selected_words(&model), alert_words("srl2", "InterfaceDown"));

        let mut list = three_alerts();
        list.remove(2);
        model.apply(Update::Alerts(Ok(list)));

        assert_eq!(
            selected_words(&model),
            alert_words("srl1", "BgpSessionNotUp")
        );
    }

    #[test]
    fn the_alert_shown_as_selected_under_a_filter_stays_selected_after_the_filter_is_cleared() {
        let mut model = observability_model(three_alerts());
        ch(&mut model, 'k');
        assert_eq!(
            selected_words(&model),
            alert_words("srl1", "BgpSessionNotUp")
        );

        ch(&mut model, '/');
        typed(&mut model, "srl2");
        code(&mut model, KeyCode::Enter);
        let under_the_filter = selected_words(&model);
        ch(&mut model, '/');
        code(&mut model, KeyCode::Esc);

        assert_eq!(under_the_filter, alert_words("srl2", "BgpSessionNotUp"));
        assert_eq!(model.visible_alerts(), [0, 1, 2]);
        assert_eq!(selected_words(&model), under_the_filter);
    }

    #[test]
    fn when_the_stack_comes_back_with_the_same_alerts_the_same_alert_is_selected_again() {
        let mut model = observability_model(three_alerts());
        ch(&mut model, 'j');
        ch(&mut model, 'j');
        assert_eq!(selected_words(&model), alert_words("srl2", "InterfaceDown"));
        let names: Vec<&str> = SERVICES.iter().map(|row| row.compose).collect();

        model.apply(Update::Services(0, defined(&names), Ok(Vec::new())));
        let while_down = (model.reach(status::Service::Alerts), selected_words(&model));
        model.apply(every_service_up(0));
        model.apply(Update::Alerts(Ok(three_alerts())));

        assert_eq!(while_down, (Reach::StackDown, None));
        assert_eq!(selected_words(&model), alert_words("srl2", "InterfaceDown"));
    }

    #[test]
    fn an_alert_nobody_chose_stays_marked_when_a_newer_alert_arrives_at_the_top() {
        let mut model = devices_model(workshop_located("infrahub"));
        ch(&mut model, '3');
        assert_eq!(model.selected_alert_row(), Some(0));

        model.apply(Update::Alerts(Ok(vec![
            alert("srl2", "BgpSessionNotUp", "2026-11-09T13:53:32Z"),
            alert("srl1", "LinkFlap", "2026-11-09T13:54:32Z"),
        ])));

        assert_eq!(model.selected_alert_row(), Some(1));
        assert_eq!(
            selected_words(&model),
            alert_words("srl2", "BgpSessionNotUp")
        );
    }

    fn opened_after_a_newer_alert_arrived_on_the_devices_workbench() -> Model {
        let mut model = devices_model(workshop_located("infrahub"));
        model.apply(Update::Alerts(Ok(vec![
            alert("srl2", "BgpSessionNotUp", "2026-11-09T13:53:32Z"),
            alert("srl1", "LinkFlap", "2026-11-09T13:54:32Z"),
        ])));
        ch(&mut model, '3');
        model
    }

    #[test]
    fn the_first_opening_marks_the_newest_alert_of_those_that_arrived_before() {
        let model = opened_after_a_newer_alert_arrived_on_the_devices_workbench();

        assert_eq!(model.selected_alert_row(), Some(0));
        assert_eq!(selected_words(&model), alert_words("srl1", "LinkFlap"));
    }

    #[test]
    fn after_the_first_opening_the_marker_stays_on_its_alert_when_a_newer_alert_arrives() {
        let mut model = opened_after_a_newer_alert_arrived_on_the_devices_workbench();

        model.apply(Update::Alerts(Ok(vec![
            alert("srl2", "BgpSessionNotUp", "2026-11-09T13:53:32Z"),
            alert("srl1", "LinkFlap", "2026-11-09T13:54:32Z"),
            alert("srl3", "InterfaceDown", "2026-11-09T13:55:32Z"),
        ])));

        assert_eq!(model.selected_alert_row(), Some(1));
        assert_eq!(selected_words(&model), alert_words("srl1", "LinkFlap"));
    }

    #[test]
    fn the_alert_marked_before_enter_is_marked_on_the_way_back_after_a_newer_alert_arrived() {
        let mut model = observability_model(three_alerts());
        ch(&mut model, 'j');
        let chosen = selected_words(&model);
        code(&mut model, KeyCode::Enter);
        assert_eq!(model.workbench(), Workbench::Devices);

        let mut list = three_alerts();
        list.push(alert("srl3", "LinkFlap", "2026-11-09T13:54:32Z"));
        model.apply(Update::Alerts(Ok(list)));
        ch(&mut model, '3');

        assert_eq!(chosen, alert_words("srl2", "BgpSessionNotUp"));
        assert_eq!(model.selected_alert_row(), Some(2));
        assert_eq!(selected_words(&model), chosen);
    }

    #[test]
    fn an_alert_that_ended_while_another_workbench_was_open_gives_the_top_on_the_way_back() {
        let mut model = observability_model(three_alerts());
        ch(&mut model, 'k');
        ch(&mut model, 'j');
        assert_eq!(
            selected_words(&model),
            alert_words("srl2", "BgpSessionNotUp")
        );
        ch(&mut model, '1');

        let mut list = three_alerts();
        list.remove(1);
        model.apply(Update::Alerts(Ok(list.clone())));
        list.push(alert("srl3", "LinkFlap", "2026-11-09T13:54:32Z"));
        model.apply(Update::Alerts(Ok(list)));
        ch(&mut model, '3');

        assert_eq!(model.selected_alert_row(), Some(0));
        assert_eq!(selected_words(&model), alert_words("srl3", "LinkFlap"));
    }

    #[test]
    fn outside_a_lab_the_selected_alert_comes_back_after_another_stack_was_selected() {
        let mut model = opened(&["a", "b"]);
        model.apply(every_service_up(0));
        model.apply(Update::Services(
            1,
            defined(&["prometheus"]),
            Ok(vec![row("prometheus", RUNNING)]),
        ));
        model.apply(Update::Alerts(Ok(three_alerts())));
        ch(&mut model, '3');
        ch(&mut model, 'j');
        let chosen = selected_words(&model);
        assert_eq!(model.workbench(), Workbench::Observability);

        ch(&mut model, '2');
        ch(&mut model, 'j');
        model.ask_changes();
        let at_b = (model.reach(status::Service::Alerts), selected_words(&model));
        ch(&mut model, 'k');
        model.ask_changes();
        model.apply(Update::Alerts(Ok(three_alerts())));

        assert_eq!(at_b, (Reach::NotInLab, None));
        assert_eq!(chosen, alert_words("srl2", "BgpSessionNotUp"));
        assert_eq!(selected_words(&model), chosen);
    }

    #[test]
    fn tab_in_the_observability_workbench_reaches_the_run_pane_only_while_it_is_drawn() {
        let mut model = observability_model(three_alerts());

        code(&mut model, KeyCode::Tab);
        assert_eq!(model.focus(), Focus::List);
        ch(&mut model, 'o');
        model.set_run_view(measured(0, 0));
        code(&mut model, KeyCode::Tab);
        assert_eq!(model.focus(), Focus::Run);
        code(&mut model, KeyCode::Tab);
        assert_eq!(model.focus(), Focus::List);
    }

    #[test]
    fn enter_on_an_alert_of_a_device_of_the_lab_opens_that_device_on_its_timeline_and_on_another_does_nothing(
    ) {
        let mut model = observability_model(three_alerts());
        ch(&mut model, 'j');

        assert_eq!(
            code(&mut model, KeyCode::Enter),
            [
                Command::SelectDevice("srl2".to_string()),
                Command::OpenTab(Tab::Timeline)
            ]
        );
        assert_eq!(model.workbench(), Workbench::Devices);
        assert_eq!(model.selected_device().unwrap().name, "srl2");
        assert_eq!(model.cursor(), Cursor::Device(1));
        assert_eq!((model.focus(), model.tab()), (Focus::Tabs, Tab::Timeline));

        let mut without_device = alert("srl1", "BgpSessionNotUp", "2026-11-09T13:53:32Z");
        without_device.labels.remove("device");
        let elsewhere = alert(
            "nobs-t-elsewhere",
            "BgpSessionNotUp",
            "2026-11-09T13:53:32Z",
        );
        for lone in [without_device, elsewhere] {
            let mut model = observability_model(vec![lone]);
            let before = snapshot(&model);

            assert_eq!(code(&mut model, KeyCode::Enter), []);

            assert_eq!(snapshot(&model), before);
            assert_eq!(model.selected_device().unwrap().name, "srl1");
        }
    }

    #[test]
    fn the_list_keys_move_filter_enter_and_open_the_logs() {
        let mut model = opened(&["a", "b", "c"]);
        assert_eq!(ch(&mut model, 'k'), []);
        assert_eq!(ch(&mut model, 'j'), [Command::SelectStack(1)]);
        assert_eq!(code(&mut model, KeyCode::Down), [Command::SelectStack(2)]);
        assert_eq!(ch(&mut model, 'j'), []);
        assert_eq!(model.selected_stack(), 2);
        assert_eq!(code(&mut model, KeyCode::Up), [Command::SelectStack(1)]);
        assert_eq!(ch(&mut model, 'k'), [Command::SelectStack(0)]);

        assert_eq!(ch(&mut model, 'l'), []);
        assert_eq!(model.tab(), Tab::Overview);

        assert_eq!(code(&mut model, KeyCode::Enter), []);
        assert_eq!(model.focus(), Focus::Tabs);
        code(&mut model, KeyCode::Tab);

        with_services(&mut model, &["grafana"]);
        assert_eq!(
            ch(&mut model, 'l'),
            [Command::OpenLogs {
                stack: 0,
                service: "grafana".to_string()
            }]
        );
        assert_eq!(model.tab(), Tab::Logs);
        assert_eq!(model.focus(), Focus::Tabs);
    }

    #[test]
    fn typing_a_filter_takes_every_character_and_backspace_enter_keeps_and_esc_clears() {
        let mut model = opened(&["a", "b"]);
        ch(&mut model, '/');
        assert!(model.typing());
        assert_eq!(model.typed_filter(), Some(""));
        typed(&mut model, "x1j?");
        code(&mut model, KeyCode::Tab);
        code(&mut model, KeyCode::Down);
        code(&mut model, KeyCode::Backspace);
        assert_eq!(model.typed_filter(), Some("x1j"));
        assert_eq!(model.workbench(), Workbench::Services);
        assert_eq!(model.focus(), Focus::List);
        assert_eq!(model.selected_stack(), 0);
        assert!(!model.help());

        assert_eq!(code(&mut model, KeyCode::Enter), []);
        assert!(!model.typing());
        assert!(model.visible_stacks().is_empty());

        ch(&mut model, '/');
        assert!(model.typing());
        assert_eq!(model.typed_filter(), Some("x1j"));
        code(&mut model, KeyCode::Esc);
        assert!(!model.typing());
        assert_eq!(model.visible_stacks(), [0, 1]);
    }

    #[test]
    fn the_overview_keys_move_over_the_services_change_the_tab_and_open_the_logs() {
        let mut model = opened(&["a"]);
        with_services(&mut model, &["grafana", "loki", "sonda"]);
        code(&mut model, KeyCode::Enter);
        assert_eq!(ch(&mut model, 'k'), []);
        assert_eq!(ch(&mut model, 'j'), []);
        code(&mut model, KeyCode::Down);
        assert_eq!(ch(&mut model, 'j'), []);
        assert_eq!(model.selected_service(), 2);
        code(&mut model, KeyCode::Up);
        assert_eq!(model.selected_service(), 1);

        assert_eq!(
            code(&mut model, KeyCode::Enter),
            [Command::OpenLogs {
                stack: 0,
                service: "loki".to_string()
            }]
        );
        assert_eq!(model.tab(), Tab::Logs);
        let (service, lines, following, scroll) = model.logs().unwrap();
        assert_eq!(
            (service, lines, following, scroll),
            ("loki", vec![], true, 0)
        );

        assert_eq!(ch(&mut model, ']'), [Command::CloseLogs]);
        assert_eq!(model.tab(), Tab::Sonda);
        assert_eq!(model.logs(), None);
        assert_eq!(ch(&mut model, ']'), []);
        assert_eq!(model.tab(), Tab::Overview);
        assert_eq!(
            ch(&mut model, ']'),
            [Command::OpenLogs {
                stack: 0,
                service: "loki".to_string()
            }]
        );
        assert_eq!(ch(&mut model, '['), [Command::CloseLogs]);
        assert_eq!(model.tab(), Tab::Overview);
        assert_eq!(ch(&mut model, '['), []);
        assert_eq!(model.tab(), Tab::Sonda);
        assert_eq!(
            ch(&mut model, '['),
            [Command::OpenLogs {
                stack: 0,
                service: "loki".to_string()
            }]
        );
        assert_eq!(model.tab(), Tab::Logs);
    }

    #[test]
    fn the_sonda_tab_keys_keep_their_slice_22_meaning() {
        let mut model = sonda_tab();
        assert_eq!(model.tab(), Tab::Sonda);
        sonda_rows(&mut model);

        assert_eq!(
            ch(&mut model, 'j'),
            [Command::Sonda(data::Command::Select("b2".to_string()))]
        );
        assert_eq!(
            ch(&mut model, 'r'),
            [Command::Sonda(data::Command::Refresh)]
        );
        assert_eq!(ch(&mut model, 'd'), []);
        assert_eq!(model.sonda().prompt(), &Prompt::Delete("b2".to_string()));
        assert_eq!(
            ch(&mut model, 'y'),
            [Command::Sonda(data::Command::Delete("b2".to_string()))]
        );

        ch(&mut model, 'l');
        typed(&mut model, "a1x.ymlo");
        code(&mut model, KeyCode::Backspace);
        assert_eq!(model.sonda().prompt(), &Prompt::Path("a1x.yml".to_string()));
        assert_eq!(
            code(&mut model, KeyCode::Enter),
            [Command::Sonda(data::Command::Load("a1x.yml".to_string()))]
        );
        assert!(!model.quit());
        assert_eq!(model.focus(), Focus::Tabs);
    }

    #[test]
    fn a_stack_gives_its_state_text() {
        assert_eq!(StackState::Unknown.text(), "?");
        assert_eq!(StackState::Failed.text(), "?");
        assert_eq!(
            StackState::Up {
                running: 7,
                total: 7
            }
            .text(),
            "up 7/7"
        );
        assert_eq!(StackState::Down { total: 0 }.text(), "down 0/0");
        assert_eq!(StackState::Down { total: 3 }.text(), "down 0/3");
    }

    #[test]
    fn services_rows_give_the_stack_state_text() {
        let mut model = opened(&["autocon5"]);
        let seven: Vec<lab::Service> = (0..7).map(|n| row(&format!("s{n}"), "running")).collect();
        model.apply(listing(0, Ok(seven)));
        assert_eq!(model.stack_state(0).text(), "up 7/7");

        let defined_seven = defined(&["s0", "s1", "s2", "s3", "s4", "s5", "s6"]);
        model.apply(Update::Services(0, defined_seven, Ok(vec![])));
        assert_eq!(model.stack_state(0).text(), "down 0/7");

        let three = [("a", "exited"), ("b", "exited"), ("c", "created")]
            .iter()
            .map(|(name, state)| row(name, state))
            .collect();
        model.apply(listing(0, Ok(three)));
        assert_eq!(model.stack_state(0).text(), "down 0/3");

        model.apply(listing(
            0,
            Ok(vec![row("a", "running"), row("b", "exited")]),
        ));
        assert_eq!(model.stack_state(0).text(), "up 1/2");
    }

    #[test]
    fn the_running_number_counts_defined_services_never_containers() {
        let mut model = opened(&["autocon5"]);
        let twice_and_old = vec![
            row("a", "running"),
            row("a", "running"),
            row("old", "running"),
        ];
        model.apply(Update::Services(0, defined(&["a"]), Ok(twice_and_old)));
        assert_eq!(model.stack_state(0).text(), "up 1/1");

        let exited_and_running = vec![row("a", "exited"), row("a", "running")];
        model.apply(Update::Services(
            0,
            defined(&["a", "b"]),
            Ok(exited_and_running),
        ));
        assert_eq!(model.stack_state(0).text(), "up 1/2");
    }

    #[test]
    fn the_table_keeps_every_listed_container_and_adds_each_defined_service_without_one() {
        let mut model = opened(&["autocon5"]);
        model.apply(Update::Services(
            0,
            defined(&["alertmanager", "grafana", "prometheus"]),
            Ok(vec![row("grafana", "running"), row("old", "running")]),
        ));

        let rows: Vec<(&str, &str)> = model
            .services()
            .iter()
            .map(|service| (service.name.as_str(), service.state.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                ("grafana", "running"),
                ("old", "running"),
                ("alertmanager", "—"),
                ("prometheus", "—")
            ]
        );
    }

    #[test]
    fn a_defined_service_without_a_container_shows_no_health() {
        let mut model = opened(&["autocon5"]);
        model.apply(Update::Health(0, vec![health("grafana", "down")]));
        model.apply(Update::Services(0, defined(&["grafana"]), Ok(Vec::new())));

        let service = &model.services()[0];
        assert_eq!(
            (service.name.as_str(), service.state.as_str()),
            ("grafana", "—")
        );
        assert_eq!(service.health, None);
    }

    #[test]
    fn a_stopped_container_shows_no_health_and_a_running_one_the_probe_s_answer() {
        let mut model = opened(&["autocon5"]);
        let probed = vec![health("grafana", "down"), health("loki", "ok")];
        let listed = vec![row("grafana", "exited"), row("loki", "running")];

        model.apply(Update::Health(0, probed.clone()));
        model.apply(listing(0, Ok(listed)));
        let before: Vec<Option<&str>> = model
            .services()
            .iter()
            .map(|service| service.health.as_deref())
            .collect();
        assert_eq!(before, [None, Some("ok")]);

        model.apply(Update::Health(0, probed));
        let after: Vec<Option<&str>> = model
            .services()
            .iter()
            .map(|service| service.health.as_deref())
            .collect();
        assert_eq!(after, [None, Some("ok")]);
    }

    #[test]
    fn a_failed_ps_gives_a_question_mark_the_error_and_keeps_the_last_services() {
        let mut model = opened(&["autocon5"]);
        with_services(&mut model, &["grafana"]);
        model.apply(listing(0, Err("exit 1".to_string())));

        assert_eq!(model.stack_state(0).text(), "?");
        let (source, text, _) = model.error().unwrap();
        assert_eq!((source, text), ("ps autocon5", "exit 1"));
        assert_eq!(model.services().len(), 1);
    }

    #[test]
    fn an_update_for_a_stack_that_is_not_selected_sets_only_its_state() {
        let mut model = opened(&["a", "b"]);
        model.apply(listing(1, Ok(vec![row("x", "running")])));
        assert_eq!(model.stack_state(1).text(), "up 1/1");
        assert_eq!(model.services(), []);
        assert_eq!(model.services_at(), None);
    }

    #[test]
    fn the_book_lab_gives_one_stack_per_chapter_then_the_workshops() {
        let dir = fixture("book-lab");
        let located = locate(&dir, Some(&dir)).unwrap();
        let one = fixture("workshop-one");

        let stacks = stacks(
            Some(&located),
            vec![("one".to_string(), "One".to_string(), one.clone())],
        )
        .unwrap();

        let names: Vec<&str> = stacks.iter().map(|stack| stack.name.as_str()).collect();
        assert_eq!(names, ["ch1", "ch2", "one"]);
        assert_eq!(stacks[2].kind, StackKind::Workshop);
        assert_eq!(stacks[2].path, one);
        assert_eq!(stacks[2].project.as_deref(), Some("nobs-t-one"));
        for (stack, chapter) in stacks.iter().zip(["ch1", "ch2"]) {
            assert_eq!(stack.kind, StackKind::BookLab);
            assert_eq!(stack.path, dir);
            assert_eq!(
                stack.compose,
                dir.join(format!("chapters/{chapter}/docker-compose.yml"))
            );
            assert_eq!(stack.project.as_deref(), Some("netobs"));
        }
    }

    #[test]
    fn workshops_become_workshop_stacks_in_the_given_order() {
        let one = fixture("workshop-one");
        let bare = fixture("book-lab");

        let stacks = stacks(
            None,
            vec![
                ("b".to_string(), "B".to_string(), bare.clone()),
                ("a".to_string(), "A".to_string(), one.clone()),
            ],
        )
        .unwrap();

        assert_eq!(stacks.len(), 2);
        assert_eq!(stacks[0].name, "b");
        assert_eq!(stacks[1].name, "a");
        assert!(stacks.iter().all(|stack| stack.kind == StackKind::Workshop));
        assert_eq!(stacks[1].path, one);
        assert_eq!(stacks[1].compose, one.join("docker-compose.yml"));
        assert_eq!(stacks[1].project.as_deref(), Some("nobs-t-one"));
        assert_eq!(stacks[0].project, None);
        assert_eq!(stacks[1].env, read_env(&one).unwrap());
    }

    #[test]
    fn a_workshop_located_lab_still_lists_the_workshops() {
        let one = fixture("workshop-one");
        let located = locate(&one, Some(&one)).unwrap();

        let stacks = stacks(
            Some(&located),
            vec![("a".to_string(), "A".to_string(), one.clone())],
        )
        .unwrap();

        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].kind, StackKind::Workshop);
    }

    #[test]
    fn a_labcli_lab_gives_one_stack_named_after_its_folder() {
        let dir = fixture("labcli-yaml");
        let located = locate(&dir, Some(&dir)).unwrap();

        let stacks = stacks(
            Some(&located),
            vec![("a".to_string(), "A".to_string(), dir.clone())],
        )
        .unwrap();

        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].name, "labcli-yaml");
        assert_eq!(stacks[0].kind, StackKind::Lab);
        assert_eq!(stacks[0].path, dir);
        assert_eq!(stacks[0].compose, located.compose(None));
        assert_eq!(stacks[0].project.as_deref(), Some("nobs-t-yaml"));
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str, env: &str) -> Scratch {
            let dir =
                std::env::temp_dir().join(format!("nobs-shell-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(".env"), env).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn an_unreadable_env_is_an_error_for_the_located_lab_and_empty_for_a_workshop() {
        let bad = Scratch::new("bad-env", "=x\n");
        let located = Located {
            dir: bad.0.clone(),
            kind: Kind::LabcliLab(None),
        };
        let error = stacks(Some(&located), vec![]).unwrap_err();
        assert!(error.contains("empty key"), "{error}");

        let workshops = vec![("w".to_string(), "W".to_string(), bad.0.clone())];
        let stacks = stacks(None, workshops).unwrap();
        assert_eq!(stacks[0].env, []);

        let error = sonda_only(&bad.0).unwrap_err();
        assert!(error.ends_with("line 1: empty key"), "{error}");
    }

    #[test]
    fn an_unreadable_env_of_the_located_workshop_is_an_error() {
        let copy = Scratch::new("bad-workshop-env", "=x\n");
        let one = fixture("workshop-one");
        for file in ["nobs.toml", "docker-compose.yml"] {
            fs::copy(one.join(file), copy.0.join(file)).unwrap();
        }
        let located = locate(&copy.0, Some(&copy.0)).unwrap();
        assert!(matches!(located.kind, Kind::Workshop(_)));

        let error = stacks(
            Some(&located),
            vec![("one".to_string(), "One".to_string(), copy.0.clone())],
        )
        .unwrap_err();

        assert!(error.ends_with("line 1: empty key"), "{error}");
    }

    #[test]
    fn a_book_lab_without_chapters_is_an_error() {
        let located = Located {
            dir: PathBuf::from("/nonexistent-book-lab"),
            kind: Kind::BookLab(None),
        };
        let error = stacks(Some(&located), vec![]).unwrap_err();
        assert!(error.contains("/nonexistent-book-lab/chapters"), "{error}");
    }

    #[test]
    fn the_sonda_only_stack_reads_the_env_of_its_folder() {
        let folder = Scratch::new("sonda-env", "SONDA_SERVER_URL=http://s:9\n");

        let stack = sonda_only(&folder.0).unwrap();

        assert_eq!(stack.name, "sonda only");
        assert_eq!(stack.kind, StackKind::Sonda);
        assert_eq!(stack.path, folder.0);
        assert_eq!(stack.compose, folder.0);
        assert_eq!(stack.project, None);
        assert_eq!(
            stack.env,
            [("SONDA_SERVER_URL".to_string(), "http://s:9".to_string())]
        );
    }

    const LINES: &str = concat!(
        r#"{"Service":"grafana","State":"running","Health":"healthy","Publishers":[{"PublishedPort":3000}],"RunningFor":"2 hours ago"}"#,
        "\n",
        r#"{"Service":"loki","State":"running","Health":"unhealthy","Publishers":[{"PublishedPort":3100}],"RunningFor":"2 hours ago"}"#,
        "\n",
        r#"{"Service":"sleeper","State":"exited","Publishers":[],"RunningFor":"5 minutes ago"}"#,
        "\n"
    );

    const LIST: &str = r#"[
        {"Service":"grafana","State":"running","Health":"healthy","Publishers":[{"PublishedPort":3000}],"RunningFor":"2 hours ago"},
        {"Service":"loki","State":"running","Health":"unhealthy","Publishers":[{"PublishedPort":3100}],"RunningFor":"2 hours ago"},
        {"Service":"sleeper","State":"exited","Publishers":[],"RunningFor":"5 minutes ago"}
    ]"#;

    fn health(service: &str, state: &str) -> Health {
        Health {
            service: service.to_string(),
            state: state.to_string(),
        }
    }

    #[test]
    fn the_services_table_has_the_five_columns_from_both_ps_shapes_and_probe_health() {
        for text in [LINES, LIST] {
            let mut model = opened(&["autocon5"]);
            let at = Instant::now();
            model.tick(at, UNIX_EPOCH, (80, 24));
            model.apply(Update::Health(
                0,
                vec![health("grafana", "ok"), health("loki", "http 503")],
            ));

            model.apply(listing(0, lab::services(text)));

            let services = model.services();
            assert_eq!(services.len(), 3);
            assert_eq!(
                services[0],
                Service {
                    name: "grafana".to_string(),
                    state: "running".to_string(),
                    health: Some("ok".to_string()),
                    port: Some("3000".to_string()),
                    age: Some("2 hours".to_string()),
                }
            );
            assert_eq!(services[1].health.as_deref(), Some("http 503"));
            assert_eq!(services[1].port.as_deref(), Some("3100"));
            assert_eq!(services[2].name, "sleeper");
            assert_eq!(services[2].state, "exited");
            assert_eq!(services[2].health, None);
            assert_eq!(services[2].port, None);
            assert_eq!(services[2].age.as_deref(), Some("5 minutes"));
            assert_eq!(model.services_at(), Some(at));
        }
    }

    #[test]
    fn health_that_arrives_after_the_services_is_merged_by_name() {
        let mut model = opened(&["autocon5"]);
        model.apply(listing(0, lab::services(LINES)));
        assert_eq!(model.services()[0].health, None);

        model.apply(Update::Health(0, vec![health("grafana", "down")]));
        model.apply(Update::Health(1, vec![health("loki", "ok")]));

        assert_eq!(model.services()[0].health.as_deref(), Some("down"));
        assert_eq!(model.services()[1].health, None);
    }

    fn url_of(model: &Model) -> String {
        model.sonda().server().to_string()
    }

    #[test]
    fn moving_to_another_stack_returns_select_stack_and_takes_that_stacks_sonda_address() {
        let stacks = vec![
            stack("x", &[("SONDA_SERVER_URL", "http://x:1")]),
            stack("y", &[]),
            stack("z", &[("SONDA_SERVER_URL", "   ")]),
        ];
        let mut model = Model::new(start(stacks));
        code(&mut model, KeyCode::Enter);
        assert_eq!(url_of(&model), "http://x:1");

        assert_eq!(ch(&mut model, 'j'), [Command::SelectStack(1)]);
        assert_eq!(url_of(&model), "http://localhost:8085");
        assert_eq!(ch(&mut model, 'j'), [Command::SelectStack(2)]);
        assert_eq!(url_of(&model), "http://localhost:8085");
        assert_eq!(ch(&mut model, 'k'), [Command::SelectStack(1)]);
        assert_eq!(ch(&mut model, 'k'), [Command::SelectStack(0)]);
        assert_eq!(url_of(&model), "http://x:1");
    }

    #[test]
    fn the_sonda_only_stack_uses_the_default_sonda_address() {
        let model = Model::new(Start {
            chooser: false,
            ..start(vec![sonda_stack()])
        });

        assert_eq!(url_of(&model), "http://localhost:8080");
        assert_eq!(model.tab(), Tab::Sonda);
        assert_eq!(model.focus(), Focus::Tabs);
    }

    #[test]
    fn the_server_flag_wins_over_the_env_and_the_defaults() {
        let stacks = vec![
            stack("x", &[("SONDA_SERVER_URL", "http://x:1")]),
            stack("y", &[]),
            sonda_stack(),
        ];
        let mut model = Model::new(Start {
            sonda: target(Some("http://flag:9")),
            ..start(stacks)
        });
        code(&mut model, KeyCode::Enter);
        assert_eq!(url_of(&model), "http://flag:9");
        ch(&mut model, 'j');
        assert_eq!(url_of(&model), "http://flag:9");
        ch(&mut model, 'j');
        assert_eq!(url_of(&model), "http://flag:9");
    }

    #[test]
    fn moving_to_another_stack_clears_the_services_health_and_logs_and_leaves_the_logs_tab() {
        let mut model = opened(&["a", "b"]);
        with_services(&mut model, &["grafana"]);
        model.apply(Update::Health(0, vec![health("grafana", "ok")]));
        ch(&mut model, 'l');
        assert_eq!(model.tab(), Tab::Logs);
        code(&mut model, KeyCode::Tab);

        assert_eq!(ch(&mut model, 'j'), [Command::SelectStack(1)]);

        assert_eq!(model.services(), []);
        assert_eq!(model.services_at(), None);
        assert_eq!(model.logs(), None);
        assert_eq!(model.tab(), Tab::Overview);
        model.apply(listing(1, Ok(vec![row("grafana", "running")])));
        assert_eq!(model.services()[0].health, None);
    }

    #[test]
    fn a_typed_filter_leaves_only_the_stacks_whose_name_contains_it() {
        let mut model = opened(&["autocon5", "packt", "paper"]);
        assert_eq!(model.visible_stacks(), [0, 1, 2]);

        ch(&mut model, '/');
        typed(&mut model, "pa");
        code(&mut model, KeyCode::Enter);

        assert_eq!(model.visible_stacks(), [1, 2]);
        assert_eq!(model.selected_stack(), 1);
        assert_eq!(ch(&mut model, 'j'), [Command::SelectStack(2)]);
        assert_eq!(ch(&mut model, 'j'), []);
        assert_eq!(ch(&mut model, 'k'), [Command::SelectStack(1)]);
        assert_eq!(ch(&mut model, 'k'), []);
        assert_eq!(model.selected_stack(), 1);
    }

    fn timed(lines: &[&str]) -> Vec<String> {
        lines
            .iter()
            .map(|line| format!("2025-01-01T00:00:00Z {line}"))
            .collect()
    }

    fn logs_open(service: &str) -> Model {
        let mut model = opened(&["a"]);
        with_services(&mut model, &[service]);
        ch(&mut model, 'l');
        model
    }

    fn shown(model: &Model) -> Vec<String> {
        let (_, lines, _, _) = model.logs().unwrap();
        lines.into_iter().map(str::to_string).collect()
    }

    #[test]
    fn l_on_a_service_opens_the_logs_and_timed_lines_are_appended_without_their_time() {
        let mut model = opened(&["a"]);
        with_services(&mut model, &["grafana"]);
        assert_eq!(
            ch(&mut model, 'l'),
            [Command::OpenLogs {
                stack: 0,
                service: "grafana".to_string()
            }]
        );
        assert_eq!(shown(&model), Vec::<String>::new());

        model.apply(Update::Logs(0, "grafana".to_string(), Ok(timed(&["one"]))));
        model.apply(Update::Logs(
            0,
            "grafana".to_string(),
            Ok(timed(&["two words"])),
        ));
        model.apply(Update::Logs(0, "loki".to_string(), Ok(timed(&["other"]))));

        assert_eq!(shown(&model), ["one", "two words"]);
    }

    #[test]
    fn the_logs_keep_the_last_2000_lines() {
        let mut model = logs_open("grafana");
        let lines: Vec<String> = (0..2100)
            .map(|n| format!("2025-01-01T00:00:00Z n{n}"))
            .collect();

        model.apply(Update::Logs(0, "grafana".to_string(), Ok(lines)));

        let kept = shown(&model);
        assert_eq!(kept.len(), 2000);
        assert_eq!(kept[0], "n100");
        assert_eq!(kept[1999], "n2099");
    }

    #[test]
    fn f_toggles_following_and_a_paused_log_scrolls() {
        let mut model = logs_open("grafana");
        let lines: Vec<String> = (0..5).map(|n| format!("t n{n}")).collect();
        model.apply(Update::Logs(0, "grafana".to_string(), Ok(lines)));
        assert_eq!(ch(&mut model, 'k'), []);
        assert_eq!(model.logs().unwrap().3, 0);

        assert_eq!(ch(&mut model, 'f'), [Command::Follow(false)]);
        assert!(!model.logs().unwrap().2);
        ch(&mut model, 'k');
        code(&mut model, KeyCode::Up);
        assert_eq!(model.logs().unwrap().3, 2);
        ch(&mut model, 'j');
        assert_eq!(model.logs().unwrap().3, 1);
        for _ in 0..9 {
            ch(&mut model, 'k');
        }
        assert_eq!(model.logs().unwrap().3, 4);

        assert_eq!(ch(&mut model, 'f'), [Command::Follow(true)]);
        assert!(model.logs().unwrap().2);
        assert_eq!(model.logs().unwrap().3, 0);
    }

    #[test]
    fn a_logs_filter_hides_the_lines_without_the_text() {
        let mut model = logs_open("grafana");
        model.apply(Update::Logs(
            0,
            "grafana".to_string(),
            Ok(timed(&["start ok", "error disk", "stop ok"])),
        ));

        ch(&mut model, '/');
        assert!(model.typing());
        typed(&mut model, "ok");
        code(&mut model, KeyCode::Enter);

        assert_eq!(shown(&model), ["start ok", "stop ok"]);
        ch(&mut model, '/');
        code(&mut model, KeyCode::Esc);
        assert_eq!(shown(&model), ["start ok", "error disk", "stop ok"]);
        assert_eq!(model.tab(), Tab::Logs);
    }

    #[test]
    fn esc_in_the_logs_returns_to_the_list_and_closes_the_logs() {
        let mut model = logs_open("grafana");

        assert_eq!(code(&mut model, KeyCode::Esc), [Command::CloseLogs]);

        assert_eq!(model.logs(), None);
        assert_eq!(model.tab(), Tab::Overview);
        assert_eq!(model.focus(), Focus::List);
        assert!(!model.quit());
    }

    #[test]
    fn a_failed_logs_poll_sets_the_error_for_that_service() {
        let mut model = logs_open("grafana");

        model.apply(Update::Logs(
            0,
            "grafana".to_string(),
            Err("gone".to_string()),
        ));

        let (source, text, _) = model.error().unwrap();
        assert_eq!((source, text), ("docker logs", "grafana: gone"));
    }

    #[test]
    fn a_logs_answer_after_a_failure_removes_that_service_s_error() {
        let mut model = logs_open("grafana");
        model.apply(Update::Logs(
            0,
            "grafana".to_string(),
            Err("gone".to_string()),
        ));

        model.apply(Update::Logs(0, "grafana".to_string(), Ok(timed(&["back"]))));

        assert_eq!(model.error(), None);
    }

    #[test]
    fn a_logs_answer_for_a_stack_not_selected_adds_no_line_and_removes_its_error() {
        let mut model = opened(&["a", "b"]);
        with_services(&mut model, &["grafana"]);
        ch(&mut model, 'l');
        model.apply(Update::Logs(
            0,
            "grafana".to_string(),
            Err("gone".to_string()),
        ));
        assert_eq!(model.selected_stack(), 0);
        assert_eq!(model.error().unwrap().0, "docker logs");

        model.apply(Update::Logs(
            1,
            "grafana".to_string(),
            Ok(timed(&["other"])),
        ));

        assert_eq!(shown(&model), Vec::<String>::new());
        assert_eq!(model.error(), None);
    }

    #[test]
    fn too_small_is_true_below_60_columns_or_15_rows_and_blocks_every_key_but_quit() {
        for (columns, rows) in [(59, 15), (60, 14)] {
            let mut model = opened(&["a", "b"]);
            small(&mut model, columns, rows);
            assert!(model.too_small(), "{columns}x{rows}");
            assert_eq!(ch(&mut model, 'j'), []);
            assert_eq!(model.selected_stack(), 0);
            assert_eq!(ch(&mut model, 'q'), [Command::Quit]);
            assert!(model.quit());
        }
        let mut model = opened(&["a", "b"]);
        small(&mut model, 60, 15);
        assert!(!model.too_small());
        assert_eq!(ch(&mut model, 'j'), [Command::SelectStack(1)]);
    }

    #[test]
    fn stopped_gives_an_error_x_clears_it_and_a_later_ps_answer_keeps_other_sources() {
        let mut model = opened(&["a"]);
        let at = Instant::now();
        model.tick(at, UNIX_EPOCH, (80, 24));

        model.apply(Update::Stopped("sonda".to_string()));

        assert_eq!(model.error(), Some(("sonda", "polling stopped", at)));
        with_services(&mut model, &["grafana"]);
        assert_eq!(model.error(), Some(("sonda", "polling stopped", at)));
        ch(&mut model, 'x');
        assert_eq!(model.error(), None);

        model.apply(listing(0, Err("exit 1".to_string())));
        assert_eq!(model.error(), Some(("ps a", "exit 1", at)));
        with_services(&mut model, &["grafana"]);
        assert_eq!(model.error(), None);
    }

    #[test]
    fn a_source_that_could_not_be_opened_is_named_on_the_error_row() {
        let reason = "INFRAHUB_API_TOKEN is required (set it in .env or pass --token).";
        let mut model = Model::new(Start {
            lab: Some(workshop_located("infrahub")),
            source: None,
            source_error: Some(reason.to_string()),
            home: Workbench::Devices,
            chooser: false,
            ..start(vec![stack("a", &[])])
        });

        model.tick(Instant::now(), UNIX_EPOCH, (80, 24));

        let (source, text, _) = model.error().unwrap();
        assert_eq!((source, text), ("sot", reason));
        ch(&mut model, 'x');
        assert_eq!(model.error(), None);
    }

    #[test]
    fn the_clock_the_time_and_the_size_come_from_tick() {
        let mut model = opened(&["a"]);
        let at = Instant::now();

        model.tick(
            at,
            UNIX_EPOCH + Duration::from_secs(1_794_231_727),
            (100, 30),
        );

        assert_eq!(model.clock(), "13:42:07");
        assert_eq!(model.now(), at);
        assert!(!model.too_small());
    }

    #[test]
    fn a_sonda_note_and_a_sonda_update_reach_the_slice_22_model() {
        let mut model = sonda_tab();
        sonda_rows(&mut model);
        model.apply(Update::Note(data::Note {
            at: Instant::now(),
            text: "loaded x1".to_string(),
        }));

        assert_eq!(model.sonda().rows().len(), 2);
        assert_eq!(model.sonda().status(Instant::now()), Some("loaded x1"));
    }

    #[test]
    fn the_header_names_the_lab_its_kind_and_the_source_of_truth() {
        let mut ws = stack("autocon5", &[("INFRAHUB_ADDRESS", "http://i:8000")]);
        ws.path = fixture("workshop-one");
        let dir = fixture("workshop-one");
        let located = locate(&dir, Some(&dir)).unwrap();
        let model = Model::new(Start {
            lab: Some(located),
            chooser: false,
            ..start(vec![ws])
        });
        assert_eq!(model.header(), "nobs  nobs-t-one workshop  sot infrahub");

        let mut own = stack("one", &[("INFRAHUB_ADDRESS", "http://i:8000")]);
        own.path = fixture("workshop-one");
        let mut model = Model::new(Start {
            lab: Some(locate(&dir, Some(&dir)).unwrap()),
            chooser: false,
            ..start(vec![own, stack("other", &[])])
        });
        ch(&mut model, 'j');
        assert_eq!(model.selected_stack(), 1);
        assert_eq!(model.header(), "nobs  nobs-t-one workshop  sot infrahub");

        let mut plain = stack("autocon5", &[("INFRAHUB_ADDRESS", "  ")]);
        plain.path = fixture("book-lab");
        let book = fixture("book-lab");
        let model = Model::new(Start {
            lab: Some(locate(&book, Some(&book)).unwrap()),
            source: Some(lab_vars_source("book-lab/containerlab/lab_vars.yml")),
            chooser: false,
            ..start(vec![plain])
        });
        assert_eq!(model.header(), "nobs  book-lab book lab  sot lab_vars");

        assert_eq!(chooser(&["a"]).header(), "nobs  no lab here");

        let model = Model::new(Start {
            chooser: false,
            ..start(vec![sonda_stack()])
        });
        assert_eq!(model.header(), "nobs  sonda only  http://localhost:8080");
    }

    #[test]
    fn the_located_labs_own_stack_is_selected_at_the_start() {
        let one = fixture("workshop-one");
        let mut second = stack("two", &[]);
        second.path = one.clone();
        let model = Model::new(Start {
            lab: Some(locate(&one, Some(&one)).unwrap()),
            chooser: false,
            ..start(vec![stack("one", &[]), second])
        });

        assert_eq!(model.selected_stack(), 1);
        assert_eq!(model.tab(), Tab::Overview);
        assert_eq!(model.focus(), Focus::List);
    }

    fn lab_vars_source(relative: &str) -> Arc<dyn SourceOfTruth> {
        let text = fs::read_to_string(fixture(relative)).unwrap();
        Arc::new(Fixed::from_lab_vars(&lab_vars::read(&text).unwrap()))
    }

    pub(crate) fn workshop_source() -> Arc<dyn SourceOfTruth> {
        lab_vars_source("workshop-one/lab_vars.yml")
    }

    pub(crate) fn workshop_located(sot: &str) -> Located {
        let dir = fixture("workshop-one");
        let mut located = locate(&dir, Some(&dir)).unwrap();
        if let Kind::Workshop(workshop) = &mut located.kind {
            workshop.sot = sot.to_string();
        }
        located
    }

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    pub(crate) fn alert(device: &str, name: &str, starts_at: &str) -> Alert {
        Alert {
            labels: map(&[("alertname", name), ("device", device)]),
            state: "active".to_string(),
            silenced: false,
            inhibited: false,
            starts_at: starts_at.to_string(),
            silenced_by: Vec::new(),
            annotations: BTreeMap::new(),
        }
    }

    fn peer_samples(value: f64) -> Vec<Sample> {
        ["10.1.2.2", "10.1.7.2", "10.1.99.2"]
            .into_iter()
            .map(|peer| Sample {
                labels: map(&[
                    ("__name__", "bgp_oper_state"),
                    ("device", "srl1"),
                    ("peer_address", peer),
                    ("afi_safi_name", "ipv4-unicast"),
                    ("name", "default"),
                ]),
                value,
            })
            .collect()
    }

    fn log_line(nanoseconds: i64, text: &str) -> Line {
        Line {
            nanoseconds,
            text: text.to_string(),
        }
    }

    fn srl1() -> Command {
        Command::SelectDevice("srl1".to_string())
    }

    pub(crate) fn devices_model(lab: Located) -> Model {
        let source = workshop_source();
        let mut model = Model::new(Start {
            lab: Some(lab),
            source: Some(source.clone()),
            home: Workbench::Devices,
            chooser: false,
            ..start(vec![stack("a", &[])])
        });
        model.apply(every_service_up(0));
        model.apply(Update::Devices(Ok(source.devices().unwrap())));
        model.apply(Update::Alerts(Ok(vec![alert(
            "srl2",
            "BgpSessionNotUp",
            "2026-11-09T13:53:32Z",
        )])));
        model
    }

    pub(crate) fn lab_devices_model() -> Model {
        lab_model("book-lab", "book-lab/containerlab/lab_vars.yml")
    }

    fn lab_model(folder: &str, lab_vars: &str) -> Model {
        lab_model_finding(folder, lab_vars, finding(Some(USER), Some(SECRET)))
    }

    fn lab_model_finding(folder: &str, lab_vars: &str, login: Lookup) -> Model {
        let dir = fixture(folder);
        let source = lab_vars_source(lab_vars);
        let mut model = Model::new(Start {
            lab: Some(locate(&dir, Some(&dir)).unwrap()),
            source: Some(source.clone()),
            home: Workbench::Devices,
            chooser: false,
            login,
            ..start(vec![stack("a", &[])])
        });
        model.apply(Update::Devices(Ok(source.devices().unwrap())));
        model
    }

    #[test]
    fn the_device_keys_open_the_tabs_move_the_window_pause_the_logs_filter_and_pick_an_alert() {
        let mut model = devices_model(workshop_located("infrahub"));
        assert_eq!(model.focus(), Focus::List);

        let commands = code(&mut model, KeyCode::Enter);
        assert_eq!((model.focus(), model.tab()), (Focus::Tabs, Tab::Overview));
        assert_eq!(commands, [srl1(), Command::OpenTab(Tab::Overview)]);

        for expected in [Tab::Interfaces, Tab::Timeline, Tab::Logs, Tab::Overview] {
            let commands = ch(&mut model, ']');
            assert_eq!(model.tab(), expected);
            assert_eq!(commands, [srl1(), Command::OpenTab(expected)]);
        }
        assert_eq!(ch(&mut model, '['), [srl1(), Command::OpenTab(Tab::Logs)]);
        assert_eq!(
            ch(&mut model, '['),
            [srl1(), Command::OpenTab(Tab::Timeline)]
        );
        assert_eq!(model.tab(), Tab::Timeline);

        assert_eq!(ch(&mut model, '-'), [Command::Window(15)]);
        assert_eq!(model.window(), 15);
        assert_eq!(devices::window_text(model.window()), "15m");
        assert_eq!(ch(&mut model, '+'), [Command::Window(30)]);
        assert_eq!(devices::window_text(model.window()), "30m");
        assert_eq!(ch(&mut model, '+'), [Command::Window(60)]);
        assert_eq!(model.window(), 60);
        assert_eq!(devices::window_text(model.window()), "1h");

        assert_eq!(code(&mut model, KeyCode::Esc), []);
        assert_eq!((model.focus(), model.tab()), (Focus::List, Tab::Timeline));

        assert_eq!(
            ch(&mut model, 'j'),
            [Command::SelectDevice("srl2".to_string())]
        );
        assert_eq!(model.cursor(), Cursor::Device(1));
        assert_eq!(ch(&mut model, 'j'), []);
        assert_eq!(model.cursor(), Cursor::Alert(0));
        assert_eq!(
            code(&mut model, KeyCode::Enter),
            [
                Command::SelectDevice("srl2".to_string()),
                Command::OpenTab(Tab::Timeline)
            ]
        );
        assert_eq!(model.selected_device().unwrap().name, "srl2");
        assert_eq!((model.focus(), model.tab()), (Focus::Tabs, Tab::Timeline));

        model.tick(
            Instant::now(),
            UNIX_EPOCH + Duration::from_secs(1_794_232_500 + 3 * 3600),
            (80, 24),
        );
        assert!(model.timeline().is_empty());
        model.tick(
            Instant::now(),
            UNIX_EPOCH + Duration::from_secs(1_794_232_500),
            (80, 24),
        );
        assert_eq!(model.timeline().len(), 1);
        assert_eq!(ch(&mut model, 'j'), []);
        assert_eq!(model.timeline_row(), 0);
        assert_eq!(code(&mut model, KeyCode::Enter), []);
        assert!(model.popup());
        assert_eq!(code(&mut model, KeyCode::Esc), []);
        assert!(!model.popup());
        assert_eq!(model.focus(), Focus::Tabs);

        ch(&mut model, '2');
        assert_eq!(model.workbench(), Workbench::Services);
        assert_eq!(model.tab(), Tab::Overview);
        ch(&mut model, '1');
        assert_eq!(model.workbench(), Workbench::Devices);
        for expected in [Tab::Interfaces, Tab::Timeline, Tab::Logs] {
            let srl2 = Command::SelectDevice("srl2".to_string());
            assert_eq!(ch(&mut model, ']'), [srl2, Command::OpenTab(expected)]);
        }
        assert_eq!(ch(&mut model, 'f'), [Command::FollowDevice(false)]);
        assert!(!model.device_logs().unwrap().2);

        assert_eq!(code(&mut model, KeyCode::Esc), []);
        assert_eq!(model.focus(), Focus::List);
        ch(&mut model, '/');
        assert!(model.typing());
        typed(&mut model, "srl1");
        assert_eq!(model.typed_filter(), Some("srl1"));
        code(&mut model, KeyCode::Enter);
        assert!(!model.typing());
        assert_eq!(model.visible_devices(), [0]);
    }

    #[test]
    fn the_services_f_does_not_pause_the_device_logs_and_the_device_f_does_not_pause_the_services_tail(
    ) {
        let mut services = logs_open("grafana");
        assert_eq!(ch(&mut services, 'f'), [Command::Follow(false)]);

        let mut devices = devices_model(workshop_located("infrahub"));
        code(&mut devices, KeyCode::Enter);
        for _ in 0..3 {
            ch(&mut devices, ']');
        }
        assert_eq!(devices.tab(), Tab::Logs);
        assert_eq!(ch(&mut devices, 'f'), [Command::FollowDevice(false)]);
        assert_eq!(ch(&mut devices, 'f'), [Command::FollowDevice(true)]);
    }

    #[test]
    fn switching_workbench_tells_the_pollers_which_device_tab_is_open() {
        let mut model = devices_model(workshop_located("infrahub"));
        code(&mut model, KeyCode::Enter);
        ch(&mut model, ']');
        ch(&mut model, ']');
        assert_eq!(model.tab(), Tab::Timeline);

        assert_eq!(ch(&mut model, '2'), [Command::OpenTab(Tab::Overview)]);
        assert_eq!(model.tab(), Tab::Overview);
        assert_eq!(
            ch(&mut model, '1'),
            [srl1(), Command::OpenTab(Tab::Overview)]
        );

        for _ in 0..3 {
            ch(&mut model, ']');
        }
        assert_eq!(model.tab(), Tab::Logs);
        assert_eq!(ch(&mut model, '2'), [Command::OpenTab(Tab::Overview)]);
        assert_eq!(model.tab(), Tab::Logs);
        assert_eq!(ch(&mut model, '1'), [srl1(), Command::OpenTab(Tab::Logs)]);

        let mut book = book_model();
        book.apply(Update::Devices(Ok(workshop_source().devices().unwrap())));
        ch(&mut book, '1');
        assert_eq!(ch(&mut book, '3'), [Command::OpenTab(Tab::Overview)]);
        assert_eq!(ch(&mut book, '2'), []);
    }

    #[test]
    fn a_dead_prometheus_thread_leaves_no_green_ok_behind() {
        let mut model = devices_model(workshop_located("infrahub"));
        model.apply(Update::Metrics(Ok(peer_samples(1.0))));
        assert_eq!(model.device_rows()[0].health, devices::Health::Ok);

        model.apply(Update::Stopped("metrics".to_string()));

        assert_eq!(model.device_rows()[0].health, devices::Health::Unknown);
        assert!(model.session_states().iter().all(|(_, state)| state == "?"));
        assert_eq!(model.error().unwrap().0, "metrics");

        model.apply(Update::Alerts(Ok(vec![alert(
            "srl1",
            "BgpSessionNotUp",
            "2026-11-09T13:53:32Z",
        )])));
        assert_eq!(model.device_rows()[0].alerts, Some((1, false)));

        model.apply(Update::Stopped("alerts".to_string()));

        assert_eq!(model.device_rows()[0].alerts, None);
        assert_eq!(model.alerts().len(), 1);
        assert_eq!(model.error().unwrap().0, "metrics");

        assert!(model.device_logs().unwrap().2);

        model.apply(Update::Stopped("logs".to_string()));

        assert!(!model.device_logs().unwrap().2);
        assert_eq!(model.error().unwrap().0, "metrics");
    }

    #[test]
    fn the_sot_word_comes_from_the_workshop_setting_and_is_lab_vars_in_the_book_lab() {
        let header = |located: Located, source: Option<Arc<dyn SourceOfTruth>>| {
            Model::new(Start {
                lab: Some(located),
                source,
                chooser: false,
                ..start(vec![stack("autocon5", &[])])
            })
            .header()
        };
        assert!(header(workshop_located("none"), None).ends_with("sot none"));
        assert!(header(workshop_located("infrahub"), None).ends_with("sot infrahub"));
        let book = fixture("book-lab");
        let located = locate(&book, Some(&book)).unwrap();
        let source = lab_vars_source("book-lab/containerlab/lab_vars.yml");
        assert!(header(located, Some(source)).ends_with("sot lab_vars"));
    }

    #[test]
    fn a_failed_prometheus_poll_shows_question_marks_and_its_success_clears_its_own_error() {
        let mut model = devices_model(workshop_located("infrahub"));
        assert_eq!(model.device_rows()[1].alerts, Some((1, false)));

        model.apply(Update::Metrics(Ok(peer_samples(1.0))));
        assert_eq!(model.device_rows()[0].health, devices::Health::Ok);

        model.apply(Update::Metrics(Err("cannot reach http://x".to_string())));
        assert_eq!(model.device_rows()[0].health, devices::Health::Unknown);
        let (source, text, _) = model.error().unwrap();
        assert_eq!(source, "metrics");
        assert!(text.contains("cannot reach"), "{text}");

        model.apply(Update::Metrics(Ok(peer_samples(1.0))));
        assert_eq!(model.device_rows()[0].health, devices::Health::Ok);
        assert_eq!(model.error(), None);

        model.apply(Update::Alerts(Err("cannot reach http://a".to_string())));
        assert_eq!(model.device_rows()[0].alerts, None);
        assert_eq!(model.alerts().len(), 1);
        assert!(!model.alerts_ok());
        model.apply(Update::Metrics(Ok(peer_samples(1.0))));
        assert_eq!(model.error().unwrap().0, "alerts");
        model.apply(Update::Alerts(Ok(Vec::new())));
        assert_eq!(model.error(), None);
    }

    #[test]
    fn the_session_states_follow_the_intended_sessions_and_show_question_marks_without_prometheus()
    {
        let mut model = devices_model(workshop_located("infrahub"));
        let states = |model: &Model| -> Vec<(String, String)> {
            model
                .session_states()
                .into_iter()
                .map(|(session, state)| (session.peer, state))
                .collect()
        };
        let pair = |peer: &str, state: &str| (peer.to_string(), state.to_string());
        assert_eq!(states(&model).len(), 3);
        assert!(states(&model).iter().all(|(_, state)| state == "?"));

        let mut samples = peer_samples(1.0);
        samples[2].value = 5.0;
        model.apply(Update::Metrics(Ok(samples)));
        assert_eq!(
            states(&model),
            [
                pair("10.1.2.2", "up"),
                pair("10.1.7.2", "up"),
                pair("10.1.99.2", "active")
            ]
        );

        model.apply(Update::Metrics(Ok(Vec::new())));
        assert!(states(&model).iter().all(|(_, state)| state == "—"));

        model.apply(Update::Metrics(Err("cannot reach http://x".to_string())));
        assert_eq!(
            states(&model),
            [
                pair("10.1.2.2", "?"),
                pair("10.1.7.2", "?"),
                pair("10.1.99.2", "?")
            ]
        );
    }

    #[test]
    fn moving_to_another_device_clears_its_logs_and_a_repeated_log_line_shows_once() {
        let mut model = devices_model(workshop_located("infrahub"));
        assert_eq!(model.selected_device().unwrap().name, "srl1");
        let first = vec![
            log_line(1, "link down"),
            log_line(2, r#"{"message":"bgp idle"}"#),
        ];
        let second = vec![
            log_line(2, r#"{"message":"bgp idle"}"#),
            log_line(3, "bgp up"),
        ];

        model.apply(Update::DeviceLogs("srl1".to_string(), Ok(first)));
        model.apply(Update::DeviceLogs("srl1".to_string(), Ok(second)));

        let (name, lines, following, _) = model.device_logs().unwrap();
        let texts: Vec<&str> = lines.iter().map(|(_, text)| *text).collect();
        assert_eq!(name, "srl1");
        assert_eq!(texts, ["bgp up", "bgp idle", "link down"]);
        assert!(following);

        model.apply(Update::DeviceLogs(
            "srl2".to_string(),
            Ok(vec![log_line(4, "other")]),
        ));
        assert_eq!(model.device_logs().unwrap().1.len(), 3);

        assert_eq!(
            ch(&mut model, 'j'),
            [Command::SelectDevice("srl2".to_string())]
        );
        let (name, lines, _, _) = model.device_logs().unwrap();
        assert_eq!(name, "srl2");
        assert!(lines.is_empty());

        model.apply(Update::DeviceLogs(
            "srl1".to_string(),
            Ok(vec![log_line(5, "late")]),
        ));
        assert!(model.device_logs().unwrap().1.is_empty());
    }

    #[test]
    fn device_logs_for_a_device_that_is_not_selected_remove_the_loki_error() {
        let mut model = devices_model(workshop_located("infrahub"));
        model.apply(Update::DeviceLogs(
            "srl1".to_string(),
            Err("loki down".to_string()),
        ));
        assert_eq!(model.error().unwrap().0, "logs");

        model.apply(Update::DeviceLogs(
            "srl2".to_string(),
            Ok(vec![log_line(1, "other")]),
        ));

        assert_eq!(model.error(), None);
        assert!(model.device_logs().unwrap().1.is_empty());
    }

    #[test]
    fn rcas_for_a_device_that_is_not_selected_remove_the_loki_error() {
        let mut model = devices_model(workshop_located("infrahub"));
        model.apply(Update::Rcas(
            "srl1".to_string(),
            Err("loki down".to_string()),
        ));
        assert_eq!(model.error().unwrap().0, "logs");

        model.apply(Update::Rcas("srl2".to_string(), Ok(vec![])));

        assert_eq!(model.error(), None);
    }

    #[test]
    fn rcas_for_a_device_that_is_not_selected_are_not_held() {
        let mut model = devices_model(workshop_located("infrahub"));
        assert_eq!(model.selected_device().unwrap().name, "srl1");

        model.apply(Update::Rcas(
            "srl2".to_string(),
            Ok(vec![log_line(1, "other")]),
        ));

        assert!(model.rcas.is_empty());
    }

    fn follow_lab(commands: &[Command]) -> Vec<bool> {
        commands
            .iter()
            .filter_map(|command| match command {
                Command::FollowLab(on) => Some(*on),
                _ => None,
            })
            .collect()
    }

    fn lab_lines(model: &Model) -> Vec<(&str, &str)> {
        model
            .lab_logs
            .shown()
            .into_iter()
            .map(|held| (held.device.as_str(), held.shown.as_str()))
            .collect()
    }

    fn labelled(pairs: &[(&str, &str)], nanoseconds: i64, text: &str) -> (Labels, Line) {
        (map(pairs), log_line(nanoseconds, text))
    }

    #[test]
    fn the_logs_view_shown_and_following_sends_follow_lab_once_and_a_pause_stops_it() {
        let mut model = observability_model(Vec::new());
        assert_eq!(follow_lab(&model.ask_changes()), [false; 0]);

        model.view = Tab::Logs;
        assert_eq!(follow_lab(&model.ask_changes()), [true]);
        assert_eq!(follow_lab(&model.ask_changes()), [false; 0]);

        model.lab_logs.following = false;
        assert_eq!(follow_lab(&model.ask_changes()), [false]);
        assert_eq!(follow_lab(&model.ask_changes()), [false; 0]);
    }

    #[test]
    fn the_logs_view_in_another_workbench_sends_no_follow_lab_and_leaving_it_sends_false() {
        let mut model = devices_model(workshop_located("infrahub"));
        model.view = Tab::Logs;
        assert_eq!(follow_lab(&model.ask_changes()), [false; 0]);

        ch(&mut model, '3');
        assert_eq!(follow_lab(&model.ask_changes()), [true]);

        ch(&mut model, '1');
        assert_eq!(follow_lab(&model.ask_changes()), [false]);
    }

    #[test]
    fn a_lab_line_without_a_device_is_not_kept() {
        let mut model = observability_model(Vec::new());

        model.apply(Update::LabLogs(Ok(vec![
            labelled(&[("device", "srl1"), ("job", "lab")], 3, "bgp up"),
            labelled(&[("job", "lab")], 2, "no device"),
            labelled(&[("device", "other")], 1, "link down"),
        ])));

        assert_eq!(
            lab_lines(&model),
            [("srl1", "bgp up"), ("other", "link down")]
        );
    }

    #[test]
    fn a_lab_answer_while_paused_adds_no_line() {
        let mut model = observability_model(Vec::new());
        model.lab_logs.following = false;

        model.apply(Update::LabLogs(Ok(vec![labelled(
            &[("device", "srl1")],
            1,
            "late",
        )])));
        assert_eq!(lab_lines(&model), []);

        model.lab_logs.following = true;
        model.apply(Update::LabLogs(Ok(vec![labelled(
            &[("device", "srl1")],
            2,
            "new",
        )])));
        assert_eq!(lab_lines(&model), [("srl1", "new")]);
    }

    #[test]
    fn a_lab_answer_while_the_logs_service_is_not_asked_is_dropped() {
        let mut model = Model::new(Start {
            lab: Some(workshop_located("infrahub")),
            chooser: false,
            ..start(vec![stack("a", &[])])
        });
        assert_eq!(model.reach(status::Service::Logs), Reach::Unknown);

        model.apply(Update::LabLogs(Ok(vec![labelled(
            &[("device", "srl1")],
            1,
            "bgp up",
        )])));

        assert_eq!(lab_lines(&model), []);
    }

    #[test]
    fn the_lab_lines_go_when_the_logs_service_goes_and_when_another_stack_is_selected() {
        let answer = || Update::LabLogs(Ok(vec![labelled(&[("device", "srl1")], 1, "bgp up")]));
        let mut model = observability_model(Vec::new());

        model.apply(answer());
        assert_eq!(lab_lines(&model), [("srl1", "bgp up")]);
        model.forget(status::Service::Logs);
        assert_eq!(lab_lines(&model), []);

        model.apply(answer());
        assert_eq!(lab_lines(&model), [("srl1", "bgp up")]);
        model.select(0);
        assert_eq!(lab_lines(&model), []);
    }

    const LAB_LINES: [(&str, i64, &str); 4] = [
        ("srl1", 4, "bgp up"),
        ("other", 3, "link down"),
        ("srl2", 2, "bgp down"),
        ("srl1", 1, "link up"),
    ];

    fn logs_view() -> Model {
        let mut model = observability_model(Vec::new());
        model.apply(Update::LabLogs(Ok(LAB_LINES
            .iter()
            .map(|(device, nanoseconds, text)| labelled(&[("device", device)], *nanoseconds, text))
            .collect())));
        ch(&mut model, ']');
        model
    }

    fn marked(model: &Model) -> Option<(&str, &str)> {
        let (rows, _, row, _) = model.lab_logs();
        row.map(|at| (rows[at].1, rows[at].2))
    }

    fn filtered(model: &mut Model, text: &str) {
        ch(model, '/');
        typed(model, text);
        code(model, KeyCode::Enter);
    }

    fn unfiltered(model: &mut Model) {
        ch(model, '/');
        code(model, KeyCode::Esc);
    }

    #[test]
    fn the_pause_marks_the_newest_shown_line() {
        let mut model = logs_view();
        assert_eq!(marked(&model), None);

        ch(&mut model, 'f');

        assert!(!model.lab_logs().1);
        assert_eq!(marked(&model), Some(("srl1", "bgp up")));
    }

    #[test]
    fn following_shows_no_marker() {
        let mut model = logs_view();
        ch(&mut model, 'j');
        assert_eq!(marked(&model), None);
        ch(&mut model, 'f');
        ch(&mut model, 'j');

        ch(&mut model, 'f');

        assert!(model.lab_logs().1);
        assert_eq!(marked(&model), None);
        assert_eq!(model.lab_mark, None);
    }

    #[test]
    fn j_and_k_move_the_marker_by_one_line() {
        let mut model = logs_view();
        ch(&mut model, 'f');

        ch(&mut model, 'j');
        assert_eq!(marked(&model), Some(("other", "link down")));
        code(&mut model, KeyCode::Down);
        assert_eq!(marked(&model), Some(("srl2", "bgp down")));
        ch(&mut model, 'k');
        assert_eq!(marked(&model), Some(("other", "link down")));
        code(&mut model, KeyCode::Up);
        assert_eq!(marked(&model), Some(("srl1", "bgp up")));
    }

    #[test]
    fn the_marker_stops_at_the_ends() {
        let mut model = logs_view();
        ch(&mut model, 'f');

        ch(&mut model, 'k');
        assert_eq!(marked(&model), Some(("srl1", "bgp up")));
        typed(&mut model, "jjjj");
        assert_eq!(marked(&model), Some(("srl1", "link up")));
    }

    #[test]
    fn a_filter_that_keeps_the_marked_line_keeps_it() {
        let mut model = logs_view();
        ch(&mut model, 'f');
        ch(&mut model, 'j');

        filtered(&mut model, "link");

        assert_eq!(marked(&model), Some(("other", "link down")));
        assert_eq!(model.lab_logs().2, Some(0));
    }

    #[test]
    fn a_filter_that_hides_the_marked_line_marks_the_newest_shown() {
        let mut model = logs_view();
        ch(&mut model, 'f');
        ch(&mut model, 'j');

        filtered(&mut model, "bgp");

        assert_eq!(marked(&model), Some(("srl1", "bgp up")));
        assert_eq!(
            model.lab_mark,
            Some(("srl1".to_string(), log_line(4, "bgp up")))
        );
    }

    #[test]
    fn the_line_marked_under_a_filter_stays_marked_when_it_is_cleared() {
        let mut model = logs_view();
        ch(&mut model, 'f');
        filtered(&mut model, "link");
        assert_eq!(marked(&model), Some(("other", "link down")));
        ch(&mut model, 'j');
        assert_eq!(marked(&model), Some(("srl1", "link up")));

        unfiltered(&mut model);

        assert_eq!(marked(&model), Some(("srl1", "link up")));
        assert_eq!(model.lab_logs().2, Some(3));
    }

    #[test]
    fn an_answer_while_paused_keeps_the_lines_and_the_marked_line() {
        let mut model = logs_view();
        ch(&mut model, 'f');
        ch(&mut model, 'j');

        model.apply(Update::LabLogs(Ok(vec![labelled(
            &[("device", "srl1")],
            5,
            "late",
        )])));

        assert_eq!(model.lab_logs().0.len(), 4);
        assert_eq!(marked(&model), Some(("other", "link down")));
    }

    #[test]
    fn a_failed_answer_keeps_the_marked_line() {
        let mut model = logs_view();
        ch(&mut model, 'f');
        ch(&mut model, 'j');

        model.apply(Update::LabLogs(Err("refused".to_string())));
        assert_eq!(model.reach(status::Service::Logs), Reach::NoAnswer);
        assert_eq!(marked(&model), Some(("other", "link down")));
        ch(&mut model, 'x');

        assert_eq!(model.reach(status::Service::Logs), Reach::Answers);
        assert_eq!(model.lab_logs().0.len(), 4);
        assert_eq!(marked(&model), Some(("other", "link down")));
    }

    #[test]
    fn a_new_window_while_paused_sends_no_request() {
        let mut model = logs_view();
        model.ask_changes();
        ch(&mut model, 'f');
        ch(&mut model, 'j');
        assert_eq!(follow_lab(&model.ask_changes()), [false]);

        ch(&mut model, '1');
        code(&mut model, KeyCode::Enter);
        typed(&mut model, "]]");
        assert_eq!(model.tab(), Tab::Timeline);
        let mut sent = ch(&mut model, '+');
        assert_eq!(sent, [Command::Window(WINDOW_MINUTES * 2)]);
        sent.extend(model.ask_changes());
        sent.extend(ch(&mut model, '3'));
        sent.extend(model.ask_changes());

        assert_eq!(follow_lab(&sent), [false; 0]);
        assert_eq!(model.tab(), Tab::Logs);
        assert_eq!(marked(&model), Some(("other", "link down")));
        ch(&mut model, 'f');
        assert_eq!(follow_lab(&model.ask_changes()), [true]);
    }

    #[test]
    fn the_stack_down_leaves_no_marked_line() {
        let mut model = logs_view();
        ch(&mut model, 'f');
        ch(&mut model, 'j');
        let names: Vec<&str> = SERVICES.iter().map(|row| row.compose).collect();

        model.apply(Update::Services(0, defined(&names), Ok(Vec::new())));
        assert_eq!(model.reach(status::Service::Logs), Reach::StackDown);
        assert_eq!(marked(&model), None);
        model.apply(every_service_up(0));
        model.apply(Update::LabLogs(Ok(vec![labelled(
            &[("device", "srl1")],
            5,
            "new",
        )])));

        assert_eq!(model.reach(status::Service::Logs), Reach::Answers);
        let (rows, following, row, held) = model.lab_logs();
        assert_eq!((rows.len(), following, row, held), (0, false, None, None));
    }

    #[test]
    fn the_marked_line_is_kept_across_the_views() {
        let mut model = logs_view();
        ch(&mut model, 'f');
        ch(&mut model, 'j');

        ch(&mut model, '[');
        assert_eq!(model.tab(), Tab::Alerts);
        ch(&mut model, ']');

        assert_eq!(model.tab(), Tab::Logs);
        assert!(!model.lab_logs().1);
        assert_eq!(marked(&model), Some(("other", "link down")));
    }

    #[test]
    fn the_marked_line_is_kept_across_the_workbenches() {
        let mut model = logs_view();
        ch(&mut model, 'f');
        ch(&mut model, 'j');
        model.ask_changes();

        let mut sent = ch(&mut model, '1');
        sent.extend(model.ask_changes());
        sent.extend(ch(&mut model, '3'));
        sent.extend(model.ask_changes());

        assert_eq!(follow_lab(&sent), [false; 0]);
        assert_eq!(model.tab(), Tab::Logs);
        assert!(!model.lab_logs().1);
        assert_eq!(marked(&model), Some(("other", "link down")));
    }

    #[test]
    fn another_stack_clears_the_lab_lines_and_the_marker() {
        let mut model = logs_view();
        ch(&mut model, 'f');
        ch(&mut model, 'j');

        model.select(0);
        model.ask_changes();

        assert_eq!(model.lab_logs().0.len(), 0);
        assert_eq!(marked(&model), None);
        assert_eq!(model.lab_mark, None);
    }

    #[test]
    fn a_stopped_poller_pauses_the_view() {
        let mut model = logs_view();
        model.ask_changes();

        let sent = model.apply(Update::Stopped("logs".to_string()));

        assert_eq!(follow_lab(&sent), [false]);
        assert!(!model.lab_logs().1);
        assert_eq!(marked(&model), Some(("srl1", "bgp up")));
    }

    #[test]
    fn a_filter_that_hides_every_line_keeps_the_marked_line() {
        let mut model = logs_view();
        ch(&mut model, 'f');
        ch(&mut model, 'j');

        filtered(&mut model, "nowhere");
        assert_eq!(marked(&model), None);
        unfiltered(&mut model);

        assert_eq!(marked(&model), Some(("other", "link down")));
    }

    #[test]
    fn f_does_nothing_where_the_view_cannot_pause() {
        let mut model = logs_view();
        let names: Vec<&str> = SERVICES.iter().map(|row| row.compose).collect();
        model.apply(Update::Services(0, defined(&names), Ok(Vec::new())));
        assert_eq!(model.reach(status::Service::Logs), Reach::StackDown);

        ch(&mut model, 'f');

        assert!(model.lab_logs().1);
    }

    #[test]
    fn without_an_answer_a_paused_view_neither_opens_a_device_nor_moves_the_marker() {
        let mut model = logs_view();
        ch(&mut model, 'f');
        typed(&mut model, "jj");
        model.apply(Update::LabLogs(Err("refused".to_string())));
        assert_eq!(model.reach(status::Service::Logs), Reach::NoAnswer);

        assert_eq!(code(&mut model, KeyCode::Enter), []);
        ch(&mut model, 'j');

        assert_eq!(model.workbench(), Workbench::Observability);
        assert_eq!(marked(&model), Some(("srl2", "bgp down")));
    }

    #[test]
    fn brackets_move_between_the_views_both_ways() {
        let mut model = observability_model(Vec::new());
        assert_eq!(model.tab(), Tab::Alerts);

        let mut views = Vec::new();
        for key in "]][[".chars() {
            assert_eq!(ch(&mut model, key), []);
            views.push(model.tab());
        }

        assert_eq!(views, [Tab::Logs, Tab::Metrics, Tab::Logs, Tab::Alerts]);
    }

    #[test]
    fn square_brackets_go_through_the_three_views_both_ways_and_around() {
        let mut model = observability_model(Vec::new());

        let mut views = Vec::new();
        for key in "]]][[[".chars() {
            assert_eq!(ch(&mut model, key), []);
            views.push(model.tab());
        }

        assert_eq!(
            views,
            [
                Tab::Logs,
                Tab::Metrics,
                Tab::Alerts,
                Tab::Metrics,
                Tab::Logs,
                Tab::Alerts
            ]
        );
    }

    #[test]
    fn the_workbench_remembers_the_metrics_view() {
        let mut model = devices_model(workshop_located("infrahub"));
        typed(&mut model, "3[");
        assert_eq!(model.tab(), Tab::Metrics);

        typed(&mut model, "13");

        assert_eq!(model.workbench(), Workbench::Observability);
        assert_eq!(model.tab(), Tab::Metrics);
    }

    fn named(names: &[&str]) -> Update {
        let device = workshop_source().devices().unwrap().remove(0);
        Update::Devices(Ok(names
            .iter()
            .map(|name| Device {
                name: name.to_string(),
                ..device.clone()
            })
            .collect()))
    }

    fn metrics_view(names: &[&str]) -> Model {
        let mut model = devices_model(workshop_located("infrahub"));
        model.apply(named(names));
        typed(&mut model, "3[");
        model
    }

    fn marked_device(model: &Model) -> Option<&str> {
        model
            .marked_metrics_device()
            .map(|index| model.devices()[index].name.as_str())
    }

    fn row_names(model: &Model) -> Vec<String> {
        model
            .metrics_rows()
            .into_iter()
            .map(|row| row.name)
            .collect()
    }

    #[test]
    fn the_first_opening_marks_the_first_row() {
        let model = metrics_view(&["srl1", "srl2", "srl3"]);

        assert_eq!(marked_device(&model), Some("srl1"));
        assert_eq!(model.marked_metrics_row(), Some(0));
    }

    #[test]
    fn j_and_k_move_the_marker_to_the_next_and_the_previous_device() {
        let mut model = metrics_view(&["srl1", "srl2", "srl3"]);

        ch(&mut model, 'j');
        assert_eq!(marked_device(&model), Some("srl2"));
        code(&mut model, KeyCode::Down);
        assert_eq!(marked_device(&model), Some("srl3"));
        ch(&mut model, 'k');
        assert_eq!(marked_device(&model), Some("srl2"));
        code(&mut model, KeyCode::Up);
        assert_eq!(marked_device(&model), Some("srl1"));
    }

    #[test]
    fn j_on_the_last_device_and_k_on_the_first_keep_the_marker() {
        let mut model = metrics_view(&["srl1", "srl2", "srl3"]);

        ch(&mut model, 'k');
        assert_eq!(marked_device(&model), Some("srl1"));
        typed(&mut model, "jjjj");

        assert_eq!(marked_device(&model), Some("srl3"));
    }

    #[test]
    fn a_filter_that_hides_the_marked_device_marks_the_first_shown_and_keeps_it() {
        let mut model = metrics_view(&["srl1", "srl2", "srl3"]);
        ch(&mut model, 'j');

        filtered(&mut model, "3");

        assert_eq!(row_names(&model), ["srl3"]);
        assert_eq!(marked_device(&model), Some("srl3"));
        assert_eq!(model.kept_device.as_deref(), Some("srl3"));
    }

    #[test]
    fn the_device_marked_under_a_filter_stays_marked_when_it_is_cleared() {
        let mut model = metrics_view(&["srl1", "srl2", "srl3"]);
        ch(&mut model, 'j');
        filtered(&mut model, "3");

        unfiltered(&mut model);

        assert_eq!(row_names(&model), ["srl1", "srl2", "srl3"]);
        assert_eq!(marked_device(&model), Some("srl3"));
    }

    #[test]
    fn a_filter_that_hides_every_device_marks_none_keeps_the_name_and_enter_does_nothing() {
        let mut model = metrics_view(&["srl1", "srl2", "srl3"]);
        ch(&mut model, 'j');

        filtered(&mut model, "nowhere");

        assert_eq!(marked_device(&model), None);
        assert_eq!(model.kept_device.as_deref(), Some("srl2"));
        assert_eq!(code(&mut model, KeyCode::Enter), []);
        assert_eq!(model.workbench(), Workbench::Observability);
        unfiltered(&mut model);
        assert_eq!(marked_device(&model), Some("srl2"));
    }

    #[test]
    fn a_new_device_list_keeps_the_marked_device_at_its_new_place() {
        let mut model = metrics_view(&["srl1", "srl2", "srl3"]);
        ch(&mut model, 'j');

        model.apply(named(&["srl2", "srl3", "srl1"]));

        assert_eq!(marked_device(&model), Some("srl2"));
        assert_eq!(model.marked_metrics_row(), Some(0));
    }

    #[test]
    fn a_new_device_list_without_the_marked_device_marks_the_first_row_and_keeps_it() {
        let mut model = metrics_view(&["srl1", "srl2", "srl3"]);
        ch(&mut model, 'j');

        model.apply(named(&["srl1", "srl3"]));

        assert_eq!(marked_device(&model), Some("srl1"));
        assert_eq!(model.kept_device.as_deref(), Some("srl1"));
    }

    #[test]
    fn an_answer_a_failed_answer_and_a_forgotten_answer_keep_the_marked_device() {
        let mut model = metrics_view(&["srl1", "srl2", "srl3"]);
        ch(&mut model, 'j');

        model.apply(Update::Metrics(Ok(peer_samples(1.0))));
        assert_eq!(marked_device(&model), Some("srl2"));
        model.apply(Update::Metrics(Err("refused".to_string())));
        assert_eq!(marked_device(&model), Some("srl2"));
        model.forget(status::Service::Metrics);
        model.ask_changes();

        assert_eq!(marked_device(&model), Some("srl2"));
    }

    #[test]
    fn the_marked_device_is_kept_across_the_views() {
        let mut model = metrics_view(&["srl1", "srl2", "srl3"]);
        ch(&mut model, 'j');

        typed(&mut model, "][[");
        assert_eq!(model.tab(), Tab::Logs);
        ch(&mut model, '[');
        assert_eq!(model.tab(), Tab::Alerts);
        ch(&mut model, '[');

        assert_eq!(model.tab(), Tab::Metrics);
        assert_eq!(marked_device(&model), Some("srl2"));
    }

    #[test]
    fn the_marked_device_is_kept_across_the_workbenches() {
        let mut model = metrics_view(&["srl1", "srl2", "srl3"]);
        ch(&mut model, 'j');

        code(&mut model, KeyCode::Enter);
        assert_eq!(model.workbench(), Workbench::Devices);
        model.apply(named(&["srl3", "srl1", "srl2"]));
        ch(&mut model, '3');
        assert_eq!(marked_device(&model), Some("srl2"));
        typed(&mut model, "13");

        assert_eq!(model.tab(), Tab::Metrics);
        assert_eq!(marked_device(&model), Some("srl2"));
    }

    #[test]
    fn outside_a_lab_another_stack_keeps_the_marked_device_while_it_is_listed() {
        let mut model = opened(&["a", "b"]);
        model.apply(every_service_up(0));
        model.apply(every_service_up(1));
        model.apply(named(&["srl1", "srl2", "srl3"]));
        typed(&mut model, "3[j");
        assert_eq!(marked_device(&model), Some("srl2"));

        typed(&mut model, "2j");
        model.ask_changes();
        ch(&mut model, '3');
        assert_eq!(model.selected_stack(), 1);
        assert_eq!(marked_device(&model), Some("srl2"));
        typed(&mut model, "2k");
        model.apply(named(&["srl1", "srl3"]));
        ch(&mut model, '3');

        assert_eq!(model.selected_stack(), 0);
        assert_eq!(marked_device(&model), Some("srl1"));
    }

    #[test]
    fn every_device_has_a_row_in_the_order_of_the_devices_list() {
        let mut model = metrics_view(&["srl3", "srl1", "srl2"]);

        model.apply(Update::Metrics(Ok(peer_samples(1.0))));

        let rows = model.metrics_rows();
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["srl3", "srl1", "srl2"]);
        assert!(rows[0].no_data);
        assert!(!rows[1].no_data);
    }

    #[test]
    fn enter_opens_the_marked_device_on_its_interfaces_tab() {
        let mut model = metrics_view(&["srl1", "srl2"]);
        ch(&mut model, 'j');
        assert!(key_line(&model, 100)[0].contains("Enter open the device"));

        let sent = code(&mut model, KeyCode::Enter);

        assert_eq!(
            sent,
            [
                Command::SelectDevice("srl2".to_string()),
                Command::OpenTab(Tab::Interfaces)
            ]
        );
        assert_eq!(model.workbench(), Workbench::Devices);
        assert_eq!(model.selected_device().unwrap().name, "srl2");
        assert_eq!((model.focus(), model.tab()), (Focus::Tabs, Tab::Interfaces));
    }

    #[test]
    fn with_no_row_enter_does_nothing_and_is_not_offered() {
        let mut filtered_out = metrics_view(&["srl1", "srl2"]);
        filtered(&mut filtered_out, "nowhere");
        let mut without_metrics = metrics_view(&["srl1", "srl2"]);
        without_metrics.apply(Update::Services(
            0,
            defined(&["alertmanager"]),
            Ok(vec![row("alertmanager", RUNNING)]),
        ));
        assert_eq!(
            without_metrics.reach(status::Service::Metrics),
            Reach::NotInLab
        );

        for mut model in [filtered_out, without_metrics] {
            assert!(!key_line(&model, 100)[0].contains("Enter"));
            assert_eq!(code(&mut model, KeyCode::Enter), []);
            assert_eq!(model.workbench(), Workbench::Observability);
        }
    }

    #[test]
    fn the_metrics_filter_reads_q_as_a_letter() {
        let mut model = metrics_view(&["srl1", "srl2"]);

        ch(&mut model, '/');
        assert!(model.typing());
        assert_eq!(ch(&mut model, 'q'), []);

        assert!(!model.quit());
        assert_eq!(model.typed_filter(), Some("q"));
    }

    #[test]
    fn the_health_cell_still_says_down_for_a_device_without_values() {
        let mut model = metrics_view(&["srl1", "srl2"]);

        model.apply(Update::Metrics(Ok(peer_samples(1.0))));

        assert_eq!(model.device_rows()[1].health, devices::Health::Down);
        assert!(model.metrics_rows()[1].no_data);
    }

    #[test]
    fn tab_keeps_its_meaning_in_the_observability_workbench() {
        let mut model = logs_view();

        code(&mut model, KeyCode::Tab);
        assert_eq!((model.focus(), model.tab()), (Focus::List, Tab::Logs));
        ch(&mut model, 'o');
        model.set_run_view(measured(0, 0));
        code(&mut model, KeyCode::Tab);
        assert_eq!((model.focus(), model.tab()), (Focus::Run, Tab::Logs));
        code(&mut model, KeyCode::Tab);
        assert_eq!((model.focus(), model.tab()), (Focus::List, Tab::Logs));
    }

    #[test]
    fn enter_opens_the_marked_device_on_its_logs_tab() {
        let mut model = logs_view();
        ch(&mut model, 'f');
        typed(&mut model, "jj");

        let sent = code(&mut model, KeyCode::Enter);

        assert_eq!(
            sent,
            [
                Command::SelectDevice("srl2".to_string()),
                Command::OpenTab(Tab::Logs)
            ]
        );
        assert_eq!(model.workbench(), Workbench::Devices);
        assert_eq!(model.selected_device().unwrap().name, "srl2");
        assert_eq!((model.focus(), model.tab()), (Focus::Tabs, Tab::Logs));
        ch(&mut model, '3');
        assert_eq!(model.tab(), Tab::Logs);
        assert_eq!(marked(&model), Some(("srl2", "bgp down")));
    }

    #[test]
    fn enter_does_nothing_while_following() {
        let mut model = logs_view();

        assert_eq!(code(&mut model, KeyCode::Enter), []);
        assert_eq!(model.workbench(), Workbench::Observability);
    }

    #[test]
    fn enter_does_nothing_on_a_device_outside_the_lab() {
        let mut model = logs_view();
        ch(&mut model, 'f');
        ch(&mut model, 'j');
        assert_eq!(marked(&model), Some(("other", "link down")));

        assert_eq!(code(&mut model, KeyCode::Enter), []);
        assert_eq!(model.workbench(), Workbench::Observability);
    }

    #[test]
    fn the_observability_workbench_opens_on_the_alerts_the_first_time() {
        let mut devices = devices_model(workshop_located("infrahub"));
        code(&mut devices, KeyCode::Enter);
        typed(&mut devices, "]]");
        let sent = ch(&mut devices, '3');
        assert_eq!(sent, [Command::OpenTab(Tab::Overview)]);
        assert_eq!(devices.tab(), Tab::Alerts);
        assert_eq!(follow_lab(&devices.ask_changes()), [false; 0]);
        ch(&mut devices, '1');
        assert_eq!(devices.tab(), Tab::Timeline);

        let mut services = logs_open("grafana");
        assert_eq!(ch(&mut services, '3'), []);
        assert_eq!(services.tab(), Tab::Alerts);

        let mut book = book_model();
        assert_eq!(ch(&mut book, '4'), []);
        assert_eq!(book.tab(), Tab::Alerts);
        ch(&mut book, ']');
        assert_eq!(follow_lab(&book.ask_changes()), [true]);
        assert_eq!(ch(&mut book, '3'), []);
        assert_eq!(book.workbench(), Workbench::Lab);
        assert_eq!(follow_lab(&book.ask_changes()), [false]);
    }

    #[test]
    fn the_observability_workbench_opens_on_the_view_it_was_left_on() {
        let mut model = devices_model(workshop_located("infrahub"));
        ch(&mut model, '3');
        ch(&mut model, ']');
        assert_eq!(follow_lab(&model.ask_changes()), [true]);
        ch(&mut model, '1');
        assert_eq!(follow_lab(&model.ask_changes()), [false]);

        assert_eq!(ch(&mut model, '3'), [Command::OpenTab(Tab::Overview)]);
        assert_eq!(model.tab(), Tab::Logs);
        assert_eq!(follow_lab(&model.ask_changes()), [true]);
        assert_eq!(ch(&mut model, '3'), []);
        assert_eq!(model.tab(), Tab::Logs);
        assert_eq!(follow_lab(&model.ask_changes()), [false; 0]);
    }

    #[test]
    fn a_visit_to_the_observability_workbench_keeps_the_device_tab() {
        let labs: [(fn() -> Model, char); 2] = [
            (|| devices_model(workshop_located("infrahub")), '3'),
            (lab_devices_model, '4'),
        ];
        for (build, key) in labs {
            for tab in [Tab::Logs, Tab::Timeline] {
                let mut model = build();
                code(&mut model, KeyCode::Enter);
                while model.tab() != tab {
                    ch(&mut model, ']');
                }
                let device = model.selected_device().unwrap().name.clone();

                assert_eq!(ch(&mut model, key), [Command::OpenTab(Tab::Overview)]);
                ch(&mut model, ']');
                assert_eq!(follow_lab(&model.ask_changes()), [true]);
                let sent = ch(&mut model, '1');

                assert_eq!(
                    sent,
                    [Command::SelectDevice(device), Command::OpenTab(tab)],
                    "{key} {tab:?}"
                );
                assert_eq!(model.tab(), tab);
                assert_eq!(follow_lab(&model.ask_changes()), [false]);
            }
        }
    }

    #[test]
    fn a_visit_to_the_observability_workbench_keeps_the_services_tab() {
        let mut model = logs_open("grafana");
        assert_eq!(model.tab(), Tab::Logs);

        assert_eq!(ch(&mut model, '3'), []);
        ch(&mut model, ']');
        assert_eq!(follow_lab(&model.ask_changes()), [true]);
        assert_eq!(ch(&mut model, '2'), []);

        assert_eq!(model.workbench(), Workbench::Services);
        assert_eq!(model.tab(), Tab::Logs);
        assert_eq!(follow_lab(&model.ask_changes()), [false]);

        let mut sonda = opened(&["a"]);
        code(&mut sonda, KeyCode::Tab);
        ch(&mut sonda, '[');
        assert_eq!(sonda.tab(), Tab::Sonda);
        ch(&mut sonda, '3');
        ch(&mut sonda, '2');
        assert_eq!(sonda.tab(), Tab::Sonda);
    }

    fn os(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn nobs(args: &[&str]) -> Command {
        Command::Run {
            program: PathBuf::from("/bin/nobs"),
            args: os(args),
            login: Login::default(),
        }
    }

    fn book_model() -> Model {
        let dir = fixture("book-lab");
        let located = locate(&dir, Some(&dir)).unwrap();
        let stacks = stacks(Some(&located), vec![]).unwrap();
        Model::new(Start {
            lab: Some(located),
            home: Workbench::Lab,
            chooser: false,
            ..start(stacks)
        })
    }

    fn book_dir() -> String {
        fixture("book-lab").to_string_lossy().into_owned()
    }

    fn running(model: &mut Model) {
        ch(model, 'u');
        ch(model, 'y');
    }

    fn state(model: &Model) -> RunState {
        model.run().unwrap().state
    }

    #[test]
    fn y_after_each_action_key_runs_nobs_with_the_folder_and_the_words() {
        let ws = "/ws/nobs-t-packt";
        for (typed, word) in [('u', "up"), ('d', "down")] {
            let mut model = opened(&["nobs-t-packt"]);
            assert_eq!(ch(&mut model, typed), []);
            assert_eq!(
                model.question().unwrap().text(),
                format!("run nobs --dir{NO_BREAK}{ws} nobs-t-packt {word}? y/n")
            );
            assert_eq!(
                ch(&mut model, 'y'),
                [nobs(&["--dir", ws, "nobs-t-packt", word])]
            );
            assert!(model.run_pane());
            assert_eq!(model.question(), None);
            assert_eq!(model.run().unwrap().words, format!("nobs-t-packt {word}"));
            assert_eq!(state(&model), RunState::Running);
        }

        let one = fixture("workshop-one").to_string_lossy().into_owned();
        let mut model = devices_model(workshop_located("infrahub"));
        ch(&mut model, 'm');
        assert_eq!(
            ch(&mut model, 'y'),
            [nobs(&[
                "--dir",
                &one,
                "nobs-t-one",
                "maintenance",
                "-d",
                "srl1",
                "--state"
            ])]
        );

        let lab = book_dir();
        for (typed, word) in [('D', "deploy"), ('X', "destroy")] {
            let mut model = book_model();
            ch(&mut model, typed);
            assert_eq!(
                ch(&mut model, 'y'),
                [nobs(&[
                    "--dir",
                    &lab,
                    "lab",
                    word,
                    "--sudo-no-prompt",
                    "-s",
                    "ch1"
                ])]
            );
            assert_eq!(
                model.run().unwrap().words,
                format!("lab {word} --sudo-no-prompt -s{NO_BREAK}ch1")
            );
        }

        let mut model = book_model();
        ch(&mut model, 'P');
        assert_eq!(
            ch(&mut model, 'y'),
            [nobs(&["--dir", &lab, "lab", "purge", "--sudo-no-prompt"])]
        );
    }

    #[test]
    fn any_answer_but_y_runs_nothing_and_leaves_one_notice() {
        for answer in [
            KeyCode::Char('n'),
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Char('q'),
        ] {
            let mut model = opened(&["nobs-t-packt"]);
            ch(&mut model, 'u');

            assert_eq!(code(&mut model, answer), [], "{answer:?}");

            assert_eq!(model.question(), None, "{answer:?}");
            assert!(model.run().is_none(), "{answer:?}");
            assert!(!model.quit(), "{answer:?}");
            assert_eq!(
                model.notice(),
                Some("not run: nobs-t-packt up"),
                "{answer:?}"
            );
        }
    }

    #[test]
    fn an_action_key_while_one_runs_starts_nothing_and_says_which_runs() {
        let mut model = opened(&["nobs-t-packt"]);
        running(&mut model);

        assert_eq!(ch(&mut model, 'd'), []);
        assert_eq!(model.question(), None);
        assert_eq!(
            model.notice(),
            Some("one action at a time: nobs-t-packt up is running")
        );
        assert_eq!(ch(&mut model, 'y'), []);
        assert_eq!(model.run().unwrap().words, "nobs-t-packt up");
    }

    #[test]
    fn m_builds_clear_or_state_by_the_flag_and_nothing_in_the_book_lab_or_without_devices() {
        let mut model = devices_model(workshop_located("infrahub"));
        let mut devices = workshop_source().devices().unwrap();
        devices[0].maintenance = true;
        model.apply(Update::Devices(Ok(devices)));
        ch(&mut model, 'm');
        assert_eq!(
            model.question().unwrap().text(),
            format!(
                "run nobs --dir{NO_BREAK}{} nobs-t-one maintenance -d{NO_BREAK}srl1 --clear? y/n",
                fixture("workshop-one").display()
            )
        );
        code(&mut model, KeyCode::Esc);
        ch(&mut model, 'j');
        ch(&mut model, 'm');
        assert!(model
            .question()
            .unwrap()
            .text()
            .ends_with(&format!("maintenance -d{NO_BREAK}srl2 --state? y/n")));

        let mut model = book_model();
        model.apply(Update::Devices(Ok(workshop_source().devices().unwrap())));
        ch(&mut model, '1');
        assert_eq!(ch(&mut model, 'm'), []);
        assert_eq!(model.question(), None);
        assert_eq!(ch(&mut model, 'y'), []);

        let mut model = devices_model(workshop_located("infrahub"));
        model.apply(Update::Devices(Ok(Vec::new())));
        assert_eq!(ch(&mut model, 'm'), []);
        assert_eq!(model.question(), None);
        assert_eq!(ch(&mut model, 'y'), []);
    }

    #[test]
    fn u_on_a_chapter_with_a_run_going_says_why_and_starts_nothing() {
        let mut model = book_model();
        assert!(!ch(&mut model, 'S').is_empty());
        let words = model.run().unwrap().words.clone();
        ch(&mut model, '2');

        assert_eq!(ch(&mut model, 'u'), []);

        assert_eq!(
            model.notice(),
            Some("u up is for a workshop; a chapter has its actions in the lab workbench")
        );
        assert_eq!(model.question(), None);
        assert_eq!(model.run().unwrap().words, words);
    }

    #[test]
    fn u_and_d_on_a_stack_that_is_not_a_workshop_run_nothing_and_say_why() {
        let mut model = book_model();
        ch(&mut model, '2');
        for (typed, hint) in [('u', "up"), ('d', "down")] {
            assert_eq!(ch(&mut model, typed), []);
            assert_eq!(model.question(), None);
            assert_eq!(
                model.notice(),
                Some(
                    format!(
                        "{typed} {hint} is for a workshop; a chapter has its actions in the lab workbench"
                    )
                    .as_str()
                )
            );
        }
        assert_eq!(ch(&mut model, 'y'), []);
    }

    #[test]
    fn u_on_a_labcli_lab_or_on_sonda_only_says_only_that_it_is_for_a_workshop() {
        let labcli = Stack {
            kind: StackKind::Lab,
            ..stack("netlab", &[])
        };
        for stack in [labcli, sonda_stack()] {
            let kind = stack.kind;
            let mut model = Model::new(start(vec![stack]));
            code(&mut model, KeyCode::Enter);
            code(&mut model, KeyCode::Tab);
            if model.focus() == Focus::Lines {
                code(&mut model, KeyCode::Tab);
            }

            assert_eq!(ch(&mut model, 'u'), [], "{kind:?}");
            assert_eq!(model.question(), None, "{kind:?}");
            assert_eq!(model.notice(), Some("u up is for a workshop"), "{kind:?}");
        }
    }

    #[test]
    fn the_first_failing_source_keeps_the_error_row_until_it_answers() {
        let mut model = devices_model(workshop_located("infrahub"));
        let first = Instant::now();
        let at = |seconds| first + Duration::from_secs(seconds);
        let fail_in_turn = [
            Update::Alerts(Err("alertmanager one".to_string())),
            Update::Metrics(Err("prometheus one".to_string())),
            Update::Alerts(Err("alertmanager two".to_string())),
            Update::Metrics(Err("prometheus two".to_string())),
        ];
        for (seconds, update) in (0..).zip(fail_in_turn) {
            model.tick(at(seconds), UNIX_EPOCH, (80, 24));
            model.apply(update);
        }

        assert_eq!(model.error(), Some(("alerts", "alertmanager two", at(2))));
        model.apply(Update::Alerts(Ok(Vec::new())));
        assert_eq!(model.error(), Some(("metrics", "prometheus two", at(3))));
        model.tick(at(4), UNIX_EPOCH, (80, 24));
        model.apply(Update::Metrics(Err("prometheus three".to_string())));
        assert_eq!(model.error(), Some(("metrics", "prometheus three", at(4))));
    }

    #[test]
    fn outside_a_lab_the_state_of_every_service_is_read_from_the_selected_stack_only() {
        let mut model = Model::new(start(vec![stack("a", &[]), stack("b", &[]), sonda_stack()]));
        let reach = |model: &Model| {
            [status::Service::Metrics, status::Service::Sonda].map(|service| model.reach(service))
        };
        let asks = |commands: &[Command], on: bool| {
            commands
                .iter()
                .any(|command| matches!(command, Command::Ask(status::Service::Metrics, asked) if *asked == on))
        };

        model.apply(Update::Services(
            1,
            defined(&["prometheus"]),
            Ok(vec![row("prometheus", RUNNING)]),
        ));
        let before_a_answers = reach(&model);
        model.apply(Update::Services(
            0,
            defined(&["prometheus", "sonda-server"]),
            Ok(Vec::new()),
        ));
        let at_a = reach(&model);
        model.ask_changes();
        ch(&mut model, 'j');
        let to_b = model.ask_changes();
        let at_b = reach(&model);
        ch(&mut model, 'j');
        let to_sonda = model.ask_changes();
        let at_sonda = reach(&model);

        assert_eq!(before_a_answers, [Reach::Unknown, Reach::Unknown]);
        assert_eq!(at_a, [Reach::StackDown, Reach::StackDown]);
        assert_eq!(at_b, [Reach::Answers, Reach::NotInLab]);
        assert_eq!(at_sonda, [Reach::NotInLab, Reach::Answers]);
        assert!(asks(&to_b, true), "{to_b:?}");
        assert!(asks(&to_sonda, false), "{to_sonda:?}");
    }

    #[test]
    fn a_stopped_sonda_poller_gives_no_answer() {
        let mut model = sonda_tab();
        let before = model.reach(status::Service::Sonda);

        model.apply(Update::Stopped(
            error_source(status::Service::Sonda).to_string(),
        ));

        assert_eq!(before, Reach::Answers);
        assert_eq!(model.reach(status::Service::Sonda), Reach::NoAnswer);
    }

    #[test]
    fn a_stopped_tail_of_the_container_logs_says_docker_logs_and_leaves_the_logs_service_as_it_was()
    {
        let mut model = Model::new(start(vec![stack("a", &[])]));
        model.apply(every_service_up(0));
        let before = model.reach(status::Service::Logs);

        model.apply(Update::Stopped(DOCKER_LOGS.to_string()));
        let stopped = model
            .error()
            .map(|(source, text, _)| (source.to_string(), text.to_string()));
        model.apply(Update::Logs(
            0,
            "grafana".to_string(),
            Err("gone".to_string()),
        ));

        assert_eq!(before, Reach::Answers);
        assert_eq!(model.reach(status::Service::Logs), Reach::Answers);
        assert_eq!(
            stopped,
            Some(("docker logs".to_string(), "polling stopped".to_string()))
        );
        assert_eq!(model.error().unwrap().0, "docker logs");
    }

    #[test]
    fn a_poller_that_stops_behind_another_error_shows_once_the_errors_before_it_are_gone() {
        let mut model = devices_model(workshop_located("infrahub"));
        let at = Instant::now();
        model.tick(at, UNIX_EPOCH, (80, 24));

        model.apply(Update::Alerts(Err("refused".to_string())));
        model.apply(Update::Stopped("metrics".to_string()));
        model.apply(Update::Devices(Err("sot down".to_string())));
        model.apply(Update::Alerts(Err("refused again".to_string())));

        assert_eq!(model.error(), Some(("alerts", "refused again", at)));
        model.apply(Update::Alerts(Ok(Vec::new())));
        assert_eq!(model.error(), Some(("metrics", "polling stopped", at)));
        ch(&mut model, 'x');
        assert_eq!(model.error(), Some(("sot", "sot down", at)));
        ch(&mut model, 'x');
        assert_eq!(model.error(), None);
    }

    const READING: [&str; 3] = ["status", "show", "fetch"];

    fn on_its_row(row: Row) -> Model {
        match row {
            Row::Stack => opened(&["nobs-t-packt"]),
            Row::Device => devices_model(workshop_located("infrahub")),
            Row::Node => lab_devices_model(),
            Row::Chapter | Row::Lab => book_model(),
        }
    }

    fn on_another_row(row: Row) -> Model {
        match row {
            Row::Stack => {
                let mut model = book_model();
                ch(&mut model, '2');
                model
            }
            Row::Device => {
                let mut model = book_model();
                model.apply(Update::Devices(Ok(workshop_source().devices().unwrap())));
                ch(&mut model, '1');
                model
            }
            Row::Node => devices_model(workshop_located("infrahub")),
            Row::Chapter | Row::Lab => Model::new(Start {
                lab: Some(workshop_located("infrahub")),
                home: Workbench::Lab,
                chooser: false,
                ..start(vec![stack("a", &[])])
            }),
        }
    }

    const CLAUSES: [(Workbench, char, &str); 9] = [
        (Workbench::Services, 'X', "removes the stack's volumes"),
        (
            Workbench::Services,
            'R',
            "clears silences, maintenance and cascade scenarios",
        ),
        (
            Workbench::Services,
            'p',
            "flaps srl1, leaves it in maintenance and can take 13 minutes",
        ),
        (
            Workbench::Lab,
            'D',
            "takes the other chapters' services down",
        ),
        (
            Workbench::Lab,
            'X',
            "removes the chapter's volumes, every node and the other chapters' services",
        ),
        (
            Workbench::Lab,
            'P',
            "takes every scenario down and removes their volumes",
        ),
        (Workbench::Lab, 'R', "takes every scenario down first"),
        (
            Workbench::Lab,
            'U',
            "recreates every service of the chapter and its unnamed volumes, and takes the other chapters' services down",
        ),
        (
            Workbench::Lab,
            'B',
            "takes the other chapters' services down",
        ),
    ];

    fn agreed_clause(action: &Action) -> Option<&'static str> {
        CLAUSES
            .iter()
            .find(|(workbench, key, _)| *workbench == action.workbench && *key == action.key)
            .map(|(.., clause)| *clause)
    }

    #[test]
    fn a_changing_action_asks_with_its_whole_command_and_a_reading_one_runs_at_once() {
        for (workbench, key, _) in &CLAUSES {
            let named = ACTIONS
                .iter()
                .any(|action| action.workbench == *workbench && action.key == *key);
            assert!(named, "{workbench:?} {key}");
        }
        for action in &ACTIONS {
            let saying = match action.ask {
                Ask::Saying(saying) => Some(saying),
                Ask::No | Ask::Yes | Ask::Text => None,
            };
            assert_eq!(saying, agreed_clause(action), "{}", action.key);
            let mut model = on_its_row(action.row);
            let target = model.action_target(action.row).unwrap();
            let args = pieces(action, &target, &model).unwrap();
            let run = Command::Run {
                program: PathBuf::from("/bin/nobs"),
                args: arguments(action, &target, &model).unwrap(),
                login: Login::default(),
            };
            let commands = ch(&mut model, action.key);
            if action.ask == Ask::Text {
                assert_eq!(commands, [], "{}", action.key);
                assert_eq!(model.question(), None, "{}", action.key);
                assert!(model.entry().is_some(), "{}", action.key);
                continue;
            }
            if action
                .words
                .last()
                .is_some_and(|word| READING.contains(word))
            {
                let opened = action.keeps.then_some(Command::OpenTab(Tab::Config));
                let expected: Vec<Command> = [run].into_iter().chain(opened).collect();
                assert_eq!(commands, expected, "{}", action.key);
                assert_eq!(model.question(), None, "{}", action.key);
                continue;
            }
            let clause =
                agreed_clause(action).map_or(String::new(), |clause| format!(" It {clause}."));
            assert_eq!(commands, [], "{}", action.key);
            assert_eq!(
                model.question().unwrap().text(),
                format!("run nobs {}?{clause} y/n", joined(&args))
            );

            let mut refused = on_its_row(action.row);
            ch(&mut refused, action.key);
            assert_eq!(ch(&mut refused, 'n'), [], "{}", action.key);
            assert!(refused.run().is_none(), "{}", action.key);
            assert_eq!(
                refused.notice(),
                Some(format!("not run: {}", words(&args)).as_str())
            );

            assert_eq!(ch(&mut model, 'y'), [run], "{}", action.key);
        }
    }

    #[test]
    fn an_action_key_on_a_row_of_another_kind_runs_nothing_and_asks_nothing() {
        for action in &ACTIONS {
            let mut model = on_another_row(action.row);
            assert_eq!(ch(&mut model, action.key), [], "{}", action.key);
            assert_eq!(model.question(), None, "{}", action.key);
            assert!(model.run().is_none(), "{}", action.key);
        }
    }

    fn offered(action: &Action, model: &Model) -> bool {
        model
            .action_target(action.row)
            .is_some_and(|target| arguments(action, &target, model).is_some())
    }

    #[test]
    fn every_action_shows_its_hint_once_where_it_is_offered_and_once_in_its_help() {
        for action in &ACTIONS {
            let hint = format!("{} {}", action.key, action.hint);
            let mut model = on_its_row(action.row);
            assert!(offered(action, &model), "{hint}");
            let line = key_line(&model, 200).concat();
            assert_eq!(line.matches(&hint).count(), 1, "{hint}: {line}");
            let steps: &[KeyCode] = match action.row {
                Row::Stack => &[KeyCode::Tab, KeyCode::Char(']'), KeyCode::Char(']')],
                Row::Device => &[
                    KeyCode::Char('j'),
                    KeyCode::Enter,
                    KeyCode::Char(']'),
                    KeyCode::Char(']'),
                    KeyCode::Enter,
                    KeyCode::Esc,
                    KeyCode::Char(']'),
                ],
                Row::Node => &[
                    KeyCode::Char('j'),
                    KeyCode::Enter,
                    KeyCode::Char(']'),
                    KeyCode::Char(']'),
                    KeyCode::Esc,
                ],
                Row::Chapter | Row::Lab => &[],
            };
            model.tick(
                Instant::now(),
                UNIX_EPOCH + Duration::from_secs(1_794_232_500),
                (80, 24),
            );
            let mut popup = false;
            for step in steps {
                code(&mut model, *step);
                popup |= model.popup();
                let line = key_line(&model, 200).concat();
                let shown = usize::from(offered(action, &model));
                assert_eq!(
                    line.matches(&hint).count(),
                    shown,
                    "{hint} {step:?}: {line}"
                );
            }
            assert_eq!(popup, action.row == Row::Device, "{hint}");

            let elsewhere = on_another_row(action.row);
            assert!(!offered(action, &elsewhere), "{hint}");
            let line = key_line(&elsewhere, 200).concat();
            assert!(!line.contains(&hint), "{hint}: {line}");

            let home = on_its_row(action.row);
            assert_eq!(home.workbench(), action.workbench, "{hint}");
            let entries = help_entries(&home);
            let entry = (action.key.to_string(), action.hint);
            assert_eq!(entries.iter().filter(|seen| **seen == entry).count(), 1);
            let key = action.key.to_string();
            let naming = entries
                .iter()
                .filter(|(keys, _)| keys.split_whitespace().any(|seen| seen == key))
                .count();
            assert_eq!(naming, 1, "{hint}: {entries:?}");
        }
    }

    #[test]
    fn an_action_hint_is_absent_from_the_key_line_on_a_row_of_another_kind() {
        for action in &ACTIONS {
            let line = key_line(&on_another_row(action.row), 200).concat();
            assert!(
                !line.split_whitespace().any(|word| word == action.hint),
                "{}: {line}",
                action.hint
            );
        }
    }

    #[test]
    fn a_notice_shows_once_and_the_next_key_clears_it_and_still_acts() {
        let mut model = opened(&["nobs-t-packt", "other"]);
        ch(&mut model, 'u');
        ch(&mut model, 'n');
        assert_eq!(model.notice(), Some("not run: nobs-t-packt up"));

        assert_eq!(ch(&mut model, 'j'), [Command::SelectStack(1)]);

        assert_eq!(model.notice(), None);
    }

    #[test]
    fn a_stopped_action_ends_stopped_and_keeps_the_mark_on_its_row() {
        let mut model = opened(&["nobs-t-packt"]);
        running(&mut model);
        let ctrl_c = with(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(handle(&mut model, ctrl_c), [Command::Stop]);
        assert_eq!(state(&model), RunState::Stopping);
        assert!(!model.quit());

        model.apply(Update::ActionEnded(End::Stopped));

        assert_eq!(state(&model), RunState::Ended(End::Stopped));
        assert!(model.failed(&Target::Stack(0)));
        assert!(model.run().unwrap().ended.is_some());
        assert_eq!(handle(&mut model, ctrl_c), [Command::Quit]);
    }

    #[test]
    fn an_action_that_did_not_stop_says_so_on_its_last_line() {
        let mut model = opened(&["nobs-t-packt"]);
        running(&mut model);
        model.apply(Update::ActionLine("pulling".to_string()));

        model.apply(Update::ActionEnded(End::NotStopped { group: 4242 }));

        assert_eq!(
            state(&model),
            RunState::Ended(End::NotStopped { group: 4242 })
        );
        assert!(model.failed(&Target::Stack(0)));
        assert_eq!(
            model.run().unwrap().lines,
            [
                "pulling",
                "nobs: the action did not stop: a program it started is still running in process group 4242"
            ]
        );
    }

    #[test]
    fn the_lines_keep_coming_after_the_end_and_only_a_clean_exit_clears_the_mark() {
        let mut model = opened(&["nobs-t-packt", "other"]);
        running(&mut model);
        model.apply(Update::ActionEnded(End::Exited(1)));
        assert!(model.failed(&Target::Stack(0)));
        model.apply(Update::ActionLine("late".to_string()));
        assert_eq!(model.run().unwrap().lines, ["late"]);
        assert_eq!(state(&model), RunState::Ended(End::Exited(1)));

        ch(&mut model, 'j');
        running(&mut model);
        model.apply(Update::ActionEnded(End::Exited(0)));
        assert!(model.failed(&Target::Stack(0)));
        assert!(!model.failed(&Target::Stack(1)));

        ch(&mut model, 'k');
        running(&mut model);
        model.apply(Update::ActionEnded(End::Exited(0)));
        assert!(!model.failed(&Target::Stack(0)));
    }

    #[test]
    fn q_while_an_action_runs_asks_and_y_stops_it_and_quits_when_it_ends() {
        let mut model = opened(&["nobs-t-packt"]);
        running(&mut model);

        assert_eq!(ch(&mut model, 'q'), []);
        assert_eq!(model.question(), Some(&Question::Quit));
        assert_eq!(
            model.question().unwrap().text(),
            "an action is running: stop it and quit? y/n"
        );
        assert_eq!(ch(&mut model, 'n'), []);
        assert_eq!(model.notice(), None);
        assert!(!model.quit());

        ch(&mut model, 'q');
        assert_eq!(ch(&mut model, 'y'), [Command::Stop]);
        assert_eq!(state(&model), RunState::Stopping);
        assert!(!model.quit());
        model.apply(Update::ActionEnded(End::Stopped));
        assert!(model.quit());

        let mut model = opened(&["nobs-t-packt"]);
        running(&mut model);
        ch(&mut model, 'q');
        model.apply(Update::ActionEnded(End::Exited(0)));
        assert_eq!(ch(&mut model, 'y'), [Command::Quit]);
        assert!(model.quit());
    }

    #[test]
    fn j_and_k_move_between_the_chapters_of_the_lab_workbench() {
        let mut model = book_model();
        assert_eq!(model.workbench(), Workbench::Lab);
        assert_eq!(model.chapters(), [0, 1]);
        assert_eq!(model.selected_chapter(), Some(0));

        assert_eq!(ch(&mut model, 'j'), []);
        assert_eq!(model.selected_chapter(), Some(1));
        ch(&mut model, 'j');
        assert_eq!(model.selected_chapter(), Some(1));
        code(&mut model, KeyCode::Up);
        assert_eq!(model.selected_chapter(), Some(0));

        ch(&mut model, 'j');
        ch(&mut model, 'D');
        assert!(model.question().unwrap().text().ends_with(&format!(
            "-s{NO_BREAK}ch2? It takes the other chapters' services down. y/n"
        )));
        assert_eq!(opened(&["a"]).selected_chapter(), None);
    }

    fn nodes(names: &[&str]) -> Update {
        Update::Nodes(Ok(names
            .iter()
            .map(|name| book::NodeRow {
                name: name.to_string(),
                kind: "nokia_srlinux".to_string(),
                state: Some("running".to_string()),
            })
            .collect()))
    }

    #[test]
    fn tab_moves_the_lab_focus_and_j_k_move_only_the_pane_with_the_focus() {
        let mut model = book_model();
        model.apply(nodes(&["a", "b", "c"]));
        assert_eq!(model.focus(), Focus::List);

        ch(&mut model, 'j');
        assert_eq!((model.selected_chapter(), model.node_row()), (Some(1), 0));

        code(&mut model, KeyCode::Tab);
        assert_eq!(model.focus(), Focus::Tabs);
        ch(&mut model, 'j');
        assert_eq!((model.selected_chapter(), model.node_row()), (Some(1), 1));

        code(&mut model, KeyCode::Tab);
        assert_eq!(model.focus(), Focus::Lines);
        code(&mut model, KeyCode::Tab);
        assert_eq!(model.focus(), Focus::List);
        ch(&mut model, 'k');
        assert_eq!((model.selected_chapter(), model.node_row()), (Some(0), 1));

        code(&mut model, KeyCode::Tab);
        code(&mut model, KeyCode::Esc);
        assert_eq!(model.focus(), Focus::List);

        ch(&mut model, '1');
        code(&mut model, KeyCode::Tab);
        assert_eq!(model.focus(), Focus::Tabs);
        ch(&mut model, '3');
        assert_eq!(
            (model.workbench(), model.focus()),
            (Workbench::Lab, Focus::List)
        );
    }

    fn tabbed(model: &mut Model) -> Focus {
        code(model, KeyCode::Tab);
        model.focus()
    }

    #[test]
    fn tab_goes_from_the_chapters_to_the_nodes_to_the_overview_and_back() {
        let mut model = book_model();
        assert_eq!(model.focus(), Focus::List);

        let stops = [tabbed(&mut model), tabbed(&mut model), tabbed(&mut model)];

        assert_eq!(stops, [Focus::Tabs, Focus::Lines, Focus::List]);
    }

    #[test]
    fn tab_goes_from_the_stacks_to_the_scenarios_to_the_details_and_back() {
        let mut model = sonda_tab();
        code(&mut model, KeyCode::Esc);
        assert_eq!((model.focus(), model.tab()), (Focus::List, Tab::Sonda));

        let stops = [tabbed(&mut model), tabbed(&mut model), tabbed(&mut model)];

        assert_eq!(stops, [Focus::Tabs, Focus::Lines, Focus::List]);
    }

    #[test]
    fn j_and_k_on_a_pane_of_lines_move_no_row_and_keys_elsewhere_move_no_pane() {
        let mut model = book_model();
        model.apply(nodes(&["a", "b", "c"]));
        ch(&mut model, 'j');
        tabbed(&mut model);
        tabbed(&mut model);
        model.set_lab_overflow(3);

        ch(&mut model, 'j');
        code(&mut model, KeyCode::Down);
        ch(&mut model, 'k');
        assert_eq!(
            (
                model.selected_chapter(),
                model.node_row(),
                model.lab_scroll()
            ),
            (Some(1), 0, 1)
        );

        tabbed(&mut model);
        ch(&mut model, 'j');
        assert_eq!((model.selected_chapter(), model.lab_scroll()), (Some(1), 1));
        tabbed(&mut model);
        ch(&mut model, 'j');
        code(&mut model, KeyCode::Down);
        ch(&mut model, 'k');
        assert_eq!((model.node_row(), model.lab_scroll()), (1, 1));

        let mut model = sonda_tab();
        sonda_rows(&mut model);
        tabbed(&mut model);
        assert_eq!(ch(&mut model, 'j'), []);
        assert_eq!(code(&mut model, KeyCode::Down), []);
        assert_eq!(model.sonda().selected().unwrap().id, "a1");
    }

    #[test]
    fn the_action_keys_act_with_the_focus_on_a_pane_of_lines() {
        let mut model = book_model();
        ch(&mut model, 'j');
        tabbed(&mut model);
        tabbed(&mut model);
        ch(&mut model, 'D');
        assert!(model.question().unwrap().text().ends_with(&format!(
            "-s{NO_BREAK}ch2? It takes the other chapters' services down. y/n"
        )));

        let mut model = sonda_tab();
        sonda_rows(&mut model);
        tabbed(&mut model);
        assert_eq!(model.focus(), Focus::Lines);
        assert_eq!(
            ch(&mut model, 'r'),
            [Command::Sonda(data::Command::Refresh)]
        );
        ch(&mut model, 'l');
        ch(&mut model, 'j');
        assert_eq!(model.sonda().prompt(), &Prompt::Path("j".to_string()));
        code(&mut model, KeyCode::Esc);
        ch(&mut model, 'd');
        assert_eq!(model.sonda().prompt(), &Prompt::Delete("a1".to_string()));
        assert_eq!(
            ch(&mut model, 'y'),
            [Command::Sonda(data::Command::Delete("a1".to_string()))]
        );
        assert_eq!(model.focus(), Focus::Lines);
    }

    #[test]
    fn a_new_chapter_or_scenario_puts_the_pane_at_its_first_line_and_a_shorter_text_keeps_it_inside(
    ) {
        let mut model = book_model();
        tabbed(&mut model);
        tabbed(&mut model);
        model.set_lab_overflow(3);
        ch(&mut model, 'j');
        ch(&mut model, 'j');
        assert_eq!(model.lab_scroll(), 2);

        model.set_lab_overflow(1);
        assert_eq!(model.lab_scroll(), 1);

        code(&mut model, KeyCode::Esc);
        ch(&mut model, 'j');
        assert_eq!((model.selected_chapter(), model.lab_scroll()), (Some(1), 0));
    }

    #[test]
    fn tab_keeps_two_stops_where_no_pane_of_lines_stands_and_a_new_view_takes_the_focus_off_a_missing_pane(
    ) {
        let mut devices = devices_model(workshop_located("infrahub"));
        let mut overview = opened(&["a"]);
        with_services(&mut overview, &["grafana"]);
        let mut logs = opened(&["a"]);
        with_services(&mut logs, &["grafana"]);
        ch(&mut logs, 'l');
        assert_eq!((logs.focus(), logs.tab()), (Focus::Tabs, Tab::Logs));
        assert_eq!(tabbed(&mut logs), Focus::List);
        for model in [&mut devices, &mut overview] {
            assert_eq!(model.focus(), Focus::List);
            assert_eq!([tabbed(model), tabbed(model)], [Focus::Tabs, Focus::List]);
        }

        let mut model = sonda_tab();
        tabbed(&mut model);
        ch(&mut model, ']');
        assert_eq!((model.tab(), model.focus()), (Tab::Overview, Focus::Tabs));

        for (key, workbench) in [('1', Workbench::Devices), ('2', Workbench::Services)] {
            let mut model = book_model();
            tabbed(&mut model);
            tabbed(&mut model);
            ch(&mut model, key);
            assert_eq!((model.workbench(), model.focus()), (workbench, Focus::Tabs));
        }

        let mut model = book_model();
        ch(&mut model, '2');
        tabbed(&mut model);
        ch(&mut model, '[');
        assert_eq!(model.tab(), Tab::Sonda);
        ch(&mut model, '3');
        tabbed(&mut model);
        tabbed(&mut model);
        ch(&mut model, '2');
        assert_eq!(
            (model.workbench(), model.tab(), model.focus()),
            (Workbench::Services, Tab::Sonda, Focus::Lines)
        );
    }

    #[test]
    fn an_action_asks_about_the_selected_chapter_in_both_focuses() {
        let mut listed = book_model();
        ch(&mut listed, 'j');
        ch(&mut listed, 'D');
        let question = listed.question().unwrap().text();
        assert!(question.ends_with(&format!(
            "-s{NO_BREAK}ch2? It takes the other chapters' services down. y/n"
        )));
        let lab = book_dir();
        let deploy = nobs(&[
            "--dir",
            &lab,
            "lab",
            "deploy",
            "--sudo-no-prompt",
            "-s",
            "ch2",
        ]);
        assert_eq!(ch(&mut listed, 'y'), std::slice::from_ref(&deploy));

        let mut model = book_model();
        model.apply(nodes(&["a", "b", "c"]));
        ch(&mut model, 'j');
        code(&mut model, KeyCode::Tab);
        ch(&mut model, 'D');
        assert_eq!(model.question().unwrap().text(), question);
        ch(&mut model, 'n');
        assert_eq!(model.question(), None);
        ch(&mut model, 'D');
        assert_eq!(model.question().unwrap().text(), question);
        assert_eq!(ch(&mut model, 'y'), [deploy]);

        ch(&mut model, '?');
        model.set_help_overflow(5);
        ch(&mut model, 'j');
        assert_eq!((model.help_scroll(), model.node_row()), (1, 0));
        code(&mut model, KeyCode::Esc);

        let running = format!(
            "one action at a time: {} is running",
            model.run().unwrap().words
        );
        assert_eq!(model.focus(), Focus::Tabs);
        assert_eq!(ch(&mut model, 'D'), []);
        assert_eq!(model.notice(), Some(running.as_str()));
        code(&mut model, KeyCode::Tab);
        code(&mut model, KeyCode::Tab);
        assert_eq!(model.focus(), Focus::List);
        assert_eq!(ch(&mut model, 'D'), []);
        assert_eq!(model.notice(), Some(running.as_str()));
        assert_eq!(model.question(), None);
    }

    #[test]
    fn a_cursor_stays_on_a_row_that_exists() {
        let mut model = book_model();
        model.apply(nodes(&["b", "a"]));
        let names: Vec<&str> = model.nodes().iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);

        model.apply(nodes(&["a", "b", "c"]));
        code(&mut model, KeyCode::Tab);
        ch(&mut model, 'j');
        ch(&mut model, 'j');
        assert_eq!(model.node_row(), 2);
        model.apply(nodes(&["a"]));
        assert_eq!(model.node_row(), 0);
        model.apply(nodes(&[]));
        assert_eq!(model.node_row(), 0);

        let mut model = devices_model(workshop_located("infrahub"));
        let mut list = model.devices().to_vec();
        let first = list[0].interfaces[0].clone();
        let named = |count: usize| {
            (1..=count)
                .map(|index| {
                    let mut interface = first.clone();
                    interface.name = format!("e{index}");
                    interface
                })
                .collect::<Vec<_>>()
        };
        list[0].interfaces = named(5);
        list[1].interfaces = named(5);
        model.apply(Update::Devices(Ok(list.clone())));
        code(&mut model, KeyCode::Enter);
        ch(&mut model, ']');
        assert_eq!(model.tab(), Tab::Interfaces);
        for _ in 0..4 {
            ch(&mut model, 'j');
        }
        assert_eq!(model.interface_row(), 4);
        list[0].interfaces = named(2);
        model.apply(Update::Devices(Ok(list)));
        assert_eq!(model.interface_row(), 1);
        code(&mut model, KeyCode::Esc);
        ch(&mut model, 'j');
        assert_eq!(model.selected_device().unwrap().name, "srl2");
        assert_eq!(model.interface_row(), 0);

        let mut model = devices_model(workshop_located("infrahub"));
        model.apply(srl1_alerts(3));
        code(&mut model, KeyCode::Enter);
        ch(&mut model, ']');
        ch(&mut model, ']');
        assert_eq!(model.tab(), Tab::Timeline);
        ch(&mut model, 'j');
        ch(&mut model, 'j');
        assert_eq!(model.timeline_row(), 2);
        model.apply(srl1_alerts(1));
        assert_eq!(model.timeline().len(), 1);
        assert_eq!(model.timeline_row(), 0);
    }

    fn srl1_alerts(count: usize) -> Update {
        Update::Alerts(Ok((0..count)
            .map(|index| {
                alert(
                    "srl1",
                    &format!("a{index}"),
                    &format!("2026-11-09T13:5{index}:00Z"),
                )
            })
            .collect()))
    }

    #[test]
    fn k_in_the_timeline_moves_from_the_row_shown_after_entries_leave() {
        let mut model = devices_model(workshop_located("infrahub"));
        model.apply(srl1_alerts(4));
        code(&mut model, KeyCode::Enter);
        ch(&mut model, ']');
        ch(&mut model, ']');
        for _ in 0..3 {
            ch(&mut model, 'j');
        }
        assert_eq!(model.timeline_row(), 3);
        model.apply(srl1_alerts(2));
        assert_eq!(model.timeline_row(), 1);

        ch(&mut model, 'k');

        assert_eq!(model.timeline_row(), 0);
    }

    #[test]
    fn a_typed_filter_moves_the_cursor_only_off_a_hidden_row_and_starts_what_j_starts() {
        let on_beta = || {
            let mut model = opened(&["alpha", "beta", "gamma"]);
            ch(&mut model, 'j');
            model
        };
        let mut twin = on_beta();
        let by_j = ch(&mut twin, 'j');
        assert_eq!(by_j, [Command::SelectStack(2)]);
        let mut model = on_beta();
        ch(&mut model, '/');
        assert_eq!(ch(&mut model, 'g'), by_j);
        code(&mut model, KeyCode::Enter);
        assert_eq!(snapshot(&model), snapshot(&twin));
        ch(&mut model, '/');
        assert_eq!(code(&mut model, KeyCode::Esc), []);
        assert_eq!(model.selected_stack(), 2);

        let mut model = on_beta();
        ch(&mut model, '/');
        assert_eq!(ch(&mut model, 'a'), []);
        code(&mut model, KeyCode::Backspace);
        assert_eq!(ch(&mut model, 'z'), []);
        assert_eq!(ch(&mut model, 'z'), []);
        assert!(model.visible_stacks().is_empty());
        assert_eq!(model.selected_stack(), 1);

        let devices = || devices_model(workshop_located("infrahub"));
        let mut twin = devices();
        let by_j = ch(&mut twin, 'j');
        assert_eq!(by_j, [Command::SelectDevice("srl2".to_string())]);
        let mut model = devices();
        ch(&mut model, '/');
        assert_eq!(ch(&mut model, '2'), by_j);
        code(&mut model, KeyCode::Enter);
        assert_eq!(
            (snapshot(&model), model.cursor(), model.device),
            (snapshot(&twin), twin.cursor(), twin.device)
        );
        ch(&mut model, '/');
        assert_eq!(code(&mut model, KeyCode::Esc), []);
        ch(&mut model, '/');
        assert_eq!(ch(&mut model, 'z'), []);
        assert_eq!(ch(&mut model, 'z'), []);
        assert!(model.visible_devices().is_empty());
        assert_eq!(model.cursor(), Cursor::Device(1));

        let mut model = devices();
        ch(&mut model, 'j');
        ch(&mut model, 'j');
        assert_eq!(model.cursor(), Cursor::Alert(0));
        ch(&mut model, '/');
        assert_eq!(ch(&mut model, '1'), []);
        assert_eq!(model.cursor(), Cursor::Alert(0));
    }

    #[test]
    fn a_new_device_list_that_hides_the_cursor_row_under_a_kept_filter_moves_it_and_tells_the_pollers(
    ) {
        let mut model = devices_model(workshop_located("infrahub"));
        ch(&mut model, 'j');
        ch(&mut model, '/');
        typed(&mut model, "srl2");
        code(&mut model, KeyCode::Enter);
        assert_eq!(model.cursor(), Cursor::Device(1));
        let mut list = model.devices().to_vec();
        let mut srl0 = list[0].clone();
        srl0.name = "srl0".to_string();
        list.insert(0, srl0);

        let commands = model.apply(Update::Devices(Ok(list)));

        assert_eq!(commands, [Command::SelectDevice("srl2".to_string())]);
        assert_eq!(model.cursor(), Cursor::Device(2));
        assert_eq!(model.selected_device().unwrap().name, "srl2");
    }

    #[test]
    fn the_last_alert_leaving_under_a_kept_filter_moves_the_cursor_where_k_moves_it() {
        let on_first_alert = || {
            let mut model = devices_model(workshop_located("infrahub"));
            ch(&mut model, 'j');
            ch(&mut model, 'j');
            assert_eq!(model.cursor(), Cursor::Alert(0));
            ch(&mut model, '/');
            ch(&mut model, '1');
            code(&mut model, KeyCode::Enter);
            model
        };
        let mut twin = on_first_alert();
        let by_k = ch(&mut twin, 'k');
        let mut model = on_first_alert();

        let commands = model.apply(Update::Alerts(Ok(Vec::new())));

        assert_eq!(commands, [Command::SelectDevice("srl1".to_string())]);
        assert_eq!(commands, by_k);
        assert_eq!(model.cursor(), Cursor::Device(0));
        assert_eq!(model.cursor(), twin.cursor());
        assert_eq!(model.selected_device().unwrap().name, "srl1");
    }

    #[test]
    fn the_last_alert_leaving_while_the_filter_shows_no_device_puts_the_cursor_on_a_shown_row_when_it_widens(
    ) {
        let mut model = devices_model(workshop_located("infrahub"));
        ch(&mut model, 'j');
        ch(&mut model, 'j');
        assert_eq!(model.cursor(), Cursor::Alert(0));
        ch(&mut model, '/');
        ch(&mut model, 'z');
        assert!(model.visible_devices().is_empty());

        model.apply(Update::Alerts(Ok(Vec::new())));
        code(&mut model, KeyCode::Backspace);

        let Cursor::Device(at) = model.cursor() else {
            panic!("cursor not on a device: {:?}", model.cursor());
        };
        assert!(model.visible_devices().contains(&at));
    }

    #[test]
    fn the_last_alert_leaving_after_enter_on_it_keeps_the_selected_device_and_the_open_box() {
        let mut model = devices_model(workshop_located("infrahub"));
        model.apply(srl1_alerts(1));
        ch(&mut model, 'j');
        ch(&mut model, 'j');
        code(&mut model, KeyCode::Enter);
        assert_eq!((model.focus(), model.tab()), (Focus::Tabs, Tab::Timeline));
        assert_eq!(model.selected_device().unwrap().name, "srl1");
        code(&mut model, KeyCode::Enter);
        let entry = model.popup_entry().cloned();
        assert!(entry.is_some());

        let commands = model.apply(Update::Alerts(Ok(Vec::new())));

        assert_eq!(commands, []);
        assert_eq!(model.popup_entry().cloned(), entry);
        assert_eq!(model.selected_device().unwrap().name, "srl1");
        assert_eq!(model.cursor(), Cursor::Device(0));
    }

    #[test]
    fn nodes_arrive_and_a_failure_goes_to_the_error_row_and_keeps_them() {
        let mut model = book_model();
        let at = Instant::now();
        model.tick(at, UNIX_EPOCH, (80, 24));
        let rows = vec![book::NodeRow {
            name: "srl1".to_string(),
            kind: "nokia_srlinux".to_string(),
            state: Some("running".to_string()),
        }];

        model.apply(Update::Nodes(Ok(rows.clone())));
        assert_eq!(model.nodes(), rows);

        model.apply(Update::Nodes(Err("docker is gone".to_string())));
        assert_eq!(model.error(), Some(("nodes", "docker is gone", at)));
        assert_eq!(model.nodes(), rows);
        model.apply(Update::Nodes(Ok(rows)));
        assert_eq!(model.error(), None);
    }

    const CONFIG_LINES: [&str; 2] = [
        "set / acl acl-filter cpm type ipv4 statistics-per-entry true",
        "set / acl acl-filter cpm type ipv4 entry 10 match ipv4 protocol icmp",
    ];
    const FAILED_GETS: [&str; 3] = [
        "nobs: ssh srl1: authentication failed",
        "nobs: ssh ceos1: no driver for the kind ceos yet",
        "nobs: ssh srl1: set NETWORK_AGENT_USER and NETWORK_AGENT_PASSWORD in the lab's .env file or in the shell",
    ];
    const FIRST_FETCH: &str = "13:40:51";

    fn at_clock(model: &mut Model, text: &str) {
        let seconds = text
            .split(':')
            .fold(0, |total, part| total * 60 + part.parse::<u64>().unwrap());
        model.tick(
            Instant::now(),
            UNIX_EPOCH + Duration::from_secs(seconds),
            (100, 30),
        );
    }

    fn answered(model: &mut Model, lines: &[&str], end: End) {
        for line in lines {
            model.apply(Update::ActionLine(line.to_string()));
        }
        model.apply(Update::ActionEnded(end));
    }

    fn fetched(model: &mut Model, lines: &[&str], clock: &str) {
        at_clock(model, clock);
        ch(model, 'g');
        answered(model, lines, End::Exited(0));
    }

    fn kept(lines: &[&str], fetched: &str) -> Option<Config> {
        Some(Config {
            lines: lines.iter().map(ToString::to_string).collect(),
            fetched: fetched.to_string(),
        })
    }

    #[test]
    fn g_in_a_lab_with_nodes_runs_device_fetch_of_the_selected_device() {
        let lab = book_dir();
        let mut model = lab_devices_model();

        let commands = ch(&mut model, 'g');

        assert_eq!(
            commands,
            [
                nobs(&["--dir", &lab, "device", "fetch", "nobs-t-srl1"]),
                Command::OpenTab(Tab::Config)
            ]
        );
        assert_eq!(model.question(), None);
        assert_eq!(model.run().unwrap().words, "device fetch nobs-t-srl1");
        answered(&mut model, &[], End::Exited(0));
        ch(&mut model, 'j');
        assert_eq!(
            ch(&mut model, 'g'),
            [
                nobs(&["--dir", &lab, "device", "fetch", "nobs-t-srl2"]),
                Command::OpenTab(Tab::Config)
            ]
        );
    }

    #[test]
    fn g_in_a_project_lab_runs_device_fetch_of_the_selected_device() {
        let lab = fixture("project-two-node").to_string_lossy().into_owned();
        let mut model = lab_model("project-two-node", "project-two-node/expected/lab_vars.yml");
        assert!(matches!(model.located().unwrap().kind, Kind::LabcliLab(_)));

        assert_eq!(
            ch(&mut model, 'g'),
            [
                nobs(&["--dir", &lab, "device", "fetch", "nobs-t-srl1"]),
                Command::OpenTab(Tab::Config)
            ]
        );
    }

    #[test]
    fn a_failed_get_shows_the_command_line_word_for_word_and_exit_1() {
        for line in FAILED_GETS {
            let mut model = lab_devices_model();
            fetched(&mut model, &CONFIG_LINES, FIRST_FETCH);
            at_clock(&mut model, "13:45:00");

            ch(&mut model, 'g');
            model.apply(Update::ActionError(line.to_string()));
            model.apply(Update::ActionEnded(End::Exited(1)));

            let run = model.run().unwrap();
            assert_eq!(run.lines, [line]);
            assert_eq!(run.state, RunState::Ended(End::Exited(1)));
            assert_eq!(
                model.config_of("nobs-t-srl1").cloned(),
                kept(&CONFIG_LINES, FIRST_FETCH)
            );
        }
    }

    #[test]
    fn with_a_run_going_g_and_s_give_the_one_action_notice_and_open_nothing() {
        let mut model = lab_devices_model();
        ch(&mut model, 'g');
        code(&mut model, KeyCode::Enter);
        assert_eq!(model.tab(), Tab::Overview);

        assert_eq!(ch(&mut model, 'g'), []);

        assert_eq!(
            model.notice(),
            Some("one action at a time: device fetch nobs-t-srl1 is running")
        );
        assert_eq!(model.tab(), Tab::Overview);
        assert_eq!(model.run().unwrap().words, "device fetch nobs-t-srl1");
        assert_eq!(ch(&mut model, 's'), []);
        assert_eq!(
            model.notice(),
            Some("one action at a time: device fetch nobs-t-srl1 is running")
        );
        assert_eq!(model.entry(), None);
        assert_eq!(model.run().unwrap().words, "device fetch nobs-t-srl1");

        for key in ['g', 's'] {
            let mut model = lab_devices_model();
            ch(&mut model, '2');
            running(&mut model);
            ch(&mut model, '1');
            assert_eq!(ch(&mut model, key), [], "{key}");
            assert_eq!(
                model.notice(),
                Some("one action at a time: a up is running"),
                "{key}"
            );
            assert_eq!(model.tab(), Tab::Overview, "{key}");
            assert_eq!(model.entry(), None, "{key}");
            assert_eq!(model.run().unwrap().words, "a up", "{key}");
        }
    }

    #[test]
    fn a_good_get_keeps_every_line_in_order_with_time_and_count() {
        let lines: Vec<String> = (0..2005)
            .map(|entry| {
                format!("set / acl acl-filter cpm type ipv4 entry {entry} match ipv4 protocol icmp")
            })
            .collect();
        let shown: Vec<&str> = lines.iter().map(String::as_str).collect();
        let mut model = lab_devices_model();

        fetched(&mut model, &shown, FIRST_FETCH);

        let config = model.config_of("nobs-t-srl1").unwrap();
        assert_eq!(config.lines.len(), 2005);
        assert_eq!(config.lines[0], lines[0]);
        assert!(config.lines == lines, "the lines are not in order");
        assert_eq!(config.fetched, FIRST_FETCH);
        assert_eq!(model.run().unwrap().lines.len(), LOG_LINES);
    }

    #[test]
    fn a_failed_or_stopped_get_leaves_the_tab_as_it_was() {
        for end in [
            End::Exited(1),
            End::Stopped,
            End::NotStopped { group: 4242 },
        ] {
            let mut model = lab_devices_model();
            at_clock(&mut model, FIRST_FETCH);
            ch(&mut model, 'g');
            answered(&mut model, &CONFIG_LINES[..1], end);
            assert_eq!(model.config_of("nobs-t-srl1"), None, "{end:?}");

            fetched(&mut model, &CONFIG_LINES, FIRST_FETCH);
            at_clock(&mut model, "13:45:00");
            ch(&mut model, 'g');
            answered(&mut model, &CONFIG_LINES[1..], end);

            assert_eq!(
                model.config_of("nobs-t-srl1").cloned(),
                kept(&CONFIG_LINES, FIRST_FETCH),
                "{end:?}"
            );
        }
    }

    #[test]
    fn error_text_never_reaches_the_tab() {
        let mut model = lab_devices_model();
        at_clock(&mut model, FIRST_FETCH);
        ch(&mut model, 'g');

        model.apply(Update::ActionLine(CONFIG_LINES[0].to_string()));
        model.apply(Update::ActionError(FAILED_GETS[0].to_string()));
        model.apply(Update::ActionLine(CONFIG_LINES[1].to_string()));
        model.apply(Update::ActionEnded(End::Exited(0)));

        assert_eq!(
            model.config_of("nobs-t-srl1").cloned(),
            kept(&CONFIG_LINES, FIRST_FETCH)
        );
        assert_eq!(
            model.run().unwrap().lines,
            [CONFIG_LINES[0], FAILED_GETS[0], CONFIG_LINES[1]]
        );
    }

    #[test]
    fn an_answer_lands_on_the_device_it_was_asked_from() {
        let mut model = lab_devices_model();
        at_clock(&mut model, FIRST_FETCH);
        ch(&mut model, 'g');

        ch(&mut model, 'j');
        assert_eq!(model.selected_device().unwrap().name, "nobs-t-srl2");
        answered(&mut model, &CONFIG_LINES, End::Exited(0));

        assert_eq!(
            model.config_of("nobs-t-srl1").cloned(),
            kept(&CONFIG_LINES, FIRST_FETCH)
        );
        assert_eq!(model.config_of("nobs-t-srl2"), None);
    }

    #[test]
    fn two_devices_keep_their_own_configuration() {
        let mut model = lab_devices_model();
        fetched(&mut model, &CONFIG_LINES[..1], FIRST_FETCH);
        ch(&mut model, 'j');
        fetched(&mut model, &CONFIG_LINES[1..], "13:42:07");
        ch(&mut model, 'k');

        assert_eq!(
            model.config_of("nobs-t-srl1").cloned(),
            kept(&CONFIG_LINES[..1], FIRST_FETCH)
        );
        assert_eq!(
            model.config_of("nobs-t-srl2").cloned(),
            kept(&CONFIG_LINES[1..], "13:42:07")
        );
    }

    #[test]
    fn in_a_workshop_g_and_s_do_nothing_and_are_not_shown() {
        let source = workshop_source();
        let mut no_lab = Model::new(Start {
            source: Some(source.clone()),
            home: Workbench::Devices,
            chooser: false,
            ..start(vec![stack("a", &[])])
        });
        no_lab.apply(Update::Devices(Ok(source.devices().unwrap())));
        let mut no_device = lab_devices_model();
        no_device.apply(Update::Devices(Ok(Vec::new())));
        let models = [
            ("workshop", devices_model(workshop_located("infrahub"))),
            ("no lab", no_lab),
            ("no device", no_device),
        ];
        for (name, mut model) in models {
            for (key, hint) in [('g', "g get config"), ('s', "s run command")] {
                let tab = model.tab();

                assert_eq!(ch(&mut model, key), [], "{name} {key}");

                assert!(model.run().is_none(), "{name} {key}");
                assert_eq!(model.question(), None, "{name} {key}");
                assert_eq!(model.entry(), None, "{name} {key}");
                assert_eq!(model.notice(), None, "{name} {key}");
                assert_eq!(model.tab(), tab, "{name} {key}");
                let line = key_line(&model, 200).concat();
                assert!(!line.contains(hint), "{name}: {line}");
                if name != "no device" {
                    let entries = help_entries(&model);
                    assert!(
                        entries.iter().all(|(seen, _)| *seen != key.to_string()),
                        "{name}: {entries:?}"
                    );
                }
            }
        }
        let line = key_line(&lab_devices_model(), 200).concat();
        assert!(line.contains("g get config  s run command"), "{line}");

        let mut busy = devices_model(workshop_located("infrahub"));
        ch(&mut busy, 'm');
        ch(&mut busy, 'y');
        let words = busy.run().unwrap().words.clone();
        for key in ['g', 's'] {
            assert_eq!(ch(&mut busy, key), [], "run going {key}");
            assert_eq!(busy.notice(), None, "run going {key}");
            assert_eq!(busy.entry(), None, "run going {key}");
            assert_eq!(busy.run().unwrap().words, words, "run going {key}");
        }
    }

    #[test]
    fn in_a_book_lab_m_with_a_run_going_gives_no_notice_and_starts_nothing() {
        let mut model = lab_devices_model();
        ch(&mut model, 'g');

        assert_eq!(ch(&mut model, 'm'), []);

        assert_eq!(model.notice(), None);
        assert_eq!(model.question(), None);
        assert_eq!(model.run().unwrap().words, "device fetch nobs-t-srl1");
    }

    #[test]
    fn g_opens_the_config_tab_and_a_refused_g_does_not() {
        let mut model = lab_devices_model();
        assert_eq!((model.focus(), model.tab()), (Focus::List, Tab::Overview));

        let commands = ch(&mut model, 'g');

        assert_eq!(commands.last(), Some(&Command::OpenTab(Tab::Config)));
        assert_eq!((model.focus(), model.tab()), (Focus::List, Tab::Config));
        assert!(model.run_pane());

        code(&mut model, KeyCode::Enter);
        assert_eq!(model.tab(), Tab::Overview);
        assert_eq!(ch(&mut model, 'g'), []);
        assert_eq!(model.tab(), Tab::Overview);

        answered(&mut model, &CONFIG_LINES, End::Exited(0));
        for expected in [Tab::Interfaces, Tab::Config, Tab::Timeline] {
            assert_eq!(
                ch(&mut model, ']').last(),
                Some(&Command::OpenTab(expected))
            );
        }
    }

    fn entries(count: usize) -> Vec<String> {
        (1..=count)
            .map(|entry| {
                format!(
                    "set / acl acl-filter cpm type ipv4 entry {} match ipv4 protocol icmp",
                    entry * 10
                )
            })
            .collect()
    }

    fn on_config(lines: &[String]) -> Model {
        let mut model = lab_devices_model();
        let shown: Vec<&str> = lines.iter().map(String::as_str).collect();
        fetched(&mut model, &shown, FIRST_FETCH);
        code(&mut model, KeyCode::Enter);
        ch(&mut model, ']');
        ch(&mut model, ']');
        assert_eq!((model.focus(), model.tab()), (Focus::Tabs, Tab::Config));
        model
    }

    fn scroll(model: &Model) -> usize {
        model.config().unwrap().3
    }

    #[test]
    fn the_config_pane_moves_filters_and_counts_hidden_lines() {
        let lines = entries(40);
        let mut model = on_config(&lines);

        ch(&mut model, 'k');
        assert_eq!(scroll(&model), 0);
        ch(&mut model, 'j');
        code(&mut model, KeyCode::Down);
        assert_eq!(scroll(&model), 2);
        code(&mut model, KeyCode::Up);
        assert_eq!(scroll(&model), 1);
        for _ in 0..50 {
            ch(&mut model, 'j');
        }
        assert_eq!(scroll(&model), 39);

        ch(&mut model, '/');
        assert!(model.typing());
        typed(&mut model, "entry 1");
        assert_eq!(model.typed_filter(), Some("entry 1"));
        code(&mut model, KeyCode::Enter);
        assert!(!model.typing());
        let (device, kept, shown, _) = model.config().unwrap();
        assert_eq!(device, "nobs-t-srl1");
        assert_eq!(kept.unwrap().lines.len(), 40);
        let wanted: Vec<&str> = lines
            .iter()
            .map(String::as_str)
            .filter(|line| line.contains("entry 1"))
            .collect();
        assert_eq!(shown, wanted);
        assert_eq!(shown.len(), 11);
        ch(&mut model, 'j');
        assert_eq!(scroll(&model), 10);
    }

    #[test]
    fn q_while_the_config_filter_types_is_text_and_does_not_quit() {
        let mut model = on_config(&entries(3));
        ch(&mut model, '/');

        assert_eq!(ch(&mut model, 'q'), []);

        assert!(!model.quit());
        assert_eq!(model.typed_filter(), Some("q"));
    }

    #[test]
    fn help_names_g_and_s_in_a_lab_with_nodes_and_m_as_today() {
        let named = |model: &Model, key: &str| {
            help_entries(model)
                .iter()
                .filter(|(keys, _)| keys == key)
                .map(|(_, meaning)| *meaning)
                .collect::<Vec<_>>()
        };
        let lab = lab_devices_model();
        assert_eq!(named(&lab, "g"), ["get config"]);
        assert_eq!(named(&lab, "s"), ["run command"]);
        assert_eq!(named(&lab, "m"), ["maintenance"]);

        let workshop = devices_model(workshop_located("infrahub"));
        assert_eq!(named(&workshop, "g"), Vec::<&str>::new());
        assert_eq!(named(&workshop, "s"), Vec::<&str>::new());
        assert_eq!(named(&workshop, "m"), ["maintenance"]);
    }

    #[test]
    fn the_config_scroll_goes_back_to_the_top_on_another_device() {
        let mut model = on_config(&entries(10));
        ch(&mut model, 'j');
        assert_eq!(scroll(&model), 1);

        code(&mut model, KeyCode::Esc);
        code(&mut model, KeyCode::Esc);
        assert_eq!(model.focus(), Focus::List);
        ch(&mut model, 'j');

        assert_eq!(model.config().unwrap().0, "nobs-t-srl2");
        assert_eq!(scroll(&model), 0);
    }

    #[test]
    fn s_runs_device_run_with_the_text_as_one_argument() {
        let lab = book_dir();
        let mut model = lab_devices_model();

        assert_eq!(ch(&mut model, 's'), []);
        assert_eq!(model.entry(), Some(("nobs-t-srl1", "")));
        typed(&mut model, "show version");
        assert_eq!(model.entry(), Some(("nobs-t-srl1", "show version")));
        let commands = code(&mut model, KeyCode::Enter);

        assert_eq!(
            commands,
            [nobs(&[
                "--dir",
                &lab,
                "device",
                "run",
                "nobs-t-srl1",
                "show version"
            ])]
        );
        assert_eq!(model.entry(), None);
        assert!(!model.typing());
        assert_eq!(model.tab(), Tab::Overview);
        assert_eq!(
            model.run().unwrap().words.replace(NO_BREAK, " "),
            "device run nobs-t-srl1 show version"
        );
    }

    #[test]
    fn the_entry_of_s_takes_every_key_as_text() {
        let mut model = lab_devices_model();
        model.apply(every_service_up(0));
        model.apply(Update::Alerts(Err("refused".to_string())));
        ch(&mut model, 's');
        let before = (model.workbench(), model.focus(), model.tab());

        for typed in "q g x ? 1  r o".chars() {
            assert_eq!(ch(&mut model, typed), [], "{typed}");
        }

        assert!(!model.quit());
        assert!(model.run().is_none());
        assert!(!model.help());
        assert!(!model.run_pane());
        assert!(model.error().is_some());
        assert_eq!((model.workbench(), model.focus(), model.tab()), before);
        assert_eq!(model.entry(), Some(("nobs-t-srl1", "q g x ? 1  r o")));
        let commands = code(&mut model, KeyCode::Enter);
        let Some(Command::Run { args, .. }) = commands.first() else {
            panic!("{commands:?}");
        };
        assert_eq!(commands.len(), 1);
        assert_eq!(args.last(), Some(&OsString::from("q g x ? 1  r o")));
    }

    #[test]
    fn esc_and_an_empty_text_run_nothing() {
        for keys in [
            vec![KeyCode::Char('s'), KeyCode::Char('w'), KeyCode::Esc],
            vec![KeyCode::Char('s'), KeyCode::Enter],
            vec![
                KeyCode::Char('s'),
                KeyCode::Char('w'),
                KeyCode::Backspace,
                KeyCode::Enter,
            ],
        ] {
            let mut model = lab_devices_model();
            let commands: Vec<Command> =
                keys.iter().flat_map(|key| code(&mut model, *key)).collect();

            assert_eq!(commands, [], "{keys:?}");
            assert!(model.run().is_none(), "{keys:?}");
            assert_eq!(model.entry(), None, "{keys:?}");
            assert!(!model.typing(), "{keys:?}");
            assert_eq!(model.question(), None, "{keys:?}");
        }
    }

    fn asking(user: Option<&'static str>, password: Option<&'static str>) -> Model {
        lab_model_finding(
            "book-lab",
            "book-lab/containerlab/lab_vars.yml",
            finding(user, password),
        )
    }

    fn login(user: Option<&str>, password: Option<&str>) -> Login {
        Login {
            user: user.map(str::to_string),
            password: password.map(|text| Password::new(text.to_string())),
        }
    }

    fn entered(model: &mut Model, text: &str) -> Vec<Command> {
        typed(model, text);
        code(model, KeyCode::Enter)
    }

    fn logged_in(model: &mut Model) -> Vec<Command> {
        let mut commands = ch(model, 'g');
        commands.extend(entered(model, USER));
        commands.extend(entered(model, SECRET));
        commands
    }

    fn fetch(lab: &str, login: Login) -> Vec<Command> {
        vec![
            Command::Run {
                program: PathBuf::from("/bin/nobs"),
                args: os(&["--dir", lab, "device", "fetch", "nobs-t-srl1"]),
                login,
            },
            Command::OpenTab(Tab::Config),
        ]
    }

    fn device_run(lab: &str, login: Login) -> Command {
        Command::Run {
            program: PathBuf::from("/bin/nobs"),
            args: os(&["--dir", lab, "device", "run", "nobs-t-srl1", "show version"]),
            login,
        }
    }

    fn refusal() -> String {
        device::error_line("nobs-t-srl1", &device::Error::AuthenticationFailed)
    }

    const USER_STEP: &str = " user for the devices:   Enter next  Esc cancel";

    fn frame(model: &Model) -> String {
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| crate::screen::draw(frame, model))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn g_without_a_login_asks_the_user_then_the_password_then_runs_with_the_login() {
        let lab = book_dir();
        let mut model = asking(None, None);

        assert_eq!(ch(&mut model, 'g'), []);
        assert!(model.run().is_none());
        assert_eq!(model.question(), None);
        assert_eq!(model.entry(), None);
        assert_eq!(key_line(&model, 100), [USER_STEP]);
        assert_eq!(entered(&mut model, USER), []);
        assert!(model.run().is_none());
        assert_eq!(
            key_line(&model, 100),
            [" password for nobs-t-x:   Enter run  Esc cancel"]
        );

        let commands = entered(&mut model, SECRET);

        assert_eq!(commands, fetch(&lab, login(Some(USER), Some(SECRET))));
        assert_eq!(model.tab(), Tab::Config);
        assert_eq!(model.run().unwrap().words, "device fetch nobs-t-srl1");
        assert!(!model.typing());
    }

    #[test]
    fn s_without_a_login_asks_the_login_then_the_command_then_runs() {
        let lab = book_dir();
        let mut model = asking(None, None);

        assert_eq!(ch(&mut model, 's'), []);
        assert_eq!(key_line(&model, 100), [USER_STEP]);
        assert_eq!(entered(&mut model, USER), []);
        assert_eq!(
            key_line(&model, 100),
            [" password for nobs-t-x:   Enter next  Esc cancel"]
        );
        assert_eq!(entered(&mut model, SECRET), []);
        assert_eq!(model.entry(), Some(("nobs-t-srl1", "")));
        assert!(model.run().is_none());

        let commands = entered(&mut model, "show version");

        assert_eq!(
            commands,
            [device_run(&lab, login(Some(USER), Some(SECRET)))]
        );
    }

    #[test]
    fn only_the_missing_part_is_asked_and_a_whole_login_asks_nothing() {
        let lab = book_dir();
        let mut user_found = asking(Some(USER), None);
        assert_eq!(ch(&mut user_found, 'g'), []);
        assert_eq!(
            key_line(&user_found, 100),
            [" password for nobs-t-x:   Enter run  Esc cancel"]
        );
        assert_eq!(
            entered(&mut user_found, SECRET),
            fetch(&lab, login(None, Some(SECRET)))
        );

        let mut password_found = asking(None, Some(SECRET));
        assert_eq!(ch(&mut password_found, 'g'), []);
        assert_eq!(
            key_line(&password_found, 100),
            [" user for the devices:   Enter run  Esc cancel"]
        );
        assert_eq!(
            entered(&mut password_found, USER),
            fetch(&lab, login(Some(USER), None))
        );

        let mut whole = asking(Some(USER), Some(SECRET));
        assert_eq!(ch(&mut whole, 'g'), fetch(&lab, Login::default()));
        let mut whole = asking(Some(USER), Some(SECRET));
        assert_eq!(ch(&mut whole, 's'), []);
        assert_eq!(whole.entry(), Some(("nobs-t-srl1", "")));
    }

    #[test]
    fn the_password_shows_one_dot_each_and_reaches_no_frame_argument_line_notice_or_debug() {
        let mut model = asking(None, None);
        let mut frames = Vec::new();
        let mut commands = ch(&mut model, 'g');
        commands.extend(entered(&mut model, USER));
        for letter in SECRET.chars() {
            commands.extend(ch(&mut model, letter));
            frames.push(frame(&model));
        }
        let dots = "•".repeat(SECRET.chars().count());
        assert_eq!(
            key_line(&model, 100),
            [format!(
                " password for nobs-t-x: {dots}  Enter run  Esc cancel"
            )]
        );
        assert!(frames.last().unwrap().contains(&dots));

        commands.extend(code(&mut model, KeyCode::Enter));
        frames.push(frame(&model));
        commands.extend(ch(&mut model, 'g'));
        let notice = model.notice().unwrap_or_default().to_string();
        model.apply(Update::ActionError(refusal()));
        model.apply(Update::ActionEnded(End::Exited(1)));
        frames.push(frame(&model));

        let args: Vec<OsString> = commands
            .iter()
            .filter_map(|command| match command {
                Command::Run { args, .. } => Some(args.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(args.len(), 5);
        assert!(notice.starts_with("one action at a time"), "{notice}");
        for text in [
            frames.concat(),
            format!("{commands:?}"),
            format!("{args:?}"),
            notice,
            format!("{:?}", model.run()),
            model.run().unwrap().lines.concat(),
        ] {
            assert!(!text.contains(SECRET), "{text}");
        }
    }

    #[test]
    fn a_typed_login_the_device_did_not_refuse_serves_every_later_g_and_s() {
        let lab = book_dir();
        let typed = login(Some(USER), Some(SECRET));
        for end in [End::Exited(0), End::Exited(2), End::Stopped] {
            let mut model = asking(None, None);
            assert_eq!(logged_in(&mut model), fetch(&lab, typed.clone()));
            answered(&mut model, &["Hostname : srl1"], end);

            assert_eq!(ch(&mut model, 'g'), fetch(&lab, typed.clone()), "{end:?}");
            answered(&mut model, &[], End::Exited(0));
            assert_eq!(ch(&mut model, 's'), [], "{end:?}");
            assert_eq!(model.entry(), Some(("nobs-t-srl1", "")), "{end:?}");
            assert_eq!(
                entered(&mut model, "show version"),
                [device_run(&lab, typed.clone())],
                "{end:?}"
            );
        }
    }

    #[test]
    fn a_refused_typed_login_is_forgotten_and_no_other_end_forgets_it() {
        let lab = book_dir();
        let mut model = asking(None, None);
        logged_in(&mut model);
        model.apply(Update::ActionError(refusal()));
        model.apply(Update::ActionEnded(End::Exited(1)));
        assert_eq!(model.run().unwrap().lines, [refusal()]);

        assert_eq!(ch(&mut model, 'g'), []);
        assert_eq!(key_line(&model, 100), [USER_STEP]);

        let unreachable = device::Error::Unreachable {
            host: "clab-nobs-t-dev-nobs-t-srl1".to_string(),
            reason: "timed out".to_string(),
        };
        let kept = [
            (
                device::error_line("nobs-t-srl1", &unreachable),
                End::Exited(1),
            ),
            (
                device::error_line("nobs-t-srl1", &device::Error::NoAnswer),
                End::Exited(1),
            ),
            (
                device::error_line("nobs-t-srl1", &device::Error::NoDriver("ceos".into())),
                End::Exited(1),
            ),
            (
                device::error_line("nobs-t-srl1", &device::Error::MissingCredentials),
                End::Exited(1),
            ),
            (
                "nobs: cannot run nobs: fake launcher failure".to_string(),
                End::Exited(1),
            ),
            ("Parsing error: Unknown token".to_string(), End::Exited(1)),
            (refusal(), End::Stopped),
            (refusal(), End::Exited(2)),
            (
                device::error_line("nobs-t-srl2", &device::Error::AuthenticationFailed),
                End::Exited(1),
            ),
        ];
        for (line, end) in kept {
            let mut model = asking(None, None);
            logged_in(&mut model);
            model.apply(Update::ActionError(line.clone()));
            model.apply(Update::ActionEnded(end));

            assert_eq!(
                ch(&mut model, 'g'),
                fetch(&lab, login(Some(USER), Some(SECRET))),
                "{line} {end:?}"
            );
        }
        let kept = [
            vec![Update::ActionLine(
                "Error: authentication failed".to_string(),
            )],
            vec![
                Update::ActionError(refusal()),
                Update::ActionLine("Hostname : srl1".to_string()),
            ],
        ];
        for updates in kept {
            let mut model = asking(None, None);
            logged_in(&mut model);
            let seen = format!("{updates:?}");
            for update in updates {
                model.apply(update);
            }
            model.apply(Update::ActionEnded(End::Exited(1)));

            assert_eq!(
                ch(&mut model, 'g'),
                fetch(&lab, login(Some(USER), Some(SECRET))),
                "{seen}"
            );
        }
    }

    #[test]
    fn a_refused_login_found_in_the_shell_or_the_files_opens_nothing_and_shows_the_line_each_time()
    {
        let lab = book_dir();
        let mut model = asking(Some(USER), Some(SECRET));
        for round in 0..2 {
            assert_eq!(
                ch(&mut model, 'g'),
                fetch(&lab, Login::default()),
                "{round}"
            );
            model.apply(Update::ActionError(refusal()));
            model.apply(Update::ActionEnded(End::Exited(1)));

            assert_eq!(model.run().unwrap().lines, [refusal()], "{round}");
            assert!(model.login_entry().is_none(), "{round}");
            assert!(!model.typing(), "{round}");
        }
    }

    #[test]
    fn at_each_login_step_the_keys_do_what_they_do_in_the_entry_of_s() {
        for password_step in [false, true] {
            let open = || {
                let mut model = asking(None, None);
                ch(&mut model, 'g');
                if password_step {
                    entered(&mut model, USER);
                }
                model
            };
            let mut model = open();
            model.apply(every_service_up(0));
            model.apply(Update::Alerts(Err("refused".to_string())));
            let before = (model.workbench(), model.focus(), model.tab());
            for letter in "q g x ? 1  r o".chars() {
                assert_eq!(ch(&mut model, letter), [], "{letter}");
            }
            for key in [KeyCode::Tab, KeyCode::Up, KeyCode::Backspace] {
                assert_eq!(code(&mut model, key), [], "{key:?}");
            }
            assert!(!model.quit());
            assert!(!model.help());
            assert!(model.run().is_none());
            assert!(model.typing());
            assert!(model.error().is_some());
            assert_eq!((model.workbench(), model.focus(), model.tab()), before);
            let shown = if password_step {
                "•".repeat(13)
            } else {
                "q g x ? 1  r ".to_string()
            };
            let line = key_line(&model, 100).concat();
            assert!(line.contains(&format!(": {shown}  Enter")), "{line}");

            for keys in [
                vec![KeyCode::Esc],
                vec![KeyCode::Char('w'), KeyCode::Esc],
                vec![KeyCode::Enter],
                vec![KeyCode::Char('w'), KeyCode::Backspace, KeyCode::Enter],
            ] {
                let mut model = open();
                let commands: Vec<Command> =
                    keys.iter().flat_map(|key| code(&mut model, *key)).collect();

                assert_eq!(commands, [], "{password_step} {keys:?}");
                assert!(model.run().is_none(), "{password_step} {keys:?}");
                assert!(!model.typing(), "{password_step} {keys:?}");
                assert!(model.login_entry().is_none(), "{password_step} {keys:?}");
                assert_eq!(ch(&mut model, 'g'), [], "{password_step} {keys:?}");
                assert_eq!(
                    key_line(&model, 100),
                    [USER_STEP],
                    "{password_step} {keys:?}"
                );
            }

            let mut model = open();
            let control_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
            assert_eq!(handle(&mut model, control_c), [Command::Quit]);
            assert!(model.quit());
        }
    }

    #[test]
    fn a_typed_value_of_only_spaces_is_no_login_part_and_enter_on_it_cancels_the_prompt() {
        let lab = book_dir();
        for password_step in [false, true] {
            let mut model = asking(None, None);
            ch(&mut model, 'g');
            if password_step {
                entered(&mut model, USER);
            }

            assert_eq!(entered(&mut model, "   "), [], "{password_step}");

            assert!(model.run().is_none(), "{password_step}");
            assert!(model.login_entry().is_none(), "{password_step}");
            assert!(!model.typing(), "{password_step}");
            assert_eq!(ch(&mut model, 'g'), [], "{password_step}");
            assert_eq!(key_line(&model, 100), [USER_STEP], "{password_step}");
        }

        let mut model = asking(None, None);
        ch(&mut model, 'g');
        entered(&mut model, " nobs-t-x ");
        assert_eq!(
            entered(&mut model, SECRET),
            fetch(&lab, login(Some(" nobs-t-x "), Some(SECRET)))
        );
    }

    #[test]
    fn for_s_the_typed_login_is_kept_only_when_the_command_runs() {
        let lab = book_dir();
        for cancel in [KeyCode::Esc, KeyCode::Enter] {
            let mut model = asking(None, None);
            ch(&mut model, 's');
            entered(&mut model, USER);
            entered(&mut model, SECRET);
            assert_eq!(model.entry(), Some(("nobs-t-srl1", "")));

            assert_eq!(code(&mut model, cancel), [], "{cancel:?}");

            assert!(model.run().is_none(), "{cancel:?}");
            assert_eq!(ch(&mut model, 'g'), [], "{cancel:?}");
            assert_eq!(key_line(&model, 100), [USER_STEP], "{cancel:?}");
        }

        let mut model = asking(None, None);
        ch(&mut model, 's');
        entered(&mut model, USER);
        entered(&mut model, SECRET);
        entered(&mut model, "show version");
        answered(&mut model, &[], End::Exited(0));
        assert_eq!(
            ch(&mut model, 'g'),
            fetch(&lab, login(Some(USER), Some(SECRET)))
        );
    }

    #[test]
    fn a_refusal_line_ending_a_run_that_did_not_carry_the_kept_login_keeps_it() {
        let lab = book_dir();
        let typed = login(Some(USER), Some(SECRET));
        let mut model = asking(None, None);
        assert_eq!(logged_in(&mut model), fetch(&lab, typed.clone()));
        answered(&mut model, &[], End::Exited(0));
        let maintenance = ACTIONS.iter().find(|action| action.key == 'm').unwrap();
        assert!(!maintenance.login);

        let asked = model.act(
            maintenance,
            vec![vec![OsString::from("maintenance")]],
            Target::Device("nobs-t-srl1".to_string()),
            typed.clone(),
        );
        assert_eq!(asked, []);
        let started = ch(&mut model, 'y');
        assert!(matches!(&started[..], [Command::Run { login, .. }] if *login == Login::default()));
        model.apply(Update::ActionError(refusal()));
        model.apply(Update::ActionEnded(End::Exited(1)));
        assert_eq!(model.run().unwrap().lines, [refusal()]);

        assert_eq!(ch(&mut model, 'g'), fetch(&lab, typed));
    }

    #[test]
    fn the_login_prompt_opens_only_where_the_key_would_act() {
        let mut no_device = asking(None, None);
        no_device.apply(Update::Devices(Ok(Vec::new())));
        let source = workshop_source();
        let mut workshop = Model::new(Start {
            lab: Some(workshop_located("infrahub")),
            source: Some(source.clone()),
            home: Workbench::Devices,
            chooser: false,
            login: finding(None, None),
            ..start(vec![stack("a", &[])])
        });
        workshop.apply(Update::Devices(Ok(source.devices().unwrap())));
        for (name, mut model) in [("no device", no_device), ("workshop", workshop)] {
            for key in ['g', 's'] {
                assert_eq!(ch(&mut model, key), [], "{name} {key}");
                assert!(model.login_entry().is_none(), "{name} {key}");
                assert!(!model.typing(), "{name} {key}");
            }
        }

        for key in ['g', 's'] {
            let mut busy = asking(None, None);
            ch(&mut busy, '2');
            running(&mut busy);
            ch(&mut busy, '1');

            assert_eq!(ch(&mut busy, key), [], "{key}");

            assert_eq!(
                busy.notice(),
                Some("one action at a time: a up is running"),
                "{key}"
            );
            assert!(busy.login_entry().is_none(), "{key}");
            assert!(!busy.typing(), "{key}");
        }
    }

    #[test]
    fn an_action_that_does_not_log_in_starts_without_the_kept_login() {
        let mut model = asking(None, None);
        logged_in(&mut model);
        answered(&mut model, &[], End::Exited(0));
        ch(&mut model, '2');

        let mut commands = ch(&mut model, 's');
        answered(&mut model, &[], End::Exited(0));
        ch(&mut model, 'u');
        commands.extend(ch(&mut model, 'y'));

        let logins: Vec<&Login> = commands
            .iter()
            .filter_map(|command| match command {
                Command::Run { login, .. } => Some(login),
                _ => None,
            })
            .collect();
        assert_eq!(logins, [&Login::default(), &Login::default()]);
    }

    #[test]
    fn the_login_is_looked_up_again_at_each_key() {
        let lab = book_dir();
        let held = Arc::new(std::sync::Mutex::new((Some(USER), Some(SECRET))));
        let seen = Arc::clone(&held);
        let mut model = lab_model_finding(
            "book-lab",
            "book-lab/containerlab/lab_vars.yml",
            Arc::new(move |name| {
                let (user, password) = *seen.lock().unwrap();
                finding(user, password)(name)
            }),
        );
        let found = |user, password| *held.lock().unwrap() = (user, password);

        assert_eq!(ch(&mut model, 'g'), fetch(&lab, Login::default()));
        answered(&mut model, &[], End::Exited(0));

        found(None, None);
        assert_eq!(ch(&mut model, 'g'), []);
        assert_eq!(key_line(&model, 100), [USER_STEP]);
        code(&mut model, KeyCode::Esc);

        found(Some(USER), None);
        assert_eq!(ch(&mut model, 'g'), []);
        assert_eq!(
            key_line(&model, 100),
            [" password for nobs-t-x:   Enter run  Esc cancel"]
        );
        code(&mut model, KeyCode::Esc);

        found(Some(USER), Some(SECRET));
        assert_eq!(ch(&mut model, 'g'), fetch(&lab, Login::default()));
    }

    fn measured(overflow: usize, cut_rows: usize) -> Option<RunMeasure> {
        Some(RunMeasure { overflow, cut_rows })
    }

    fn walk(model: &mut Model, steps: usize) -> Vec<Focus> {
        (0..steps).map(|_| tabbed(model)).collect()
    }

    fn to_run_pane(model: &mut Model) {
        while model.focus() != Focus::Run {
            tabbed(model);
        }
    }

    fn pushed(model: &mut Model, lines: std::ops::Range<usize>) {
        for index in lines {
            model.apply(Update::ActionLine(format!("line {index}")));
        }
    }

    #[test]
    fn a_moved_up_view_clamped_by_a_smaller_overflow_is_at_the_end_and_follows_again() {
        let mut model = opened(&["nobs-t-packt"]);
        running(&mut model);
        pushed(&mut model, 0..20);
        model.set_run_view(measured(10, 0));
        to_run_pane(&mut model);
        for _ in 0..3 {
            ch(&mut model, 'k');
        }
        assert_eq!((model.run_scroll(), model.run_view.following), (7, false));

        model.set_run_view(measured(4, 0));
        assert_eq!(model.run_scroll(), 4);
        model.set_run_view(measured(12, 0));

        assert_eq!((model.run_scroll(), model.run_view.following), (12, true));
    }

    fn shown_line(model: &Model) -> &str {
        &model.run().unwrap().lines[model.run_scroll()]
    }

    #[test]
    fn tab_stops_on_the_run_pane_only_while_it_is_drawn_after_the_stops_of_each_workbench() {
        let mut sonda = sonda_tab();
        code(&mut sonda, KeyCode::Esc);
        let cases: [(Model, &[Focus]); 4] = [
            (opened(&["a"]), &[Focus::Tabs, Focus::Run, Focus::List]),
            (sonda, &[Focus::Tabs, Focus::Lines, Focus::Run, Focus::List]),
            (
                devices_model(workshop_located("infrahub")),
                &[Focus::Tabs, Focus::Run, Focus::List],
            ),
            (
                book_model(),
                &[Focus::Tabs, Focus::Lines, Focus::Run, Focus::List],
            ),
        ];
        for (mut model, stops) in cases {
            let workbench = model.workbench();
            let folded: Vec<Focus> = stops
                .iter()
                .copied()
                .filter(|stop| *stop != Focus::Run)
                .collect();
            assert_eq!(walk(&mut model, folded.len()), folded, "{workbench:?}");

            ch(&mut model, 'o');
            model.set_run_view(measured(0, 0));
            assert_eq!(walk(&mut model, stops.len()), stops, "{workbench:?}");

            model.set_run_view(None);
            assert!(model.run_pane());
            assert_eq!(walk(&mut model, folded.len()), folded, "{workbench:?}");
        }
    }

    #[test]
    fn esc_and_o_fold_the_pane_and_a_pane_no_longer_drawn_gives_the_focus_to_the_list() {
        for fold in [KeyCode::Esc, KeyCode::Char('o')] {
            let mut model = opened(&["a"]);
            ch(&mut model, 'o');
            model.set_run_view(measured(0, 0));
            to_run_pane(&mut model);

            assert_eq!(code(&mut model, fold), []);

            assert_eq!(
                (model.run_pane(), model.run_drawn(), model.focus()),
                (false, false, Focus::List),
                "{fold:?}"
            );
        }
        let mut model = book_model();
        ch(&mut model, 'o');
        model.set_run_view(measured(0, 0));
        to_run_pane(&mut model);

        model.set_run_view(None);

        assert_eq!(
            (model.run_pane(), model.run_drawn(), model.focus()),
            (true, false, Focus::List)
        );
    }

    #[test]
    fn j_and_k_reach_every_row_of_the_pane_down_to_the_oldest_kept_line() {
        let mut model = opened(&["nobs-t-packt"]);
        running(&mut model);
        pushed(&mut model, 0..2100);
        model.set_run_view(measured(1990, 0));
        to_run_pane(&mut model);
        assert_eq!(model.run_scroll(), 1990);

        for _ in 0..1991 {
            ch(&mut model, 'k');
        }
        assert_eq!((model.run_scroll(), shown_line(&model)), (0, "line 100"));
        assert!(!model.run_view.following);

        for _ in 0..1991 {
            code(&mut model, KeyCode::Down);
        }
        assert_eq!((model.run_scroll(), model.run_view.following), (1990, true));
    }

    #[test]
    fn a_moved_up_view_keeps_its_rows_and_f_the_last_row_or_a_new_action_follow_again() {
        let mut model = opened(&["nobs-t-packt"]);
        running(&mut model);
        pushed(&mut model, 0..5);
        model.set_run_view(measured(5, 0));
        to_run_pane(&mut model);
        for _ in 0..3 {
            ch(&mut model, 'k');
        }
        assert_eq!((model.run_scroll(), model.run_view.following), (2, false));

        pushed(&mut model, 5..8);
        model.set_run_view(measured(8, 0));
        assert_eq!((model.run_scroll(), model.run_view.following), (2, false));

        ch(&mut model, 'f');
        assert_eq!((model.run_scroll(), model.run_view.following), (8, true));
        pushed(&mut model, 8..9);
        model.set_run_view(measured(9, 0));
        assert_eq!(model.run_scroll(), 9);
        ch(&mut model, 'k');
        assert!(!model.run_view.following);
        ch(&mut model, 'j');
        assert_eq!((model.run_scroll(), model.run_view.following), (9, true));

        pushed(&mut model, 9..2100);
        assert!(model.run_view.cut.is_empty());
        model.set_run_view(measured(1990, 0));
        for _ in 0..10 {
            ch(&mut model, 'k');
        }
        let before = shown_line(&model).to_string();

        pushed(&mut model, 2100..2103);
        assert_eq!(model.run_view.cut, ["line 100", "line 101", "line 102"]);
        model.set_run_view(measured(1990, 3));

        assert_eq!(
            (model.run_scroll(), shown_line(&model)),
            (1977, before.as_str())
        );
        assert!(model.run_view.cut.is_empty());

        model.apply(Update::ActionEnded(End::Exited(0)));
        tabbed(&mut model);
        running(&mut model);
        assert_eq!(
            (
                model.run_scroll(),
                model.run_view.following,
                model.run_drawn()
            ),
            (0, true, true)
        );
    }

    #[test]
    fn no_action_key_and_no_key_of_a_list_acts_while_the_run_pane_has_the_focus() {
        for mut model in [
            opened(&["nobs-t-packt"]),
            devices_model(workshop_located("infrahub")),
            book_model(),
        ] {
            let workbench = model.workbench();
            ch(&mut model, 'o');
            model.set_run_view(measured(0, 0));
            to_run_pane(&mut model);
            let keys = ACTIONS
                .iter()
                .filter(|action| action.workbench == workbench)
                .map(|action| KeyCode::Char(action.key))
                .chain("/[]l-+".chars().map(KeyCode::Char).chain([KeyCode::Enter]));
            for typed in keys {
                let before = snapshot(&model);

                assert_eq!(code(&mut model, typed), [], "{workbench:?} {typed:?}");

                assert_eq!(snapshot(&model), before, "{workbench:?} {typed:?}");
                assert!(model.run().is_none(), "{workbench:?} {typed:?}");
                assert_eq!(model.question(), None, "{workbench:?} {typed:?}");
                assert_eq!(model.entry(), None, "{workbench:?} {typed:?}");
                assert_eq!(model.notice(), None, "{workbench:?} {typed:?}");
            }
        }
        let mut model = opened(&["nobs-t-packt"]);
        running(&mut model);
        model.set_run_view(measured(0, 0));
        to_run_pane(&mut model);
        let ctrl_c = with(KeyCode::Char('c'), KeyModifiers::CONTROL);

        assert_eq!(handle(&mut model, ctrl_c), [Command::Stop]);
        assert_eq!(state(&model), RunState::Stopping);
    }

    #[test]
    fn r_refreshes_from_the_run_pane_over_the_sonda_tab() {
        let mut model = sonda_tab();
        ch(&mut model, 'o');
        model.set_run_view(measured(0, 0));
        to_run_pane(&mut model);

        assert_eq!(ch(&mut model, 'r'), [Command::Refresh]);
        assert_eq!((model.focus(), model.tab()), (Focus::Run, Tab::Sonda));
    }
}
