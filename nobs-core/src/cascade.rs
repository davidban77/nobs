use std::net::Ipv4Addr;
use std::time::Duration;

use serde_json::{json, Value};

use crate::lab_vars::LabVars;

pub const DEVICES: [&str; 2] = ["srl1", "srl2"];

const POLL: Duration = Duration::from_secs(5);

struct Profile {
    device_label: &'static str,
    interface_label: &'static str,
    peer_label: &'static str,
    asn_label: &'static str,
    collection_type: &'static str,
    oper_state_metric: &'static str,
    bgp_metrics: [(&'static str, f64); 6],
}

static SRL1: Profile = Profile {
    device_label: "source",
    interface_label: "name",
    peer_label: "peer_address",
    asn_label: "neighbor_asn",
    collection_type: "gnmi",
    oper_state_metric: "srl_interface_oper_state",
    bgp_metrics: [
        ("srl_bgp_oper_state", 2.0),
        ("srl_bgp_neighbor_state", 2.0),
        ("srl_bgp_prefixes_accepted", 0.0),
        ("srl_bgp_received_routes", 0.0),
        ("srl_bgp_sent_routes", 0.0),
        ("srl_bgp_active_routes", 0.0),
    ],
};

static SRL2: Profile = Profile {
    device_label: "agent_host",
    interface_label: "ifDescr",
    peer_label: "bgpPeerRemoteAddr",
    asn_label: "bgpPeerRemoteAs",
    collection_type: "snmp",
    oper_state_metric: "ifOperStatus",
    bgp_metrics: [
        ("cbgpPeerOperStatus", 2.0),
        ("bgpPeerState", 2.0),
        ("cbgpPeerAcceptedPrefixes", 0.0),
        ("bgpPeerInPrefixes", 0.0),
        ("bgpPeerOutPrefixes", 0.0),
        ("cbgpPeerActivePrefixes", 0.0),
    ],
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub address: String,
    pub asn: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub id: String,
    pub name: String,
    pub state: String,
}

fn profile(device: &str) -> Result<&'static Profile, String> {
    match device {
        "srl1" => Ok(&SRL1),
        "srl2" => Ok(&SRL2),
        _ => Err(format!(
            "unknown device {device}; expected one of {}",
            DEVICES.join(", ")
        )),
    }
}

pub fn peers_on_link(lab: &LabVars, device: &str, interface: &str) -> Vec<Peer> {
    let Some(device) = lab.devices.iter().find(|d| d.name == device) else {
        return Vec::new();
    };
    let Some((address, length)) = device
        .interfaces
        .iter()
        .find(|i| i.name == interface)
        .and_then(|i| i.ip.as_deref())
        .and_then(address_and_length)
    else {
        return Vec::new();
    };
    let mask = u32::MAX.checked_shl(32 - length).unwrap_or(0);
    device
        .sessions
        .iter()
        .filter(|session| {
            session
                .peer
                .parse::<Ipv4Addr>()
                .is_ok_and(|peer| u32::from(peer) & mask == u32::from(address) & mask)
        })
        .map(|session| Peer {
            address: session.peer.clone(),
            asn: session
                .remote_as
                .map(|asn| asn.to_string())
                .unwrap_or_default(),
        })
        .collect()
}

fn address_and_length(ip: &str) -> Option<(Ipv4Addr, u32)> {
    let (address, length) = ip.split_once('/')?;
    let length = length.parse().ok().filter(|length| *length <= 32)?;
    Some((address.parse().ok()?, length))
}

pub fn interface_name(prefix: &str, device: &str, interface: &str) -> String {
    format!("{prefix}{device}-intf-{}", interface.replace('/', "-"))
}

pub fn bgp_name(prefix: &str, device: &str, peer: &str) -> String {
    format!("{prefix}{device}-bgp-{}", peer.replace('.', "-"))
}

fn cascade_active(up: &str, down: &str) -> Value {
    json!({
        "id": "cascade_active",
        "signal_type": "metrics",
        "name": "cascade_active",
        "generator": {
            "type": "flap",
            "up_duration": up,
            "down_duration": down,
            "up_value": 0,
            "down_value": 1,
        },
    })
}

fn while_active() -> Value {
    json!({"ref": "cascade_active", "op": ">", "value": 0})
}

#[allow(clippy::too_many_arguments)]
pub fn interface_cascade(
    prefix: &str,
    label: &str,
    device: &str,
    interface: &str,
    duration: &str,
    up: &str,
    down: &str,
    delay: &str,
    loki_url: &str,
) -> Result<Value, String> {
    let profile = profile(device)?;
    Ok(json!({
        "version": 2,
        "kind": "runnable",
        "scenario_name": interface_name(prefix, device, interface),
        "category": "network",
        "description": format!(
            "{label} inversion cascade \u{2014} pauses {device}:{interface} baseline during DOWN phase."
        ),
        "defaults": {
            "rate": 1,
            "duration": duration,
            "labels": {(profile.device_label): device},
        },
        "scenarios": [
            cascade_active(up, down),
            {
                "id": "cascade_oper_state",
                "signal_type": "metrics",
                "name": profile.oper_state_metric,
                "generator": {"type": "constant", "value": 2.0},
                "while": while_active(),
                "labels": {
                    (profile.interface_label): interface,
                    "collection_type": profile.collection_type,
                },
            },
            {
                "id": "updown_logs_down",
                "signal_type": "logs",
                "name": "updown_logs_down",
                "rate": 0.5,
                "while": while_active(),
                "delay": {"open": delay, "close": "0s"},
                "labels": {
                    "device": device,
                    "type": "srlinux",
                    "vendor_facility": "srlinux",
                    "vendor_facility_process": "UPDOWN",
                    "interface": interface,
                    "interface_status": "down",
                    "pipeline": "direct",
                },
                "log_generator": {
                    "type": "template",
                    "templates": [
                        {"message": format!("Interface {interface} changed state to down")},
                    ],
                    "severity_weights": {"warning": 1.0},
                },
                "encoder": {"type": "json_lines"},
                "sink": {"type": "loki", "url": loki_url},
            },
        ],
    }))
}

#[allow(clippy::too_many_arguments)]
pub fn bgp_cascade(
    prefix: &str,
    label: &str,
    device: &str,
    peer: &Peer,
    duration: &str,
    up: &str,
    down: &str,
    delay: &str,
) -> Result<Value, String> {
    let profile = profile(device)?;
    let (up, down, delay) = (seconds(up)?, seconds(down)?, seconds(delay)?);
    let bgp_up = duration_text(up + delay);
    let bgp_down = duration_text((down - delay).max(0.0));
    let labels = json!({
        (profile.peer_label): peer.address,
        (profile.asn_label): peer.asn,
        "afi_safi_name": "ipv4-unicast",
        "name": "default",
        "collection_type": profile.collection_type,
    });
    let mut scenarios = vec![cascade_active(&bgp_up, &bgp_down)];
    for (metric, down_value) in profile.bgp_metrics {
        scenarios.push(json!({
            "id": format!("cascade_{metric}"),
            "signal_type": "metrics",
            "name": metric,
            "generator": {"type": "constant", "value": down_value},
            "while": while_active(),
            "labels": labels,
        }));
    }
    Ok(json!({
        "version": 2,
        "kind": "runnable",
        "scenario_name": bgp_name(prefix, device, &peer.address),
        "category": "network",
        "description": format!(
            "{label} inversion cascade \u{2014} pauses {device} peer {} baseline during BGP-DOWN phase.",
            peer.address
        ),
        "defaults": {
            "rate": 1,
            "duration": duration,
            "labels": {(profile.device_label): device},
        },
        "scenarios": scenarios,
    }))
}

pub fn link_failover(
    device: &str,
    primary: &str,
    backup: &str,
    duration: &str,
    prom_url: &str,
) -> Value {
    json!({
        "version": 2,
        "kind": "runnable",
        "scenario_name": format!("incident-link-failover-{device}"),
        "category": "network",
        "description": format!("Link failover cascade on {device} ({primary} \u{2192} {backup})"),
        "defaults": {
            "rate": 1,
            "duration": duration,
            "encoder": {"type": "remote_write"},
            "sink": {"type": "remote_write", "url": prom_url},
            "labels": {
                "device": device,
                "pipeline": "direct",
                "collection_type": "gnmi",
                "intf_role": "peer",
                "source": "incident-cascade",
            },
        },
        "scenarios": [
            {
                "id": "primary_flap",
                "signal_type": "metrics",
                "name": "interface_oper_state",
                "generator": {"type": "flap", "up_duration": "60s", "down_duration": "30s"},
                "labels": {"name": primary},
            },
            {
                "id": "backup_saturation",
                "signal_type": "metrics",
                "name": "incident_backup_link_utilization",
                "generator": {
                    "type": "saturation",
                    "baseline": 20,
                    "ceiling": 85,
                    "time_to_saturate": "2m",
                },
                "labels": {"name": backup, "path": "backup"},
                "after": {"ref": "primary_flap", "op": "<", "value": 1},
            },
            {
                "id": "latency_degrade",
                "signal_type": "metrics",
                "name": "incident_latency_ms",
                "generator": {
                    "type": "degradation",
                    "baseline": 5,
                    "ceiling": 150,
                    "time_to_degrade": "3m",
                },
                "labels": {"name": backup, "path": "backup"},
                "after": {"ref": "backup_saturation", "op": ">", "value": 70},
            },
        ],
    })
}

pub fn seconds(duration: &str) -> Result<f64, String> {
    let text = duration.trim().to_lowercase();
    let (number, factor) = if let Some(number) = text.strip_suffix("ms") {
        (number, 0.001)
    } else if let Some(number) = text.strip_suffix('s') {
        (number, 1.0)
    } else if let Some(number) = text.strip_suffix('m') {
        (number, 60.0)
    } else if let Some(number) = text.strip_suffix('h') {
        (number, 3600.0)
    } else {
        (text.as_str(), 1.0)
    };
    number
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
        .map(|number| number * factor)
        .ok_or_else(|| format!("{duration} is not a duration"))
}

pub fn with_unit(duration: &str) -> String {
    if !duration.is_empty() && duration.bytes().all(|byte| byte.is_ascii_digit()) {
        format!("{duration}s")
    } else {
        duration.to_string()
    }
}

fn duration_text(seconds: f64) -> String {
    let rounded: f64 = format!("{seconds:.5e}").parse().unwrap_or(seconds);
    format!("{rounded}s")
}

pub fn created(body: &str) -> Result<(Vec<Created>, Vec<String>), String> {
    let unexpected = || body.chars().take(300).collect::<String>();
    let value: Value = serde_json::from_str(body).map_err(|_| unexpected())?;
    let entries = match value.get("scenarios").and_then(Value::as_array) {
        Some(list) => list.iter().map(entry).collect(),
        None if value.get("id").is_some() => vec![entry(&value)],
        None => return Err(unexpected()),
    };
    let warnings = value
        .get("warnings")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|warning| match warning {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok((entries, warnings))
}

fn entry(value: &Value) -> Created {
    let text = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Created {
        id: text("id"),
        name: text("name"),
        state: text("state"),
    }
}

pub fn state(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("state")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn follow(
    ids: &[String],
    done: &[&str],
    mut state: impl FnMut(&str) -> Result<String, String>,
    mut sleep: impl FnMut(Duration),
    mut line: impl FnMut(String),
    mut warn: impl FnMut(String),
) {
    let mut pending: Vec<&String> = ids.iter().collect();
    while !pending.is_empty() {
        sleep(POLL);
        pending.retain(|id| match state(id) {
            Ok(current) => {
                line(format!(
                    "  {} state={current}",
                    id.chars().take(8).collect::<String>()
                ));
                !done.contains(&current.as_str())
            }
            Err(error) => {
                warn(format!("poll error on {id}: {error}"));
                true
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lab_vars;

    const LAB: &str = include_str!("../../tests/fixtures/workshop-one/lab_vars.yml");
    const INTERFACE_SRL1: &str =
        include_str!("../../tests/fixtures/scenario-bodies/flap-interface-srl1-eth1-1.json");
    const BGP_SRL1: &str =
        include_str!("../../tests/fixtures/scenario-bodies/flap-bgp-srl1-10.1.2.2.json");
    const INTERFACE_SRL2: &str =
        include_str!("../../tests/fixtures/scenario-bodies/flap-interface-srl2-eth1-1.json");
    const BGP_SRL2: &str =
        include_str!("../../tests/fixtures/scenario-bodies/flap-bgp-srl2-10.1.2.1.json");
    const INCIDENT: &str =
        include_str!("../../tests/fixtures/scenario-bodies/incident-link-failover-srl1.json");

    fn fixture(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    fn peer(address: &str, asn: &str) -> Peer {
        Peer {
            address: address.to_string(),
            asn: asn.to_string(),
        }
    }

    fn lab() -> LabVars {
        lab_vars::read(LAB).unwrap()
    }

    fn interface(device: &str) -> Result<Value, String> {
        interface_cascade(
            "packt-cascade-",
            "Packt",
            device,
            "ethernet-1/1",
            "4m",
            "30s",
            "60s",
            "10s",
            "http://loki:3001",
        )
    }

    fn bgp(
        device: &str,
        address: &str,
        up: &str,
        down: &str,
        delay: &str,
    ) -> Result<Value, String> {
        bgp_cascade(
            "packt-cascade-",
            "Packt",
            device,
            &peer(address, "65102"),
            "4m",
            up,
            down,
            delay,
        )
    }

    fn bgp_durations(body: &Value) -> (&str, &str) {
        let generator = &body["scenarios"][0]["generator"];
        (
            generator["up_duration"].as_str().unwrap(),
            generator["down_duration"].as_str().unwrap(),
        )
    }

    #[test]
    fn names_are_the_prefix_the_device_the_kind_and_the_target_with_dashes() {
        assert_eq!(
            interface_name("packt-cascade-", "srl1", "ethernet-1/1"),
            "packt-cascade-srl1-intf-ethernet-1-1"
        );
        assert_eq!(
            bgp_name("packt-cascade-", "srl1", "10.1.2.2"),
            "packt-cascade-srl1-bgp-10-1-2-2"
        );
    }

    #[test]
    fn peers_on_link_gives_the_sessions_inside_the_network_of_the_interface() {
        let lab = lab();

        assert_eq!(
            peers_on_link(&lab, "srl1", "ethernet-1/1"),
            [peer("10.1.2.2", "65102")]
        );
        assert_eq!(
            peers_on_link(&lab, "srl1", "ethernet-1/10"),
            [peer("10.1.7.2", "65102")]
        );
        assert_eq!(
            peers_on_link(&lab, "srl2", "ethernet-1/1"),
            [peer("10.1.2.1", "65101")]
        );
    }

    #[test]
    fn peers_on_link_gives_nothing_for_the_broken_link_an_unknown_interface_or_device() {
        let lab = lab();

        assert_eq!(peers_on_link(&lab, "srl1", "ethernet-1/11"), []);
        assert_eq!(peers_on_link(&lab, "srl1", "ethernet-1/99"), []);
        assert_eq!(peers_on_link(&lab, "srl9", "ethernet-1/1"), []);
    }

    #[test]
    fn peers_on_link_uses_the_prefix_length_of_the_interface_address() {
        let lab = lab_vars::read(
            "nodes:\n  a:\n    interfaces:\n      - {name: e1, ipv4: 10.0.0.1/30}\nobservability_intent:\n  bgp:\n    intended_peers:\n      a:\n        - peer_ip: 10.0.0.2\n        - peer_ip: 10.0.0.5\n",
        )
        .unwrap();

        assert_eq!(peers_on_link(&lab, "a", "e1"), [peer("10.0.0.2", "")]);
    }

    #[test]
    fn peers_on_link_skips_an_interface_without_a_prefix_length_and_a_peer_that_is_not_ipv4() {
        let lab = lab_vars::read(
            "nodes:\n  a:\n    interfaces:\n      - {name: e1, ipv4: 10.0.0.1/24}\n      - {name: e2, ipv4: 10.0.0.1}\n      - {name: e3}\nobservability_intent:\n  bgp:\n    intended_peers:\n      a:\n        - peer_ip: 10.0.0.2\n        - peer_ip: \"fe80::1\"\n",
        )
        .unwrap();

        assert_eq!(peers_on_link(&lab, "a", "e1"), [peer("10.0.0.2", "")]);
        assert_eq!(peers_on_link(&lab, "a", "e2"), []);
        assert_eq!(peers_on_link(&lab, "a", "e3"), []);
    }

    #[test]
    fn interface_cascade_equals_the_fixture_for_each_device() {
        assert_eq!(interface("srl1").unwrap(), fixture(INTERFACE_SRL1));
        assert_eq!(interface("srl2").unwrap(), fixture(INTERFACE_SRL2));
    }

    #[test]
    fn bgp_cascade_equals_the_fixture_for_each_device() {
        assert_eq!(
            bgp("srl1", "10.1.2.2", "30s", "60s", "10s").unwrap(),
            fixture(BGP_SRL1)
        );
        let srl2 = bgp_cascade(
            "packt-cascade-",
            "Packt",
            "srl2",
            &peer("10.1.2.1", "65101"),
            "4m",
            "30s",
            "60s",
            "10s",
        );
        assert_eq!(srl2.unwrap(), fixture(BGP_SRL2));
    }

    #[test]
    fn link_failover_equals_the_fixture() {
        assert_eq!(
            link_failover(
                "srl1",
                "ethernet-1/10",
                "ethernet-1/1",
                "3m",
                "http://prometheus:9090/api/v1/write"
            ),
            fixture(INCIDENT)
        );
    }

    #[test]
    fn the_description_starts_with_the_label_given() {
        let interface = interface_cascade(
            "p-",
            "Demo",
            "srl1",
            "e1",
            "4m",
            "30s",
            "60s",
            "10s",
            "http://loki:3001",
        )
        .unwrap();
        let bgp = bgp_cascade(
            "p-",
            "Demo",
            "srl1",
            &peer("10.1.2.2", "65102"),
            "4m",
            "30s",
            "60s",
            "10s",
        )
        .unwrap();

        assert!(interface["description"]
            .as_str()
            .unwrap()
            .starts_with("Demo inversion cascade"));
        assert!(bgp["description"]
            .as_str()
            .unwrap()
            .starts_with("Demo inversion cascade"));
    }

    #[test]
    fn an_unknown_device_is_an_error_naming_the_devices_expected() {
        let expected = "unknown device srl9; expected one of srl1, srl2".to_string();

        assert_eq!(interface("srl9"), Err(expected.clone()));
        assert_eq!(bgp("srl9", "10.1.2.2", "30s", "60s", "10s"), Err(expected));
    }

    #[test]
    fn seconds_reads_ms_s_m_h_and_a_bare_number() {
        assert_eq!(seconds("500ms"), Ok(0.5));
        assert_eq!(seconds("30s"), Ok(30.0));
        assert_eq!(seconds("5m"), Ok(300.0));
        assert_eq!(seconds("1h"), Ok(3600.0));
        assert_eq!(seconds("45"), Ok(45.0));
    }

    #[test]
    fn seconds_refuses_text_that_is_not_a_duration_and_repeats_it() {
        assert_eq!(seconds("soon"), Err("soon is not a duration".to_string()));
        assert_eq!(seconds("inf"), Err("inf is not a duration".to_string()));
    }

    #[test]
    fn with_unit_gives_bare_digits_a_unit_and_leaves_the_rest() {
        assert_eq!(with_unit("90"), "90s");
        assert_eq!(with_unit("2m"), "2m");
        assert_eq!(with_unit("1.5"), "1.5");
        assert_eq!(with_unit(""), "");
    }

    #[test]
    fn the_bgp_cascade_flaps_longer_up_and_shorter_down_by_the_delay() {
        let body = bgp("srl1", "10.1.2.2", "30s", "60s", "10s").unwrap();

        assert_eq!(bgp_durations(&body), ("40s", "50s"));
    }

    #[test]
    fn the_bgp_down_phase_does_not_go_below_zero() {
        let body = bgp("srl1", "10.1.2.2", "30s", "60s", "90s").unwrap();

        assert_eq!(bgp_durations(&body), ("120s", "0s"));
    }

    #[test]
    fn the_bgp_cascade_refuses_a_delay_that_is_not_a_duration() {
        assert_eq!(
            bgp("srl1", "10.1.2.2", "30s", "60s", "soon"),
            Err("soon is not a duration".to_string())
        );
    }

    #[test]
    fn duration_text_rounds_to_six_digits_and_never_uses_an_exponent() {
        assert_eq!(duration_text(0.1 + 0.2), "0.3s");
        assert_eq!(duration_text(40.0), "40s");
        assert_eq!(duration_text(0.5), "0.5s");
        assert_eq!(duration_text(0.0), "0s");
        assert_eq!(duration_text(1234567.0), "1234570s");
    }

    #[test]
    fn created_reads_the_list_shape_with_its_warnings() {
        let body = r#"{"scenarios":[{"id":"a1","name":"one","state":"running"},{"id":"b2","name":"two","state":"pending"}],"warnings":["slow"]}"#;

        let (entries, warnings) = created(body).unwrap();

        assert_eq!(
            entries,
            [
                Created {
                    id: "a1".to_string(),
                    name: "one".to_string(),
                    state: "running".to_string()
                },
                Created {
                    id: "b2".to_string(),
                    name: "two".to_string(),
                    state: "pending".to_string()
                },
            ]
        );
        assert_eq!(warnings, ["slow"]);
    }

    #[test]
    fn created_reads_the_flat_shape_without_warnings() {
        let (entries, warnings) = created(r#"{"id":"a1","name":"one","state":"running"}"#).unwrap();

        assert_eq!(
            entries,
            [Created {
                id: "a1".to_string(),
                name: "one".to_string(),
                state: "running".to_string()
            }]
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn created_reads_the_warnings_of_the_flat_shape() {
        let (_, warnings) =
            created(r#"{"id":"a1","name":"n","state":"s","warnings":["w"]}"#).unwrap();

        assert_eq!(warnings, ["w"]);
    }

    #[test]
    fn created_gives_any_other_body_back_as_the_error() {
        assert_eq!(created(r#"{"ok":true}"#), Err(r#"{"ok":true}"#.to_string()));
        assert_eq!(created("not json"), Err("not json".to_string()));
    }

    #[test]
    fn created_cuts_the_body_in_the_error_at_300_characters() {
        let body = format!(r#"{{"ok":"{}"}}"#, "\u{e9}".repeat(400));

        let error = created(&body).unwrap_err();

        assert_eq!(error.chars().count(), 300);
        assert!(body.starts_with(&error));
    }

    #[test]
    fn state_is_the_text_of_the_state_field_and_unknown_for_anything_else() {
        assert_eq!(state(r#"{"id":"a1","state":"running"}"#), "running");
        assert_eq!(state(r#"{"state":"-"}"#), "-");
        assert_eq!(state(r#"{"id":"a1"}"#), "unknown");
        assert_eq!(state(r#"{"state":3}"#), "unknown");
        assert_eq!(state("not json"), "unknown");
    }

    struct Round {
        sleeps: Vec<Duration>,
        lines: Vec<String>,
        warnings: Vec<String>,
    }

    fn run(ids: &[String], state: impl FnMut(&str) -> Result<String, String>) -> Round {
        let (mut sleeps, mut lines, mut warnings) = (Vec::new(), Vec::new(), Vec::new());
        follow(
            ids,
            &["finished", "stopped"],
            state,
            |wait| sleeps.push(wait),
            |text| lines.push(text),
            |text| warnings.push(text),
        );
        Round {
            sleeps,
            lines,
            warnings,
        }
    }

    #[test]
    fn follow_asks_each_pending_id_after_a_sleep_until_every_id_is_done() {
        let ids = ["aaaaaaaa-1111".to_string(), "bbbbbbbb-2222".to_string()];
        let mut asked_a = 0;

        let round = run(&ids, |id| {
            if id.starts_with('a') {
                asked_a += 1;
                Ok(if asked_a == 1 { "running" } else { "finished" }.to_string())
            } else {
                Ok("stopped".to_string())
            }
        });

        assert_eq!(round.sleeps, [Duration::from_secs(5); 2]);
        assert_eq!(
            round.lines,
            [
                "  aaaaaaaa state=running",
                "  bbbbbbbb state=stopped",
                "  aaaaaaaa state=finished",
            ]
        );
        assert!(round.warnings.is_empty());
    }

    #[test]
    fn follow_warns_on_an_error_and_keeps_the_id_pending() {
        let ids = ["aaaaaaaa-1111".to_string()];
        let mut asked = 0;

        let round = run(&ids, |_| {
            asked += 1;
            if asked == 1 {
                Err("timed out".to_string())
            } else {
                Ok("finished".to_string())
            }
        });

        assert_eq!(round.warnings, ["poll error on aaaaaaaa-1111: timed out"]);
        assert_eq!(round.lines, ["  aaaaaaaa state=finished"]);
        assert_eq!(round.sleeps.len(), 2);
    }

    #[test]
    fn follow_sleeps_before_asking_and_not_after_the_last_round() {
        let events = std::cell::RefCell::new(Vec::new());
        let ids = ["aaaaaaaa-1111".to_string()];

        follow(
            &ids,
            &["finished"],
            |_| {
                events.borrow_mut().push("ask");
                Ok("finished".to_string())
            },
            |_| events.borrow_mut().push("sleep"),
            |_| {},
            |_| {},
        );

        assert_eq!(*events.borrow(), ["sleep", "ask"]);
    }

    #[test]
    fn follow_with_no_ids_returns_at_once_without_sleeping() {
        let round = run(&[], |_| Err("never asked".to_string()));

        assert!(round.sleeps.is_empty());
        assert!(round.lines.is_empty());
        assert!(round.warnings.is_empty());
    }
}
