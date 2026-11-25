use std::borrow::Cow;
#[cfg(any(test, feature = "fake"))]
use std::cell::RefCell;
use std::fmt;
use std::io::{self, Read, Write};
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
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Words<'a> {
    Read(Vec<Cow<'a, str>>),
    Unread(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    SrLinux,
}

const SEPARATORS: [char; 2] = [' ', '\t'];

impl Syntax {
    pub fn words(self, line: &str) -> Words<'_> {
        let line = line.trim();
        match self {
            Syntax::SrLinux => srlinux_words(line).map_or(Words::Unread(line), Words::Read),
        }
    }
}

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
        match transport.send(host, credentials, &session_lines(driver, text)) {
            Ok(output) if output.code == 0 => Ok(Pushed::Taken(output)),
            Ok(output) => Ok(Pushed::Refused(output)),
            Err(error) if error.says_nothing_was_sent() => Err(error),
            Err(error) => Ok(Pushed::NotKnown(error)),
        }
    })
}

fn session_lines(driver: Driver, text: &str) -> Vec<String> {
    driver
        .open
        .iter()
        .copied()
        .chain(intended::lines(text))
        .chain([driver.commit])
        .map(str::to_string)
        .collect()
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

pub fn not_known_line(name: &str) -> String {
    format!("nobs: push {name}: not known whether the device took the push; run nobs device check")
}

#[cfg(any(test, feature = "fake"))]
pub struct FakeTransport {
    answers: Vec<(Option<String>, Result<Output, Error>)>,
    calls: RefCell<Vec<(String, String)>>,
}

#[cfg(any(test, feature = "fake"))]
impl FakeTransport {
    pub fn answering(answer: Result<Output, Error>) -> Self {
        Self {
            answers: vec![(None, answer)],
            calls: RefCell::new(Vec::new()),
        }
    }

    pub fn per_host(answers: Vec<(String, Result<Output, Error>)>) -> Self {
        Self {
            answers: answers
                .into_iter()
                .map(|(host, answer)| (Some(host), answer))
                .collect(),
            calls: RefCell::new(Vec::new()),
        }
    }

    fn answer(&self, host: &str) -> Result<Output, Error> {
        self.answers
            .iter()
            .find(|(known, _)| known.as_deref().is_none_or(|known| known == host))
            .map(|(_, answer)| answer.clone())
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
                running_config: "nobs-t-running",
                open,
                commit: "nobs-t-commit",
                syntax: Syntax::SrLinux,
            };
            assert_eq!(session_lines(driver, text), sent, "{open:?}");
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
        Syntax::SrLinux.words(line)
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
}
