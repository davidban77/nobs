use std::time::{Duration, SystemTime};

use crate::loki::Loki;

const DEVICE: &str = "srl1";
const POLL: Duration = Duration::from_secs(5);
const WINDOW_MINUTES: u64 = 5;
const LINE_LIMIT: u64 = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    pub title: &'static str,
    pub body: &'static str,
    pub status: &'static str,
    pub device: &'static str,
    pub peer: &'static str,
    pub key: String,
    pub fingerprint: String,
    pub query: String,
    pub label: &'static str,
    pub maintenance: bool,
}

pub fn paths(workflow_label: &str) -> [Path; 4] {
    let path = |number: u8,
                title: &'static str,
                body: &'static str,
                status: &'static str,
                peer: &'static str,
                filters: &str,
                label: &'static str,
                maintenance: bool| {
        let fingerprint = format!("try-it-{number}");
        Path {
            title,
            body,
            status,
            device: DEVICE,
            peer,
            key: format!("try-it-{fingerprint}"),
            query: format!(
                r#"{{source="prefect", workflow="{workflow_label}", device="{DEVICE}"{filters}}}"#
            ),
            fingerprint,
            label,
            maintenance,
        }
    };
    [
        path(
            1,
            "Path 1 - Actionable / mismatch → proceed",
            "The lab ships srl1→10.1.99.2 and srl2→10.1.11.1 as intentionally broken.\nReplay a firing payload for one of those peers and expect decision='proceed'.",
            "firing",
            "10.1.99.2",
            r#", peer_address="10.1.99.2", decision="proceed""#,
            "quarantine flow decided 'proceed' for the actionable mismatch",
            false,
        ),
        path(
            2,
            "Path 2 - In-maintenance → skip",
            "Mark srl1 as in maintenance, replay an alert, expect decision='skip'.",
            "firing",
            "10.1.99.2",
            r#", decision="skip""#,
            "quarantine flow saw maintenance=true and skipped",
            true,
        ),
        path(
            3,
            "Path 3 - Healthy peer → skip",
            "Replay an alert payload for srl1→10.1.2.2 (a healthy peer). Decision should be 'skip'.",
            "firing",
            "10.1.2.2",
            r#", peer_address="10.1.2.2", decision="skip""#,
            "quarantine flow decided 'skip' for healthy peer",
            false,
        ),
        path(
            4,
            "Path 4 - Resolved → audit",
            "Replay a 'resolved' payload. The resolved_bgp_flow should annotate decision='resolved'.",
            "resolved",
            "10.1.99.2",
            r#", decision="resolved""#,
            "resolved_bgp_flow ran and annotated 'resolved'",
            false,
        ),
    ]
}

pub fn count(loki: &Loki, query: &str, now: SystemTime) -> u64 {
    loki.query_range(query, WINDOW_MINUTES, LINE_LIMIT, now)
        .map_or(0, |lines| lines.len() as u64)
}

pub fn wait(
    mut poll: impl FnMut() -> u64,
    timeout: Duration,
    mut sleep: impl FnMut(Duration),
) -> Option<u64> {
    let mut left = timeout;
    loop {
        let found = poll();
        if found > 0 {
            return Some(found);
        }
        if left.is_zero() {
            return None;
        }
        let pause = left.min(POLL);
        sleep(pause);
        left -= pause;
    }
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use super::*;
    use crate::http::stub::Stub;

    const TWO_LINES: &str = r#"{"status":"success","data":{"resultType":"streams","result":[{"stream":{},"values":[["1793960390000000000","a"],["1793960395000000000","b"]]}]}}"#;

    fn seconds(count: u64) -> Duration {
        Duration::from_secs(count)
    }

    #[test]
    fn the_four_paths_come_in_order_with_their_queries_keys_and_fingerprints() {
        let paths = paths("nobs_t_one_quarantine_bgp");

        let queries: Vec<&str> = paths.iter().map(|path| path.query.as_str()).collect();
        assert_eq!(
            queries,
            [
                r#"{source="prefect", workflow="nobs_t_one_quarantine_bgp", device="srl1", peer_address="10.1.99.2", decision="proceed"}"#,
                r#"{source="prefect", workflow="nobs_t_one_quarantine_bgp", device="srl1", decision="skip"}"#,
                r#"{source="prefect", workflow="nobs_t_one_quarantine_bgp", device="srl1", peer_address="10.1.2.2", decision="skip"}"#,
                r#"{source="prefect", workflow="nobs_t_one_quarantine_bgp", device="srl1", decision="resolved"}"#,
            ]
        );
        let maintenance: Vec<bool> = paths.iter().map(|path| path.maintenance).collect();
        assert_eq!(maintenance, [false, true, false, false]);
        let statuses: Vec<&str> = paths.iter().map(|path| path.status).collect();
        assert_eq!(statuses, ["firing", "firing", "firing", "resolved"]);
        let keys: Vec<&str> = paths.iter().map(|path| path.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "try-it-try-it-1",
                "try-it-try-it-2",
                "try-it-try-it-3",
                "try-it-try-it-4"
            ]
        );
        let fingerprints: Vec<&str> = paths.iter().map(|path| path.fingerprint.as_str()).collect();
        assert_eq!(
            fingerprints,
            ["try-it-1", "try-it-2", "try-it-3", "try-it-4"]
        );
    }

    #[test]
    fn wait_gives_the_first_count_above_zero_after_a_sleep_of_five_seconds_per_empty_poll() {
        let mut answers = [0, 0, 3].into_iter();
        let mut sleeps = Vec::new();

        let found = wait(
            || answers.next().unwrap(),
            seconds(60),
            |pause| sleeps.push(pause),
        );

        assert_eq!(found, Some(3));
        assert_eq!(sleeps, [seconds(5), seconds(5)]);
    }

    #[test]
    fn wait_sleeps_only_the_time_left_and_polls_once_more_before_giving_up() {
        let mut polls = 0;
        let mut sleeps = Vec::new();

        let found = wait(
            || {
                polls += 1;
                0
            },
            seconds(12),
            |pause| sleeps.push(pause),
        );

        assert_eq!(found, None);
        assert_eq!(sleeps, [seconds(5), seconds(5), seconds(2)]);
        assert_eq!(polls, 4);
    }

    #[test]
    fn wait_with_no_time_polls_once_and_never_sleeps() {
        let mut polls = 0;
        let mut sleeps = Vec::new();

        let found = wait(
            || {
                polls += 1;
                0
            },
            Duration::ZERO,
            |pause| sleeps.push(pause),
        );

        assert_eq!(found, None);
        assert_eq!(polls, 1);
        assert!(sleeps.is_empty());
    }

    #[test]
    fn count_is_the_number_of_lines_and_zero_when_loki_cannot_answer() {
        let route = "/loki/api/v1/query_range*";
        let stub = Stub::start(&[("GET", route, 200, TWO_LINES), ("GET", route, 500, "boom")]);
        let loki = Loki::new(stub.url());
        let now = UNIX_EPOCH + seconds(1_793_960_400);

        assert_eq!(count(&loki, "q", now), 2);
        assert_eq!(count(&loki, "q", now), 0);
    }
}
