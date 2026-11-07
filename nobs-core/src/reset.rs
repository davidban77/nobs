use serde_json::Value;

use crate::alertmanager::Silence;
use crate::sonda::ScenarioRow;

pub const CASCADE_NAMES: [&str; 4] = [
    "cascade_active",
    "incident_backup_link_utilization",
    "incident_latency_ms",
    "updown_logs_down",
];
pub const INCIDENT_PREFIX: &str = "incident-link-failover-";

pub fn workshop_silences(silences: &[Silence], hints: &[String]) -> Vec<String> {
    silences
        .iter()
        .filter(|silence| silence.state == "active" && !silence.id.is_empty())
        .filter(|silence| {
            hints
                .iter()
                .any(|hint| silence.comment.contains(hint) || silence.created_by.contains(hint))
        })
        .map(|silence| silence.id.clone())
        .collect()
}

pub fn cascade_ids(rows: &[ScenarioRow]) -> Vec<String> {
    rows.iter()
        .filter(|row| CASCADE_NAMES.contains(&row.name.as_str()))
        .filter(|row| !row.id.is_empty() && row.id != "-")
        .map(|row| row.id.clone())
        .collect()
}

pub fn is_wedged(show_body: &str, cascade_prefix: &str) -> bool {
    serde_json::from_str::<Value>(show_body)
        .ok()
        .and_then(|body| {
            let name = body.pointer("/pending_ref/scenario_name")?.as_str()?;
            Some(name.starts_with(cascade_prefix) || name.starts_with(INCIDENT_PREFIX))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silence(id: &str, state: &str, created_by: &str, comment: &str) -> Silence {
        Silence {
            id: id.to_string(),
            state: state.to_string(),
            starts_at: String::new(),
            ends_at: String::new(),
            matchers: Vec::new(),
            created_by: created_by.to_string(),
            comment: comment.to_string(),
        }
    }

    fn row(id: &str, name: &str) -> ScenarioRow {
        ScenarioRow {
            id: id.to_string(),
            name: name.to_string(),
            state: "running".to_string(),
            elapsed_secs: "1".to_string(),
            degraded: "false".to_string(),
        }
    }

    fn pending(name: &str) -> String {
        format!(r#"{{"pending_ref":{{"scenario_name":"{name}"}}}}"#)
    }

    #[test]
    fn workshop_silences_keeps_active_ones_with_a_hint_in_the_comment_or_the_creator() {
        let silences = [
            silence("a", "active", "alice", "Workshop test silence"),
            silence("b", "active", "workshop-bot", ""),
            silence("c", "expired", "alice", "Workshop test silence"),
            silence("d", "active", "alice", "my own silence"),
            silence("", "active", "alice", "Workshop test silence"),
            silence("e", "active", "alice", "workshop"),
        ];
        let hints = ["Workshop".to_string(), "workshop".to_string()];

        assert_eq!(workshop_silences(&silences, &hints), ["a", "b", "e"]);
    }

    #[test]
    fn cascade_ids_keeps_the_four_cascade_names_in_list_order() {
        let rows = [
            row("1", "cascade_active"),
            row("2", "incident_backup_link_utilization"),
            row("3", "incident_latency_ms"),
            row("4", "updown_logs_down"),
        ];

        assert_eq!(cascade_ids(&rows), ["1", "2", "3", "4"]);
    }

    #[test]
    fn cascade_ids_drops_other_names_an_empty_id_and_the_dash_of_a_missing_id() {
        let rows = [
            row("1", "bgpPeerState"),
            row("", "cascade_active"),
            row("-", "cascade_active"),
            row("5", "cascade_active"),
        ];

        assert_eq!(cascade_ids(&rows), ["5"]);
    }

    #[test]
    fn is_wedged_is_true_for_the_cascade_prefix_and_for_the_incident_prefix() {
        let prefix = "nobs-t-one-cascade-";

        assert!(is_wedged(
            &pending("nobs-t-one-cascade-srl1-intf-ethernet-1-1"),
            prefix
        ));
        assert!(is_wedged(&pending("incident-link-failover-srl1"), prefix));
    }

    #[test]
    fn is_wedged_is_false_for_another_name_a_missing_field_and_text_that_is_not_json() {
        let prefix = "nobs-t-one-cascade-";

        assert!(!is_wedged(&pending("baseline-srl1"), prefix));
        assert!(!is_wedged(r#"{"name":"x"}"#, prefix));
        assert!(!is_wedged(r#"{"pending_ref":{}}"#, prefix));
        assert!(!is_wedged("not json", prefix));
    }
}
