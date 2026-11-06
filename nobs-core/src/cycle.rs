use serde_json::Value;

use crate::prefect::{FlowRun, TaskRun};
use crate::time;

const CHILD_NAME_SEPARATOR: &str = " | ";
const UNKNOWN_FIELD: &str = "?";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub run: FlowRun,
    pub children: Vec<FlowRun>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionRecord {
    pub timestamp: String,
    pub decision: String,
    pub message: String,
}

pub fn select(
    runs: Vec<FlowRun>,
    device: &str,
    peer: &str,
    minutes: u64,
    now: i64,
) -> Vec<FlowRun> {
    let window = i64::try_from(minutes)
        .unwrap_or(i64::MAX)
        .saturating_mul(60);
    let oldest = now.saturating_sub(window);
    runs.into_iter()
        .filter(|run| run.device.as_deref() == Some(device) && run.peer.as_deref() == Some(peer))
        .filter(|run| time::parse_rfc3339(&run.start_time).is_some_and(|start| start >= oldest))
        .collect()
}

pub fn group(runs: Vec<FlowRun>, task_runs: &[TaskRun]) -> Vec<Group> {
    let ids: Vec<String> = runs.iter().map(|run| run.id.clone()).collect();
    let mut groups = Vec::new();
    let mut children = Vec::new();
    for run in runs {
        match parent_among(&run, &ids, task_runs) {
            Some(parent) => children.push((parent, run)),
            None => groups.push(Group {
                run,
                children: Vec::new(),
            }),
        }
    }
    children.sort_by(|(_, first), (_, second)| first.start_time.cmp(&second.start_time));
    for group in &mut groups {
        group.children = children
            .iter()
            .filter(|(parent, _)| *parent == group.run.id)
            .map(|(_, child)| FlowRun {
                name: child
                    .name
                    .split_once(CHILD_NAME_SEPARATOR)
                    .map_or(child.name.as_str(), |(head, _)| head)
                    .to_string(),
                ..child.clone()
            })
            .collect();
    }
    groups
}

fn parent_among(run: &FlowRun, ids: &[String], task_runs: &[TaskRun]) -> Option<String> {
    let task = run.parent_task_run_id.as_deref()?;
    let flow = &task_runs
        .iter()
        .find(|candidate| candidate.id == task)?
        .flow_run_id;
    ids.contains(flow).then(|| flow.clone())
}

pub fn newer_than(runs: &[FlowRun], baseline: i64) -> Option<&FlowRun> {
    runs.iter()
        .find(|run| time::parse_rfc3339(&run.start_time).is_some_and(|start| start > baseline))
}

pub fn decision_record(line: &str) -> Option<DecisionRecord> {
    let record: Value = serde_json::from_str(line).ok()?;
    record.as_object()?;
    let text = |value: Option<&Value>, missing: &str| match value {
        None | Some(Value::Null) => missing.to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    };
    Some(DecisionRecord {
        timestamp: text(record.get("timestamp"), ""),
        decision: text(record.pointer("/labels/decision"), UNKNOWN_FIELD),
        message: text(record.get("message"), ""),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_793_960_400;
    const DECISION: &str = r#"{"timestamp":"2026-11-06T10:15:48.201Z","severity":"info","message":"SoT expects peer up, but metrics show mismatch","labels":{"decision":"proceed","device":"srl1","peer_address":"10.1.99.2"}}"#;

    fn run(
        id: &str,
        name: &str,
        start_time: &str,
        device: Option<&str>,
        parent_task_run_id: Option<&str>,
    ) -> FlowRun {
        FlowRun {
            id: id.to_string(),
            name: name.to_string(),
            device: device.map(str::to_string),
            peer: device.map(|_| "10.1.99.2".to_string()),
            start_time: start_time.to_string(),
            state: "COMPLETED".to_string(),
            parent_task_run_id: parent_task_run_id.map(str::to_string),
        }
    }

    fn task(id: &str, flow_run_id: &str) -> TaskRun {
        TaskRun {
            id: id.to_string(),
            flow_run_id: flow_run_id.to_string(),
        }
    }

    fn ids(runs: &[FlowRun]) -> Vec<&str> {
        runs.iter().map(|run| run.id.as_str()).collect()
    }

    fn quarantine() -> FlowRun {
        run(
            "q1",
            "quarantine_bgp | srl1:10.1.99.2",
            "2026-11-06T10:15:40.512345Z",
            Some("srl1"),
            Some("t0"),
        )
    }

    fn receiver() -> FlowRun {
        run(
            "r0",
            "alert_receiver | BgpSessionNotUp:firing",
            "2026-11-06T10:15:39Z",
            None,
            None,
        )
    }

    fn blocks_newest_first() -> Vec<FlowRun> {
        vec![
            run(
                "a1",
                "action | srl1:10.1.99.2",
                "2026-11-06T10:15:45Z",
                Some("srl1"),
                Some("t3"),
            ),
            run(
                "p1",
                "policy | srl1:10.1.99.2",
                "2026-11-06T10:15:44Z",
                Some("srl1"),
                Some("t2"),
            ),
            run(
                "e1",
                "evidence | srl1:10.1.99.2",
                "2026-11-06T10:15:41Z",
                Some("srl1"),
                Some("t1"),
            ),
        ]
    }

    fn packt_task_runs() -> Vec<TaskRun> {
        vec![
            task("t0", "r0"),
            task("t1", "q1"),
            task("t2", "q1"),
            task("t3", "q1"),
        ]
    }

    #[test]
    fn select_keeps_the_runs_of_the_device_and_peer_inside_the_window_in_order() {
        let other_peer = FlowRun {
            peer: Some("10.9.9.9".to_string()),
            ..quarantine()
        };
        let other_device = FlowRun {
            device: Some("srl2".to_string()),
            ..quarantine()
        };
        let older = run("old", "x", "2026-11-06T09:49:59Z", Some("srl1"), None);
        let at_the_edge = run("edge", "x", "2026-11-06T09:50:00Z", Some("srl1"), None);
        let unparsable = run("bad", "x", "soon", Some("srl1"), None);
        let not_started = run("new", "x", "", Some("srl1"), None);

        let kept = select(
            vec![
                receiver(),
                quarantine(),
                other_peer,
                other_device,
                older,
                unparsable,
                not_started,
                at_the_edge,
            ],
            "srl1",
            "10.1.99.2",
            30,
            NOW,
        );

        assert_eq!(ids(&kept), ["q1", "edge"]);
    }

    #[test]
    fn group_puts_the_blocks_under_their_parent_oldest_first_with_the_names_cut() {
        let mut runs = vec![quarantine()];
        runs.extend(blocks_newest_first());

        let groups = group(runs, &packt_task_runs());

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].run, quarantine());
        assert_eq!(ids(&groups[0].children), ["e1", "p1", "a1"]);
        let names: Vec<&str> = groups[0]
            .children
            .iter()
            .map(|child| child.name.as_str())
            .collect();
        assert_eq!(names, ["evidence", "policy", "action"]);
    }

    #[test]
    fn group_without_task_runs_leaves_every_run_top_level_in_the_order_given() {
        let mut runs = vec![quarantine()];
        runs.extend(blocks_newest_first());

        let groups = group(runs, &[]);

        let tops: Vec<&str> = groups.iter().map(|group| group.run.id.as_str()).collect();
        assert_eq!(tops, ["q1", "a1", "p1", "e1"]);
        assert!(groups.iter().all(|group| group.children.is_empty()));
        assert_eq!(groups[1].run.name, "action | srl1:10.1.99.2");
    }

    #[test]
    fn group_keeps_a_run_top_level_when_its_parent_flow_run_is_not_among_the_runs() {
        let groups = group(blocks_newest_first(), &packt_task_runs());

        let tops: Vec<&str> = groups.iter().map(|group| group.run.id.as_str()).collect();
        assert_eq!(tops, ["a1", "p1", "e1"]);
        assert!(groups.iter().all(|group| group.children.is_empty()));
    }

    #[test]
    fn newer_than_picks_the_first_run_after_the_baseline_and_none_when_there_is_none() {
        let runs = [
            run("n2", "x", "2026-11-06T10:20:05Z", Some("srl1"), None),
            run("n1", "x", "2026-11-06T10:20:01Z", Some("srl1"), None),
            run("same", "x", "2026-11-06T10:20:00Z", Some("srl1"), None),
            run("bad", "x", "soon", Some("srl1"), None),
        ];

        assert_eq!(
            newer_than(&runs, NOW).map(|found| found.id.as_str()),
            Some("n2")
        );
        assert_eq!(newer_than(&runs, NOW + 5), None);
        assert_eq!(newer_than(&[], NOW), None);
    }

    #[test]
    fn decision_record_reads_the_timestamp_the_decision_and_the_message() {
        assert_eq!(
            decision_record(DECISION),
            Some(DecisionRecord {
                timestamp: "2026-11-06T10:15:48.201Z".to_string(),
                decision: "proceed".to_string(),
                message: "SoT expects peer up, but metrics show mismatch".to_string(),
            })
        );
    }

    #[test]
    fn a_missing_or_null_field_reads_as_a_question_mark_for_the_decision_and_empty_otherwise() {
        for line in [
            "{}",
            r#"{"timestamp":null,"message":null,"labels":{"decision":null}}"#,
            r#"{"labels":"proceed"}"#,
        ] {
            assert_eq!(
                decision_record(line),
                Some(DecisionRecord {
                    timestamp: String::new(),
                    decision: "?".to_string(),
                    message: String::new(),
                }),
                "{line}"
            );
        }
    }

    #[test]
    fn a_line_that_is_not_a_json_object_is_not_a_record() {
        for line in [
            "",
            "not json",
            r#"{"message":"#,
            "[1]",
            r#""x""#,
            "123",
            "null",
        ] {
            assert_eq!(decision_record(line), None, "{line}");
        }
    }
}
