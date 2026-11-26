use std::borrow::Cow;
#[cfg(any(test, feature = "fake"))]
use std::cell::RefCell;
#[cfg(any(test, feature = "fake"))]
use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Read, Write};
use std::ops::RangeInclusive;
use std::path::PathBuf;
#[cfg(any(test, feature = "fake"))]
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::count::counted;
use crate::runner::Output;
use crate::{intended, lab};

pub const ANSWER_LIMIT: Duration = Duration::from_secs(10);

pub const USER: &str = "NETWORK_AGENT_USER";
pub const PASSWORD: &str = "NETWORK_AGENT_PASSWORD";

#[derive(Clone, PartialEq, Eq)]
pub struct Password(String);

impl Password {
    pub fn new(text: String) -> Self {
        Password(text)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(..)")
    }
}

#[derive(Debug)]
pub struct Credentials {
    pub user: String,
    pub password: Password,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    NotANode(PathBuf),
    NoDriver(String),
    MissingCredentials,
    Unreachable { host: String, reason: String },
    NoAnswer,
    AuthenticationFailed,
    NoExitStatus,
    ShellRefused,
    Library(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotANode(topology) => write!(f, "not a node of {}", topology.display()),
            Error::NoDriver(kind) => write!(f, "no driver for the kind {kind} yet"),
            Error::MissingCredentials => f.write_str(
                "set NETWORK_AGENT_USER and NETWORK_AGENT_PASSWORD in the lab's .env file or in the shell",
            ),
            Error::Unreachable { host, reason } => write!(f, "cannot reach {host}: {reason}"),
            Error::NoAnswer => write!(f, "no answer in {} seconds", ANSWER_LIMIT.as_secs()),
            Error::AuthenticationFailed => f.write_str("authentication failed"),
            Error::NoExitStatus => f.write_str("the device sent no exit status"),
            Error::ShellRefused => f.write_str("the device refused a shell"),
            Error::Library(text) => f.write_str(text),
        }
    }
}

impl Error {
    pub fn says_nothing_was_sent(&self) -> bool {
        matches!(
            self,
            Error::Unreachable { .. } | Error::AuthenticationFailed | Error::ShellRefused
        )
    }
}

pub trait Transport {
    fn run(&self, host: &str, credentials: &Credentials, command: &str) -> Result<Output, Error>;
    fn shell(
        &self,
        host: &str,
        credentials: &Credentials,
        terminal: &mut dyn Terminal,
    ) -> Result<i32, Error>;
    fn send(
        &self,
        host: &str,
        credentials: &Credentials,
        lines: &[String],
    ) -> Result<Output, Error>;
}

pub trait Terminal {
    fn kind(&self) -> String;
    fn size(&self) -> (u16, u16);
    /// Taken once, after the device granted the shell.
    fn keys(&mut self) -> io::Result<Box<dyn Read + Send>>;
    fn screen(&mut self) -> &mut dyn Write;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Driver {
    pub running_config: &'static str,
    pub open: &'static [&'static str],
    pub commit: &'static str,
    pub syntax: Syntax,
    pub flap: Flapping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flapping {
    pub down: &'static str,
    pub admin_state: &'static str,
    pub commit_after: &'static str,
    pub management: &'static str,
    pub name_letters: &'static str,
}

impl Flapping {
    fn down(&self, interface: &str) -> String {
        self.down.replace("{}", interface)
    }

    fn admin_state(&self, interface: &str) -> String {
        self.admin_state.replace("{}", interface)
    }

    fn commit_after(&self, delay: Duration) -> String {
        self.commit_after
            .replace("{}", &delay.as_secs().to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Words<'a> {
    Read(Vec<Cow<'a, str>>),
    Unread(&'a str),
}

impl Words<'_> {
    pub(crate) fn of(syntax: Option<Syntax>, line: &str) -> Words<'_> {
        let line = line.trim();
        match syntax {
            Some(Syntax::SrLinux) => srlinux_words(line).map_or(Words::Unread(line), Words::Read),
            None => Words::Unread(line),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    SrLinux,
}

const SEPARATORS: [char; 2] = [' ', '\t'];

fn srlinux_words(line: &str) -> Option<Vec<Cow<'_, str>>> {
    let mut words = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        let (word, after) = srlinux_word(rest)?;
        if !after.is_empty() && !after.starts_with(SEPARATORS) {
            return None;
        }
        words.push(word);
        rest = after.trim_start_matches(SEPARATORS);
    }
    Some(words)
}

fn srlinux_word(text: &str) -> Option<(Cow<'_, str>, &str)> {
    if let Some(inner) = text.strip_prefix('\'') {
        let (value, after) = inner.split_once('\'')?;
        return Some((Cow::Borrowed(value), after));
    }
    if let Some(inner) = text.strip_prefix('"') {
        return double_quoted(inner);
    }
    let end = text.find(SEPARATORS).unwrap_or(text.len());
    let (word, after) = text.split_at(end);
    (!word.contains(['"', '\'', '\\'])).then_some((Cow::Borrowed(word), after))
}

fn double_quoted(text: &str) -> Option<(Cow<'_, str>, &str)> {
    let mut value = String::new();
    let mut chars = text.char_indices();
    while let Some((at, letter)) = chars.next() {
        match letter {
            '"' => return Some((Cow::Owned(value), &text[at + 1..])),
            '\\' => match chars.next()? {
                (_, escaped @ ('"' | '\\')) => value.push(escaped),
                _ => return None,
            },
            _ => value.push(letter),
        }
    }
    None
}

pub fn driver(kind: &str) -> Result<Driver, Error> {
    if lab::SRLINUX_KINDS.contains(&kind) {
        Ok(Driver {
            running_config: "info flat",
            open: &["enter candidate private name nobs", "discard stay"],
            commit: "commit now",
            syntax: Syntax::SrLinux,
            flap: Flapping {
                down: "set / interface {} admin-state disable",
                admin_state: "info flat interface {} admin-state",
                commit_after: "commit confirmed timeout {}",
                management: "mgmt0",
                name_letters: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-/",
            },
        })
    } else {
        Err(Error::NoDriver(kind.to_string()))
    }
}

pub fn step<T>(
    node: &lab::TopologyNode,
    inside: impl FnOnce(&str, &Credentials, Driver) -> Result<T, Error>,
) -> Result<T, Error> {
    let driver = driver(&node.kind)?;
    let credentials = credentials()?;
    inside(&node.container, &credentials, driver)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Pushed {
    Declined,
    Taken(Output),
    Refused(Output),
    NotKnown(Error),
}

pub fn push(
    node: &lab::TopologyNode,
    transport: &dyn Transport,
    text: &str,
    yes: &mut dyn FnMut() -> bool,
) -> Result<Pushed, Error> {
    step(node, |host, credentials, driver| {
        if !yes() {
            return Ok(Pushed::Declined);
        }
        let lines = session_lines(driver, intended::lines(text), driver.commit);
        Ok(match sent(transport, host, credentials, &lines)? {
            Sent::Taken(output) => Pushed::Taken(output),
            Sent::Refused(output) => Pushed::Refused(output),
            Sent::NotKnown(error) => Pushed::NotKnown(error),
        })
    })
}

enum Sent {
    Taken(Output),
    Refused(Output),
    NotKnown(Error),
}

fn sent(
    transport: &dyn Transport,
    host: &str,
    credentials: &Credentials,
    lines: &[String],
) -> Result<Sent, Error> {
    match transport.send(host, credentials, lines) {
        Ok(output) if output.code == 0 => Ok(Sent::Taken(output)),
        Ok(output) => Ok(Sent::Refused(output)),
        Err(error) if error.says_nothing_was_sent() => Err(error),
        Err(error) => Ok(Sent::NotKnown(error)),
    }
}

fn session_lines<'a>(
    driver: Driver,
    lines: impl Iterator<Item = &'a str>,
    commit: &'a str,
) -> Vec<String> {
    driver
        .open
        .iter()
        .copied()
        .chain(lines)
        .chain([commit])
        .map(str::to_string)
        .collect()
}

pub const FLAP_DELAY: u64 = 5;
pub const FLAP_COUNT: u32 = 1;
pub(crate) const FLAP_DELAYS: RangeInclusive<u64> = 1..=300;
pub(crate) const FLAP_COUNTS: RangeInclusive<u32> = 1..=10;
const BACK_READS: u32 = 10;
const BACK_READ_GAP: Duration = Duration::from_secs(1);

pub struct Flap<'a> {
    pub interface: &'a str,
    pub delay: Duration,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unfit {
    Name(String),
    Management(&'static str),
    Delay,
    Count,
}

impl fmt::Display for Unfit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unfit::Name(interface) => {
                write!(f, "\"{interface}\" is not the name of one interface")
            }
            Unfit::Management(name) => write!(f, "{name} is the management interface"),
            Unfit::Delay => write!(f, "the delay must be {}", delay_limits()),
            Unfit::Count => write!(f, "the count must be {}", count_limits()),
        }
    }
}

pub fn delay_limits() -> String {
    format!("{} to {} seconds", FLAP_DELAYS.start(), FLAP_DELAYS.end())
}

pub fn count_limits() -> String {
    format!("{} to {}", FLAP_COUNTS.start(), FLAP_COUNTS.end())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Flapped {
    Declined,
    Unfit(Unfit),
    Disabled,
    Refused(Output),
    NotKnown(Error),
    NotBack(NotBack),
    Back,
}

#[derive(Debug, PartialEq, Eq)]
pub enum NotBack {
    ReadFailed(Error),
    Refused(Output),
    StillDisabled,
}

pub enum Turn {
    Down,
    Back,
}

pub fn flap(
    node: &lab::TopologyNode,
    transport: &dyn Transport,
    flap: &Flap,
    yes: &mut dyn FnMut() -> bool,
    wait: &mut dyn FnMut(Duration),
    told: &mut dyn FnMut(u32, Turn),
) -> Result<Flapped, Error> {
    let driver = driver(&node.kind)?;
    if let Some(unfit) = unfit(driver, flap) {
        return Ok(Flapped::Unfit(unfit));
    }
    let credentials = credentials()?;
    let flapper = Flapper {
        transport,
        host: &node.container,
        credentials: &credentials,
        driver,
        flap,
    };
    flapper.flapped(yes, wait, told)
}

pub fn flap_question(name: &str, flap: &Flap) -> String {
    format!(
        "flap {} of {name}? It goes down for {}, {}; the device brings it back. y/n",
        flap.interface,
        counted(flap.delay.as_secs(), "second"),
        counted(flap.count, "time")
    )
}

pub fn unfit(driver: Driver, flap: &Flap) -> Option<Unfit> {
    let interface = flap.interface;
    if interface.is_empty()
        || !interface
            .chars()
            .all(|c| driver.flap.name_letters.contains(c))
    {
        Some(Unfit::Name(interface.to_string()))
    } else if interface == driver.flap.management {
        Some(Unfit::Management(driver.flap.management))
    } else if !FLAP_DELAYS.contains(&flap.delay.as_secs()) {
        Some(Unfit::Delay)
    } else if !FLAP_COUNTS.contains(&flap.count) {
        Some(Unfit::Count)
    } else {
        None
    }
}

struct Flapper<'a> {
    transport: &'a dyn Transport,
    host: &'a str,
    credentials: &'a Credentials,
    driver: Driver,
    flap: &'a Flap<'a>,
}

impl Flapper<'_> {
    fn flapped(
        &self,
        yes: &mut dyn FnMut() -> bool,
        wait: &mut dyn FnMut(Duration),
        told: &mut dyn FnMut(u32, Turn),
    ) -> Result<Flapped, Error> {
        if !yes() {
            return Ok(Flapped::Declined);
        }
        let before = self.read()?;
        if before.code != 0 {
            return Ok(Flapped::Refused(before));
        }
        if self.disabled(&before) {
            return Ok(Flapped::Disabled);
        }
        for n in 1..=self.flap.count {
            if n > 1 {
                wait(self.flap.delay);
            }
            if let Some(end) = self.once(n, wait, told)? {
                return Ok(end);
            }
            told(n, Turn::Back);
        }
        Ok(Flapped::Back)
    }

    fn once(
        &self,
        n: u32,
        wait: &mut dyn FnMut(Duration),
        told: &mut dyn FnMut(u32, Turn),
    ) -> Result<Option<Flapped>, Error> {
        let flapping = self.driver.flap;
        let down = flapping.down(self.flap.interface);
        let commit = flapping.commit_after(self.flap.delay);
        let lines = session_lines(self.driver, [down.as_str()].into_iter(), &commit);
        match sent(self.transport, self.host, self.credentials, &lines)? {
            Sent::Taken(_) => {}
            Sent::Refused(output) => return Ok(Some(Flapped::Refused(output))),
            Sent::NotKnown(error) => return Ok(Some(Flapped::NotKnown(error))),
        }
        told(n, Turn::Down);
        wait(self.flap.delay);
        Ok(self.not_back(wait))
    }

    fn not_back(&self, wait: &mut dyn FnMut(Duration)) -> Option<Flapped> {
        let mut refused = None;
        for read in 1..=BACK_READS {
            if read > 1 {
                wait(BACK_READ_GAP);
            }
            match self.read() {
                Err(error) => return Some(Flapped::NotBack(NotBack::ReadFailed(error))),
                Ok(output) if output.code != 0 => refused = Some(output),
                Ok(output) if !self.disabled(&output) => return None,
                Ok(_) => {}
            }
        }
        let last = refused.map_or(NotBack::StillDisabled, NotBack::Refused);
        Some(Flapped::NotBack(last))
    }

    fn read(&self) -> Result<Output, Error> {
        let command = self.driver.flap.admin_state(self.flap.interface);
        self.transport.run(self.host, self.credentials, &command)
    }

    fn disabled(&self, read: &Output) -> bool {
        let syntax = Some(self.driver.syntax);
        let down = self.driver.flap.down(self.flap.interface);
        let down = Words::of(syntax, &down);
        read.stdout
            .lines()
            .any(|line| Words::of(syntax, line) == down)
    }
}

pub fn credentials() -> Result<Credentials, Error> {
    let found = |name| credential(name, std::env::var(name).ok(), &[]);
    Ok(Credentials {
        user: found(USER).ok_or(Error::MissingCredentials)?,
        password: Password::new(found(PASSWORD).ok_or(Error::MissingCredentials)?),
    })
}

pub fn credential(name: &str, shell: Option<String>, lab: &[(String, String)]) -> Option<String> {
    let filled = |value: &String| !value.trim().is_empty();
    shell.filter(filled).or_else(|| {
        lab.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .filter(filled)
    })
}

pub fn error_line(name: &str, error: &Error) -> String {
    format!("nobs: ssh {name}: {error}")
}

pub fn pushed_line(name: &str, count: usize) -> String {
    format!("{name}  pushed {}", counted(count, "line"))
}

pub fn unfit_line(name: &str, unfit: &Unfit) -> String {
    format!("nobs: flap {name}: {unfit}")
}

pub fn not_known_line(name: &str) -> String {
    format!("nobs: push {name}: not known whether the device took the push; run nobs device check")
}

#[cfg(any(test, feature = "fake"))]
pub struct FakeTransport {
    answers: Answers,
    calls: RefCell<Vec<(String, String)>>,
}

#[cfg(any(test, feature = "fake"))]
enum Answers {
    PerHost(Vec<(Option<String>, Result<Output, Error>)>),
    InOrder(RefCell<VecDeque<Result<Output, Error>>>),
}

#[cfg(any(test, feature = "fake"))]
impl FakeTransport {
    fn with(answers: Answers) -> Self {
        Self {
            answers,
            calls: RefCell::new(Vec::new()),
        }
    }

    pub fn answering(answer: Result<Output, Error>) -> Self {
        Self::with(Answers::PerHost(vec![(None, answer)]))
    }

    pub fn per_host(answers: Vec<(String, Result<Output, Error>)>) -> Self {
        Self::with(Answers::PerHost(
            answers
                .into_iter()
                .map(|(host, answer)| (Some(host), answer))
                .collect(),
        ))
    }

    pub fn in_order(answers: Vec<Result<Output, Error>>) -> Self {
        Self::with(Answers::InOrder(RefCell::new(answers.into())))
    }

    fn answer(&self, host: &str) -> Result<Output, Error> {
        match &self.answers {
            Answers::PerHost(answers) => answers
                .iter()
                .find(|(known, _)| known.as_deref().is_none_or(|known| known == host))
                .map(|(_, answer)| answer.clone()),
            Answers::InOrder(answers) => answers.borrow_mut().pop_front(),
        }
        .unwrap_or_else(|| panic!("no answer for {host}"))
    }

    pub fn calls(&self) -> Vec<(String, String)> {
        self.calls.borrow().clone()
    }
}

#[cfg(any(test, feature = "fake"))]
impl Transport for FakeTransport {
    fn run(&self, host: &str, _credentials: &Credentials, command: &str) -> Result<Output, Error> {
        self.calls
            .borrow_mut()
            .push((host.to_string(), command.to_string()));
        self.answer(host)
    }

    fn shell(
        &self,
        host: &str,
        _credentials: &Credentials,
        terminal: &mut dyn Terminal,
    ) -> Result<i32, Error> {
        self.calls
            .borrow_mut()
            .push((host.to_string(), terminal.kind()));
        let _ = terminal.keys();
        self.answer(host).map(|output| output.code)
    }

    fn send(
        &self,
        host: &str,
        _credentials: &Credentials,
        lines: &[String],
    ) -> Result<Output, Error> {
        self.calls
            .borrow_mut()
            .push((host.to_string(), lines.join("\n")));
        self.answer(host)
    }
}

#[cfg(any(test, feature = "fake"))]
pub struct FakeTerminal {
    pub kind: String,
    pub size: Arc<Mutex<(u16, u16)>>,
    pub keys: Option<Box<dyn Read + Send>>,
    pub screen: Box<dyn Write + Send>,
}

#[cfg(any(test, feature = "fake"))]
impl Terminal for FakeTerminal {
    fn kind(&self) -> String {
        self.kind.clone()
    }

    fn size(&self) -> (u16, u16) {
        *self.size.lock().unwrap()
    }

    fn keys(&mut self) -> io::Result<Box<dyn Read + Send>> {
        self.keys.take().ok_or(io::ErrorKind::NotFound.into())
    }

    fn screen(&mut self) -> &mut dyn Write {
        &mut self.screen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_error_prints_its_pinned_reason() {
        let cases = [
            (
                Error::NotANode(PathBuf::from("/lab/containerlab/lab.yml")),
                "not a node of /lab/containerlab/lab.yml",
            ),
            (
                Error::NoDriver("ceos".to_string()),
                "no driver for the kind ceos yet",
            ),
            (
                Error::MissingCredentials,
                "set NETWORK_AGENT_USER and NETWORK_AGENT_PASSWORD in the lab's .env file or in the shell",
            ),
            (
                Error::Unreachable {
                    host: "clab-lab-srl1".to_string(),
                    reason: "connection refused".to_string(),
                },
                "cannot reach clab-lab-srl1: connection refused",
            ),
            (Error::NoAnswer, "no answer in 10 seconds"),
            (Error::AuthenticationFailed, "authentication failed"),
            (Error::NoExitStatus, "the device sent no exit status"),
            (Error::ShellRefused, "the device refused a shell"),
            (
                Error::Library("Connection closed by the remote side".to_string()),
                "Connection closed by the remote side",
            ),
        ];
        for (error, reason) in cases {
            assert_eq!(error.to_string(), reason);
        }
    }

    #[test]
    fn credentials_debug_shows_the_user_and_hides_the_password() {
        let credentials = Credentials {
            user: "admin".to_string(),
            password: Password::new("nobs-t-not-a-secret".to_string()),
        };
        let printed = format!("{credentials:?}");
        assert!(printed.contains("admin"), "{printed}");
        assert!(!printed.contains("nobs-t-not-a-secret"), "{printed}");
    }

    #[test]
    fn a_credential_is_the_shell_value_when_not_blank_else_the_first_lab_value_when_not_blank() {
        let shell = |value: &str| Some(value.to_string());
        let lab = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect()
        };
        let cases = [
            (
                shell("nobs-t-x"),
                lab(&[(USER, "nobs-t-not-a-secret")]),
                Some("nobs-t-x"),
            ),
            (shell(""), lab(&[(USER, "nobs-t-x")]), Some("nobs-t-x")),
            (shell("  "), lab(&[(USER, "nobs-t-x")]), Some("nobs-t-x")),
            (shell(" \t"), lab(&[]), None),
            (None, lab(&[(USER, "  ")]), None),
            (shell(""), lab(&[(USER, " ")]), None),
            (
                None,
                lab(&[(USER, "nobs-t-x"), (USER, "nobs-t-not-a-secret")]),
                Some("nobs-t-x"),
            ),
            (None, lab(&[(USER, ""), (USER, "nobs-t-x")]), None),
            (None, lab(&[(USER, "  "), (USER, "nobs-t-x")]), None),
            (None, lab(&[(PASSWORD, "nobs-t-x")]), None),
            (None, lab(&[]), None),
            (shell(" nobs-t-x "), lab(&[]), Some(" nobs-t-x ")),
        ];
        for (case, (shell, lab, found)) in cases.into_iter().enumerate() {
            assert_eq!(
                credential(USER, shell, &lab).as_deref(),
                found,
                "case {case}"
            );
        }
    }

    #[test]
    fn the_error_line_names_the_device_and_the_error() {
        assert_eq!(
            error_line("nobs-t-srl1", &Error::AuthenticationFailed),
            "nobs: ssh nobs-t-srl1: authentication failed"
        );
    }

    #[test]
    fn each_srlinux_kind_fetches_info_flat_and_any_other_kind_has_no_driver() {
        for kind in ["nokia_srlinux", "srl"] {
            assert_eq!(
                driver(kind),
                Ok(Driver {
                    running_config: "info flat",
                    open: &["enter candidate private name nobs", "discard stay"],
                    commit: "commit now",
                    syntax: Syntax::SrLinux,
                    flap: Flapping {
                        down: "set / interface {} admin-state disable",
                        admin_state: "info flat interface {} admin-state",
                        commit_after: "commit confirmed timeout {}",
                        management: "mgmt0",
                        name_letters:
                            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-/",
                    },
                }),
                "{kind}"
            );
        }
        assert_eq!(driver("ceos"), Err(Error::NoDriver("ceos".to_string())));
    }

    const SRL1: &str =
        include_str!("../../tests/fixtures/project-two-node/expected/configs/nobs-t-srl1.cfg");
    const SRL2: &str =
        include_str!("../../tests/fixtures/project-two-node/expected/configs/nobs-t-srl2.cfg");

    fn node(name: &str, kind: &str) -> lab::TopologyNode {
        lab::TopologyNode {
            name: name.to_string(),
            kind: kind.to_string(),
            container: host(name),
        }
    }

    fn host(name: &str) -> String {
        format!("clab-{name}")
    }

    fn logged_in() {
        std::env::set_var(USER, "admin");
        std::env::set_var(PASSWORD, "nobs-t-not-a-secret");
    }

    fn replied(code: i32, stderr: &str) -> Result<Output, Error> {
        Ok(Output {
            code,
            stdout: String::new(),
            stderr: stderr.to_string(),
        })
    }

    fn session(lines: &[&str]) -> String {
        ["enter candidate private name nobs", "discard stay"]
            .iter()
            .chain(lines)
            .chain(&["commit now"])
            .copied()
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn pushed_with_yes(
        node: &lab::TopologyNode,
        transport: &FakeTransport,
        text: &str,
    ) -> Result<Pushed, Error> {
        push(node, transport, text, &mut || true)
    }

    #[test]
    fn a_push_gives_one_session_the_opening_lines_then_every_line_of_the_text_in_order_then_commit()
    {
        logged_in();
        let cases = [
            ("nobs-t-srl1", "set / a b on\n", session(&["set / a b on"])),
            (
                "nobs-t-srl1",
                "  set / a b on \n\n\tset / c d on\n \nset / e f on",
                session(&["set / a b on", "set / c d on", "set / e f on"]),
            ),
            (
                "nobs-t-srl1",
                SRL1,
                session(&SRL1.lines().collect::<Vec<_>>()),
            ),
            (
                "nobs-t-srl2",
                SRL2,
                session(&SRL2.lines().collect::<Vec<_>>()),
            ),
        ];
        for (case, (name, text, sent)) in cases.into_iter().enumerate() {
            let transport = FakeTransport::answering(replied(0, ""));
            assert_eq!(
                pushed_with_yes(&node(name, "nokia_srlinux"), &transport, text),
                Ok(Pushed::Taken(replied(0, "").unwrap())),
                "case {case}"
            );
            assert_eq!(transport.calls(), [(host(name), sent)], "case {case}");
        }
    }

    #[test]
    fn the_session_lines_are_the_driver_opening_lines_in_order_then_the_text_lines_then_its_commit()
    {
        let text = "  set / a b on \n\n\tset / c d on\n";
        let cases: [(&'static [&'static str], &[&str]); 2] = [
            (
                &["nobs-t-open-1"],
                &[
                    "nobs-t-open-1",
                    "set / a b on",
                    "set / c d on",
                    "nobs-t-commit",
                ],
            ),
            (
                &["nobs-t-open-1", "nobs-t-open-2", "nobs-t-open-3"],
                &[
                    "nobs-t-open-1",
                    "nobs-t-open-2",
                    "nobs-t-open-3",
                    "set / a b on",
                    "set / c d on",
                    "nobs-t-commit",
                ],
            ),
        ];
        for (open, sent) in cases {
            let driver = Driver {
                open,
                ..test_driver()
            };
            assert_eq!(
                session_lines(driver, intended::lines(text), driver.commit),
                sent,
                "{open:?}"
            );
        }
    }

    #[test]
    fn a_no_gives_declined_and_no_session() {
        logged_in();
        let transport = FakeTransport::answering(replied(0, ""));
        let mut asked = 0;
        let mut no = || {
            asked += 1;
            false
        };
        assert_eq!(
            push(&node("nobs-t-srl1", "srl"), &transport, SRL1, &mut no),
            Ok(Pushed::Declined)
        );
        assert_eq!(asked, 1);
        assert_eq!(transport.calls(), []);
    }

    fn pushed_and_asked(node: &lab::TopologyNode) -> (Result<Pushed, Error>, usize, usize) {
        let transport = FakeTransport::answering(replied(0, ""));
        let mut asked = 0;
        let mut yes = || {
            asked += 1;
            true
        };
        let pushed = push(node, &transport, SRL1, &mut yes);
        (pushed, asked, transport.calls().len())
    }

    #[test]
    fn no_driver_or_no_login_never_asks_and_never_opens_a_session() {
        logged_in();
        assert_eq!(
            pushed_and_asked(&node("nobs-t-ceos1", "ceos")),
            (Err(Error::NoDriver("ceos".to_string())), 0, 0)
        );
        std::env::remove_var(USER);
        std::env::remove_var(PASSWORD);
        assert_eq!(
            pushed_and_asked(&node("nobs-t-srl1", "srl")),
            (Err(Error::MissingCredentials), 0, 0)
        );
    }

    #[test]
    fn an_exit_not_zero_is_refused_with_the_device_answer() {
        logged_in();
        let transport = FakeTransport::answering(replied(1, "Error: refused\n"));
        assert_eq!(
            pushed_with_yes(&node("nobs-t-srl1", "srl"), &transport, SRL1),
            Ok(Pushed::Refused(replied(1, "Error: refused\n").unwrap()))
        );
    }

    #[test]
    fn an_unreachable_device_a_refused_login_or_a_refused_shell_in_the_session_is_an_error() {
        logged_in();
        let errors = [
            Error::Unreachable {
                host: host("nobs-t-srl1"),
                reason: "connection refused".to_string(),
            },
            Error::AuthenticationFailed,
            Error::ShellRefused,
        ];
        for error in errors {
            let transport = FakeTransport::answering(Err(error.clone()));
            assert_eq!(
                pushed_with_yes(&node("nobs-t-srl1", "srl"), &transport, SRL1),
                Err(error.clone()),
                "{error:?}"
            );
        }
    }

    #[test]
    fn a_session_with_no_answer_no_exit_status_or_a_library_error_is_not_known() {
        logged_in();
        let errors = [
            Error::NoAnswer,
            Error::NoExitStatus,
            Error::Library("Connection closed by the remote side".to_string()),
        ];
        for error in errors {
            let transport = FakeTransport::answering(Err(error.clone()));
            assert_eq!(
                pushed_with_yes(&node("nobs-t-srl1", "srl"), &transport, SRL1),
                Ok(Pushed::NotKnown(error.clone())),
                "{error:?}"
            );
        }
    }

    #[test]
    fn a_push_to_the_second_of_three_devices_opens_a_session_to_the_second_only() {
        logged_in();
        let nodes = [
            node("nobs-t-srl1", "srl"),
            node("nobs-t-srl2", "srl"),
            node("nobs-t-srl3", "srl"),
        ];
        let transport = FakeTransport::per_host(
            nodes
                .iter()
                .map(|node| (node.container.clone(), replied(0, "")))
                .collect(),
        );
        pushed_with_yes(&nodes[1], &transport, SRL2).unwrap();
        let hosts: Vec<String> = transport
            .calls()
            .into_iter()
            .map(|(host, _)| host)
            .collect();
        assert_eq!(hosts, [host("nobs-t-srl2")]);
    }

    fn read(line: &str) -> Words<'_> {
        Words::of(Some(Syntax::SrLinux), line)
    }

    fn read_as(words: &[&str]) -> Words<'static> {
        Words::Read(
            words
                .iter()
                .map(|word| Cow::Owned(word.to_string()))
                .collect(),
        )
    }

    #[test]
    fn each_line_typed_in_a_template_reads_as_the_line_the_device_prints() {
        let pairs = [
            (
                "set / system information location \"nobs-t-x\"",
                "set / system information location nobs-t-x",
            ),
            (
                "set / system information contact \"nobs-t a b\"",
                "set / system information contact   \"nobs-t a b\"",
            ),
            (
                "set / interface ethernet-1/1 description \"nobs-t:1/2,x=y\"",
                "set / interface ethernet-1/1 description nobs-t:1/2,x=y",
            ),
            (
                "set / system information location 'nobs-t-single'",
                "set / system information location nobs-t-single",
            ),
            (
                "set / system information location 'nobs-t \"q\" s'",
                "set / system information location \"nobs-t \\\"q\\\" s\"",
            ),
            (
                "set / system information contact \"nobs-t it's\"",
                "set / system information contact  \"nobs-t it's\"",
            ),
            (
                "set / system information contact \"nobs-t #x !y\"",
                "set / system information contact 'nobs-t #x !y'",
            ),
            (
                "set / system information location \"nobs-t a\\\\b\"",
                "set / system information location 'nobs-t a\\b'",
            ),
            (
                "set   /   system   information   location   nobs-t-spaces",
                "set / system information location nobs-t-spaces",
            ),
            (
                "set / interface ethernet-1/1 admin-state \"enable\"",
                "set / interface ethernet-1/1 admin-state enable",
            ),
            (
                "set / interface \"ethernet-1/1\" description nobs-t-k",
                "set / interface ethernet-1/1 description nobs-t-k",
            ),
            ("set /\tsystem\t name", "set / system name"),
        ];
        for (typed, printed) in pairs {
            assert!(matches!(read(typed), Words::Read(_)), "{typed}");
            assert_eq!(read(typed), read(printed), "{typed}");
            assert_eq!(read(printed), read(typed), "{printed}");
        }
        assert_eq!(
            read("set / system information contact \"nobs-t a b\""),
            read_as(&["set", "/", "system", "information", "contact", "nobs-t a b"])
        );
        assert_eq!(
            read("location \"nobs-t \\\"q\\\" s\""),
            read_as(&["location", "nobs-t \"q\" s"])
        );
        assert_eq!(read("x \"\""), read_as(&["x", ""]));
    }

    #[test]
    fn lines_the_device_reads_as_other_words_stay_different() {
        let pairs = [
            ("location nobs-t-x", "location nobs-t-y"),
            ("set / a b on", "set / b a on"),
            ("location \"nobs-t a b\"", "location \"nobs-t a  b\""),
            ("location \"nobs-t a b\"", "location nobs-t a b"),
            ("location nobs-t-Lab", "location nobs-t-lab"),
            ("x \"\"", "x"),
        ];
        for (one, other) in pairs {
            assert_ne!(read(one), read(other), "{one} / {other}");
        }
    }

    #[test]
    fn a_form_no_device_showed_is_its_own_text_only() {
        let unread = [
            ("x \"a b", "x \"a b\""),
            ("x 'a", "x a"),
            ("x \"a\\\"", "x \"a\\\"\""),
            ("x \"a\\", "x a"),
            ("x a\\", "x a"),
            ("x \"a\\b\"", "x \"a\\\\b\""),
            ("x it's", "x its"),
            ("x a\"b", "x ab"),
            ("x \"a\"b", "x ab"),
            ("x 'a'b'", "x ab"),
        ];
        for (line, look_alike) in unread {
            assert_eq!(read(line), Words::Unread(line), "{line}");
            assert_eq!(read(&format!("  {line}\t")), read(line), "{line}");
            assert_ne!(read(line), read(look_alike), "{line}");
        }
    }

    #[test]
    fn no_text_of_up_to_five_characters_makes_the_words_panic() {
        let letters = ['a', ' ', '\t', '"', '\'', '\\', 'é'];
        let mut texts = vec![String::new()];
        for _ in 0..5 {
            texts = texts
                .iter()
                .flat_map(|text| letters.map(|letter| format!("{text}{letter}")))
                .chain(texts.iter().cloned())
                .collect();
            texts.sort();
            texts.dedup();
        }
        for text in &texts {
            read(text);
        }
        assert_eq!(texts.len(), 19608);
    }

    #[test]
    fn the_lines_sent_hold_no_save_and_no_startup() {
        logged_in();
        let transport = FakeTransport::answering(replied(0, ""));
        pushed_with_yes(&node("nobs-t-srl1", "srl"), &transport, SRL1).unwrap();
        let [(_, sent)] = transport.calls().try_into().unwrap();
        assert!(!sent.contains("save"), "{sent}");
        assert!(!sent.contains("startup"), "{sent}");
    }

    fn test_driver() -> Driver {
        Driver {
            running_config: "nobs-t-running",
            open: &["nobs-t-open-1"],
            commit: "nobs-t-commit",
            syntax: Syntax::SrLinux,
            flap: Flapping {
                down: "nobs-t-down {}",
                admin_state: "nobs-t-read {}",
                commit_after: "nobs-t-commit-after {}",
                management: "nobs-t-mgmt",
                name_letters: "bfgimnost-*",
            },
        }
    }

    const UP: &str = "set / interface ethernet-1/1 admin-state enable\n";
    const DOWN: &str = "set / interface ethernet-1/1 admin-state disable\n";
    const READ: &str = "info flat interface ethernet-1/1 admin-state";

    fn printed(stdout: &str) -> Result<Output, Error> {
        Ok(Output {
            code: 0,
            stdout: stdout.to_string(),
            stderr: String::new(),
        })
    }

    fn flap_session(seconds: u64) -> String {
        format!(
            "enter candidate private name nobs\ndiscard stay\nset / interface ethernet-1/1 admin-state disable\ncommit confirmed timeout {seconds}"
        )
    }

    fn ethernet(delay: u64, count: u32) -> Flap<'static> {
        Flap {
            interface: "ethernet-1/1",
            delay: Duration::from_secs(delay),
            count,
        }
    }

    fn merged(calls: Vec<(String, String)>, marks: Vec<(usize, String)>) -> Vec<String> {
        let mut log = Vec::new();
        let mut marks = marks.into_iter().peekable();
        for (at, (_, text)) in calls.into_iter().enumerate() {
            while let Some((_, mark)) = marks.next_if(|(before, _)| *before <= at) {
                log.push(mark);
            }
            log.push(text);
        }
        log.extend(marks.map(|(_, mark)| mark));
        log
    }

    fn flapped_on(
        node: &lab::TopologyNode,
        wanted: &Flap,
        answers: Vec<Result<Output, Error>>,
        answer: bool,
    ) -> (Result<Flapped, Error>, Vec<String>) {
        let transport = FakeTransport::in_order(answers);
        let marks = RefCell::new(Vec::new());
        let mark = |text: String| marks.borrow_mut().push((transport.calls().len(), text));
        let flapped = flap(
            node,
            &transport,
            wanted,
            &mut || {
                mark("yes".to_string());
                answer
            },
            &mut |delay| mark(format!("wait {}", delay.as_secs())),
            &mut |n, turn| {
                let turn = match turn {
                    Turn::Down => "down",
                    Turn::Back => "back",
                };
                mark(format!("{turn} {n}"));
            },
        );
        let calls = transport.calls();
        assert!(
            calls.iter().all(|(to, _)| *to == node.container),
            "{calls:?}"
        );
        (flapped, merged(calls, marks.into_inner()))
    }

    fn flapped(
        wanted: &Flap,
        answers: Vec<Result<Output, Error>>,
    ) -> (Result<Flapped, Error>, Vec<String>) {
        logged_in();
        flapped_on(&node("nobs-t-srl1", "srl"), wanted, answers, true)
    }

    fn log(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|entry| entry.to_string()).collect()
    }

    #[test]
    fn one_flap_reads_then_sends_the_down_with_the_delay_as_its_timer_then_waits_and_reads_it_back()
    {
        let sent = flap_session(20);
        assert_eq!(
            flapped(
                &ethernet(20, 1),
                vec![printed(""), printed(""), printed(UP)]
            ),
            (
                Ok(Flapped::Back),
                log(&["yes", READ, &sent, "down 1", "wait 20", READ, "back 1"])
            )
        );
    }

    #[test]
    fn three_flaps_send_the_same_lines_each_after_the_read_back_and_the_time_up() {
        let sent = flap_session(8);
        let (flapped, calls) = flapped(
            &ethernet(8, 3),
            vec![
                printed(UP),
                printed(""),
                printed(UP),
                printed(""),
                printed(UP),
                printed(""),
                printed(""),
            ],
        );
        assert_eq!(flapped, Ok(Flapped::Back));
        assert_eq!(
            calls,
            log(&[
                "yes", READ, &sent, "down 1", "wait 8", READ, "back 1", "wait 8", &sent, "down 2",
                "wait 8", READ, "back 2", "wait 8", &sent, "down 3", "wait 8", READ, "back 3",
            ])
        );
        for call in &calls {
            for word in ["enable", "save", "startup", "commit now"] {
                assert!(!call.contains(word), "{call}");
            }
        }
    }

    #[test]
    fn a_no_after_the_question_sends_nothing() {
        logged_in();
        assert_eq!(
            flapped_on(&node("nobs-t-srl1", "srl"), &ethernet(5, 1), vec![], false),
            (Ok(Flapped::Declined), log(&["yes"]))
        );
    }

    #[test]
    fn each_unfit_flap_is_refused_with_its_words_before_the_question_and_sends_nothing() {
        let not_one_interface = [
            "",
            "ethernet-1/1 x",
            "ethernet-1/1\tx",
            "ethernet-1/1\nshow",
            "ethernet-1/1\u{7}",
            "ethernet-1/{1,2}",
            "ethernet-1/{1..2}",
            "ethernet-1/*",
            "*",
            "ethernet-1/1;",
            "ethernet-1/1\"",
            "ethernet-1/é",
        ];
        let mut cases: Vec<(&str, u64, u32, Unfit)> = not_one_interface
            .into_iter()
            .map(|name| (name, 5, 1, Unfit::Name(name.to_string())))
            .collect();
        cases.extend([
            ("mgmt0", 5, 1, Unfit::Management("mgmt0")),
            ("ethernet-1/1", 0, 1, Unfit::Delay),
            ("ethernet-1/1", 301, 1, Unfit::Delay),
            ("ethernet-1/1", 5, 0, Unfit::Count),
            ("ethernet-1/1", 5, 11, Unfit::Count),
        ]);
        for (interface, delay, count, unfit) in cases {
            let wanted = Flap {
                interface,
                delay: Duration::from_secs(delay),
                count,
            };
            assert_eq!(
                flapped(&wanted, vec![]),
                (Ok(Flapped::Unfit(unfit)), vec![]),
                "{interface:?} {delay} {count}"
            );
        }
        let words = [
            (
                Unfit::Name("ethernet-1/*".to_string()),
                "\"ethernet-1/*\" is not the name of one interface",
            ),
            (
                Unfit::Management("mgmt0"),
                "mgmt0 is the management interface",
            ),
            (Unfit::Delay, "the delay must be 1 to 300 seconds"),
            (Unfit::Count, "the count must be 1 to 10"),
        ];
        for (unfit, words) in words {
            assert_eq!(unfit.to_string(), words);
        }
    }

    #[test]
    fn the_limit_words_give_the_ends_of_the_limits() {
        assert_eq!(
            delay_limits(),
            format!("{} to {} seconds", FLAP_DELAYS.start(), FLAP_DELAYS.end())
        );
        assert_eq!(
            count_limits(),
            format!("{} to {}", FLAP_COUNTS.start(), FLAP_COUNTS.end())
        );
    }

    #[test]
    fn the_limits_themselves_are_fit() {
        let srlinux = driver("srl").unwrap();
        for (delay, count) in [(1, 1), (300, 10), (1, 10), (300, 1)] {
            assert_eq!(
                unfit(srlinux, &ethernet(delay, count)),
                None,
                "{delay} {count}"
            );
        }
    }

    #[test]
    fn the_names_of_one_srlinux_interface_pass_the_name_check() {
        let srlinux = driver("srl").unwrap();
        for interface in ["ethernet-1/1", "ethernet-1/1/1", "lag1", "irb0", "system0"] {
            let wanted = Flap {
                interface,
                ..ethernet(5, 1)
            };
            assert_eq!(unfit(srlinux, &wanted), None, "{interface}");
        }
    }

    #[test]
    fn an_interface_the_device_shows_disabled_already_is_read_once_and_not_flapped() {
        let shown = [
            DOWN,
            "set / interface \"ethernet-1/1\" admin-state disable",
            "set / interface ethernet-1/2 admin-state enable\nset /  interface ethernet-1/1 admin-state disable\n",
        ];
        for stdout in shown {
            assert_eq!(
                flapped(&ethernet(5, 1), vec![printed(stdout)]),
                (Ok(Flapped::Disabled), log(&["yes", READ])),
                "{stdout}"
            );
        }
    }

    #[test]
    fn a_refused_read_or_a_refused_commit_ends_with_the_device_answer_and_sends_no_more() {
        let refused = replied(
            1,
            "Error: There is a commit already in progress for this candidate\n",
        );
        assert_eq!(
            flapped(&ethernet(5, 2), vec![refused.clone()]),
            (
                Ok(Flapped::Refused(refused.clone().unwrap())),
                log(&["yes", READ])
            )
        );
        let sent = flap_session(5);
        assert_eq!(
            flapped(&ethernet(5, 2), vec![printed(UP), refused.clone()]),
            (
                Ok(Flapped::Refused(refused.unwrap())),
                log(&["yes", READ, &sent])
            )
        );
    }

    #[test]
    fn a_read_that_fails_after_the_delay_is_not_known_whether_back() {
        let sent = flap_session(5);
        for error in [Error::NoAnswer, Error::AuthenticationFailed] {
            assert_eq!(
                flapped(
                    &ethernet(5, 2),
                    vec![printed(UP), printed(""), Err(error.clone())]
                ),
                (
                    Ok(Flapped::NotBack(NotBack::ReadFailed(error.clone()))),
                    log(&["yes", READ, &sent, "down 1", "wait 5", READ])
                ),
                "{error:?}"
            );
        }
    }

    #[test]
    fn an_interface_still_disabled_after_ten_reads_one_second_apart_is_not_known_whether_back() {
        let sent = flap_session(5);
        let mut answers = vec![printed(UP), printed("")];
        answers.extend((0..10).map(|_| printed(DOWN)));
        let mut expected = log(&["yes", READ, &sent, "down 1", "wait 5", READ]);
        for _ in 1..10 {
            expected.extend(log(&["wait 1", READ]));
        }
        assert_eq!(
            flapped(&ethernet(5, 2), answers),
            (Ok(Flapped::NotBack(NotBack::StillDisabled)), expected)
        );
    }

    #[test]
    fn ten_reads_with_a_refused_one_and_none_back_are_not_known_back_with_the_last_refusal() {
        let busy = |n: u32| replied(1, &format!("Error: busy {n}\n"));
        let mut every_read_refused = vec![printed(UP), printed("")];
        every_read_refused.extend((1..=10).map(busy));
        let mut refused_then_disabled = vec![printed(UP), printed(""), busy(1), busy(2)];
        refused_then_disabled.extend((0..8).map(|_| printed(DOWN)));
        for (answers, last) in [(every_read_refused, 10), (refused_then_disabled, 2)] {
            let (flapped, calls) = flapped(&ethernet(5, 2), answers);
            assert_eq!(
                flapped,
                Ok(Flapped::NotBack(NotBack::Refused(busy(last).unwrap())))
            );
            assert_eq!(calls.iter().filter(|call| *call == READ).count(), 11);
        }
    }

    #[test]
    fn a_read_the_device_refuses_after_the_delay_is_read_again() {
        let sent = flap_session(5);
        assert_eq!(
            flapped(
                &ethernet(5, 1),
                vec![
                    printed(UP),
                    printed(""),
                    replied(1, "Error: busy\n"),
                    printed(UP)
                ]
            ),
            (
                Ok(Flapped::Back),
                log(&["yes", READ, &sent, "down 1", "wait 5", READ, "wait 1", READ, "back 1"])
            )
        );
    }

    #[test]
    fn a_session_that_fails_after_the_lines_went_is_not_known_and_reads_nothing_more() {
        let sent = flap_session(5);
        for error in [
            Error::NoAnswer,
            Error::NoExitStatus,
            Error::Library("Connection closed by the remote side".to_string()),
        ] {
            assert_eq!(
                flapped(&ethernet(5, 2), vec![printed(UP), Err(error.clone())]),
                (
                    Ok(Flapped::NotKnown(error.clone())),
                    log(&["yes", READ, &sent])
                ),
                "{error:?}"
            );
        }
    }

    #[test]
    fn a_session_that_fails_before_any_line_went_is_an_error() {
        let sent = flap_session(5);
        for error in [Error::AuthenticationFailed, Error::NoAnswer] {
            assert_eq!(
                flapped(&ethernet(5, 1), vec![Err(error.clone())]),
                (Err(error.clone()), log(&["yes", READ])),
                "{error:?}"
            );
        }
        let unreachable = Error::Unreachable {
            host: host("nobs-t-srl1"),
            reason: "connection refused".to_string(),
        };
        assert_eq!(
            flapped(&ethernet(5, 1), vec![printed(UP), Err(unreachable.clone())]),
            (Err(unreachable), log(&["yes", READ, &sent]))
        );
    }

    #[test]
    fn no_driver_or_no_login_never_asks_and_never_reads() {
        logged_in();
        assert_eq!(
            flapped_on(&node("nobs-t-ceos1", "ceos"), &ethernet(5, 1), vec![], true),
            (Err(Error::NoDriver("ceos".to_string())), vec![])
        );
        std::env::remove_var(USER);
        std::env::remove_var(PASSWORD);
        assert_eq!(
            flapped_on(&node("nobs-t-srl1", "srl"), &ethernet(5, 1), vec![], true),
            (Err(Error::MissingCredentials), vec![])
        );
    }

    #[test]
    fn with_no_login_an_unfit_flap_gives_its_own_line_and_a_node_with_no_driver_its_error() {
        std::env::remove_var(USER);
        std::env::remove_var(PASSWORD);
        let cases = [
            (
                Flap {
                    interface: "ethernet-1/1 x",
                    ..ethernet(5, 1)
                },
                Unfit::Name("ethernet-1/1 x".to_string()),
                "nobs: flap nobs-t-srl1: \"ethernet-1/1 x\" is not the name of one interface",
            ),
            (
                Flap {
                    interface: "mgmt0",
                    ..ethernet(5, 1)
                },
                Unfit::Management("mgmt0"),
                "nobs: flap nobs-t-srl1: mgmt0 is the management interface",
            ),
            (
                ethernet(301, 1),
                Unfit::Delay,
                "nobs: flap nobs-t-srl1: the delay must be 1 to 300 seconds",
            ),
            (
                ethernet(5, 11),
                Unfit::Count,
                "nobs: flap nobs-t-srl1: the count must be 1 to 10",
            ),
        ];
        for (wanted, unfit, line) in cases {
            assert_eq!(
                flapped_on(&node("nobs-t-srl1", "srl"), &wanted, vec![], true),
                (Ok(Flapped::Unfit(unfit.clone())), vec![]),
                "{line}"
            );
            assert_eq!(unfit_line("nobs-t-srl1", &unfit), line);
            assert_eq!(
                flapped_on(&node("nobs-t-ceos1", "ceos"), &wanted, vec![], true),
                (Err(Error::NoDriver("ceos".to_string())), vec![]),
                "{line}"
            );
        }
    }

    #[test]
    fn every_command_text_of_a_flap_is_a_value_of_the_driver() {
        let credentials = Credentials {
            user: "admin".to_string(),
            password: Password::new("nobs-t-not-a-secret".to_string()),
        };
        let flapped_with = |interface, answers| {
            let transport = FakeTransport::in_order(answers);
            let wanted = Flap {
                interface,
                delay: Duration::from_secs(7),
                count: 1,
            };
            let flapper = Flapper {
                transport: &transport,
                host: "nobs-t-host",
                credentials: &credentials,
                driver: test_driver(),
                flap: &wanted,
            };
            let flapped = match unfit(flapper.driver, flapper.flap) {
                Some(unfit) => Ok(Flapped::Unfit(unfit)),
                None => flapper.flapped(&mut || true, &mut |_| {}, &mut |_, _| {}),
            };
            let texts: Vec<String> = transport
                .calls()
                .into_iter()
                .map(|(_, text)| text)
                .collect();
            (flapped, texts)
        };
        assert_eq!(
            flapped_with("nobs-t-if", vec![printed(""), printed(""), printed(UP)]),
            (
                Ok(Flapped::Back),
                log(&[
                    "nobs-t-read nobs-t-if",
                    "nobs-t-open-1\nnobs-t-down nobs-t-if\nnobs-t-commit-after 7",
                    "nobs-t-read nobs-t-if",
                ])
            )
        );
        assert_eq!(
            flapped_with("nobs-t-if", vec![printed("nobs-t-down nobs-t-if\n")]),
            (Ok(Flapped::Disabled), log(&["nobs-t-read nobs-t-if"]))
        );
        assert_eq!(
            flapped_with("nobs-t-mgmt", vec![]),
            (Ok(Flapped::Unfit(Unfit::Management("nobs-t-mgmt"))), vec![])
        );
        assert_eq!(
            flapped_with("nobs-t-*", vec![printed("nobs-t-down nobs-t-*\n")]),
            (Ok(Flapped::Disabled), log(&["nobs-t-read nobs-t-*"]))
        );
        assert_eq!(
            flapped_with("nobs-t-if1", vec![]),
            (
                Ok(Flapped::Unfit(Unfit::Name("nobs-t-if1".to_string()))),
                vec![]
            )
        );
    }

    #[test]
    fn the_question_names_the_device_the_interface_the_delay_and_the_count() {
        let flap = |interface, delay, count| Flap {
            interface,
            delay: Duration::from_secs(delay),
            count,
        };
        assert_eq!(
            flap_question("nobs-t-srl1", &flap("ethernet-1/1", 5, 1)),
            "flap ethernet-1/1 of nobs-t-srl1? It goes down for 5 seconds, 1 time; the device brings it back. y/n"
        );
        assert_eq!(
            flap_question("nobs-t-srl2", &flap("ethernet-1/2", 1, 2)),
            "flap ethernet-1/2 of nobs-t-srl2? It goes down for 1 second, 2 times; the device brings it back. y/n"
        );
    }
}
