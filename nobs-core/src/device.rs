#[cfg(any(test, feature = "fake"))]
use std::cell::RefCell;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use crate::lab;
use crate::runner::Output;

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
            Error::Library(text) => f.write_str(text),
        }
    }
}

pub trait Transport {
    fn run(&self, host: &str, credentials: &Credentials, command: &str) -> Result<Output, Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Driver {
    pub running_config: &'static str,
}

pub fn driver(kind: &str) -> Result<Driver, Error> {
    if lab::SRLINUX_KINDS.contains(&kind) {
        Ok(Driver {
            running_config: "info flat",
        })
    } else {
        Err(Error::NoDriver(kind.to_string()))
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

#[cfg(any(test, feature = "fake"))]
pub struct FakeTransport {
    answer: Result<Output, Error>,
    calls: RefCell<Vec<(String, String)>>,
}

#[cfg(any(test, feature = "fake"))]
impl FakeTransport {
    pub fn answering(answer: Result<Output, Error>) -> Self {
        Self {
            answer,
            calls: RefCell::new(Vec::new()),
        }
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
        self.answer.clone()
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
                    running_config: "info flat"
                }),
                "{kind}"
            );
        }
        assert_eq!(driver("ceos"), Err(Error::NoDriver("ceos".to_string())));
    }
}
