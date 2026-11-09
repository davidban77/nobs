use std::fmt;
use std::sync::Mutex;

use crate::infrahub;
use crate::lab_vars::{self, LabVars};
use crate::settings::Workshop;

const BODY_CUT: usize = 200;

pub trait SourceOfTruth: Send + Sync {
    fn devices(&self) -> Result<Vec<Device>, Error>;
    fn device(&self, name: &str) -> Result<Option<Device>, Error>;
    fn set_maintenance(&self, device: &str, state: bool) -> Result<Maintenance, Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub kind: Option<String>,
    pub site: Option<String>,
    pub role: Option<String>,
    pub asn: Option<i64>,
    pub maintenance: bool,
    pub interfaces: Vec<Interface>,
    pub sessions: Vec<Session>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub role: Option<String>,
    pub address: Option<String>,
    pub expected_state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub peer: String,
    pub afi_safi: Option<String>,
    pub expected_state: Option<String>,
    pub remote_as: Option<i64>,
    pub expected_prefixes: Option<i64>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Maintenance {
    pub previous: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Reach {
        url: String,
        reason: String,
    },
    Status {
        status: u16,
        path: String,
        body: String,
    },
    Graphql(String),
    NotReady(Vec<String>),
    NotFound {
        kind: String,
        device: String,
    },
    NoSource,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Reach { url, reason } => write!(f, "cannot reach {url}: {reason}"),
            Error::Status { status, path, body } => {
                let body: String = body.chars().take(BODY_CUT).collect();
                write!(f, "Infrahub answered {status} on {path}: {body}")
            }
            Error::Graphql(message) => write!(f, "Infrahub: {message}"),
            Error::NotReady(kinds) => write!(
                f,
                "workshop schema kinds {} not visible to Infrahub after 30 s",
                kinds.join(", ")
            ),
            Error::NotFound { kind, device } => write!(f, "{kind} {device} not found in Infrahub."),
            Error::NoSource => write!(f, "this lab has no source of truth"),
        }
    }
}

pub struct Fixed {
    devices: Mutex<Vec<Device>>,
}

impl Fixed {
    pub fn from_lab_vars(vars: &LabVars) -> Fixed {
        Fixed {
            devices: Mutex::new(vars.devices.iter().map(device_from).collect()),
        }
    }
}

impl SourceOfTruth for Fixed {
    fn devices(&self) -> Result<Vec<Device>, Error> {
        Ok(self.devices.lock().unwrap().clone())
    }

    fn device(&self, name: &str) -> Result<Option<Device>, Error> {
        Ok(self
            .devices
            .lock()
            .unwrap()
            .iter()
            .find(|device| device.name == name)
            .cloned())
    }

    fn set_maintenance(&self, device: &str, state: bool) -> Result<Maintenance, Error> {
        let mut devices = self.devices.lock().unwrap();
        let found = devices
            .iter_mut()
            .find(|candidate| candidate.name == device)
            .ok_or_else(|| Error::NotFound {
                kind: "device".to_string(),
                device: device.to_string(),
            })?;
        Ok(Maintenance {
            previous: std::mem::replace(&mut found.maintenance, state),
        })
    }
}

struct Empty;

impl SourceOfTruth for Empty {
    fn devices(&self) -> Result<Vec<Device>, Error> {
        Ok(Vec::new())
    }

    fn device(&self, _name: &str) -> Result<Option<Device>, Error> {
        Ok(None)
    }

    fn set_maintenance(&self, _device: &str, _state: bool) -> Result<Maintenance, Error> {
        Err(Error::NoSource)
    }
}

pub fn open(
    workshop: &Workshop,
    address: Option<&str>,
    token: Option<&str>,
    kind: Option<&str>,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> (Option<String>, Result<Box<dyn SourceOfTruth>, String>) {
    if workshop.sot == "none" {
        return (None, Ok(Box::new(Empty)));
    }
    let (line, client) = infrahub::connect(address, token, lookup);
    let source = client.map(|mut client| {
        if let Some(kind) = kind {
            client.set_maintenance_kind(kind);
        }
        Box::new(client) as Box<dyn SourceOfTruth>
    });
    (line, source)
}

fn device_from(device: &lab_vars::Device) -> Device {
    Device {
        name: device.name.clone(),
        kind: None,
        site: Some(device.site.clone()),
        role: Some(device.role.clone()),
        asn: device.asn,
        maintenance: device.maintenance,
        interfaces: device
            .interfaces
            .iter()
            .map(|interface| Interface {
                name: interface.name.clone(),
                role: Some(interface.role.clone()),
                address: interface.ip.clone(),
                expected_state: Some(interface.expected_state.clone()),
            })
            .collect(),
        sessions: device
            .sessions
            .iter()
            .map(|session| Session {
                peer: session.peer.clone(),
                afi_safi: Some(session.afi_safi.clone()),
                expected_state: Some(session.expected_state.clone()),
                remote_as: session.remote_as,
                expected_prefixes: session.expected_prefixes,
                reason: session.reason.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::thread;

    use super::*;
    use crate::settings::{read_settings, Settings};

    const NO_SOURCE_WORKSHOP: &str = r#"kind = "workshop"
sot = "none"
name = "nobs-t-one"
title = "Workshop one"
project = "nobs-t-one"
network = "workshop-nobs-t-one"
workflow_label = "nobs_t_one_quarantine_bgp"
cascade_prefix = "nobs-t-one-cascade-"
scenario_prefix = "nobs-t-one-"
evidence_query = "{device=\"srl1\"}"
silence_hints = ["Workshop"]
schema_file = "infrahub/schema.yml"
"#;

    fn book_lab() -> Fixed {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/book-lab/containerlab/lab_vars.yml");
        let text = fs::read_to_string(path).unwrap();
        Fixed::from_lab_vars(&lab_vars::read(&text).unwrap())
    }

    fn text(value: &str) -> Option<String> {
        Some(value.to_string())
    }

    fn expected_device(name: &str, address: &str, peer: &str, remote_as: i64) -> Device {
        Device {
            name: name.to_string(),
            kind: None,
            site: text("lab"),
            role: text("edge"),
            asn: None,
            maintenance: false,
            interfaces: vec![Interface {
                name: "ethernet-1/1".to_string(),
                role: text("nobs-t-peer"),
                address: text(address),
                expected_state: text("up"),
            }],
            sessions: vec![Session {
                peer: peer.to_string(),
                afi_safi: text("ipv4-unicast"),
                expected_state: text("established"),
                remote_as: Some(remote_as),
                expected_prefixes: None,
                reason: None,
            }],
        }
    }

    #[test]
    fn fixed_from_the_book_lab_gives_its_two_devices_in_file_order_with_everything_placed() {
        assert_eq!(
            book_lab().devices(),
            Ok(vec![
                expected_device("nobs-t-srl1", "192.0.2.0/31", "192.0.2.1", 65002),
                expected_device("nobs-t-srl2", "192.0.2.1/31", "192.0.2.0", 65001),
            ])
        );
    }

    #[test]
    fn fixed_device_finds_a_device_by_name_and_an_unknown_name_is_not_found() {
        let fixed = book_lab();

        assert_eq!(
            fixed.device("nobs-t-srl2"),
            Ok(Some(fixed.devices().unwrap()[1].clone()))
        );
        assert_eq!(fixed.device("srl9"), Ok(None));
        assert_eq!(
            fixed.set_maintenance("srl9", true),
            Err(Error::NotFound {
                kind: "device".to_string(),
                device: "srl9".to_string()
            })
        );
    }

    #[test]
    fn fixed_set_maintenance_returns_the_previous_state_and_the_next_read_shows_the_flip() {
        let fixed = book_lab();

        assert_eq!(
            fixed.set_maintenance("nobs-t-srl1", true),
            Ok(Maintenance { previous: false })
        );
        assert!(fixed.device("nobs-t-srl1").unwrap().unwrap().maintenance);
        assert!(!fixed.device("nobs-t-srl2").unwrap().unwrap().maintenance);
        assert_eq!(
            fixed.set_maintenance("nobs-t-srl1", false),
            Ok(Maintenance { previous: true })
        );
        assert!(!fixed.device("nobs-t-srl1").unwrap().unwrap().maintenance);
    }

    #[test]
    fn a_boxed_source_moves_into_a_thread_and_reads_there() {
        let source: Box<dyn SourceOfTruth> = Box::new(book_lab());

        let names = thread::spawn(move || {
            source
                .devices()
                .unwrap()
                .into_iter()
                .map(|device| device.name)
                .collect::<Vec<_>>()
        })
        .join()
        .unwrap();

        assert_eq!(names, ["nobs-t-srl1", "nobs-t-srl2"]);
    }

    #[test]
    fn open_for_a_lab_with_no_source_makes_no_connection_and_has_nothing_to_set() {
        let Ok(Settings::Workshop(workshop)) = read_settings(NO_SOURCE_WORKSHOP) else {
            panic!("the workshop text does not parse");
        };

        let (line, source) = open(
            &workshop,
            Some("http://infrahub-server:8000"),
            Some("nobs-t-token"),
            None,
            &|_| None,
        );

        assert_eq!(line, None);
        let source = source.unwrap();
        assert_eq!(source.devices(), Ok(Vec::new()));
        assert_eq!(source.device("srl1"), Ok(None));
        assert_eq!(
            source
                .set_maintenance("srl1", true)
                .unwrap_err()
                .to_string(),
            "this lab has no source of truth"
        );
    }
}
