use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use crate::runner::Output;

pub const ANSWER_LIMIT: Duration = Duration::from_secs(10);

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
}
