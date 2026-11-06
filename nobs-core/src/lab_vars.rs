use std::collections::HashMap;
use std::fmt;

use serde::de::{Deserializer, MapAccess, Visitor};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabVars {
    pub devices: Vec<Device>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub asn: Option<i64>,
    pub site: String,
    pub role: String,
    pub maintenance: bool,
    pub interfaces: Vec<Interface>,
    pub sessions: Vec<BgpSession>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub ip: Option<String>,
    pub role: String,
    pub expected_state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BgpSession {
    pub peer: String,
    pub remote_as: Option<i64>,
    pub afi_safi: String,
    pub expected_state: String,
    pub expected_prefixes: Option<i64>,
    pub reason: Option<String>,
}

#[derive(Deserialize)]
struct File {
    #[serde(default, deserialize_with = "in_file_order")]
    nodes: Vec<(String, Value)>,
    observability_intent: Option<Intent>,
}

#[derive(Deserialize)]
struct Intent {
    bgp: Option<Bgp>,
}

#[derive(Deserialize)]
struct Bgp {
    afi_safi: Option<String>,
    intended_peers: Option<HashMap<String, Option<Vec<Value>>>>,
}

#[derive(Deserialize)]
struct Block {
    asn: Option<i64>,
    site: Option<String>,
    role: Option<String>,
    maintenance: Option<bool>,
    interfaces: Option<Vec<RawInterface>>,
}

#[derive(Deserialize)]
struct RawInterface {
    name: String,
    ipv4: Option<Address>,
    ip: Option<Address>,
    role: Option<String>,
    expected_state: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Address {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct RawSession {
    peer_ip: String,
    remote_as: Option<i64>,
    expected_state: Option<String>,
    expected_prefixes_received: Option<i64>,
    reason: Option<String>,
}

impl Address {
    fn first(self) -> Option<String> {
        match self {
            Address::One(address) => Some(address),
            Address::Many(addresses) => addresses.into_iter().next(),
        }
    }
}

fn in_file_order<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<(String, Value)>, D::Error> {
    struct Entries;

    impl<'de> Visitor<'de> for Entries {
        type Value = Vec<(String, Value)>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a map from device name to its block")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut entries = Vec::new();
            while let Some(entry) = map.next_entry()? {
                entries.push(entry);
            }
            Ok(entries)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
    }

    deserializer.deserialize_any(Entries)
}

pub fn read(text: &str) -> Result<LabVars, String> {
    let file: File = serde_saphyr::from_str(text).map_err(|error| error.to_string())?;
    let bgp = file.observability_intent.and_then(|intent| intent.bgp);
    let afi_safi = bgp
        .as_ref()
        .and_then(|bgp| bgp.afi_safi.clone())
        .unwrap_or_else(|| "ipv4-unicast".to_string());
    let mut peers = bgp.and_then(|bgp| bgp.intended_peers).unwrap_or_default();
    let devices = file
        .nodes
        .into_iter()
        .map(|(name, block)| {
            let sessions = peers.remove(&name).flatten().unwrap_or_default();
            device(name, block, &afi_safi, sessions)
        })
        .collect::<Result<Vec<Device>, String>>()?;
    Ok(LabVars { devices })
}

fn device(
    name: String,
    block: Value,
    afi_safi: &str,
    sessions: Vec<Value>,
) -> Result<Device, String> {
    let context = |error: serde_json::Error| format!("{name}: {error}");
    if !block.is_object() {
        return Err(format!("{name}: the block is not a map"));
    }
    let block: Block = serde_json::from_value(block).map_err(context)?;
    let sessions = sessions
        .into_iter()
        .map(|session| {
            let session: RawSession = serde_json::from_value(session).map_err(context)?;
            Ok(BgpSession {
                peer: session.peer_ip,
                remote_as: session.remote_as,
                afi_safi: afi_safi.to_string(),
                expected_state: session
                    .expected_state
                    .unwrap_or_else(|| "established".to_string()),
                expected_prefixes: session.expected_prefixes_received,
                reason: session.reason,
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(Device {
        asn: block.asn,
        site: block.site.unwrap_or_else(|| "lab".to_string()),
        role: block.role.unwrap_or_else(|| "edge".to_string()),
        maintenance: block.maintenance.unwrap_or(false),
        interfaces: block
            .interfaces
            .unwrap_or_default()
            .into_iter()
            .map(interface)
            .collect(),
        sessions,
        name,
    })
}

fn interface(raw: RawInterface) -> Interface {
    Interface {
        name: raw.name,
        ip: raw
            .ipv4
            .and_then(Address::first)
            .or_else(|| raw.ip.and_then(Address::first)),
        role: raw.role.unwrap_or_else(|| "peer".to_string()),
        expected_state: raw.expected_state.unwrap_or_else(|| "up".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/workshop-one/lab_vars.yml");

    fn fixture() -> LabVars {
        read(FIXTURE).unwrap()
    }

    fn names(lab: &LabVars) -> Vec<&str> {
        lab.devices
            .iter()
            .map(|device| device.name.as_str())
            .collect()
    }

    #[test]
    fn the_fixture_has_two_devices_with_their_asn_and_the_defaults() {
        let lab = fixture();

        assert_eq!(names(&lab), ["srl1", "srl2"]);
        let srl1 = &lab.devices[0];
        assert_eq!(srl1.asn, Some(65101));
        assert_eq!(srl1.site, "lab");
        assert_eq!(srl1.role, "edge");
        assert!(!srl1.maintenance);
        assert_eq!(lab.devices[1].asn, Some(65102));
    }

    #[test]
    fn devices_come_in_the_order_of_the_file_not_sorted() {
        let lab = read("nodes:\n  zeta: {}\n  alpha: {}\n  mid: {}\n").unwrap();
        assert_eq!(names(&lab), ["zeta", "alpha", "mid"]);
    }

    #[test]
    fn the_fixture_gives_srl1_four_interfaces_and_lo0_takes_the_first_address_of_its_list() {
        let srl1 = &fixture().devices[0];

        let seen: Vec<(&str, Option<&str>, &str)> = srl1
            .interfaces
            .iter()
            .map(|i| (i.name.as_str(), i.ip.as_deref(), i.role.as_str()))
            .collect();
        assert_eq!(
            seen,
            [
                ("ethernet-1/1", Some("10.1.2.1/24"), "peer"),
                ("ethernet-1/10", Some("10.1.7.1/24"), "peer"),
                ("ethernet-1/11", Some("10.1.11.1/24"), "peer"),
                ("lo0", Some("10.17.17.1/32"), "loopback"),
            ]
        );
    }

    #[test]
    fn an_interface_keeps_its_expected_state_and_defaults_to_up() {
        let lab = fixture();
        let states: Vec<&str> = lab.devices[0]
            .interfaces
            .iter()
            .map(|i| i.expected_state.as_str())
            .collect();
        assert_eq!(states, ["up", "up", "down", "up"]);
    }

    #[test]
    fn the_fixture_gives_srl1_three_sessions_and_only_the_third_has_a_reason() {
        let srl1 = &fixture().devices[0];

        assert_eq!(
            srl1.sessions[0],
            BgpSession {
                peer: "10.1.2.2".to_string(),
                remote_as: Some(65102),
                afi_safi: "ipv4-unicast".to_string(),
                expected_state: "established".to_string(),
                expected_prefixes: Some(10),
                reason: None,
            }
        );
        assert_eq!(srl1.sessions.len(), 3);
        assert_eq!(srl1.sessions[2].peer, "10.1.99.2");
        assert_eq!(srl1.sessions[2].reason.as_deref(), Some("ip-mismatch-demo"));
        assert_eq!(srl1.sessions[2].expected_prefixes, None);
        assert_eq!(fixture().devices[1].sessions.len(), 3);
    }

    #[test]
    fn afi_safi_comes_from_the_intent_and_defaults_to_ipv4_unicast() {
        let given = read(
            "nodes:\n  a: {}\nobservability_intent:\n  bgp:\n    afi_safi: l2vpn-evpn\n    intended_peers:\n      a:\n        - peer_ip: 10.0.0.1\n",
        )
        .unwrap();
        assert_eq!(given.devices[0].sessions[0].afi_safi, "l2vpn-evpn");

        let missing = read(
            "nodes:\n  a: {}\nobservability_intent:\n  bgp:\n    intended_peers:\n      a:\n        - peer_ip: 10.0.0.1\n",
        )
        .unwrap();
        assert_eq!(missing.devices[0].sessions[0].afi_safi, "ipv4-unicast");
        assert_eq!(missing.devices[0].sessions[0].expected_state, "established");
    }

    #[test]
    fn a_file_without_observability_intent_gives_devices_without_sessions() {
        let lab = read("nodes:\n  a:\n    asn: 1\n").unwrap();
        assert!(lab.devices[0].sessions.is_empty());
    }

    #[test]
    fn a_file_without_nodes_gives_no_devices() {
        assert!(read("prefixes: []\n").unwrap().devices.is_empty());
        assert!(read("nodes:\n").unwrap().devices.is_empty());
    }

    #[test]
    fn ip_is_accepted_as_the_address_key_and_ipv4_wins_when_both_are_given() {
        let lab = read(
            "nodes:\n  a:\n    interfaces:\n      - {name: e1, ip: 10.0.0.1/24}\n      - {name: e2, ip: [10.0.0.2/24, 10.0.0.3/24]}\n      - {name: e3, ipv4: 10.0.0.4/24, ip: 10.0.0.5/24}\n      - {name: e4}\n",
        )
        .unwrap();

        let ips: Vec<Option<&str>> = lab.devices[0]
            .interfaces
            .iter()
            .map(|i| i.ip.as_deref())
            .collect();
        assert_eq!(
            ips,
            [
                Some("10.0.0.1/24"),
                Some("10.0.0.2/24"),
                Some("10.0.0.4/24"),
                None
            ]
        );
    }

    #[test]
    fn a_key_with_a_null_value_counts_as_absent_so_the_default_applies() {
        let lab = read(
            "nodes:\n  a:\n    site:\n    role:\n    maintenance:\n    interfaces:\n      - name: e1\n        role:\n        expected_state:\n",
        )
        .unwrap();

        let device = &lab.devices[0];
        assert_eq!(device.site, "lab");
        assert_eq!(device.role, "edge");
        assert!(!device.maintenance);
        assert_eq!(device.interfaces[0].role, "peer");
        assert_eq!(device.interfaces[0].expected_state, "up");
    }

    #[test]
    fn the_site_the_role_and_maintenance_of_a_device_are_read() {
        let lab = read("nodes:\n  a: {site: rack1, role: spine, maintenance: true}\n").unwrap();
        let device = &lab.devices[0];
        assert_eq!(device.site, "rack1");
        assert_eq!(device.role, "spine");
        assert!(device.maintenance);
    }

    #[test]
    fn a_device_block_that_is_not_a_map_is_an_error_naming_the_device() {
        for text in [
            "nodes:\n  srl9: 5\n",
            "nodes:\n  srl9:\n",
            "nodes:\n  srl9: [1]\n",
        ] {
            let error = read(text).unwrap_err();
            assert!(error.contains("srl9"), "{text}: {error}");
        }
    }

    #[test]
    fn an_interface_without_a_name_is_an_error_naming_the_device() {
        let error = read("nodes:\n  srl9:\n    interfaces:\n      - {role: peer}\n").unwrap_err();
        assert!(error.contains("srl9") && error.contains("name"), "{error}");
    }

    #[test]
    fn a_session_without_a_peer_ip_is_an_error_naming_the_device() {
        let error = read(
            "nodes:\n  srl9: {}\nobservability_intent:\n  bgp:\n    intended_peers:\n      srl9:\n        - remote_as: 1\n",
        )
        .unwrap_err();
        assert!(
            error.contains("srl9") && error.contains("peer_ip"),
            "{error}"
        );
    }

    #[test]
    fn text_that_is_not_yaml_is_an_error() {
        assert!(read("nodes: [").is_err());
    }

    #[test]
    fn a_device_listed_only_under_intended_peers_is_not_a_device() {
        let lab = read(
            "nodes:\n  a: {}\nobservability_intent:\n  bgp:\n    intended_peers:\n      ghost:\n        - peer_ip: 10.0.0.1\n",
        )
        .unwrap();
        assert_eq!(names(&lab), ["a"]);
    }
}
