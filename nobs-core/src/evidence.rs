use serde_json::Value;

use crate::sot::{Device, Session};

const MISSING: &str = "—";
const UNKNOWN_FIELD: &str = "?";
const RCA_PREFIX: &str = "AI RCA:";
const ADMIN: [&str; 2] = ["enable", "disable"];
const OPER: [&str; 5] = ["up", "down", "idle", "connect", "active"];
const DEFAULT_AFI_SAFI: &str = "ipv4-unicast";

#[derive(Debug, Clone, PartialEq)]
pub struct Metrics {
    pub admin: Option<f64>,
    pub oper: Option<f64>,
    pub received: Option<f64>,
    pub sent: Option<f64>,
    pub active: Option<f64>,
    pub suppressed: Option<f64>,
}

pub fn decode_admin(value: Option<f64>) -> String {
    decode(value, &ADMIN)
}

pub fn decode_oper(value: Option<f64>) -> String {
    decode(value, &OPER)
}

fn decode(value: Option<f64>, names: &[&str]) -> String {
    match value {
        None => MISSING.to_string(),
        Some(-1.0) => MISSING.to_string(),
        Some(value) => names
            .iter()
            .enumerate()
            .find(|(index, _)| value == (index + 1) as f64)
            .map_or("unknown", |(_, name)| name)
            .to_string(),
    }
}

pub fn value_text(value: Option<f64>, missing: &str) -> String {
    match value {
        None => missing.to_string(),
        Some(value) if value.is_finite() => value.to_string(),
        Some(_) => "?".to_string(),
    }
}

pub fn session<'a>(intent: &'a Device, peer: &str, afi_safi: &str) -> Option<&'a Session> {
    intent.sessions.iter().find(|session| {
        session.peer == peer && session.afi_safi.as_deref().unwrap_or(DEFAULT_AFI_SAFI) == afi_safi
    })
}

pub fn decision(
    intent: Option<&Device>,
    session: Option<&Session>,
    metrics: &Metrics,
    reason_when_missing: &str,
) -> (String, String) {
    let (verdict, reason) = rule(intent, session, metrics, reason_when_missing);
    (verdict.to_string(), reason.to_string())
}

fn rule<'a>(
    intent: Option<&Device>,
    session: Option<&Session>,
    metrics: &Metrics,
    reason_when_missing: &'a str,
) -> (&'static str, &'a str) {
    let Some(intent) = intent else {
        return ("stop", reason_when_missing);
    };
    if intent.maintenance {
        return ("skip", "device under maintenance");
    }
    let Some(session) = session else {
        return ("skip", "peer not intended in SoT");
    };
    let expected = session
        .expected_state
        .as_deref()
        .unwrap_or("established")
        .to_lowercase();
    if expected == "down" || expected == "disabled" {
        return ("skip", "SoT expects this peer to be down/disabled");
    }
    if metrics.admin == Some(1.0) && metrics.oper == Some(1.0) {
        return ("skip", "peer matches SoT intent (enabled + up)");
    }
    ("proceed", "SoT expects peer up, but metrics show mismatch")
}

pub fn fill(query: &str, device: &str, peer: &str) -> String {
    query
        .split("{device}")
        .map(|part| part.replace("{peer}", peer))
        .collect::<Vec<_>>()
        .join(device)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RcaRecord {
    pub timestamp: String,
    pub device: String,
    pub peer: String,
    pub message: String,
}

pub fn rca_record(line: &str) -> Option<RcaRecord> {
    let record: Value = serde_json::from_str(line).ok()?;
    let text = |value: Option<&Value>, missing: &str| match value {
        None | Some(Value::Null) => missing.to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    };
    let message = text(record.get("message"), "");
    let message = message
        .strip_prefix(RCA_PREFIX)
        .map_or(message.as_str(), |rest| rest.trim_start_matches('\n'));
    Some(RcaRecord {
        timestamp: text(record.get("timestamp"), ""),
        device: text(record.pointer("/labels/device"), UNKNOWN_FIELD),
        peer: text(record.pointer("/labels/peer_address"), UNKNOWN_FIELD),
        message: message.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKT: &str = r#"{device="{device}",peer_address="{peer}"} != "license""#;
    const AUTOCON5: &str = r#"{device="{device}"} != "license" |~ "(bgp|BGP|neighbor|session|route|ipv4-unicast|{peer})""#;
    const MISSING_REASON: &str = "device not found in Infrahub";

    fn intent(maintenance: bool, sessions: Vec<Session>) -> Device {
        Device {
            name: "srl1".to_string(),
            kind: None,
            maintenance,
            site: None,
            role: None,
            asn: None,
            interfaces: vec![],
            sessions,
        }
    }

    fn intended(expected_state: Option<&str>) -> Session {
        Session {
            peer: "10.1.99.2".to_string(),
            afi_safi: None,
            expected_state: expected_state.map(str::to_string),
            remote_as: None,
            expected_prefixes: None,
            reason: None,
        }
    }

    fn metrics(admin: Option<f64>, oper: Option<f64>) -> Metrics {
        Metrics {
            admin,
            oper,
            received: None,
            sent: None,
            active: None,
            suppressed: None,
        }
    }

    fn decided(
        intent: Option<&Device>,
        session: Option<&Session>,
        metrics: &Metrics,
    ) -> (String, String) {
        decision(intent, session, metrics, MISSING_REASON)
    }

    fn pair(verdict: &str, reason: &str) -> (String, String) {
        (verdict.to_string(), reason.to_string())
    }

    #[test]
    fn fill_puts_the_device_and_the_peer_into_the_packt_query() {
        assert_eq!(
            fill(PACKT, "srl1", "10.1.99.2"),
            r#"{device="srl1",peer_address="10.1.99.2"} != "license""#
        );
    }

    #[test]
    fn fill_puts_the_device_and_the_peer_into_the_autocon5_query_and_keeps_its_braces() {
        assert_eq!(
            fill(AUTOCON5, "srl1", "10.1.99.2"),
            r#"{device="srl1"} != "license" |~ "(bgp|BGP|neighbor|session|route|ipv4-unicast|10.1.99.2)""#
        );
    }

    #[test]
    fn fill_replaces_every_placeholder_and_leaves_a_query_without_any_alone() {
        assert_eq!(fill("{device}{device}{peer}{peer}", "a", "b"), "aabb");
        assert_eq!(fill(r#"{device="x"}"#, "a", "b"), r#"{device="x"}"#);
    }

    #[test]
    fn fill_does_not_read_a_placeholder_inside_a_value_it_inserted() {
        assert_eq!(
            fill("{device} {peer}", "{peer}", "{device}"),
            "{peer} {device}"
        );
    }

    #[test]
    fn decode_admin_names_1_and_2_and_calls_every_other_number_unknown() {
        let decoded: Vec<String> = [1.0, 2.0, 5.0, 7.0, 1.5, 0.0, -2.0]
            .iter()
            .map(|value| decode_admin(Some(*value)))
            .collect();

        assert_eq!(
            decoded,
            ["enable", "disable", "unknown", "unknown", "unknown", "unknown", "unknown"]
        );
    }

    #[test]
    fn decode_oper_names_1_to_5_and_calls_every_other_number_unknown() {
        let decoded: Vec<String> = [1.0, 2.0, 3.0, 4.0, 5.0, 7.0, 1.5]
            .iter()
            .map(|value| decode_oper(Some(*value)))
            .collect();

        assert_eq!(
            decoded,
            ["up", "down", "idle", "connect", "active", "unknown", "unknown"]
        );
    }

    #[test]
    fn a_missing_value_and_minus_one_decode_to_a_dash() {
        for decode in [decode_admin, decode_oper] {
            assert_eq!(decode(None), "—");
            assert_eq!(decode(Some(-1.0)), "—");
        }
    }

    #[test]
    fn nan_and_infinite_values_decode_unknown() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(decode_admin(Some(value)), "unknown");
            assert_eq!(decode_oper(Some(value)), "unknown");
        }
    }

    #[test]
    fn value_text_prints_whole_numbers_without_a_fraction_and_others_as_they_are() {
        assert_eq!(value_text(Some(1.0), "0"), "1");
        assert_eq!(value_text(Some(10.0), "0"), "10");
        assert_eq!(value_text(Some(0.5), "0"), "0.5");
        assert_eq!(value_text(Some(-1.0), "0"), "-1");
    }

    #[test]
    fn value_text_prints_the_missing_text_for_none_and_a_question_mark_for_nan_or_infinity() {
        assert_eq!(value_text(None, "-1"), "-1");
        assert_eq!(value_text(None, "0"), "0");
        assert_eq!(value_text(Some(f64::NAN), "0"), "?");
        assert_eq!(value_text(Some(f64::INFINITY), "0"), "?");
        assert_eq!(value_text(Some(f64::NEG_INFINITY), "0"), "?");
    }

    #[test]
    fn session_matches_the_peer_and_the_afi_safi_and_counts_an_absent_afi_safi_as_ipv4_unicast() {
        let mut ipv6 = intended(None);
        ipv6.afi_safi = Some("ipv6-unicast".to_string());
        let mut other = intended(None);
        other.peer = "10.1.2.2".to_string();
        let device = intent(false, vec![other, ipv6, intended(Some("established"))]);

        let default = session(&device, "10.1.99.2", "ipv4-unicast").unwrap();
        let v6 = session(&device, "10.1.99.2", "ipv6-unicast").unwrap();

        assert_eq!(default.afi_safi, None);
        assert_eq!(v6.afi_safi.as_deref(), Some("ipv6-unicast"));
        assert!(session(&device, "10.9.9.9", "ipv4-unicast").is_none());
        assert!(session(&device, "10.1.99.2", "l2vpn-evpn").is_none());
    }

    #[test]
    fn rule_1_no_intent_stops_with_the_reason_it_was_given() {
        let result = decided(None, Some(&intended(None)), &metrics(Some(1.0), Some(1.0)));

        assert_eq!(result, pair("stop", MISSING_REASON));
    }

    #[test]
    fn rule_2_maintenance_skips_before_anything_else_is_read() {
        let device = intent(true, vec![]);

        let result = decided(Some(&device), None, &metrics(None, None));

        assert_eq!(result, pair("skip", "device under maintenance"));
    }

    #[test]
    fn rule_3_a_peer_that_is_not_intended_skips() {
        let device = intent(false, vec![]);

        let result = decided(Some(&device), None, &metrics(Some(1.0), Some(1.0)));

        assert_eq!(result, pair("skip", "peer not intended in SoT"));
    }

    #[test]
    fn rule_4_an_expected_state_of_down_or_disabled_skips_in_any_letter_case() {
        let device = intent(false, vec![]);
        for state in ["down", "disabled", "DOWN", "Disabled"] {
            let session = intended(Some(state));

            let result = decided(
                Some(&device),
                Some(&session),
                &metrics(Some(1.0), Some(1.0)),
            );

            assert_eq!(
                result,
                pair("skip", "SoT expects this peer to be down/disabled"),
                "{state}"
            );
        }
    }

    #[test]
    fn rule_5_admin_1_and_oper_1_skips_because_the_peer_matches_the_intent() {
        let device = intent(false, vec![]);
        let session = intended(Some("established"));

        let result = decided(
            Some(&device),
            Some(&session),
            &metrics(Some(1.0), Some(1.0)),
        );

        assert_eq!(
            result,
            pair("skip", "peer matches SoT intent (enabled + up)")
        );
    }

    #[test]
    fn rule_6_anything_else_proceeds() {
        let device = intent(false, vec![]);
        let session = intended(Some("established"));
        let mismatches = [
            metrics(Some(1.0), Some(5.0)),
            metrics(Some(2.0), Some(1.0)),
            metrics(Some(1.0), None),
            metrics(None, Some(1.0)),
            metrics(Some(1.5), Some(1.0)),
            metrics(Some(f64::NAN), Some(1.0)),
        ];
        for metrics in mismatches {
            let result = decided(Some(&device), Some(&session), &metrics);

            assert_eq!(
                result,
                pair("proceed", "SoT expects peer up, but metrics show mismatch"),
                "{metrics:?}"
            );
        }
    }

    #[test]
    fn an_absent_expected_state_counts_as_established() {
        let device = intent(false, vec![]);

        let result = decided(
            Some(&device),
            Some(&intended(None)),
            &metrics(Some(1.0), Some(5.0)),
        );

        assert_eq!(
            result,
            pair("proceed", "SoT expects peer up, but metrics show mismatch")
        );
    }

    fn record(line: &str) -> RcaRecord {
        rca_record(line).unwrap()
    }

    fn message_of(text: &str) -> String {
        record(&format!(r#"{{"message":"{text}"}}"#)).message
    }

    fn all_unknown() -> RcaRecord {
        RcaRecord {
            timestamp: String::new(),
            device: "?".to_string(),
            peer: "?".to_string(),
            message: String::new(),
        }
    }

    #[test]
    fn a_line_that_is_not_json_is_not_a_record() {
        for line in ["", "not json", r#"{"message":"#, "2026-11-06 BGP down"] {
            assert_eq!(rca_record(line), None, "{line}");
        }
    }

    #[test]
    fn rca_record_reads_the_timestamp_the_labels_and_the_message() {
        let line = r#"{"timestamp":"2026-11-06T10:15:48.201Z","message":"plain text","labels":{"device":"srl1","peer_address":"10.1.99.2"}}"#;

        assert_eq!(
            record(line),
            RcaRecord {
                timestamp: "2026-11-06T10:15:48.201Z".to_string(),
                device: "srl1".to_string(),
                peer: "10.1.99.2".to_string(),
                message: "plain text".to_string(),
            }
        );
    }

    #[test]
    fn the_device_and_the_peer_are_read_one_by_one() {
        let device_only = record(r#"{"labels":{"device":"srl1"}}"#);
        let peer_only = record(r#"{"labels":{"peer_address":"10.1.99.2"}}"#);

        assert_eq!(device_only.device, "srl1");
        assert_eq!(device_only.peer, "?");
        assert_eq!(peer_only.device, "?");
        assert_eq!(peer_only.peer, "10.1.99.2");
    }

    #[test]
    fn the_prefix_and_the_newlines_right_after_it_are_removed_and_a_following_space_is_kept() {
        assert_eq!(message_of(r"AI RCA:\n\n## Cause\nmore"), "## Cause\nmore");
        assert_eq!(message_of("AI RCA: Check it"), " Check it");
        assert_eq!(message_of(r"AI RCA:\n Check it"), " Check it");
        assert_eq!(message_of("AI RCA:"), "");
    }

    #[test]
    fn a_message_without_the_prefix_at_its_start_is_kept_whole() {
        assert_eq!(message_of(r"\nAI RCA: later"), "\nAI RCA: later");
        assert_eq!(message_of("Note: AI RCA: later"), "Note: AI RCA: later");
    }

    #[test]
    fn a_missing_or_null_label_reads_as_a_question_mark_and_a_missing_timestamp_or_message_as_empty(
    ) {
        for line in [
            "{}",
            r#"{"timestamp":null,"message":null,"labels":null}"#,
            r#"{"labels":{"device":null,"peer_address":null}}"#,
            r#"{"labels":"srl1"}"#,
        ] {
            assert_eq!(record(line), all_unknown(), "{line}");
        }
    }

    #[test]
    fn a_json_value_that_is_not_an_object_has_an_empty_timestamp_and_question_mark_labels() {
        for line in ["123", "null", r#""AI RCA: text""#, r#"["srl1"]"#] {
            assert_eq!(record(line), all_unknown(), "{line}");
        }
    }
}
