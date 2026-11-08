use std::path::{Path, PathBuf};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use nobs_core::env::read_env;
use nobs_core::lab;
use nobs_core::locate::{Kind, Located, FILE};
use nobs_core::settings::{read_settings_file, Settings};
use nobs_core::sonda::Sonda;

use crate::data::{self, Prompt};

const LOG_LINES: usize = 2000;
const MIN_SIZE: (u16, u16) = (60, 15);
const STACK_SONDA_URL: &str = "http://localhost:8085";
const SONDA_ENV: &str = "SONDA_SERVER_URL";
const INFRAHUB_ENV: &str = "INFRAHUB_ADDRESS";

#[derive(Debug, Clone)]
pub struct Start {
    pub lab: Option<Located>,
    pub stacks: Vec<Stack>,
    pub sonda: SondaTarget,
    pub home: Workbench,
    pub chooser: bool,
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
    Logs,
    Sonda,
}

impl Tab {
    fn next(self) -> Tab {
        match self {
            Tab::Overview => Tab::Logs,
            Tab::Logs => Tab::Sonda,
            Tab::Sonda => Tab::Overview,
        }
    }

    fn previous(self) -> Tab {
        match self {
            Tab::Overview => Tab::Sonda,
            Tab::Logs => Tab::Overview,
            Tab::Sonda => Tab::Logs,
        }
    }
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
    clock: String,
    quit: bool,
    exit_code: i32,
}

impl Model {
    pub fn new(start: Start) -> Model {
        let Start {
            lab,
            mut stacks,
            sonda,
            home,
            chooser,
        } = start;
        let sonda_only_start = lab.is_none() && !chooser;
        if sonda_only_start && !stacks.iter().any(|stack| stack.kind == StackKind::Sonda) {
            stacks.push(sonda_only(Path::new(".")));
        }
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
            clock: String::new(),
            quit: false,
            exit_code: 0,
        }
    }

    pub fn tick(&mut self, now: Instant, clock: String, size: (u16, u16)) {
        self.now = now;
        self.clock = clock;
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
        }
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

    pub fn filter(&self) -> Option<&str> {
        self.filter.text.as_deref()
    }

    pub fn typing(&self) -> bool {
        self.filter.typing || self.logs.as_ref().is_some_and(|logs| logs.filter.typing)
    }

    pub fn typed_filter(&self) -> Option<&str> {
        if self.filter.typing {
            return self.filter.text.as_deref();
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

    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    pub fn now(&self) -> Instant {
        self.now
    }

    pub fn clock(&self) -> &str {
        &self.clock
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
        let infrahub = stack
            .and_then(|stack| env_value(&stack.env, INFRAHUB_ENV))
            .is_some();
        let sot = if infrahub { "infrahub" } else { "none" };
        format!("nobs  {name} {kind}  sot {sot}")
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
        self.sonda = data::Model::new(sonda_url(self.server.as_deref(), self.stacks.get(index)));
        self.services.clear();
        self.services_at = None;
        self.health.clear();
        self.service = 0;
        self.logs = None;
        if self.tab == Tab::Logs {
            self.tab = Tab::Overview;
        }
    }

    fn sonda_stack(&mut self) -> usize {
        if let Some(index) = self
            .stacks
            .iter()
            .position(|stack| stack.kind == StackKind::Sonda)
        {
            return index;
        }
        self.stacks.push(sonda_only(Path::new(".")));
        self.states.push(StackState::Unknown);
        self.stacks.len() - 1
    }

    fn chooser_key(&mut self, code: KeyCode) -> Vec<Command> {
        match code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.selected = (self.selected + 1).min(self.stacks.len());
            }
            KeyCode::Char('k') | KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Enter => {
                let mut tab = Tab::Overview;
                let index = if self.selected < self.stacks.len() {
                    self.selected
                } else {
                    tab = Tab::Sonda;
                    self.sonda_stack()
                };
                self.chooser = false;
                self.select(index);
                self.tab = tab;
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
            KeyCode::Char('1') => self.workbench = Workbench::Devices,
            KeyCode::Char('2') => self.workbench = Workbench::Services,
            KeyCode::Char('3') => self.workbench = Workbench::Lab,
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
            _ => {}
        }
        Vec::new()
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
                KeyCode::Char('[') => return self.go_tab(self.tab.previous()),
                KeyCode::Char(']') => return self.go_tab(self.tab.next()),
                _ => {}
            }
        }
        match (self.focus, self.tab) {
            (Focus::List, _) => self.list_key(key.code),
            (Focus::Tabs, Tab::Overview) => self.overview_key(key.code),
            (Focus::Tabs, Tab::Logs) => self.logs_key(key.code),
            (Focus::Tabs, Tab::Sonda) => self.sonda_key(key),
        }
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
    match lab {
        Some(lab) if matches!(lab.kind, Kind::BookLab(_)) => book_stacks(lab),
        Some(lab) if matches!(lab.kind, Kind::LabcliLab(_)) => Ok(vec![lab_stack(lab)?]),
        _ => Ok(workshops
            .into_iter()
            .map(|(name, _, dir)| Stack {
                kind: StackKind::Workshop,
                compose: dir.join("docker-compose.yml"),
                project: workshop_project(&dir),
                env: read_env(&dir).unwrap_or_default(),
                name,
                path: dir,
            })
            .collect()),
    }
}

pub fn sonda_only(here: &Path) -> Stack {
    Stack {
        name: "sonda only".to_string(),
        kind: StackKind::Sonda,
        path: here.to_path_buf(),
        compose: here.to_path_buf(),
        project: None,
        env: read_env(here).unwrap_or_default(),
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

fn workshop_project(dir: &Path) -> Option<String> {
    match read_settings_file(&dir.join(FILE)) {
        Ok(Settings::Workshop(workshop)) => Some(workshop.project),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use std::fs;

    use nobs_core::locate::locate;

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

    fn sonda_tab() -> Model {
        Model::new(Start {
            chooser: false,
            ..start(vec![sonda_only(Path::new("/nonexistent-sonda-folder"))])
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
                model.filter(),
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
        model.tick(Instant::now(), "13:42:07".to_string(), (columns, rows));
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
        assert_eq!(model.filter(), Some("q"));
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
        ch(&mut model, 'a');
        code(&mut model, KeyCode::Esc);
        assert!(!model.typing());
        assert_eq!(model.filter(), None);

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
    fn the_chooser_moves_over_the_stacks_and_the_sonda_row_and_enter_opens_the_row() {
        let mut model = chooser(&["a", "b"]);
        assert!(model.chooser());
        assert_eq!(ch(&mut model, 'k'), []);
        assert_eq!(model.selected_stack(), 0);
        ch(&mut model, 'j');
        code(&mut model, KeyCode::Down);
        code(&mut model, KeyCode::Down);
        assert_eq!(model.selected_stack(), 2);
        code(&mut model, KeyCode::Up);
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
    fn the_chooser_sonda_row_appends_the_sonda_stack_and_opens_the_sonda_tab() {
        let mut model = chooser(&["a"]);
        ch(&mut model, 'j');
        assert_eq!(model.selected_stack(), 1);
        ch(&mut model, 'j');
        assert_eq!(model.selected_stack(), 1);

        assert_eq!(code(&mut model, KeyCode::Enter), [Command::SelectStack(1)]);

        assert!(!model.chooser());
        assert_eq!(model.tab(), Tab::Sonda);
        assert_eq!(model.stacks().len(), 2);
        assert_eq!(model.stacks()[1].kind, StackKind::Sonda);
        assert_eq!(model.stacks()[1].name, "sonda only");
        assert_eq!(model.stack_state(1), &StackState::Unknown);
    }

    #[test]
    fn a_placeholder_workbench_acts_on_the_global_keys_only() {
        for number in ['1', '3'] {
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
        assert_eq!(model.filter(), Some(""));
        typed(&mut model, "x1j?");
        code(&mut model, KeyCode::Tab);
        code(&mut model, KeyCode::Down);
        code(&mut model, KeyCode::Backspace);
        assert_eq!(model.filter(), Some("x1j"));
        assert_eq!(model.workbench(), Workbench::Services);
        assert_eq!(model.focus(), Focus::List);
        assert_eq!(model.selected_stack(), 0);
        assert!(!model.help());

        assert_eq!(code(&mut model, KeyCode::Enter), []);
        assert!(!model.typing());
        assert_eq!(model.filter(), Some("x1j"));

        ch(&mut model, '/');
        assert!(model.typing());
        assert_eq!(model.filter(), Some("x1j"));
        code(&mut model, KeyCode::Esc);
        assert!(!model.typing());
        assert_eq!(model.filter(), None);
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
    fn the_book_lab_gives_one_stack_per_chapter() {
        let dir = fixture("book-lab");
        let located = locate(&dir, Some(&dir)).unwrap();

        let stacks = stacks(Some(&located), vec![]).unwrap();

        let names: Vec<&str> = stacks.iter().map(|stack| stack.name.as_str()).collect();
        assert_eq!(names, ["ch1", "ch2"]);
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

        let sonda = sonda_only(&bad.0);
        assert_eq!(sonda.env, []);
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

        let stack = sonda_only(&folder.0);

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
            model.tick(at, "13:42:07".to_string(), (80, 24));
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
            ..start(vec![sonda_only(Path::new("/nonexistent-sonda-folder"))])
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
            sonda_only(Path::new("/nonexistent-sonda-folder")),
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

        assert_eq!(model.filter(), Some("pa"));
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
            assert_eq!(model.size(), (columns, rows));
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
        model.tick(at, "13:42:07".to_string(), (80, 24));

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

        model.tick(at, "13:42:07".to_string(), (100, 30));

        assert_eq!(model.clock(), "13:42:07");
        assert_eq!(model.now(), at);
        assert_eq!(model.size(), (100, 30));
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

        let mut plain = stack("autocon5", &[("INFRAHUB_ADDRESS", "  ")]);
        plain.path = fixture("book-lab");
        let book = fixture("book-lab");
        let model = Model::new(Start {
            lab: Some(locate(&book, Some(&book)).unwrap()),
            chooser: false,
            ..start(vec![plain])
        });
        assert_eq!(model.header(), "nobs  book-lab book lab  sot none");

        assert_eq!(chooser(&["a"]).header(), "nobs  no lab here");

        let model = Model::new(Start {
            chooser: false,
            ..start(vec![sonda_only(Path::new("/nonexistent-sonda-folder"))])
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
}
