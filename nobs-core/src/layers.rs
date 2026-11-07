use std::fmt::Display;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};

use crate::http::Reply;
use crate::loki::Loki;
use crate::prometheus::Prometheus;
use crate::time::rfc3339_utc;

const POLL: Duration = Duration::from_secs(4);
const CONVERGENCE_LIMIT: Duration = Duration::from_secs(180);
const EVENTS_LIMIT: Duration = Duration::from_secs(30);
const WINDOW_FILL_SECONDS: u64 = 130;
const FILL_STEP_SECONDS: u64 = 20;
const SERIES_AT_LEAST: i64 = 6;
const SERIES_WINDOW_SECONDS: u64 = 300;
const EVENT_WINDOW_MINUTES: u64 = 5;
const EVENT_LINE_LIMIT: u64 = 200;
const BGP_QUERY: &str = r#"count(bgp_oper_state{pipeline=~".+"})"#;
const INTERFACE_QUERY: &str = r#"count(interface_oper_state{intf_role="peer", pipeline=~".+"})"#;
const UPDOWN_QUERY: &str = r#"{vendor_facility_process="UPDOWN"}"#;
const FLAP_QUERY: &str =
    r#"{device="srl1", vendor_facility_process="UPDOWN"} |~ "Interface .* changed state""#;
const CONFIG_PUSH_QUERY: &str =
    r#"{device="srl1", source="workshop-trigger", event="config-push"}"#;

const DEVICES: [&str; 2] = ["srl1", "srl2"];
const NO_TEMPLATE: &str = "(no template)";
const NO_TEMPLATE_DEVICE: &str = "srl1";
const QUERY_WINDOW_MILLISECONDS: u128 = 300_000;
const INTERVAL_MILLISECONDS: u64 = 30_000;
const MAX_DATA_POINTS: u64 = 600;
const TITLE_CUT: usize = 38;
const TITLE_WIDTH: usize = 40;
const SUMMARY_CUT: usize = 140;
const EXPRESSION_CUT: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Wait {
    pub label: String,
    pub ok: bool,
    pub elapsed_s: f64,
    pub detail: String,
    pub expected: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Target {
    pub ok: bool,
    pub ds: String,
    pub summary: String,
    pub expr: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PanelResult {
    pub panel_id: Value,
    pub panel_title: String,
    pub panel_type: String,
    pub device: String,
    pub status: String,
    pub summary: String,
    pub targets: Vec<Target>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Dashboard {
    pub dashboard: String,
    pub file: String,
    pub panels: Vec<PanelResult>,
}

pub fn wait(
    label: &str,
    expected: &str,
    limit: Duration,
    mut predicate: impl FnMut() -> Result<(bool, String), String>,
    mut sleep: impl FnMut(Duration),
    mut now: impl FnMut() -> Instant,
    mut say: impl FnMut(String),
) -> Wait {
    let start = now();
    let mut last_detail = String::new();
    let mut last_error = String::new();
    loop {
        match predicate() {
            Ok((true, detail)) => {
                let elapsed = now().duration_since(start);
                return finish(label, expected, true, elapsed, detail, &mut say);
            }
            Ok((false, detail)) => last_detail = detail,
            Err(error) => last_error = error,
        }
        let elapsed = now().duration_since(start);
        if elapsed >= limit {
            let detail = [last_detail, last_error]
                .into_iter()
                .find(|text| !text.is_empty())
                .unwrap_or_else(|| "no data".to_string());
            return finish(label, expected, false, elapsed, detail, &mut say);
        }
        sleep(POLL.min(limit - elapsed));
    }
}

fn finish(
    label: &str,
    expected: &str,
    ok: bool,
    elapsed: Duration,
    detail: String,
    say: &mut impl FnMut(String),
) -> Wait {
    let seconds = elapsed.as_secs_f64();
    let verdict = if ok { "OK  " } else { "FAIL" };
    say(format!("  [{seconds:5.1}s] {verdict} {label} — {detail}"));
    Wait {
        label: label.to_string(),
        ok,
        elapsed_s: (seconds * 10.0).round() / 10.0,
        detail,
        expected: expected.to_string(),
    }
}

fn series_count(prometheus: &Prometheus, query: &str) -> Result<(bool, String), String> {
    let count = prometheus
        .instant(query)
        .map_err(|error| error.to_string())?
        .map_or(0, |value| value as i64);
    Ok((count >= SERIES_AT_LEAST, format!("count = {count}")))
}

fn pipelines(loki: &Loki, now: SystemTime) -> Result<(bool, String), String> {
    let series = loki
        .series(UPDOWN_QUERY, SERIES_WINDOW_SECONDS, now)
        .map_err(|error| error.to_string())?;
    let mut names: Vec<&str> = series
        .iter()
        .filter_map(|labels| labels.get("pipeline"))
        .map(String::as_str)
        .filter(|name| !name.is_empty())
        .collect();
    names.sort_unstable();
    names.dedup();
    Ok((
        names.contains(&"direct") && names.contains(&"vector"),
        format!("pipelines = {}", names.join(", ")),
    ))
}

fn event_count(
    loki: &Loki,
    query: &str,
    at_least: usize,
    name: &str,
    now: SystemTime,
) -> Result<(bool, String), String> {
    let count = loki
        .query_range(query, EVENT_WINDOW_MINUTES, EVENT_LINE_LIMIT, now)
        .map_err(|error| error.to_string())?
        .len();
    Ok((count >= at_least, format!("{name} = {count}")))
}

pub struct Steps<F, M> {
    pub flap: F,
    pub maintenance: M,
}

pub struct Timing<C, S, N> {
    pub clock: C,
    pub sleep: S,
    pub now: N,
}

pub fn layer_a<F, M, C, S, N>(
    prometheus: &Prometheus,
    loki: &Loki,
    steps: Steps<F, M>,
    timing: Timing<C, S, N>,
    mut say: impl FnMut(String),
) -> (Vec<Wait>, i32)
where
    F: FnOnce() -> i32,
    M: FnOnce() -> i32,
    C: FnMut() -> SystemTime,
    S: FnMut(Duration),
    N: FnMut() -> Instant,
{
    let Steps { flap, maintenance } = steps;
    let Timing {
        mut clock,
        mut sleep,
        mut now,
    } = timing;
    say("Layer A — wait for data shape".to_string());
    let mut waits = vec![
        wait(
            "bgp_oper_state pipeline convergence",
            ">= 6 series",
            CONVERGENCE_LIMIT,
            || series_count(prometheus, BGP_QUERY),
            &mut sleep,
            &mut now,
            &mut say,
        ),
        wait(
            "interface_oper_state intf_role=peer convergence",
            ">= 6 series",
            CONVERGENCE_LIMIT,
            || series_count(prometheus, INTERFACE_QUERY),
            &mut sleep,
            &mut now,
            &mut say,
        ),
    ];
    waits.push(wait(
        "logs pipeline convergence (UPDOWN)",
        "{direct, vector}",
        CONVERGENCE_LIMIT,
        || pipelines(loki, clock()),
        &mut sleep,
        &mut now,
        &mut say,
    ));

    say("Layer A — flap-interface (in-process, --no-cascade)".to_string());
    let flap_code = flap();
    say("Layer A — maintenance toggle (in-process)".to_string());
    let maintenance_code = maintenance();

    say(format!(
        "Layer A — sleeping {WINDOW_FILL_SECONDS}s for [2m] windows to fill"
    ));
    let mut left = WINDOW_FILL_SECONDS;
    while left > 0 {
        say(format!("  ... {left}s left"));
        sleep(Duration::from_secs(left.min(FILL_STEP_SECONDS)));
        left = left.saturating_sub(FILL_STEP_SECONDS);
    }

    let mut event_wait = |label: &str, expected: &str, query: &str, at_least: usize, name: &str| {
        wait(
            label,
            expected,
            EVENTS_LIMIT,
            || event_count(loki, query, at_least, name, clock()),
            &mut sleep,
            &mut now,
            &mut say,
        )
    };
    waits.push(event_wait(
        "Loki has UPDOWN events from both producers",
        ">= 8 events",
        UPDOWN_QUERY,
        8,
        "events last 5m",
    ));
    waits.push(event_wait(
        "Interface Flap annotation source has lines",
        ">= 4 lines",
        FLAP_QUERY,
        4,
        "flap lines",
    ));
    waits.push(event_wait(
        "Device Config Push annotation source has lines",
        ">= 1 line",
        CONFIG_PUSH_QUERY,
        1,
        "config-push lines",
    ));

    let failed = waits.iter().any(|wait| !wait.ok) || flap_code != 0 || maintenance_code != 0;
    (waits, i32::from(failed))
}

pub fn panels(dashboard: &Value) -> Vec<Value> {
    let mut found = Vec::new();
    collect_panels(dashboard.get("panels"), &mut found);
    found
}

fn collect_panels(list: Option<&Value>, found: &mut Vec<Value>) {
    for panel in list.and_then(Value::as_array).into_iter().flatten() {
        if panel["type"] == "row" {
            collect_panels(panel.get("panels"), found);
        } else {
            found.push(panel.clone());
        }
    }
}

pub fn render(target: &Value, device: &str) -> Value {
    match target {
        Value::String(text) => {
            Value::String(text.replace("${device}", device).replace("$device", device))
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| render(item, device)).collect())
        }
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), render(value, device)))
                .collect(),
        ),
        other => other.clone(),
    }
}

pub fn judge(status: u16, body: &str) -> (bool, String) {
    if status != 200 {
        return (false, format!("http {status}"));
    }
    let answer: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let Some(result) = answer
        .get("results")
        .and_then(Value::as_object)
        .and_then(|results| results.values().next())
    else {
        return (false, "no results".to_string());
    };
    let result_status = result.get("status").and_then(Value::as_u64).unwrap_or(200);
    let error = result
        .get("error")
        .and_then(Value::as_str)
        .filter(|error| !error.is_empty());
    if result_status >= 400 || error.is_some() {
        let reason = error.map_or_else(|| result_status.to_string(), str::to_string);
        return (false, format!("ds error: {reason}"));
    }
    let frames = result
        .get("frames")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if frames.is_empty() {
        return (false, "0 frames".to_string());
    }
    let columns: Vec<&Vec<Value>> = frames
        .iter()
        .filter_map(|frame| frame.pointer("/data/values").and_then(Value::as_array))
        .filter(|columns| !columns.is_empty())
        .collect();
    let rows: usize = columns
        .iter()
        .filter_map(|columns| columns[0].as_array())
        .map(Vec::len)
        .sum();
    (rows > 0, format!("{} frames, {rows} rows", columns.len()))
}

pub fn dashboards(folder: &Path) -> Result<Vec<(String, Value)>, String> {
    let unreadable =
        |path: &Path, error: &dyn Display| format!("cannot read {}: {error}", path.display());
    let mut files = Vec::new();
    for entry in fs::read_dir(folder).map_err(|error| unreadable(folder, &error))? {
        let path = entry.map_err(|error| unreadable(folder, &error))?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            files.push(path);
        }
    }
    files.sort();
    let mut found = Vec::new();
    for path in files {
        let text = fs::read_to_string(&path).map_err(|error| unreadable(&path, &error))?;
        let parsed = serde_json::from_str(&text).map_err(|error| unreadable(&path, &error))?;
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        found.push((name.into_owned(), parsed));
    }
    if found.is_empty() {
        return Err(format!("no dashboard files in {}", folder.display()));
    }
    Ok(found)
}

pub fn layer_b(
    dashboards: &[(String, Value)],
    mut query: impl FnMut(&Value) -> Result<Reply, String>,
    mut clock: impl FnMut() -> SystemTime,
    mut say: impl FnMut(String),
) -> (Vec<Dashboard>, i32) {
    let (mut passed, mut failed, mut skipped) = (0, 0, 0);
    let mut results = Vec::new();
    for (file, dashboard) in dashboards {
        let title = dashboard
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                let stem = Path::new(file).file_stem().unwrap_or_default();
                stem.to_string_lossy().into_owned()
            });
        let found = panels(dashboard);
        say(String::new());
        say(format!("=== {title} ({file}) — {} panels ===", found.len()));
        let mut panel_results = Vec::new();
        for panel in &found {
            let id = panel.get("id").cloned().unwrap_or(Value::Null);
            let panel_title = panel.get("title").and_then(Value::as_str).unwrap_or("");
            let panel_type = panel.get("type").and_then(Value::as_str).unwrap_or("");
            let targets = panel
                .get("targets")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let templated = targets.iter().any(|target| {
                let text = target.to_string();
                text.contains("$device") || text.contains("${device}")
            });
            let devices: &[&str] = if templated { &DEVICES } else { &[NO_TEMPLATE] };
            for &device in devices {
                let rendered_for = if device == NO_TEMPLATE {
                    NO_TEMPLATE_DEVICE
                } else {
                    device
                };
                let target_results: Vec<Target> = targets
                    .iter()
                    .map(|target| {
                        let rendered = render(target, rendered_for);
                        let (ok, summary) = ask(&rendered, &mut query, &mut clock);
                        Target {
                            ok,
                            ds: datasource_type(panel, &rendered),
                            summary,
                            expr: expression(&rendered),
                        }
                    })
                    .collect();
                let (status, mark) = if target_results.is_empty() {
                    skipped += 1;
                    ("SKIP", "SKIP")
                } else if target_results.iter().any(|target| target.ok) {
                    passed += 1;
                    ("PASS", "OK  ")
                } else {
                    failed += 1;
                    ("FAIL", "FAIL")
                };
                let summary = if target_results.is_empty() {
                    "no targets".to_string()
                } else {
                    target_results
                        .iter()
                        .map(|target| format!("{}:{}", target.ds, target.summary))
                        .collect::<Vec<_>>()
                        .join("; ")
                };
                say(format!(
                    "  [{mark}] panel #{} {panel_type:14} device={device:12} {:TITLE_WIDTH$} → {}",
                    panel_number(&id),
                    quote(&cut(panel_title, TITLE_CUT)),
                    cut(&summary, SUMMARY_CUT),
                ));
                panel_results.push(PanelResult {
                    panel_id: id.clone(),
                    panel_title: panel_title.to_string(),
                    panel_type: panel_type.to_string(),
                    device: device.to_string(),
                    status: status.to_string(),
                    summary,
                    targets: target_results,
                });
            }
        }
        results.push(Dashboard {
            dashboard: title,
            file: file.clone(),
            panels: panel_results,
        });
    }
    say(String::new());
    say(format!(
        "Layer B summary: PASS={passed} FAIL={failed} SKIP={skipped}"
    ));
    (results, i32::from(failed > 0))
}

fn ask(
    target: &Value,
    query: &mut impl FnMut(&Value) -> Result<Reply, String>,
    clock: &mut impl FnMut() -> SystemTime,
) -> (bool, String) {
    let to = clock()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let from = to.saturating_sub(QUERY_WINDOW_MILLISECONDS);
    let mut sent = target.clone();
    if let Some(fields) = sent.as_object_mut() {
        fields.entry("refId").or_insert_with(|| json!("A"));
        fields.insert("intervalMs".to_string(), json!(INTERVAL_MILLISECONDS));
        fields.insert("maxDataPoints".to_string(), json!(MAX_DATA_POINTS));
    }
    let body = json!({
        "queries": [sent],
        "from": from.to_string(),
        "to": to.to_string(),
    });
    match query(&body) {
        Ok(reply) => judge(reply.status, &reply.body),
        Err(text) => (false, text),
    }
}

fn datasource_type(panel: &Value, target: &Value) -> String {
    let datasource = [target, panel]
        .into_iter()
        .filter_map(|source| source.get("datasource"))
        .find(|datasource| match datasource {
            Value::Null => false,
            Value::String(name) => !name.is_empty(),
            Value::Object(fields) => !fields.is_empty(),
            _ => true,
        });
    match datasource {
        Some(Value::String(name)) => name.to_lowercase(),
        Some(other) => other
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase(),
        None => String::new(),
    }
}

fn expression(target: &Value) -> String {
    let text = ["expr", "queryText"]
        .into_iter()
        .filter_map(|key| target.get(key).and_then(Value::as_str))
        .find(|text| !text.is_empty())
        .unwrap_or("");
    cut(text, EXPRESSION_CUT)
}

fn panel_number(id: &Value) -> String {
    match id {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn cut(text: &str, length: usize) -> String {
    text.chars().take(length).collect()
}

fn quote(text: &str) -> String {
    let mark = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut quoted = String::from(mark);
    for character in text.chars() {
        if character == '\\' || character == mark {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push(mark);
    quoted
}

pub fn report(
    title: &str,
    when: &str,
    waits: &[Wait],
    dashboards: &[Dashboard],
    a: i32,
    b: i32,
) -> String {
    let mut parts = vec![
        format!("# {title} preflight — {when}\n"),
        "## Summary\n".to_string(),
        format!("- Layer A (data shape): exit={a}"),
        format!("- Layer B (per-panel /api/ds/query): exit={b}"),
        String::new(),
        "## Layer A — data shape waits\n".to_string(),
    ];
    for wait in waits {
        let verdict = if wait.ok { "PASS" } else { "FAIL" };
        parts.push(format!(
            "- **{verdict}** `{}` — {} (elapsed {:.1}s)",
            wait.label, wait.detail, wait.elapsed_s
        ));
    }
    parts.push(String::new());
    parts.push("## Layer B — per-panel /api/ds/query\n".to_string());
    for dashboard in dashboards {
        parts.push(format!(
            "### {} ({})\n",
            dashboard.dashboard, dashboard.file
        ));
        parts.push("| panel | type | device | status | summary |".to_string());
        parts.push("|---|---|---|---|---|".to_string());
        for panel in &dashboard.panels {
            parts.push(format!(
                "| #{} {} | {} | {} | **{}** | {} |",
                panel_number(&panel.panel_id),
                panel.panel_title,
                panel.panel_type,
                panel.device,
                panel.status,
                panel.summary
            ));
        }
        parts.push(String::new());
    }
    parts.join("\n")
}

pub fn waits_json(waits: &[Wait]) -> String {
    serde_json::to_string_pretty(waits).unwrap_or_default()
}

pub fn dashboards_json(dashboards: &[Dashboard]) -> String {
    serde_json::to_string_pretty(dashboards).unwrap_or_default()
}

pub fn stamp(time: SystemTime) -> String {
    let seconds = time.duration_since(UNIX_EPOCH).map_or(0, |elapsed| {
        i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
    });
    rfc3339_utc(seconds).trim_end_matches('Z').replace('T', " ")
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tail {
    pub layer_c: String,
    pub summary: String,
    pub report: String,
    pub code: i32,
}

pub fn tail(a: i32, b: i32, seconds: f64, report_file: &Path) -> Tail {
    let code = a.max(b);
    let verdict = if code == 0 {
        "preflight green"
    } else {
        "preflight failures"
    };
    Tail {
        layer_c: "Layer C — per-panel screenshots: not built in nobs, skipped".to_string(),
        summary: format!(
            "{verdict} ({seconds:.1}s) — Layer A exit={a} | Layer B exit={b} | Layer C skipped"
        ),
        report: format!("report: {}", report_file.display()),
        code,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;

    use super::*;
    use crate::http::stub::Stub;

    fn seconds(count: u64) -> Duration {
        Duration::from_secs(count)
    }

    struct Clock {
        start: Instant,
        elapsed: Cell<Duration>,
    }

    impl Clock {
        fn new() -> Self {
            Self {
                start: Instant::now(),
                elapsed: Cell::new(Duration::ZERO),
            }
        }

        fn now(&self) -> Instant {
            self.start + self.elapsed.get()
        }

        fn advance(&self, by: Duration) {
            self.elapsed.set(self.elapsed.get() + by);
        }
    }

    struct Waited {
        outcome: Wait,
        sleeps: Vec<Duration>,
        lines: Vec<String>,
        polls: usize,
    }

    fn run_wait(
        limit: u64,
        mut predicate: impl FnMut(usize) -> Result<(bool, String), String>,
    ) -> Waited {
        let clock = Clock::new();
        let mut polls = 0;
        let mut sleeps = Vec::new();
        let mut lines = Vec::new();
        let outcome = wait(
            "the label",
            "the expected",
            seconds(limit),
            || {
                polls += 1;
                predicate(polls)
            },
            |pause| {
                sleeps.push(pause);
                clock.advance(pause);
            },
            || clock.now(),
            |line| lines.push(line),
        );
        Waited {
            outcome,
            sleeps,
            lines,
            polls,
        }
    }

    fn held(detail: &str) -> Result<(bool, String), String> {
        Ok((true, detail.to_string()))
    }

    fn not_held(detail: &str) -> Result<(bool, String), String> {
        Ok((false, detail.to_string()))
    }

    #[test]
    fn wait_holds_at_the_first_poll_with_no_sleep_and_prints_the_ok_line() {
        let run = run_wait(180, |_| held("count = 6"));

        assert_eq!(
            run.outcome,
            Wait {
                label: "the label".to_string(),
                ok: true,
                elapsed_s: 0.0,
                detail: "count = 6".to_string(),
                expected: "the expected".to_string(),
            }
        );
        assert!(run.sleeps.is_empty());
        assert_eq!(run.lines, ["  [  0.0s] OK   the label — count = 6"]);
    }

    #[test]
    fn wait_holds_at_the_third_poll_after_two_four_second_sleeps_and_retries_an_error() {
        let run = run_wait(180, |poll| match poll {
            1 => Err("boom".to_string()),
            2 => not_held("count = 3"),
            _ => held("count = 6"),
        });

        assert_eq!(run.polls, 3);
        assert_eq!(run.sleeps, [seconds(4), seconds(4)]);
        assert!(run.outcome.ok);
        assert_eq!(run.outcome.elapsed_s, 8.0);
        assert_eq!(run.lines, ["  [  8.0s] OK   the label — count = 6"]);
    }

    #[test]
    fn wait_polls_at_the_limit_and_cuts_the_last_sleep_to_the_time_left() {
        let run = run_wait(10, |_| not_held("count = 2"));

        assert_eq!(run.polls, 4);
        assert_eq!(run.sleeps, [seconds(4), seconds(4), seconds(2)]);
        assert!(!run.outcome.ok);
        assert_eq!(run.outcome.elapsed_s, 10.0);
        assert_eq!(run.lines, ["  [ 10.0s] FAIL the label — count = 2"]);
    }

    #[test]
    fn wait_that_fails_says_the_last_detail_even_when_an_error_came_after_it() {
        let run = run_wait(8, |poll| match poll {
            1 => not_held("count = 1"),
            2 => not_held("count = 2"),
            _ => Err("boom".to_string()),
        });

        assert_eq!(run.outcome.detail, "count = 2");
    }

    #[test]
    fn wait_that_fails_says_the_last_error_when_no_detail_was_seen() {
        let run = run_wait(8, |poll| Err(format!("error {poll}")));

        assert!(!run.outcome.ok);
        assert_eq!(run.outcome.detail, "error 3");
        assert_eq!(run.lines, ["  [  8.0s] FAIL the label — error 3"]);
    }

    #[test]
    fn wait_that_fails_says_no_data_when_there_was_neither_detail_nor_error() {
        let run = run_wait(8, |_| not_held(""));

        assert_eq!(run.outcome.detail, "no data");
        assert_eq!(run.lines, ["  [  8.0s] FAIL the label — no data"]);
    }

    const PROMETHEUS_SIX: &str = r#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{},"value":[1793960400,"6"]}]}}"#;
    const BOTH_PIPELINES: &str = r#"{"status":"success","data":[{"pipeline":"vector"},{"pipeline":"direct"},{"pipeline":"vector"}]}"#;

    fn lines_reply(count: usize) -> String {
        let pairs: Vec<String> = (0..count)
            .map(|index| {
                format!(
                    r#"["{}","line"]"#,
                    1_793_960_000_000_000_000_u64 + index as u64
                )
            })
            .collect();
        format!(
            r#"{{"status":"success","data":{{"resultType":"streams","result":[{{"stream":{{}},"values":[{}]}}]}}}}"#,
            pairs.join(",")
        )
    }

    struct LayerA {
        waits: Vec<Wait>,
        code: i32,
        lines: Vec<String>,
        sleeps: Vec<Duration>,
        loki_paths: Vec<String>,
        prometheus_paths: Vec<String>,
    }

    fn run_layer_a(counts: &[usize], flap_code: i32, maintenance_code: i32) -> LayerA {
        let prometheus_stub = Stub::start(&[("GET", "/api/v1/query*", 200, PROMETHEUS_SIX)]);
        let replies: Vec<String> = counts.iter().map(|count| lines_reply(*count)).collect();
        let mut routes = vec![("GET", "/loki/api/v1/series*", 200, BOTH_PIPELINES)];
        routes.extend(
            replies
                .iter()
                .map(|reply| ("GET", "/loki/api/v1/query_range*", 200, reply.as_str())),
        );
        let loki_stub = Stub::start(&routes);
        let clock = Clock::new();
        let lines = RefCell::new(Vec::new());
        let sleeps = RefCell::new(Vec::new());

        let (waits, code) = layer_a(
            &Prometheus::new(prometheus_stub.url()),
            &Loki::new(loki_stub.url()),
            Steps {
                flap: || {
                    lines.borrow_mut().push("<flap called>".to_string());
                    flap_code
                },
                maintenance: || {
                    lines.borrow_mut().push("<maintenance called>".to_string());
                    maintenance_code
                },
            },
            Timing {
                clock: || UNIX_EPOCH + seconds(1_793_960_400),
                sleep: |pause| {
                    sleeps.borrow_mut().push(pause);
                    clock.advance(pause);
                },
                now: || clock.now(),
            },
            |line| lines.borrow_mut().push(line),
        );

        LayerA {
            waits,
            code,
            lines: lines.into_inner(),
            sleeps: sleeps.into_inner(),
            loki_paths: loki_stub
                .requests()
                .into_iter()
                .map(|request| request.path)
                .collect(),
            prometheus_paths: prometheus_stub
                .requests()
                .into_iter()
                .map(|request| request.path)
                .collect(),
        }
    }

    fn fill_sleeps() -> Vec<Duration> {
        let mut sleeps = vec![seconds(20); 6];
        sleeps.push(seconds(10));
        sleeps
    }

    #[test]
    fn layer_a_runs_the_six_waits_in_order_with_the_two_steps_between_the_third_and_fourth() {
        let run = run_layer_a(&[8, 4, 1], 0, 0);

        let labels: Vec<&str> = run.waits.iter().map(|wait| wait.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "bgp_oper_state pipeline convergence",
                "interface_oper_state intf_role=peer convergence",
                "logs pipeline convergence (UPDOWN)",
                "Loki has UPDOWN events from both producers",
                "Interface Flap annotation source has lines",
                "Device Config Push annotation source has lines",
            ]
        );
        let expected: Vec<&str> = run
            .waits
            .iter()
            .map(|wait| wait.expected.as_str())
            .collect();
        assert_eq!(
            expected,
            [
                ">= 6 series",
                ">= 6 series",
                "{direct, vector}",
                ">= 8 events",
                ">= 4 lines",
                ">= 1 line"
            ]
        );
        assert!(run
            .waits
            .iter()
            .all(|wait| wait.ok && wait.elapsed_s == 0.0));
        assert_eq!(
            run.lines,
            [
                "Layer A — wait for data shape",
                "  [  0.0s] OK   bgp_oper_state pipeline convergence — count = 6",
                "  [  0.0s] OK   interface_oper_state intf_role=peer convergence — count = 6",
                "  [  0.0s] OK   logs pipeline convergence (UPDOWN) — pipelines = direct, vector",
                "Layer A — flap-interface (in-process, --no-cascade)",
                "<flap called>",
                "Layer A — maintenance toggle (in-process)",
                "<maintenance called>",
                "Layer A — sleeping 130s for [2m] windows to fill",
                "  ... 130s left",
                "  ... 110s left",
                "  ... 90s left",
                "  ... 70s left",
                "  ... 50s left",
                "  ... 30s left",
                "  ... 10s left",
                "  [  0.0s] OK   Loki has UPDOWN events from both producers — events last 5m = 8",
                "  [  0.0s] OK   Interface Flap annotation source has lines — flap lines = 4",
                "  [  0.0s] OK   Device Config Push annotation source has lines — config-push lines = 1",
            ]
        );
    }

    #[test]
    fn layer_a_asks_prometheus_and_loki_with_the_queries_and_windows_of_each_wait() {
        let run = run_layer_a(&[8, 4, 1], 0, 0);

        assert_eq!(
            run.prometheus_paths,
            [
                "/api/v1/query?query=count(bgp_oper_state%7Bpipeline%3D~%22.%2B%22%7D)",
                "/api/v1/query?query=count(interface_oper_state%7Bintf_role%3D%22peer%22%2C%20pipeline%3D~%22.%2B%22%7D)",
            ]
        );
        assert_eq!(
            run.loki_paths,
            [
                "/loki/api/v1/series?match%5B%5D=%7Bvendor_facility_process%3D%22UPDOWN%22%7D&start=1793960100000000000&end=1793960400000000000",
                "/loki/api/v1/query_range?query=%7Bvendor_facility_process%3D%22UPDOWN%22%7D&start=1793960100000000000&end=1793960400000000000&limit=200&direction=BACKWARD",
                "/loki/api/v1/query_range?query=%7Bdevice%3D%22srl1%22%2C%20vendor_facility_process%3D%22UPDOWN%22%7D%20%7C~%20%22Interface%20.*%20changed%20state%22&start=1793960100000000000&end=1793960400000000000&limit=200&direction=BACKWARD",
                "/loki/api/v1/query_range?query=%7Bdevice%3D%22srl1%22%2C%20source%3D%22workshop-trigger%22%2C%20event%3D%22config-push%22%7D&start=1793960100000000000&end=1793960400000000000&limit=200&direction=BACKWARD",
            ]
        );
    }

    #[test]
    fn layer_a_fills_the_windows_with_six_twenty_second_sleeps_and_one_of_ten() {
        let run = run_layer_a(&[8, 4, 1], 0, 0);

        let mut expected = vec![seconds(20); 6];
        expected.push(seconds(10));
        assert_eq!(run.sleeps, expected);
        assert_eq!(run.sleeps.iter().sum::<Duration>(), seconds(130));
    }

    #[test]
    fn layer_a_is_zero_when_every_wait_held_and_both_steps_returned_zero() {
        assert_eq!(run_layer_a(&[8, 4, 1], 0, 0).code, 0);
    }

    #[test]
    fn layer_a_is_one_when_a_wait_failed_and_goes_on_to_the_waits_after_it() {
        let mut counts = vec![3; 9];
        counts.extend([4, 1]);

        let run = run_layer_a(&counts, 0, 0);

        let held: Vec<bool> = run.waits.iter().map(|wait| wait.ok).collect();
        assert_eq!(held, [true, true, true, false, true, true]);
        assert_eq!(run.waits[3].detail, "events last 5m = 3");
        assert_eq!(run.waits[3].elapsed_s, 30.0);
        assert_eq!(run.code, 1);
        assert!(run.lines.last().unwrap().contains("config-push lines = 1"));
    }

    #[test]
    fn layer_a_is_one_when_every_wait_held_but_flap_returned_non_zero() {
        let run = run_layer_a(&[8, 4, 1], 1, 0);

        assert_eq!(run.waits.len(), 6);
        assert!(run.waits.iter().all(|wait| wait.ok));
        assert_eq!(run.sleeps, fill_sleeps());
        assert!(run.lines.iter().any(|line| line == "<maintenance called>"));
        assert_eq!(run.code, 1);
    }

    #[test]
    fn layer_a_is_one_when_every_wait_held_but_maintenance_returned_non_zero() {
        let run = run_layer_a(&[8, 4, 1], 0, 2);

        assert_eq!(run.waits.len(), 6);
        assert!(run.waits.iter().all(|wait| wait.ok));
        assert_eq!(run.sleeps, fill_sleeps());
        assert_eq!(run.code, 1);
    }

    #[test]
    fn panels_flattens_a_row_into_the_place_where_the_row_stood() {
        let dashboard = json!({"panels": [
            {"id": 1, "type": "stat"},
            {"id": 2, "type": "row", "panels": [{"id": 3, "type": "table"}]},
            {"id": 4, "type": "text"},
        ]});

        let ids: Vec<i64> = panels(&dashboard)
            .iter()
            .map(|panel| panel["id"].as_i64().unwrap())
            .collect();

        assert_eq!(ids, [1, 3, 4]);
    }

    #[test]
    fn render_replaces_both_spellings_in_nested_strings_and_keeps_the_rest() {
        let target = json!({
            "expr": "up{device=\"$device\"}",
            "options": {"body": "{ d(name: \"${device}\") }", "list": ["$device", {"q": "${device}"}]},
            "count": 3,
            "flag": true,
            "$device": "key stays",
        });

        assert_eq!(
            render(&target, "srl2"),
            json!({
                "expr": "up{device=\"srl2\"}",
                "options": {"body": "{ d(name: \"srl2\") }", "list": ["srl2", {"q": "srl2"}]},
                "count": 3,
                "flag": true,
                "$device": "key stays",
            })
        );
    }

    #[test]
    fn judge_gives_the_text_of_each_shape_the_reply_can_have() {
        let cases = [
            (500, "{}", false, "http 500"),
            (200, r#"{"message":"hi"}"#, false, "no results"),
            (200, r#"{"results":{}}"#, false, "no results"),
            (200, "not json", false, "no results"),
            (
                200,
                r#"{"results":{"A":{"status":500,"error":"boom"}}}"#,
                false,
                "ds error: boom",
            ),
            (
                200,
                r#"{"results":{"A":{"status":400}}}"#,
                false,
                "ds error: 400",
            ),
            (200, r#"{"results":{"A":{"frames":[]}}}"#, false, "0 frames"),
            (200, r#"{"results":{"A":{}}}"#, false, "0 frames"),
            (
                200,
                r#"{"results":{"A":{"frames":[{"data":{"values":[[]]}}]}}}"#,
                false,
                "1 frames, 0 rows",
            ),
            (
                200,
                r#"{"results":{"A":{"frames":[
                    {"data":{"values":[[1,2,3],["a","b","c"]]}},
                    {"data":{"values":[[1,2,3,4]]}},
                    {"data":{"values":[]}}
                ]}}}"#,
                true,
                "2 frames, 7 rows",
            ),
        ];

        for (status, body, ok, summary) in cases {
            assert_eq!(judge(status, body), (ok, summary.to_string()), "{body}");
        }
    }

    fn fixture_folder() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/workshop-one/grafana/dashboards")
    }

    const OK_REPLY: &str =
        r#"{"results":{"A":{"status":200,"frames":[{"data":{"values":[[1,2,3]]}}]}}}"#;
    const FAILING_REPLY: &str = r#"{"results":{"A":{"status":400,"error":"boom"}}}"#;

    struct LayerB {
        dashboards: Vec<Dashboard>,
        code: i32,
        lines: Vec<String>,
        bodies: Vec<Value>,
    }

    fn reply(status: u16, body: &str) -> Result<Reply, String> {
        Ok(Reply {
            status,
            body: body.to_string(),
        })
    }

    fn fixture_answer(body: &Value) -> Result<Reply, String> {
        if body["queries"][0]["expr"] == "metric_that_does_not_exist" {
            reply(200, FAILING_REPLY)
        } else {
            reply(200, OK_REPLY)
        }
    }

    fn run_layer_b(
        dashboards: &[(String, Value)],
        answer: impl Fn(&Value) -> Result<Reply, String>,
    ) -> LayerB {
        let mut bodies = Vec::new();
        let mut lines = Vec::new();
        let (results, code) = layer_b(
            dashboards,
            |body| {
                bodies.push(body.clone());
                answer(body)
            },
            || UNIX_EPOCH + seconds(1_793_960_400),
            |line| lines.push(line),
        );
        LayerB {
            dashboards: results,
            code,
            lines,
            bodies,
        }
    }

    fn query_of(body: &Value) -> &Value {
        &body["queries"][0]
    }

    #[test]
    fn layer_b_asks_every_panel_of_the_fixture_for_the_devices_its_targets_name() {
        let run = run_layer_b(&dashboards(&fixture_folder()).unwrap(), fixture_answer);

        assert_eq!(run.bodies.len(), 7);
        assert_eq!(
            run.bodies[0],
            json!({
                "queries": [{
                    "refId": "A",
                    "expr": "bgp_oper_state{device=\"srl1\"}",
                    "intervalMs": 30000,
                    "maxDataPoints": 600,
                }],
                "from": "1793960100000",
                "to": "1793960400000",
            })
        );
        assert_eq!(
            query_of(&run.bodies[1])["expr"],
            "bgp_oper_state{device=\"srl2\"}"
        );
        assert_eq!(
            query_of(&run.bodies[2])["expr"],
            "{vendor_facility_process=\"UPDOWN\"}"
        );
        assert_eq!(query_of(&run.bodies[2])["refId"], "A");
        assert_eq!(
            query_of(&run.bodies[3])["url_options"]["body_graphql_query"],
            "{ device(name: \"srl1\") { name } }"
        );
        assert_eq!(
            query_of(&run.bodies[4])["url_options"]["body_graphql_query"],
            "{ device(name: \"srl2\") { name } }"
        );
        assert_eq!(query_of(&run.bodies[5])["expr"], "up");
        assert_eq!(
            query_of(&run.bodies[6])["expr"],
            "metric_that_does_not_exist"
        );
    }

    #[test]
    fn layer_b_judges_a_panel_pass_when_any_target_is_ok_and_skip_when_it_has_none() {
        let run = run_layer_b(&dashboards(&fixture_folder()).unwrap(), fixture_answer);

        assert_eq!(run.dashboards.len(), 1);
        assert_eq!(run.dashboards[0].dashboard, "Workshop One");
        assert_eq!(run.dashboards[0].file, "one.json");
        let panels = &run.dashboards[0].panels;
        let seen: Vec<(i64, &str, &str)> = panels
            .iter()
            .map(|panel| {
                (
                    panel.panel_id.as_i64().unwrap(),
                    panel.device.as_str(),
                    panel.status.as_str(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                (1, "srl1", "PASS"),
                (1, "srl2", "PASS"),
                (2, "(no template)", "PASS"),
                (4, "srl1", "PASS"),
                (4, "srl2", "PASS"),
                (5, "(no template)", "PASS"),
                (6, "(no template)", "SKIP"),
            ]
        );
        assert_eq!(
            panels[5],
            PanelResult {
                panel_id: json!(5),
                panel_title: "Lab is up".to_string(),
                panel_type: "stat".to_string(),
                device: "(no template)".to_string(),
                status: "PASS".to_string(),
                summary: "prometheus:1 frames, 3 rows; prometheus:ds error: boom".to_string(),
                targets: vec![
                    Target {
                        ok: true,
                        ds: "prometheus".to_string(),
                        summary: "1 frames, 3 rows".to_string(),
                        expr: "up".to_string(),
                    },
                    Target {
                        ok: false,
                        ds: "prometheus".to_string(),
                        summary: "ds error: boom".to_string(),
                        expr: "metric_that_does_not_exist".to_string(),
                    },
                ],
            }
        );
        assert_eq!(panels[6].summary, "no targets");
        assert!(panels[6].targets.is_empty());
        assert_eq!(panels[3].targets[0].ds, "yesoreyeram-infinity-datasource");
        assert_eq!(panels[3].targets[0].expr, "device record");
        assert_eq!(run.code, 0);
    }

    #[test]
    fn layer_b_prints_the_dashboard_header_the_panel_lines_and_the_summary() {
        let run = run_layer_b(&dashboards(&fixture_folder()).unwrap(), fixture_answer);

        assert_eq!(
            run.lines,
            [
                "",
                "=== Workshop One (one.json) — 5 panels ===",
                "  [OK  ] panel #1 timeseries     device=srl1         'BGP sessions'                           → prometheus:1 frames, 3 rows",
                "  [OK  ] panel #1 timeseries     device=srl2         'BGP sessions'                           → prometheus:1 frames, 3 rows",
                "  [OK  ] panel #2 logs           device=(no template) 'Interface events'                       → loki:1 frames, 3 rows",
                "  [OK  ] panel #4 table          device=srl1         'Device record'                          → yesoreyeram-infinity-datasource:1 frames, 3 rows",
                "  [OK  ] panel #4 table          device=srl2         'Device record'                          → yesoreyeram-infinity-datasource:1 frames, 3 rows",
                "  [OK  ] panel #5 stat           device=(no template) 'Lab is up'                              → prometheus:1 frames, 3 rows; prometheus:ds error: boom",
                "  [SKIP] panel #6 text           device=(no template) 'Notes'                                  → no targets",
                "",
                "Layer B summary: PASS=6 FAIL=0 SKIP=1",
            ]
        );
    }

    #[test]
    fn layer_b_is_one_when_a_panel_failed_for_every_target() {
        let run = run_layer_b(&dashboards(&fixture_folder()).unwrap(), |_| {
            reply(500, "boom")
        });

        assert_eq!(run.code, 1);
        assert_eq!(
            run.lines.last().unwrap(),
            "Layer B summary: PASS=0 FAIL=6 SKIP=1"
        );
        assert!(run.lines[2].starts_with("  [FAIL] panel #1 "));
        assert!(run.lines[2].ends_with("→ prometheus:http 500"));
    }

    #[test]
    fn layer_b_takes_the_error_text_of_a_failed_request_and_cuts_a_long_title_and_summary() {
        let long_title = "T".repeat(50);
        let dashboard = json!({"panels": [{
            "id": 7,
            "type": "stat",
            "title": long_title,
            "datasource": {"type": "prometheus"},
            "targets": [{"expr": "up"}],
        }]});

        let run = run_layer_b(&[("long.json".to_string(), dashboard)], |_| {
            Err("x".repeat(200))
        });

        assert_eq!(run.dashboards[0].dashboard, "long");
        assert_eq!(
            run.dashboards[0].panels[0].summary,
            format!("prometheus:{}", "x".repeat(200))
        );
        let line = &run.lines[2];
        let title = format!("'{}'", "T".repeat(38));
        assert!(
            line.contains(&format!("device=(no template) {title} → ")),
            "{line}"
        );
        assert_eq!(line.chars().count() - line.find('→').unwrap(), 2 + 140);
        assert_eq!(run.code, 1);
    }

    #[test]
    fn layer_b_lower_cases_a_datasource_type_and_a_string_datasource_and_falls_back_to_the_panel() {
        let dashboard = json!({"title": "D", "panels": [
            {"id": 1, "type": "stat", "datasource": "Loki", "targets": [{"expr": "a"}]},
            {"id": 2, "type": "stat", "datasource": {"type": "Loki"}, "targets": [
                {"expr": "b", "datasource": {"type": "Prometheus"}},
                {"expr": "c", "datasource": {}},
            ]},
        ]});

        let run = run_layer_b(&[("d.json".to_string(), dashboard)], fixture_answer);

        let types: Vec<Vec<&str>> = run.dashboards[0]
            .panels
            .iter()
            .map(|panel| {
                panel
                    .targets
                    .iter()
                    .map(|target| target.ds.as_str())
                    .collect()
            })
            .collect();
        assert_eq!(types, [vec!["loki"], vec!["prometheus", "loki"]]);
    }

    #[test]
    fn quote_follows_the_repr_of_python_for_quotes_and_backslashes() {
        assert_eq!(quote("plain"), "'plain'");
        assert_eq!(quote("it's"), r#""it's""#);
        assert_eq!(quote(r#"say "hi""#), r#"'say "hi"'"#);
        assert_eq!(quote(r#"it's "x""#), r#"'it\'s "x"'"#);
        assert_eq!(quote(r"a\b"), r"'a\\b'");
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let folder =
                std::env::temp_dir().join(format!("nobs-layers-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&folder);
            fs::create_dir_all(&folder).unwrap();
            Self(folder)
        }

        fn write(&self, name: &str, text: &str) {
            fs::write(self.0.join(name), text).unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn dashboards_reads_the_json_files_sorted_by_name_and_skips_the_others() {
        let folder = Scratch::new("sorted");
        folder.write("b.json", r#"{"title": "B"}"#);
        folder.write("a.json", r#"{"title": "A"}"#);
        folder.write("notes.txt", "not a dashboard");

        let found = dashboards(&folder.0).unwrap();

        assert_eq!(
            found,
            [
                ("a.json".to_string(), json!({"title": "A"})),
                ("b.json".to_string(), json!({"title": "B"})),
            ]
        );
    }

    #[test]
    fn dashboards_of_a_missing_folder_says_it_cannot_read_the_folder() {
        let folder = Scratch::new("missing");
        let missing = folder.0.join("nothing-here");

        let error = dashboards(&missing).unwrap_err();

        assert!(
            error.starts_with(&format!("cannot read {}: ", missing.display())),
            "{error}"
        );
    }

    #[test]
    fn dashboards_with_a_file_that_is_not_json_says_it_cannot_read_that_file() {
        let folder = Scratch::new("not-json");
        folder.write("broken.json", "this is not json");

        let error = dashboards(&folder.0).unwrap_err();

        assert!(
            error.starts_with(&format!(
                "cannot read {}: ",
                folder.0.join("broken.json").display()
            )),
            "{error}"
        );
    }

    #[test]
    fn dashboards_of_an_empty_folder_says_there_are_no_dashboard_files() {
        let folder = Scratch::new("empty");
        folder.write("notes.txt", "not a dashboard");

        assert_eq!(
            dashboards(&folder.0),
            Err(format!("no dashboard files in {}", folder.0.display()))
        );
    }

    #[test]
    fn report_is_the_markdown_of_the_python_without_a_layer_c_part() {
        let waits = [
            Wait {
                label: "bgp_oper_state pipeline convergence".to_string(),
                ok: true,
                elapsed_s: 8.0,
                detail: "count = 6".to_string(),
                expected: ">= 6 series".to_string(),
            },
            Wait {
                label: "Loki has UPDOWN events from both producers".to_string(),
                ok: false,
                elapsed_s: 30.0,
                detail: "events last 5m = 3".to_string(),
                expected: ">= 8 events".to_string(),
            },
        ];
        let panel =
            |id: i64, title: &str, kind: &str, device: &str, status: &str, summary: &str| {
                PanelResult {
                    panel_id: json!(id),
                    panel_title: title.to_string(),
                    panel_type: kind.to_string(),
                    device: device.to_string(),
                    status: status.to_string(),
                    summary: summary.to_string(),
                    targets: vec![],
                }
            };
        let dashboards = [Dashboard {
            dashboard: "Workshop One".to_string(),
            file: "one.json".to_string(),
            panels: vec![
                panel(
                    1,
                    "BGP sessions",
                    "timeseries",
                    "srl1",
                    "PASS",
                    "prometheus:1 frames, 3 rows",
                ),
                panel(6, "Notes", "text", "(no template)", "SKIP", "no targets"),
            ],
        }];

        let text = report("Packt", "2026-11-06 10:15:30", &waits, &dashboards, 1, 0);

        assert_eq!(
            text,
            "# Packt preflight — 2026-11-06 10:15:30\n\
             \n\
             ## Summary\n\
             \n\
             - Layer A (data shape): exit=1\n\
             - Layer B (per-panel /api/ds/query): exit=0\n\
             \n\
             ## Layer A — data shape waits\n\
             \n\
             - **PASS** `bgp_oper_state pipeline convergence` — count = 6 (elapsed 8.0s)\n\
             - **FAIL** `Loki has UPDOWN events from both producers` — events last 5m = 3 (elapsed 30.0s)\n\
             \n\
             ## Layer B — per-panel /api/ds/query\n\
             \n\
             ### Workshop One (one.json)\n\
             \n\
             | panel | type | device | status | summary |\n\
             |---|---|---|---|---|\n\
             | #1 BGP sessions | timeseries | srl1 | **PASS** | prometheus:1 frames, 3 rows |\n\
             | #6 Notes | text | (no template) | **SKIP** | no targets |\n"
        );
    }

    #[test]
    fn stamp_is_the_utc_date_and_time_with_a_space_between() {
        assert_eq!(
            stamp(UNIX_EPOCH + seconds(1_793_960_130)),
            "2026-11-06 10:15:30"
        );
    }

    fn tail_of(a: i32, b: i32) -> Tail {
        tail(a, b, 772.34, Path::new("/tmp/preflight-out/REPORT.md"))
    }

    #[test]
    fn tail_is_green_with_code_zero_when_both_layers_are_zero() {
        assert_eq!(
            tail_of(0, 0),
            Tail {
                layer_c: "Layer C — per-panel screenshots: not built in nobs, skipped".to_string(),
                summary:
                    "preflight green (772.3s) — Layer A exit=0 | Layer B exit=0 | Layer C skipped"
                        .to_string(),
                report: "report: /tmp/preflight-out/REPORT.md".to_string(),
                code: 0,
            }
        );
    }

    #[test]
    fn tail_is_failures_with_code_one_when_layer_a_failed() {
        assert_eq!(
            tail_of(1, 0),
            Tail {
                layer_c: "Layer C — per-panel screenshots: not built in nobs, skipped".to_string(),
                summary:
                    "preflight failures (772.3s) — Layer A exit=1 | Layer B exit=0 | Layer C skipped"
                        .to_string(),
                report: "report: /tmp/preflight-out/REPORT.md".to_string(),
                code: 1,
            }
        );
    }

    #[test]
    fn tail_is_failures_with_code_one_when_layer_b_failed() {
        assert_eq!(
            tail_of(0, 1),
            Tail {
                layer_c: "Layer C — per-panel screenshots: not built in nobs, skipped".to_string(),
                summary:
                    "preflight failures (772.3s) — Layer A exit=0 | Layer B exit=1 | Layer C skipped"
                        .to_string(),
                report: "report: /tmp/preflight-out/REPORT.md".to_string(),
                code: 1,
            }
        );
    }

    #[test]
    fn waits_json_is_a_list_indented_by_two_spaces_with_the_keys_in_order() {
        let waits = [Wait {
            label: "l".to_string(),
            ok: false,
            elapsed_s: 30.0,
            detail: "d".to_string(),
            expected: "e".to_string(),
        }];

        assert_eq!(
            waits_json(&waits),
            "[\n  {\n    \"label\": \"l\",\n    \"ok\": false,\n    \"elapsed_s\": 30.0,\n    \"detail\": \"d\",\n    \"expected\": \"e\"\n  }\n]"
        );
    }

    #[test]
    fn dashboards_json_is_a_list_indented_by_two_spaces_with_the_keys_in_order() {
        let found = [Dashboard {
            dashboard: "D".to_string(),
            file: "d.json".to_string(),
            panels: vec![PanelResult {
                panel_id: json!(3),
                panel_title: "T".to_string(),
                panel_type: "stat".to_string(),
                device: "srl1".to_string(),
                status: "PASS".to_string(),
                summary: "s".to_string(),
                targets: vec![Target {
                    ok: true,
                    ds: "prometheus".to_string(),
                    summary: "1 frames, 2 rows".to_string(),
                    expr: "up".to_string(),
                }],
            }],
        }];

        assert_eq!(
            dashboards_json(&found),
            concat!(
                "[\n",
                "  {\n",
                "    \"dashboard\": \"D\",\n",
                "    \"file\": \"d.json\",\n",
                "    \"panels\": [\n",
                "      {\n",
                "        \"panel_id\": 3,\n",
                "        \"panel_title\": \"T\",\n",
                "        \"panel_type\": \"stat\",\n",
                "        \"device\": \"srl1\",\n",
                "        \"status\": \"PASS\",\n",
                "        \"summary\": \"s\",\n",
                "        \"targets\": [\n",
                "          {\n",
                "            \"ok\": true,\n",
                "            \"ds\": \"prometheus\",\n",
                "            \"summary\": \"1 frames, 2 rows\",\n",
                "            \"expr\": \"up\"\n",
                "          }\n",
                "        ]\n",
                "      }\n",
                "    ]\n",
                "  }\n",
                "]"
            )
        );
    }
}
