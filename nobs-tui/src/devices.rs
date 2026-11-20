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

#[derive(Debug, Clone, PartialEq)]
pub struct MetricsRow {
    pub name: String,
    pub interfaces: String,
    pub sessions: String,
    pub short: bool,
    pub no_data: bool,
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

pub fn session_states(device: &Device, samples: Option<&[Sample]>) -> Vec<(Session, String)> {
    let values = samples.map(|samples| oper_states(&device.name, &device.sessions, samples));
    device
        .sessions
        .iter()
        .map(|session| {
            let state = match &values {
                Some(values) => evidence::decode_oper(values.get(&session.peer).copied()),
                None => UNKNOWN.to_string(),
            };
            (session.clone(), state)
        })
        .collect()
}

pub fn sessions_up(states: &[(Session, String)]) -> (usize, usize) {
    let established = states.iter().filter(|(_, state)| state == "up").count();
    (established, states.len())
}

pub fn metrics_row(device: &Device, samples: Option<&[Sample]>) -> MetricsRow {
    let name = device.name.clone();
    let Some(samples) = samples else {
        return MetricsRow {
            name,
            interfaces: UNKNOWN.to_string(),
            sessions: UNKNOWN.to_string(),
            short: false,
            no_data: false,
        };
    };
    if !any_series(&device.name, samples) {
        return MetricsRow {
            name,
            interfaces: "no data".to_string(),
            sessions: String::new(),
            short: false,
            no_data: true,
        };
    }
    let interfaces = interfaces_up(&join(
        &device.name,
        &device.interfaces,
        &device.sessions,
        Some(samples),
    ));
    let sessions = sessions_up(&session_states(device, Some(samples)));
    MetricsRow {
        name,
        interfaces: count_text(interfaces),
        sessions: count_text(sessions),
        short: interfaces.0 < interfaces.1 || sessions.0 < sessions.1,
        no_data: false,
    }
}

fn interfaces_up(rows: &[Row]) -> (usize, usize) {
    let wanted: Vec<&Row> = rows
        .iter()
        .filter(|row| row.intent == "up" && row.oper != DASH)
        .collect();
    let up = wanted.iter().filter(|row| row.oper == "up").count();
    (up, wanted.len())
}

fn any_series(device: &str, samples: &[Sample]) -> bool {
    samples
        .iter()
        .any(|sample| label(sample, "device") == Some(device))
}

fn count_text((up, wanted): (usize, usize)) -> String {
    if wanted == 0 {
        DASH.to_string()
    } else {
        format!("{up} of {wanted} up")
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE: &str = "nobs-t-srl1";

    fn interface(name: &str, expected_state: Option<&str>) -> Interface {
        Interface {
            name: name.to_string(),
            role: None,
            address: None,
            expected_state: expected_state.map(str::to_string),
        }
    }

    fn session(peer: &str) -> Session {
        Session {
            peer: peer.to_string(),
            afi_safi: None,
            expected_state: None,
            remote_as: None,
            expected_prefixes: None,
            reason: None,
        }
    }

    fn device(interfaces: Vec<Interface>, sessions: Vec<Session>) -> Device {
        Device {
            name: DEVICE.to_string(),
            kind: None,
            site: None,
            role: None,
            asn: None,
            maintenance: false,
            interfaces,
            sessions,
        }
    }

    fn sample(pairs: &[(&str, &str)], value: f64) -> Sample {
        Sample {
            labels: pairs
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            value,
        }
    }

    fn oper(device: &str, name: &str, value: f64) -> Sample {
        sample(
            &[
                ("__name__", "interface_oper_state"),
                ("device", device),
                ("name", name),
            ],
            value,
        )
    }

    fn peer(address: &str, value: f64) -> Sample {
        sample(
            &[
                ("__name__", "bgp_oper_state"),
                ("device", DEVICE),
                ("peer_address", address),
                ("afi_safi_name", "ipv4-unicast"),
                ("name", "default"),
            ],
            value,
        )
    }

    fn row_of(interfaces: Vec<Interface>, samples: &[Sample]) -> MetricsRow {
        metrics_row(&device(interfaces, Vec::new()), Some(samples))
    }

    #[test]
    fn an_interface_wanted_up_and_up_counts_up() {
        let row = row_of(
            vec![interface("ethernet-1/1", Some("up"))],
            &[oper(DEVICE, "ethernet-1/1", 1.0)],
        );

        assert_eq!(row.interfaces, "1 of 1 up");
        assert!(!row.short, "{row:?}");
    }

    #[test]
    fn an_interface_wanted_up_and_down_counts_wanted_not_up() {
        let row = row_of(
            vec![interface("ethernet-1/1", Some("up"))],
            &[oper(DEVICE, "ethernet-1/1", 2.0)],
        );

        assert_eq!(row.interfaces, "0 of 1 up");
        assert!(row.short, "{row:?}");
    }

    #[test]
    fn an_interface_wanted_up_and_not_reported_is_not_counted() {
        let row = row_of(
            vec![
                interface("ethernet-1/1", Some("up")),
                interface("lo0", Some("up")),
            ],
            &[oper(DEVICE, "ethernet-1/1", 1.0)],
        );

        assert_eq!(row.interfaces, "1 of 1 up");
        assert!(!row.short, "{row:?}");
    }

    #[test]
    fn an_interface_wanted_down_is_not_counted() {
        let row = row_of(
            vec![interface("ethernet-1/1", Some("down"))],
            &[oper(DEVICE, "ethernet-1/1", 2.0)],
        );

        assert_eq!(row.interfaces, "—");
        assert!(!row.short, "{row:?}");
    }

    #[test]
    fn an_interface_without_intent_is_not_counted() {
        let row = row_of(
            vec![interface("ethernet-1/1", None)],
            &[oper(DEVICE, "ethernet-1/1", 2.0)],
        );

        assert_eq!(row.interfaces, "—");
        assert!(!row.short, "{row:?}");
    }

    #[test]
    fn a_device_without_interfaces_shows_a_dash() {
        let row = row_of(Vec::new(), &[peer("10.1.2.2", 1.0)]);

        assert_eq!(row.interfaces, "—");
        assert!(!row.no_data, "{row:?}");
    }

    #[test]
    fn an_unknown_state_counts_wanted_not_up() {
        let row = row_of(
            vec![interface("ethernet-1/1", Some("up"))],
            &[oper(DEVICE, "ethernet-1/1", 7.0)],
        );

        assert_eq!(row.interfaces, "0 of 1 up");
        assert!(row.short, "{row:?}");
    }

    #[test]
    fn wanted_interfaces_none_reported_show_a_dash() {
        let row = row_of(
            vec![
                interface("ethernet-1/1", Some("up")),
                interface("lo0", Some("up")),
            ],
            &[peer("10.1.2.2", 1.0)],
        );

        assert_eq!(row.interfaces, "—");
        assert!(!row.no_data, "{row:?}");
    }

    #[test]
    fn the_session_words_and_their_count_are_up_of_every_intended_session() {
        let device = device(
            Vec::new(),
            vec![
                session("10.1.2.2"),
                session("10.1.7.2"),
                session("10.1.99.2"),
            ],
        );
        let samples = [peer("10.1.2.2", 1.0), peer("10.1.7.2", 3.0)];

        let states = session_states(&device, Some(&samples));
        let unanswered = session_states(&device, None);

        let words: Vec<&str> = states.iter().map(|(_, state)| state.as_str()).collect();
        assert_eq!(words, ["up", "idle", "—"]);
        assert_eq!(sessions_up(&states), (1, 3));
        let words: Vec<&str> = unanswered.iter().map(|(_, state)| state.as_str()).collect();
        assert_eq!(words, ["?", "?", "?"]);
    }

    #[test]
    fn the_bgp_cell_of_a_row_counts_the_established_of_the_intended_sessions() {
        let device = device(
            Vec::new(),
            vec![
                session("10.1.2.2"),
                session("10.1.7.2"),
                session("10.1.99.2"),
            ],
        );
        let samples = [peer("10.1.2.2", 1.0), peer("10.1.7.2", 3.0)];

        let row = metrics_row(&device, Some(&samples));

        assert_eq!(row.sessions, "1 of 3 up");
        assert!(row.short, "{row:?}");
    }

    #[test]
    fn a_device_without_values_says_no_data() {
        let row = row_of(
            vec![interface("ethernet-1/1", Some("up"))],
            &[oper("nobs-t-srl2", "ethernet-1/1", 1.0)],
        );

        assert_eq!(row.interfaces, "no data");
        assert_eq!(row.sessions, "");
        assert!(row.no_data, "{row:?}");
        assert!(!row.short, "{row:?}");
    }

    #[test]
    fn a_row_without_an_answer_shows_a_question_mark_in_both_cells() {
        let row = metrics_row(
            &device(
                vec![interface("ethernet-1/1", Some("up"))],
                vec![session("10.1.2.2")],
            ),
            None,
        );

        assert_eq!((row.interfaces.as_str(), row.sessions.as_str()), ("?", "?"));
        assert!(!row.no_data && !row.short, "{row:?}");
    }
}
