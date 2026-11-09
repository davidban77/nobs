use std::cmp::Reverse;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use nobs_core::alertmanager::Alert;
use nobs_core::env::read_env;
use nobs_core::lab;
use nobs_core::locate::{locate, Kind, Located};
use nobs_core::loki::Line;
use nobs_core::prefect::FlowRun;
use nobs_core::prometheus::Sample;
use nobs_core::sonda::Sonda;
use nobs_core::sot::{Device, SourceOfTruth};
use nobs_core::status::Urls;
use nobs_core::time;

use crate::data::{self, Prompt};
use crate::devices;

const LOG_LINES: usize = 2000;
pub const WINDOW_MINUTES: u64 = 30;
const WINDOW_MIN: u64 = 1;
const WINDOW_MAX: u64 = 1440;
const SERVICES_TABS: [Tab; 3] = [Tab::Overview, Tab::Logs, Tab::Sonda];
const DEVICE_TABS: [Tab; 4] = [Tab::Overview, Tab::Interfaces, Tab::Timeline, Tab::Logs];
const MIN_SIZE: (u16, u16) = (60, 15);
const STACK_SONDA_URL: &str = "http://localhost:8085";
const SONDA_ENV: &str = "SONDA_SERVER_URL";
const INFRAHUB_ENV: &str = "INFRAHUB_ADDRESS";

#[derive(Clone)]
pub struct Start {
    pub lab: Option<Located>,
    pub stacks: Vec<Stack>,
    pub sonda: SondaTarget,
    pub home: Workbench,
    pub chooser: bool,
    pub source: Option<Arc<dyn SourceOfTruth>>,
    pub source_error: Option<String>,
    pub urls: Urls,
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
            .field("urls", &self.urls)
            .finish()
    }
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Tabs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Interfaces,
    Timeline,
    Logs,
    Sonda,
}

impl Tab {
    pub fn title(self, wide: bool) -> &'static str {
        match self {
            Tab::Overview => "overview",
            Tab::Interfaces if wide => "interfaces",
            Tab::Interfaces => "ifaces",
            Tab::Timeline => "timeline",
            Tab::Logs => "logs",
            Tab::Sonda => "sonda",
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StackState {
    Unknown,
    Up { running: usize, listed: usize },
    Down { listed: usize },
    Failed,
}

impl StackState {
    pub fn text(&self) -> String {
        match self {
            StackState::Unknown | StackState::Failed => "?".to_string(),
            StackState::Up { running, listed } => format!("up {running}/{listed}"),
            StackState::Down { listed: 0 } => "down".to_string(),
            StackState::Down { listed } => format!("down 0/{listed}"),
        }
    }
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
    Services(usize, Result<Vec<lab::Service>, String>),
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
    FlowRuns(Result<Vec<FlowRun>, String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Quit,
    Refresh,
    SelectStack(usize),
    OpenLogs { stack: usize, service: String },
    Follow(bool),
    CloseLogs,
    Sonda(data::Command),
    SelectDevice(String),
    OpenTab(Tab),
    Window(u64),
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
}

struct DeviceLine {
    line: Line,
    shown: String,
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
    error: Option<(String, String, Instant)>,
    help: bool,
    run_pane: bool,
    size: (u16, u16),
    now: Instant,
    wall: SystemTime,
    clock: String,
    quit: bool,
    exit_code: i32,
    source: bool,
    source_error: Option<String>,
    devices: Vec<Device>,
    devices_at: Option<Instant>,
    device: usize,
    cursor: Cursor,
    device_filter: Filter,
    metrics: Option<Vec<Sample>>,
    alerts: Vec<Alert>,
    alerts_ok: bool,
    runs: Vec<FlowRun>,
    rcas: Vec<Line>,
    device_lines: Vec<DeviceLine>,
    logs_following: bool,
    logs_scroll: usize,
    logs_filter: Filter,
    window: u64,
    timeline_row: usize,
    popup: bool,
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
            urls: _,
        } = start;
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
        Model {
            lab,
            states: vec![StackState::Unknown; stacks.len()],
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
            error: None,
            help: false,
            run_pane: false,
            size: (80, 24),
            now: Instant::now(),
            wall: UNIX_EPOCH,
            clock: String::new(),
            quit: false,
            exit_code: 0,
            source: source.is_some(),
            source_error,
            devices: Vec::new(),
            devices_at: None,
            device: 0,
            cursor: Cursor::Device(0),
            device_filter: Filter::default(),
            metrics: None,
            alerts: Vec::new(),
            alerts_ok: false,
            runs: Vec::new(),
            rcas: Vec::new(),
            device_lines: Vec::new(),
            logs_following: true,
            logs_scroll: 0,
            logs_filter: Filter::default(),
            window: WINDOW_MINUTES,
            timeline_row: 0,
            popup: false,
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

    pub fn apply(&mut self, update: Update) {
        match update {
            Update::Services(index, Ok(rows)) => self.apply_services(index, rows),
            Update::Services(index, Err(text)) => {
                let Some(name) = self.stacks.get(index).map(|stack| stack.name.clone()) else {
                    return;
                };
                self.states[index] = StackState::Failed;
                self.error = Some((format!("ps {name}"), text, self.now));
            }
            Update::Health(index, list) => {
                if index != self.selected {
                    return;
                }
                self.health = list;
                for service in &mut self.services {
                    service.health = health_of(&self.health, &service.name);
                }
            }
            Update::Logs(index, service, Ok(lines)) => {
                if index != self.selected {
                    return;
                }
                let Some(logs) = self.logs.as_mut().filter(|logs| logs.service == service) else {
                    return;
                };
                logs.lines
                    .extend(lines.iter().map(|line| untimed(line).to_string()));
                let extra = logs.lines.len().saturating_sub(LOG_LINES);
                logs.lines.drain(..extra);
            }
            Update::Logs(_, service, Err(text)) => {
                self.error = Some((format!("logs {service}"), text, self.now));
            }
            Update::Sonda(update) => self.sonda.apply(update),
            Update::Note(note) => self.sonda.note(note.text, note.at),
            Update::Stopped(source) => {
                self.error = Some((source, "polling stopped".to_string(), self.now));
            }
            Update::Devices(Ok(list)) => {
                self.devices = list;
                self.devices_at = Some(self.now);
                let last = self.devices.len().saturating_sub(1);
                self.device = self.device.min(last);
                if let Cursor::Device(at) = self.cursor {
                    self.cursor = Cursor::Device(at.min(last));
                }
                self.clear_error("sot");
            }
            Update::Devices(Err(text)) => self.fail("sot", text),
            Update::Metrics(Ok(samples)) => {
                self.metrics = Some(samples);
                self.clear_error("prometheus");
            }
            Update::Metrics(Err(text)) => {
                self.metrics = None;
                self.fail("prometheus", text);
            }
            Update::Alerts(Ok(mut list)) => {
                list.sort_by_key(|alert| Reverse(time::parse_rfc3339(&alert.starts_at)));
                self.alerts = list;
                self.alerts_ok = true;
                if let Cursor::Alert(at) = self.cursor {
                    self.cursor = match self.alerts.len() {
                        0 => Cursor::Device(self.devices.len().saturating_sub(1)),
                        len => Cursor::Alert(at.min(len - 1)),
                    };
                }
                self.clear_error("alertmanager");
            }
            Update::Alerts(Err(text)) => {
                self.alerts_ok = false;
                self.fail("alertmanager", text);
            }
            Update::DeviceLogs(device, Ok(lines)) => {
                if !self.is_selected(&device) {
                    return;
                }
                for line in lines {
                    if self.device_lines.iter().all(|held| held.line != line) {
                        let shown = line.message().unwrap_or_else(|| line.text.clone());
                        self.device_lines.push(DeviceLine { line, shown });
                    }
                }
                self.device_lines
                    .sort_by_key(|held| Reverse(held.line.nanoseconds));
                self.device_lines.truncate(LOG_LINES);
                self.clear_error("loki");
            }
            Update::DeviceLogs(_, Err(text)) => self.fail("loki", text),
            Update::Rcas(device, Ok(lines)) => {
                if self.is_selected(&device) {
                    self.rcas = lines;
                    self.clear_error("loki");
                }
            }
            Update::Rcas(_, Err(text)) => self.fail("loki", text),
            Update::FlowRuns(Ok(runs)) => {
                self.runs = runs;
                self.clear_error("prefect");
            }
            Update::FlowRuns(Err(text)) => self.fail("prefect", text),
        }
    }

    fn fail(&mut self, source: &str, text: String) {
        self.error = Some((source.to_string(), text, self.now));
    }

    fn clear_error(&mut self, source: &str) {
        if self
            .error
            .as_ref()
            .is_some_and(|(held, _, _)| held == source)
        {
            self.error = None;
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
        self.tab
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
        self.filter.typing
            || self.device_filter.typing
            || self.logs_filter.typing
            || self.logs.as_ref().is_some_and(|logs| logs.filter.typing)
    }

    pub fn typed_filter(&self) -> Option<&str> {
        if self.filter.typing {
            return self.filter.text.as_deref();
        }
        if self.device_filter.typing {
            return self.device_filter.text.as_deref();
        }
        if self.logs_filter.typing {
            return self.logs_filter.text.as_deref();
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
        self.error
            .as_ref()
            .map(|(source, text, at)| (source.as_str(), text.as_str(), *at))
    }

    pub fn help(&self) -> bool {
        self.help
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
        let needle = self.device_filter.text.as_deref().map(str::to_lowercase);
        (0..self.devices.len())
            .filter(|index| {
                needle
                    .as_deref()
                    .is_none_or(|needle| self.devices[*index].name.to_lowercase().contains(needle))
            })
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

    pub fn source_error(&self) -> Option<&str> {
        self.source_error.as_deref()
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

    pub fn timeline(&self) -> Vec<devices::Entry> {
        let Some(device) = self.selected_device() else {
            return Vec::new();
        };
        let logs: Vec<Line> = self
            .device_lines
            .iter()
            .map(|held| held.line.clone())
            .collect();
        devices::merge(
            &device.name,
            &self.alerts,
            &self.runs,
            &self.rcas,
            &logs,
            Duration::from_secs(self.window * 60),
            self.wall,
        )
    }

    pub fn timeline_row(&self) -> usize {
        self.timeline_row
    }

    pub fn popup(&self) -> bool {
        self.popup
    }

    pub fn device_logs(&self) -> Option<(&str, Vec<&str>, bool, usize)> {
        let device = self.selected_device()?;
        Some((
            device.name.as_str(),
            self.shown_device_logs(),
            self.logs_following,
            self.logs_scroll,
        ))
    }

    fn shown_device_logs(&self) -> Vec<&str> {
        self.device_lines
            .iter()
            .map(|held| held.shown.as_str())
            .filter(|text| self.logs_filter.matches(text))
            .collect()
    }

    pub fn window(&self) -> u64 {
        self.window
    }

    pub fn sot(&self) -> String {
        match self.lab.as_ref().map(|lab| &lab.kind) {
            Some(Kind::Workshop(workshop)) => workshop.sot.clone(),
            Some(Kind::BookLab(_)) => "lab_vars".to_string(),
            Some(Kind::LabcliLab(_)) => "none".to_string(),
            None => {
                let infrahub = self
                    .stacks
                    .get(self.selected)
                    .and_then(|stack| env_value(&stack.env, INFRAHUB_ENV))
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

    fn types_q(&self) -> bool {
        !self.too_small() && (self.typing() || self.asking())
    }

    fn asking(&self) -> bool {
        self.workbench == Workbench::Services
            && self.focus == Focus::Tabs
            && self.tab == Tab::Sonda
            && self.sonda.prompt() != &Prompt::None
    }

    fn apply_services(&mut self, index: usize, rows: Vec<lab::Service>) {
        let Some(name) = self.stacks.get(index).map(|stack| stack.name.clone()) else {
            return;
        };
        let running = rows.iter().filter(|row| row.state == "running").count();
        let listed = rows.len();
        self.states[index] = if running > 0 {
            StackState::Up { running, listed }
        } else {
            StackState::Down { listed }
        };
        if self
            .error
            .as_ref()
            .is_some_and(|(source, _, _)| *source == format!("ps {name}"))
        {
            self.error = None;
        }
        if index != self.selected {
            return;
        }
        let services = rows
            .into_iter()
            .map(|row| Service {
                health: health_of(&self.health, &row.name),
                name: row.name,
                state: row.state,
                port: row.port,
                age: row.age,
            })
            .collect();
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
        if self.filter.typing {
            self.filter.type_key(key.code);
            return Vec::new();
        }
        if self.device_filter.typing {
            self.device_filter.type_key(key.code);
            return Vec::new();
        }
        if self.logs_filter.typing {
            self.logs_filter.type_key(key.code);
            return Vec::new();
        }
        if let Some(logs) = self.logs.as_mut().filter(|logs| logs.filter.typing) {
            logs.filter.type_key(key.code);
            return Vec::new();
        }
        let sonda_keys = self.workbench == Workbench::Services
            && self.focus == Focus::Tabs
            && self.tab == Tab::Sonda;
        if sonda_keys && self.sonda.prompt() != &Prompt::None && key.code != KeyCode::Esc {
            return self.sonda_key(key);
        }
        match key.code {
            KeyCode::Char('1') => self.switch(Workbench::Devices),
            KeyCode::Char('2') => self.switch(Workbench::Services),
            KeyCode::Char('3') => self.switch(Workbench::Lab),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('x') => self.error = None,
            KeyCode::Char('o') => self.run_pane = !self.run_pane,
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::List => Focus::Tabs,
                    Focus::Tabs => Focus::List,
                };
            }
            KeyCode::Esc => return self.escape(key),
            KeyCode::Char('r') if !sonda_keys => return vec![Command::Refresh],
            _ if self.workbench == Workbench::Services => return self.services_key(key),
            _ if self.workbench == Workbench::Devices => return self.devices_key(key),
            _ => {}
        }
        Vec::new()
    }

    fn switch(&mut self, workbench: Workbench) {
        self.workbench = workbench;
        let tabs: &[Tab] = match workbench {
            Workbench::Devices => &DEVICE_TABS,
            Workbench::Services => &SERVICES_TABS,
            Workbench::Lab => return,
        };
        if !tabs.contains(&self.tab) {
            self.tab = Tab::Overview;
        }
    }

    fn escape(&mut self, key: KeyEvent) -> Vec<Command> {
        if self.run_pane {
            self.run_pane = false;
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
        if self.workbench == Workbench::Devices && self.popup {
            self.popup = false;
            return Vec::new();
        }
        self.focus = Focus::List;
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
        if self.focus == Focus::Tabs {
            match key.code {
                KeyCode::Char('[') => return self.go_tab(step(&SERVICES_TABS, self.tab, false)),
                KeyCode::Char(']') => return self.go_tab(step(&SERVICES_TABS, self.tab, true)),
                _ => {}
            }
        }
        match (self.focus, self.tab) {
            (Focus::List, _) => self.list_key(key.code),
            (Focus::Tabs, Tab::Overview | Tab::Interfaces | Tab::Timeline) => {
                self.overview_key(key.code)
            }
            (Focus::Tabs, Tab::Logs) => self.logs_key(key.code),
            (Focus::Tabs, Tab::Sonda) => self.sonda_key(key),
        }
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
            Tab::Timeline => self.timeline_key(key.code),
            Tab::Logs => self.device_logs_key(key.code),
            _ => Vec::new(),
        }
    }

    fn open_device_tab(&mut self, forward: bool) -> Vec<Command> {
        self.tab = step(&DEVICE_TABS, self.tab, forward);
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
        self.cursor = next;
        match next {
            Cursor::Device(index) => self.select_device(index),
            Cursor::Alert(_) => Vec::new(),
        }
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
                let Some(index) = self
                    .alerts
                    .get(at)
                    .and_then(|alert| alert.labels.get("device"))
                    .and_then(|name| self.devices.iter().position(|device| device.name == *name))
                else {
                    return Vec::new();
                };
                let mut commands = self.select_device(index);
                self.focus = Focus::Tabs;
                self.tab = Tab::Timeline;
                commands.push(Command::OpenTab(Tab::Timeline));
                commands
            }
        }
    }

    fn select_device(&mut self, index: usize) -> Vec<Command> {
        let Some(name) = self.devices.get(index).map(|device| device.name.clone()) else {
            return Vec::new();
        };
        if index != self.device {
            self.device = index;
            self.device_lines.clear();
            self.rcas.clear();
            self.logs_scroll = 0;
            self.timeline_row = 0;
            self.popup = false;
        }
        vec![Command::SelectDevice(name)]
    }

    fn timeline_key(&mut self, code: KeyCode) -> Vec<Command> {
        let entries = self.timeline().len();
        let last = entries.saturating_sub(1);
        match code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.timeline_row = (self.timeline_row + 1).min(last);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.timeline_row = self.timeline_row.saturating_sub(1).min(last);
            }
            KeyCode::Enter if entries > 0 => self.popup = true,
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
                self.logs_following = !self.logs_following;
                if self.logs_following {
                    self.logs_scroll = 0;
                }
                return vec![Command::Follow(self.logs_following)];
            }
            KeyCode::Char('/') => self.logs_filter.start(),
            KeyCode::Char('j') | KeyCode::Down if !self.logs_following => {
                self.logs_scroll = self.logs_scroll.saturating_sub(1);
            }
            KeyCode::Char('k') | KeyCode::Up if !self.logs_following => {
                let shown = self.shown_device_logs().len();
                self.logs_scroll = (self.logs_scroll + 1).min(shown.saturating_sub(1));
            }
            _ => {}
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
            Some(index) => {
                self.select(index);
                vec![Command::SelectStack(index)]
            }
            None => Vec::new(),
        }
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
    if key.kind != KeyEventKind::Press {
        return Vec::new();
    }
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let quit = match key.code {
        KeyCode::Char('c') => control,
        KeyCode::Char('q') => !control && !alt && !model.types_q(),
        _ => false,
    };
    if quit {
        model.quit = true;
        model.exit_code = i32::from(model.chooser);
        return vec![Command::Quit];
    }
    if control || alt || model.too_small() {
        return Vec::new();
    }
    if model.help {
        if matches!(key.code, KeyCode::Char('?') | KeyCode::Esc) {
            model.help = false;
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

pub(crate) fn sonda_url(flag: Option<&str>, stack: Option<&Stack>) -> String {
    if let Some(url) = flag {
        return url.to_string();
    }
    match stack.and_then(|stack| env_value(&stack.env, SONDA_ENV)) {
        Some(url) => url.to_string(),
        None if stack.is_some_and(|stack| stack.kind != StackKind::Sonda) => {
            STACK_SONDA_URL.to_string()
        }
        None => Sonda::DEFAULT_URL.to_string(),
    }
}

fn health_of(health: &[Health], name: &str) -> Option<String> {
    health
        .iter()
        .find(|entry| entry.service == name)
        .map(|entry| entry.state.clone())
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
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use nobs_core::lab_vars;
    use nobs_core::locate::locate;
    use nobs_core::sot::Fixed;

    use super::*;

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

    fn fixture(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures")
            .join(relative)
    }

    fn stack(name: &str, env: &[(&str, &str)]) -> Stack {
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
        }
    }

    fn target(server: Option<&str>) -> SondaTarget {
        SondaTarget {
            server: server.map(str::to_string),
            api_key: None,
        }
    }

    fn start(stacks: Vec<Stack>) -> Start {
        Start {
            lab: None,
            stacks,
            sonda: target(None),
            home: Workbench::Services,
            chooser: true,
            source: None,
            source_error: None,
            urls: Urls::defaults(),
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
            ..start(vec![sonda_stack()])
        })
    }

    fn row(name: &str, state: &str) -> lab::Service {
        lab::Service {
            name: name.to_string(),
            state: state.to_string(),
            health: String::new(),
            port: None,
            age: None,
        }
    }

    fn with_services(model: &mut Model, names: &[&str]) {
        let rows = names.iter().map(|name| row(name, "running")).collect();
        model.apply(Update::Services(0, Ok(rows)));
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

    fn sonda_rows(model: &mut Model) {
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
        ch(&mut model, '3');
        assert_eq!(model.workbench(), Workbench::Lab);
        ch(&mut model, '2');
        assert_eq!(model.workbench(), Workbench::Services);

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
            env: vec![(SONDA_ENV.to_string(), "http://s:9".to_string())],
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
    fn a_placeholder_workbench_acts_on_the_global_keys_only() {
        for number in ['3'] {
            let mut model = opened(&["a", "b"]);
            ch(&mut model, number);
            let before = snapshot(&model);
            for ignored in ['j', 'k', 'l', '/', 'f', '[', ']'] {
                assert_eq!(ch(&mut model, ignored), [], "{ignored}");
            }
            assert_eq!(code(&mut model, KeyCode::Enter), []);
            assert_eq!(code(&mut model, KeyCode::Down), []);
            assert_eq!(snapshot(&model), before);
            assert_eq!(ch(&mut model, 'r'), [Command::Refresh]);
            ch(&mut model, 'o');
            assert!(model.run_pane());
            ch(&mut model, '2');
            assert_eq!(model.workbench(), Workbench::Services);
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
                listed: 7
            }
            .text(),
            "up 7/7"
        );
        assert_eq!(StackState::Down { listed: 0 }.text(), "down");
        assert_eq!(StackState::Down { listed: 3 }.text(), "down 0/3");
    }

    #[test]
    fn services_rows_give_the_stack_state_text() {
        let mut model = opened(&["autocon5"]);
        let seven: Vec<lab::Service> = (0..7).map(|n| row(&format!("s{n}"), "running")).collect();
        model.apply(Update::Services(0, Ok(seven)));
        assert_eq!(model.stack_state(0).text(), "up 7/7");

        model.apply(Update::Services(0, Ok(vec![])));
        assert_eq!(model.stack_state(0).text(), "down");

        let three = ["exited", "exited", "created"]
            .iter()
            .map(|state| row("s", state))
            .collect();
        model.apply(Update::Services(0, Ok(three)));
        assert_eq!(model.stack_state(0).text(), "down 0/3");

        model.apply(Update::Services(
            0,
            Ok(vec![row("a", "running"), row("b", "exited")]),
        ));
        assert_eq!(model.stack_state(0).text(), "up 1/2");
    }

    #[test]
    fn a_failed_ps_gives_a_question_mark_the_error_and_keeps_the_last_services() {
        let mut model = opened(&["autocon5"]);
        with_services(&mut model, &["grafana"]);
        model.apply(Update::Services(0, Err("exit 1".to_string())));

        assert_eq!(model.stack_state(0).text(), "?");
        let (source, text, _) = model.error().unwrap();
        assert_eq!((source, text), ("ps autocon5", "exit 1"));
        assert_eq!(model.services().len(), 1);
    }

    #[test]
    fn an_update_for_a_stack_that_is_not_selected_sets_only_its_state() {
        let mut model = opened(&["a", "b"]);
        model.apply(Update::Services(1, Ok(vec![row("x", "running")])));
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

            model.apply(Update::Services(0, lab::services(text)));

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
        model.apply(Update::Services(0, lab::services(LINES)));
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
        model.apply(Update::Services(1, Ok(vec![row("grafana", "running")])));
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
        assert_eq!(ch(&mut model, 'j'), [Command::SelectStack(1)]);
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
        assert_eq!((source, text), ("logs grafana", "gone"));
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

        model.apply(Update::Services(0, Err("exit 1".to_string())));
        assert_eq!(model.error(), Some(("ps a", "exit 1", at)));
        with_services(&mut model, &["grafana"]);
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

    fn workshop_source() -> Arc<dyn SourceOfTruth> {
        let text = fs::read_to_string(fixture("workshop-one/lab_vars.yml")).unwrap();
        Arc::new(Fixed::from_lab_vars(&lab_vars::read(&text).unwrap()))
    }

    fn workshop_located(sot: &str) -> Located {
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

    fn alert(device: &str, name: &str, starts_at: &str) -> Alert {
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

    fn devices_model() -> Model {
        let source = workshop_source();
        let mut model = Model::new(Start {
            lab: Some(workshop_located("infrahub")),
            source: Some(source.clone()),
            home: Workbench::Devices,
            chooser: false,
            ..start(vec![stack("a", &[])])
        });
        model.apply(Update::Devices(Ok(source.devices().unwrap())));
        model.apply(Update::Alerts(Ok(vec![alert(
            "srl2",
            "BgpSessionNotUp",
            "2026-11-09T13:53:32Z",
        )])));
        model
    }

    #[test]
    fn the_device_keys_open_the_tabs_move_the_window_pause_the_logs_filter_and_pick_an_alert() {
        let mut model = devices_model();
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
        assert_eq!(ch(&mut model, '+'), [Command::Window(30)]);
        assert_eq!(ch(&mut model, '+'), [Command::Window(60)]);
        assert_eq!(model.window(), 60);

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
        assert_eq!(ch(&mut model, 'f'), [Command::Follow(false)]);
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
    fn the_sot_word_comes_from_the_workshop_setting_and_is_lab_vars_in_the_book_lab() {
        let header = |located: Located| {
            Model::new(Start {
                lab: Some(located),
                chooser: false,
                ..start(vec![stack("autocon5", &[])])
            })
            .header()
        };
        assert!(header(workshop_located("none")).ends_with("sot none"));
        assert!(header(workshop_located("infrahub")).ends_with("sot infrahub"));
        let book = fixture("book-lab");
        let located = locate(&book, Some(&book)).unwrap();
        assert!(header(located).ends_with("sot lab_vars"));
    }

    #[test]
    fn a_failed_prometheus_poll_shows_question_marks_and_its_success_clears_its_own_error() {
        let mut model = devices_model();
        assert_eq!(model.device_rows()[1].alerts, Some((1, false)));

        model.apply(Update::Metrics(Ok(peer_samples(1.0))));
        assert_eq!(model.device_rows()[0].health, devices::Health::Ok);

        model.apply(Update::Metrics(Err("cannot reach http://x".to_string())));
        assert_eq!(model.device_rows()[0].health, devices::Health::Unknown);
        let (source, text, _) = model.error().unwrap();
        assert_eq!(source, "prometheus");
        assert!(text.contains("cannot reach"), "{text}");

        model.apply(Update::Metrics(Ok(peer_samples(1.0))));
        assert_eq!(model.device_rows()[0].health, devices::Health::Ok);
        assert_eq!(model.error(), None);

        model.apply(Update::Alerts(Err("cannot reach http://a".to_string())));
        assert_eq!(model.device_rows()[0].alerts, None);
        assert_eq!(model.alerts().len(), 1);
        assert!(!model.alerts_ok());
        model.apply(Update::Metrics(Ok(peer_samples(1.0))));
        assert_eq!(model.error().unwrap().0, "alertmanager");
        model.apply(Update::Alerts(Ok(Vec::new())));
        assert_eq!(model.error(), None);
    }

    #[test]
    fn moving_to_another_device_clears_its_logs_and_a_repeated_log_line_shows_once() {
        let mut model = devices_model();
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
        assert_eq!(name, "srl1");
        assert_eq!(lines, ["bgp up", "bgp idle", "link down"]);
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
}
