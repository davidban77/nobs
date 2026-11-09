use std::ffi::OsString;
use std::io;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use nobs_core::alertmanager::Alertmanager;
use nobs_core::compose::Compose;
use nobs_core::lab;
use nobs_core::loki::Loki;
use nobs_core::prefect::Prefect;
use nobs_core::prometheus::Prometheus;
use nobs_core::runner::{Output, Runner};
use nobs_core::sonda::Sonda;
use nobs_core::sot::SourceOfTruth;
use nobs_core::status::{probe_all, State, Urls};

use crate::data;
use crate::shell::{
    env_value, sonda_url, Command, Health, SondaTarget, Stack, StackKind, Tab, Update,
    WINDOW_MINUTES,
};

const PS_PERIOD: Duration = Duration::from_secs(2);
const HEALTH_PERIOD: Duration = Duration::from_secs(5);
const LOGS_PERIOD: Duration = Duration::from_secs(2);
const SOT_PERIOD: Duration = Duration::from_secs(10);
const METRICS_PERIOD: Duration = Duration::from_secs(2);
const ALERTS_PERIOD: Duration = Duration::from_secs(2);
const DEVICE_LOGS_PERIOD: Duration = Duration::from_secs(2);
const FLOWS_PERIOD: Duration = Duration::from_secs(5);
const DEVICE_LOG_LIMIT: u64 = 200;
const FLOW_RUN_LIMIT: u64 = 100;
const METRICS_QUERY: &str =
    r#"{__name__=~"bgp_oper_state|interface_admin_state|interface_oper_state"}"#;
const FORWARD_WAIT: Duration = Duration::from_millis(50);
const SONDA_KEY_ENV: &str = "SONDA_API_KEY";

struct Source {
    commands: Sender<Command>,
}

pub struct Pollers {
    ps: Source,
    health: Source,
    logs: Source,
    sonda: Source,
    sot: Option<Source>,
    prometheus: Option<Source>,
    alertmanager: Option<Source>,
    loki: Option<Source>,
    prefect: Option<Source>,
}

enum Wake {
    Command(Command),
    Timeout,
    Stop,
}

struct Guard {
    source: &'static str,
    updates: Sender<Update>,
    armed: bool,
}

impl Guard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.updates.send(Update::Stopped(self.source.to_string()));
        }
    }
}

struct Tail {
    stack: usize,
    service: String,
    compose: Compose,
    last: Option<String>,
    following: bool,
}

impl Pollers {
    pub fn start(
        runner: Arc<dyn Runner + Send + Sync>,
        stacks: Vec<Stack>,
        selected: usize,
        sonda: SondaTarget,
        source: Option<Arc<dyn SourceOfTruth>>,
        urls: Option<Urls>,
    ) -> (Pollers, Receiver<Update>) {
        let (updates, out) = mpsc::channel();
        let stacks: Arc<[Stack]> = stacks.into();
        let ps = {
            let (runner, stacks) = (Arc::clone(&runner), Arc::clone(&stacks));
            spawn("ps", &updates, move |inbox, out| {
                ps(runner.as_ref(), &stacks, inbox, out)
            })
        };
        let health = {
            let stacks = Arc::clone(&stacks);
            spawn("health", &updates, move |inbox, out| {
                health(&stacks, selected, inbox, out)
            })
        };
        let logs = {
            let stacks = Arc::clone(&stacks);
            spawn("logs", &updates, move |inbox, out| {
                logs(runner.as_ref(), &stacks, inbox, out)
            })
        };
        let sonda = spawn("sonda", &updates, move |inbox, out| {
            forward_sonda(&stacks, selected, &sonda, inbox, out)
        });
        let alertmanager = urls.as_ref().map(|urls| {
            let client = Alertmanager::new(&urls.alertmanager);
            spawn("alertmanager", &updates, move |inbox, out| {
                every(ALERTS_PERIOD, inbox, out, || {
                    Update::Alerts(client.alerts(true, true).map_err(|error| error.to_string()))
                })
            })
        });
        let with_source = urls.as_ref().zip(source);
        let sot = with_source.as_ref().map(|(_, source)| {
            let source = Arc::clone(source);
            spawn("sot", &updates, move |inbox, out| {
                every(SOT_PERIOD, inbox, out, || {
                    Update::Devices(source.devices().map_err(|error| error.to_string()))
                })
            })
        });
        let prometheus = with_source.as_ref().map(|(urls, _)| {
            let client = Prometheus::new(&urls.prometheus);
            spawn("prometheus", &updates, move |inbox, out| {
                every(METRICS_PERIOD, inbox, out, || {
                    Update::Metrics(
                        client
                            .samples(METRICS_QUERY)
                            .map_err(|error| error.to_string()),
                    )
                })
            })
        });
        let loki = with_source.as_ref().map(|(urls, _)| {
            let client = Loki::new(&urls.loki);
            spawn("loki", &updates, move |inbox, out| {
                device_logs(&client, inbox, out)
            })
        });
        let prefect = with_source.as_ref().map(|(urls, _)| {
            let client = Prefect::new(&urls.prefect);
            spawn("prefect", &updates, move |inbox, out| {
                flows(&client, inbox, out)
            })
        });
        (
            Pollers {
                ps,
                health,
                logs,
                sonda,
                sot,
                prometheus,
                alertmanager,
                loki,
                prefect,
            },
            out,
        )
    }

    pub fn send(&self, command: Command) {
        let (services, devices): (&[&Source], &[&Option<Source>]) = match command {
            Command::Quit | Command::Refresh => (
                &[&self.ps, &self.health, &self.logs, &self.sonda],
                &[
                    &self.sot,
                    &self.prometheus,
                    &self.alertmanager,
                    &self.loki,
                    &self.prefect,
                ],
            ),
            Command::SelectStack(_) => (&[&self.health, &self.sonda], &[]),
            Command::OpenLogs { .. } | Command::CloseLogs => (&[&self.logs], &[]),
            Command::Follow(_) => (&[&self.logs], &[]),
            Command::FollowDevice(_) => (&[], &[&self.loki]),
            Command::Sonda(_) => (&[&self.sonda], &[]),
            Command::SelectDevice(_) | Command::OpenTab(_) | Command::Window(_) => {
                (&[], &[&self.loki, &self.prefect])
            }
        };
        for source in services
            .iter()
            .copied()
            .chain(devices.iter().copied().flatten())
        {
            let _ = source.commands.send(command.clone());
        }
    }
}

fn spawn(
    name: &'static str,
    updates: &Sender<Update>,
    work: impl FnOnce(&Receiver<Command>, &Sender<Update>) -> bool + Send + 'static,
) -> Source {
    let (commands, inbox) = mpsc::channel();
    let mut guard = Guard {
        source: name,
        updates: updates.clone(),
        armed: true,
    };
    thread::spawn(move || {
        if work(&inbox, &guard.updates) {
            guard.disarm();
        }
    });
    Source { commands }
}

fn wait(inbox: &Receiver<Command>, until: Option<Instant>) -> Wake {
    let received = match until {
        Some(at) => inbox.recv_timeout(at.saturating_duration_since(Instant::now())),
        None => inbox.recv().map_err(|_| RecvTimeoutError::Disconnected),
    };
    match received {
        Ok(Command::Quit) | Err(RecvTimeoutError::Disconnected) => Wake::Stop,
        Ok(command) => Wake::Command(command),
        Err(RecvTimeoutError::Timeout) => Wake::Timeout,
    }
}

fn ps(
    runner: &dyn Runner,
    stacks: &[Stack],
    inbox: &Receiver<Command>,
    out: &Sender<Update>,
) -> bool {
    loop {
        for (index, stack) in stacks
            .iter()
            .enumerate()
            .filter(|(_, stack)| stack.kind != StackKind::Sonda)
        {
            if out
                .send(Update::Services(index, services(runner, stack)))
                .is_err()
            {
                return true;
            }
        }
        let next = Instant::now() + PS_PERIOD;
        loop {
            match wait(inbox, Some(next)) {
                Wake::Stop => return true,
                Wake::Timeout | Wake::Command(Command::Refresh) => break,
                Wake::Command(_) => {}
            }
        }
    }
}

fn health(
    stacks: &[Stack],
    mut selected: usize,
    inbox: &Receiver<Command>,
    out: &Sender<Update>,
) -> bool {
    loop {
        if let Some(stack) = stacks.get(selected) {
            if out.send(Update::Health(selected, probe(stack))).is_err() {
                return true;
            }
        }
        let next = Instant::now() + HEALTH_PERIOD;
        loop {
            match wait(inbox, Some(next)) {
                Wake::Stop => return true,
                Wake::Timeout | Wake::Command(Command::Refresh) => break,
                Wake::Command(Command::SelectStack(index)) => {
                    selected = index;
                    break;
                }
                Wake::Command(_) => {}
            }
        }
    }
}

fn logs(
    runner: &dyn Runner,
    stacks: &[Stack],
    inbox: &Receiver<Command>,
    out: &Sender<Update>,
) -> bool {
    let mut tail: Option<Tail> = None;
    let mut next = None;
    loop {
        let polling = match wait(inbox, next) {
            Wake::Stop => return true,
            Wake::Timeout => true,
            Wake::Command(Command::OpenLogs { stack, service }) => {
                tail = stacks.get(stack).map(|found| Tail {
                    stack,
                    service,
                    compose: compose_of(found),
                    last: None,
                    following: true,
                });
                tail.is_some()
            }
            Wake::Command(Command::Follow(on)) => match tail.as_mut() {
                Some(tail) => {
                    tail.following = on;
                    on
                }
                None => false,
            },
            Wake::Command(Command::CloseLogs) => {
                tail = None;
                false
            }
            Wake::Command(Command::Refresh) => tail.as_ref().is_some_and(|tail| tail.following),
            Wake::Command(_) => continue,
        };
        let Some(current) = tail.as_mut().filter(|_| polling) else {
            next = None;
            continue;
        };
        let update = Update::Logs(
            current.stack,
            current.service.clone(),
            fetch(runner, current),
        );
        if out.send(update).is_err() {
            return true;
        }
        next = Some(Instant::now() + LOGS_PERIOD);
    }
}

fn every(
    period: Duration,
    inbox: &Receiver<Command>,
    out: &Sender<Update>,
    mut poll: impl FnMut() -> Update,
) -> bool {
    loop {
        if out.send(poll()).is_err() {
            return true;
        }
        let next = Instant::now() + period;
        loop {
            match wait(inbox, Some(next)) {
                Wake::Stop => return true,
                Wake::Timeout | Wake::Command(Command::Refresh) => break,
                Wake::Command(_) => {}
            }
        }
    }
}

fn device_logs(client: &Loki, inbox: &Receiver<Command>, out: &Sender<Update>) -> bool {
    let mut device: Option<String> = None;
    let mut tab = Tab::Overview;
    let mut window = WINDOW_MINUTES;
    let mut following = true;
    let mut next = None;
    loop {
        let polling = match wait(inbox, next) {
            Wake::Stop => return true,
            Wake::Timeout | Wake::Command(Command::Refresh) => true,
            Wake::Command(Command::SelectDevice(name)) => {
                device = Some(name);
                true
            }
            Wake::Command(Command::OpenTab(opened)) => {
                tab = opened;
                true
            }
            Wake::Command(Command::Window(minutes)) => {
                window = minutes;
                true
            }
            Wake::Command(Command::FollowDevice(on)) => {
                following = on;
                on
            }
            Wake::Command(_) => continue,
        };
        let asked = tab == Tab::Timeline || (tab == Tab::Logs && following);
        let Some(name) = device.as_deref().filter(|_| polling && asked) else {
            next = None;
            continue;
        };
        let now = SystemTime::now();
        let lines = client
            .query_range(
                &format!(r#"{{device="{name}"}}"#),
                window,
                DEVICE_LOG_LIMIT,
                now,
            )
            .map_err(|error| error.to_string());
        if out
            .send(Update::DeviceLogs(name.to_string(), lines))
            .is_err()
        {
            return true;
        }
        if tab == Tab::Timeline {
            let query = format!(r#"{{source="prefect", ai_rca="true", device="{name}"}}"#);
            let rcas = client
                .query_range(&query, window, DEVICE_LOG_LIMIT, now)
                .map_err(|error| error.to_string());
            if out.send(Update::Rcas(name.to_string(), rcas)).is_err() {
                return true;
            }
        }
        next = Some(Instant::now() + DEVICE_LOGS_PERIOD);
    }
}

fn flows(client: &Prefect, inbox: &Receiver<Command>, out: &Sender<Update>) -> bool {
    let mut timeline = false;
    let mut next = None;
    loop {
        let polling = match wait(inbox, next) {
            Wake::Stop => return true,
            Wake::Timeout | Wake::Command(Command::Refresh) => true,
            Wake::Command(Command::OpenTab(opened)) => {
                timeline = opened == Tab::Timeline;
                true
            }
            Wake::Command(_) => continue,
        };
        if !(polling && timeline) {
            next = None;
            continue;
        }
        let runs = client
            .flow_runs(FLOW_RUN_LIMIT)
            .map_err(|error| error.to_string());
        if out.send(Update::FlowRuns(runs)).is_err() {
            return true;
        }
        next = Some(Instant::now() + FLOWS_PERIOD);
    }
}

fn forward_sonda(
    stacks: &[Stack],
    selected: usize,
    target: &SondaTarget,
    inbox: &Receiver<Command>,
    out: &Sender<Update>,
) -> bool {
    let (mut poller, mut updates, mut notes) = start_sonda(stacks, selected, target);
    loop {
        match wait(inbox, Some(Instant::now() + FORWARD_WAIT)) {
            Wake::Stop => {
                poller.send(data::Command::Stop);
                return true;
            }
            Wake::Timeout => {}
            Wake::Command(Command::Sonda(command)) => poller.send(command),
            Wake::Command(Command::Refresh) => poller.send(data::Command::Refresh),
            Wake::Command(Command::SelectStack(index)) => {
                poller.send(data::Command::Stop);
                (poller, updates, notes) = start_sonda(stacks, index, target);
            }
            Wake::Command(_) => {}
        }
        loop {
            match updates.try_recv() {
                Ok(update) => {
                    if out.send(Update::Sonda(update)).is_err() {
                        return true;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
        for note in notes.try_iter() {
            if out.send(Update::Note(note)).is_err() {
                return true;
            }
        }
    }
}

fn start_sonda(
    stacks: &[Stack],
    index: usize,
    target: &SondaTarget,
) -> (data::Poller, Receiver<data::Update>, Receiver<data::Note>) {
    let stack = stacks.get(index);
    let key = target.api_key.clone().or_else(|| {
        stack
            .and_then(|stack| env_value(&stack.env, SONDA_KEY_ENV))
            .map(str::to_string)
    });
    let client = Sonda::new(sonda_url(target.server.as_deref(), stack)).api_key(key);
    data::Poller::start(client, crate::PERIOD)
}

fn compose_of(stack: &Stack) -> Compose {
    let compose = Compose::new(stack.compose.clone());
    match &stack.project {
        Some(project) => compose.project(project.clone()),
        None => compose,
    }
}

fn services(runner: &dyn Runner, stack: &Stack) -> Result<Vec<lab::Service>, String> {
    let mut args = compose_of(stack).ps_args(&[]);
    args.extend(["--format", "json"].map(OsString::from));
    answer(runner.output(Compose::PROGRAM, &args), |stdout| {
        lab::services(stdout).map_err(|error| format!("cannot read ps output: {error}"))
    })
}

fn fetch(runner: &dyn Runner, tail: &mut Tail) -> Result<Vec<String>, String> {
    let args = tail
        .compose
        .logs_poll_args(&tail.service, tail.last.as_deref());
    let lines = answer(runner.output(Compose::PROGRAM, &args), |stdout| {
        Ok(stdout.lines().map(str::to_string).collect::<Vec<_>>())
    })?;
    let fresh: Vec<String> = lines
        .into_iter()
        .filter(|line| {
            tail.last
                .as_deref()
                .is_none_or(|last| first_word(line) != last)
        })
        .collect();
    if let Some(line) = fresh.last() {
        tail.last = Some(first_word(line).to_string());
    }
    Ok(fresh)
}

fn first_word(line: &str) -> &str {
    line.split_once(' ').map_or(line, |(word, _)| word)
}

fn answer<T>(
    output: io::Result<Output>,
    parse: impl FnOnce(&str) -> Result<T, String>,
) -> Result<T, String> {
    let output = output.map_err(|error| format!("cannot run docker: {error}"))?;
    if output.code == 0 {
        return parse(&output.stdout);
    }
    Err(output
        .stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map_or_else(|| format!("exit {}", output.code), str::to_string))
}

fn probe(stack: &Stack) -> Vec<Health> {
    let urls = Urls::from_lookup(|name| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| env_value(&stack.env, name).map(str::to_string))
    });
    probe_all(&urls)
        .into_iter()
        .map(|probe| Health {
            service: probe.target.name.to_lowercase(),
            state: match probe.state {
                State::Ok => "ok".to_string(),
                State::Http(code) => format!("http {code}"),
                State::Down(_) => "down".to_string(),
            },
        })
        .collect()
}
