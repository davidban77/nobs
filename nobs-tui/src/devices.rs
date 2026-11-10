use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nobs_core::alertmanager::Alert;
use nobs_core::loki::Line;
use nobs_core::prefect::FlowRun;
use nobs_core::prometheus::Sample;
use nobs_core::sot::{Device, Interface, Session};
use nobs_core::{evidence, time};

const DASH: &str = "—";
const UNKNOWN: &str = "?";
const NANOSECONDS_PER_SECOND: i64 = 1_000_000_000;
const SECONDS_PER_DAY: i64 = 86_400;
const BGP_OPER_STATE: &str = "bgp_oper_state";
const AFI_SAFI: &str = "ipv4-unicast";
const INSTANCE: &str = "default";
const INTERFACE_ADMIN_STATE: &str = "interface_admin_state";
const INTERFACE_OPER_STATE: &str = "interface_oper_state";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Ok,
    Warn,
    Down,
    Unknown,
    NoSessions,
}

impl Health {
    pub fn text(self) -> &'static str {
        match self {
            Health::Ok => "ok",
            Health::Warn => "warn",
            Health::Down => "down",
            Health::Unknown => UNKNOWN,
            Health::NoSessions => DASH,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListRow {
    pub name: String,
    pub kind: String,
    pub health: Health,
    pub alerts: Option<(usize, bool)>,
    pub maintenance: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub name: String,
    pub intent: String,
    pub admin: String,
    pub oper: String,
    pub peer: String,
    pub bgp: String,
    pub differs: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Alert,
    Flow,
    Rca,
    Log,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Alert => "alert",
            Kind::Flow => "flow",
            Kind::Rca => "rca",
            Kind::Log => "log",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub nanoseconds: i64,
    pub kind: Kind,
    pub text: String,
}

fn label<'a>(sample: &'a Sample, name: &str) -> Option<&'a str> {
    sample.labels.get(name).map(String::as_str)
}

fn value_of(
    samples: &[Sample],
    metric: &str,
    device: &str,
    key: &str,
    wanted: &str,
) -> Option<f64> {
    samples
        .iter()
        .find(|sample| {
            label(sample, "__name__") == Some(metric)
                && label(sample, "device") == Some(device)
                && label(sample, key) == Some(wanted)
        })
        .map(|sample| sample.value)
}

pub fn oper_states(
    device: &str,
    sessions: &[Session],
    samples: &[Sample],
) -> BTreeMap<String, f64> {
    sessions
        .iter()
        .filter_map(|session| {
            let afi_safi = session.afi_safi.as_deref().unwrap_or(AFI_SAFI);
            let value = samples
                .iter()
                .find(|sample| {
                    label(sample, "__name__") == Some(BGP_OPER_STATE)
                        && label(sample, "device") == Some(device)
                        && label(sample, "peer_address") == Some(session.peer.as_str())
                        && label(sample, "afi_safi_name") == Some(afi_safi)
                        && label(sample, "name") == Some(INSTANCE)
                })
                .map(|sample| sample.value)?;
            Some((session.peer.clone(), value))
        })
        .collect()
}

pub fn has_bgp_series(device: &str, samples: &[Sample]) -> bool {
    samples.iter().any(|sample| {
        label(sample, "__name__") == Some(BGP_OPER_STATE) && label(sample, "device") == Some(device)
    })
}

pub fn health(
    sessions: &[Session],
    values: Option<&BTreeMap<String, f64>>,
    any_series: bool,
) -> Health {
    if sessions.is_empty() {
        return Health::NoSessions;
    }
    let Some(values) = values else {
        return Health::Unknown;
    };
    if values.is_empty() && !any_series {
        Health::Down
    } else if sessions
        .iter()
        .all(|session| values.get(&session.peer) == Some(&1.0))
    {
        Health::Ok
    } else {
        Health::Warn
    }
}

pub fn kind_short(kind: Option<&str>) -> String {
    match kind {
        None => DASH.to_string(),
        Some("nokia_srlinux" | "srl") => "srl".to_string(),
        Some("ceos" | "arista_ceos") => "ceos".to_string(),
        Some(other) => other.to_string(),
    }
}

pub fn list_rows(
    devices: &[Device],
    samples: Option<&[Sample]>,
    alerts: Option<&[Alert]>,
) -> Vec<ListRow> {
    devices
        .iter()
        .map(|device| ListRow {
            name: device.name.clone(),
            kind: kind_short(device.kind.as_deref()),
            health: health(
                &device.sessions,
                samples
                    .map(|samples| oper_states(&device.name, &device.sessions, samples))
                    .as_ref(),
                samples.is_some_and(|samples| has_bgp_series(&device.name, samples)),
            ),
            alerts: alerts.map(|alerts| alert_count(alerts, &device.name)),
            maintenance: device.maintenance,
        })
        .collect()
}

pub fn alert_count(alerts: &[Alert], device: &str) -> (usize, bool) {
    let counted: Vec<&Alert> = alerts
        .iter()
        .filter(|alert| {
            alert.labels.get("device").map(String::as_str) == Some(device)
                && alert.state_word() == "firing"
        })
        .collect();
    let critical = counted
        .iter()
        .any(|alert| alert.labels.get("severity").map(String::as_str) == Some("critical"));
    (counted.len(), critical)
}

pub fn join(
    device: &str,
    interfaces: &[Interface],
    sessions: &[Session],
    samples: Option<&[Sample]>,
) -> Vec<Row> {
    let states = samples.map(|samples| oper_states(device, sessions, samples));
    interfaces
        .iter()
        .map(|interface| {
            let session = interface.address.as_deref().and_then(|address| {
                sessions
                    .iter()
                    .find(|session| contains(address, &session.peer))
            });
            let live = |metric: &str| match samples {
                None => UNKNOWN.to_string(),
                Some(samples) => value_of(samples, metric, device, "name", &interface.name)
                    .map_or(DASH.to_string(), interface_word),
            };
            let admin = live(INTERFACE_ADMIN_STATE);
            let oper = live(INTERFACE_OPER_STATE);
            let bgp = match (session, &states) {
                (None, _) => String::new(),
                (Some(_), None) => UNKNOWN.to_string(),
                (Some(session), Some(states)) => {
                    states.get(&session.peer).map_or(DASH.to_string(), |value| {
                        evidence::decode_oper(Some(*value))
                    })
                }
            };
            let differs = !matches!(bgp.as_str(), "up" | UNKNOWN | DASH | "")
                || (admin == "up" && oper == "down");
            Row {
                name: interface.name.clone(),
                intent: interface
                    .expected_state
                    .clone()
                    .unwrap_or_else(|| DASH.to_string()),
                admin,
                oper,
                peer: session.map_or(String::new(), |session| session.peer.clone()),
                bgp,
                differs,
            }
        })
        .collect()
}

fn interface_word(value: f64) -> String {
    if value == 1.0 {
        "up".to_string()
    } else if value == 2.0 {
        "down".to_string()
    } else {
        "unknown".to_string()
    }
}

fn contains(subnet: &str, address: &str) -> bool {
    let Some((network, length)) = subnet.split_once('/') else {
        return false;
    };
    let (Ok(network), Ok(length), Ok(address)) = (
        network.parse::<Ipv4Addr>(),
        length.parse::<u32>(),
        address.parse::<Ipv4Addr>(),
    ) else {
        return false;
    };
    if length > 32 {
        return false;
    }
    let mask = u32::MAX.checked_shl(32 - length).unwrap_or(0);
    u32::from(network) & mask == u32::from(address) & mask
}

pub fn merge(
    device: &str,
    alerts: &[Alert],
    runs: &[FlowRun],
    rcas: &[Line],
    logs: &[(i64, &str)],
    window: Duration,
    now: SystemTime,
) -> Vec<Entry> {
    let alerts = alerts
        .iter()
        .filter(|alert| alert.labels.get("device").map(String::as_str) == Some(device))
        .filter_map(|alert| {
            let mut text = format!(
                "{}  {}",
                alert.labels.get("alertname").map_or(DASH, String::as_str),
                alert.state_word()
            );
            for (name, value) in &alert.labels {
                text.push_str(&format!("\n{name}={value}"));
            }
            Some(Entry {
                nanoseconds: nanoseconds_of(&alert.starts_at)?,
                kind: Kind::Alert,
                text,
            })
        });
    let flows = runs
        .iter()
        .filter(|run| run.device.as_deref() == Some(device))
        .filter_map(|run| {
            Some(Entry {
                nanoseconds: nanoseconds_of(&run.start_time)?,
                kind: Kind::Flow,
                text: format!("{}  {}", run.name, run.state),
            })
        });
    let rcas = rcas.iter().filter_map(|line| {
        Some(Entry {
            nanoseconds: line.nanoseconds,
            kind: Kind::Rca,
            text: evidence::rca_record(&line.text)?.message,
        })
    });
    let logs = logs.iter().map(|(nanoseconds, text)| Entry {
        nanoseconds: *nanoseconds,
        kind: Kind::Log,
        text: text.to_string(),
    });
    let oldest = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_nanos()).unwrap_or(i64::MAX)
        })
        .saturating_sub(i64::try_from(window.as_nanos()).unwrap_or(i64::MAX));
    let mut entries: Vec<Entry> = alerts
        .chain(flows)
        .chain(rcas)
        .chain(logs)
        .filter(|entry| entry.nanoseconds >= oldest)
        .collect();
    entries.sort_by_key(|entry| Reverse(entry.nanoseconds));
    entries
}

fn nanoseconds_of(text: &str) -> Option<i64> {
    time::parse_rfc3339(text)?.checked_mul(NANOSECONDS_PER_SECOND)
}

pub fn counts(entries: &[Entry]) -> [usize; 4] {
    let count = |kind| entries.iter().filter(|entry| entry.kind == kind).count();
    [
        count(Kind::Alert),
        count(Kind::Flow),
        count(Kind::Rca),
        count(Kind::Log),
    ]
}

pub fn window_text(minutes: u64) -> String {
    if minutes >= 60 && minutes.is_multiple_of(60) {
        format!("{}h", minutes / 60)
    } else {
        format!("{minutes}m")
    }
}

pub fn clock_of(nanoseconds: i64) -> String {
    let of_day = nanoseconds
        .div_euclid(NANOSECONDS_PER_SECOND)
        .rem_euclid(SECONDS_PER_DAY);
    format!(
        "{:02}:{:02}:{:02}",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
}
