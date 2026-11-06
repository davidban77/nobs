use std::fmt;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use ureq::Agent;

use crate::http;

pub const DEFAULT_URL: &str = "http://localhost:4200";

const TIMEOUT: Duration = Duration::from_secs(10);
const FLOW_RUNS_PATH: &str = "/api/flow_runs/filter";
const TASK_RUNS_PATH: &str = "/api/task_runs/filter";
const BODY_CUT: usize = 200;
const UNKNOWN_STATE: &str = "?";

pub struct Prefect {
    url: String,
    agent: Agent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowRun {
    pub id: String,
    pub name: String,
    pub device: Option<String>,
    pub peer: Option<String>,
    pub start_time: String,
    pub state: String,
    pub parent_task_run_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRun {
    pub id: String,
    pub flow_run_id: String,
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
    Body(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Reach { url, reason } => write!(f, "cannot reach {url}: {reason}"),
            Error::Status { status, path, body } => {
                let body: String = body.chars().take(BODY_CUT).collect();
                write!(f, "Prefect answered {status} on {path}: {body}")
            }
            Error::Body(reason) => {
                write!(f, "Prefect answered with text nobs cannot read: {reason}")
            }
        }
    }
}

#[derive(Serialize)]
struct FlowRunsRequest {
    sort: &'static str,
    limit: u64,
}

#[derive(Serialize)]
struct TaskRunsRequest<'a> {
    task_runs: TaskRunsFilter<'a>,
    limit: usize,
}

#[derive(Serialize)]
struct TaskRunsFilter<'a> {
    id: AnyOf<'a>,
}

#[derive(Serialize)]
struct AnyOf<'a> {
    any_: &'a [String],
}

impl Prefect {
    pub fn new(url: impl Into<String>) -> Self {
        let url = url.into();
        let url = url.trim_end_matches('/');
        Self {
            url: url.strip_suffix("/api").unwrap_or(url).to_string(),
            agent: http::agent(TIMEOUT),
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn flow_runs(&self, limit: u64) -> Result<Vec<FlowRun>, Error> {
        let request = FlowRunsRequest {
            sort: "START_TIME_DESC",
            limit,
        };
        Ok(self
            .post(FLOW_RUNS_PATH, &request)?
            .iter()
            .map(flow_run)
            .collect())
    }

    pub fn task_runs(&self, ids: &[String]) -> Result<Vec<TaskRun>, Error> {
        let request = TaskRunsRequest {
            task_runs: TaskRunsFilter {
                id: AnyOf { any_: ids },
            },
            limit: ids.len(),
        };
        Ok(self
            .post(TASK_RUNS_PATH, &request)?
            .iter()
            .map(|run| TaskRun {
                id: text(&run["id"]),
                flow_run_id: text(&run["flow_run_id"]),
            })
            .collect())
    }

    fn post(&self, path: &str, request: &impl Serialize) -> Result<Vec<Value>, Error> {
        let body = serde_json::to_string(request).expect("a request body is always valid JSON");
        let sent = self
            .agent
            .post(format!("{}{path}", self.url))
            .header("content-type", "application/json")
            .send(body.as_bytes());
        let reply = http::read(&self.url, sent).map_err(|error| Error::Reach {
            url: self.url.clone(),
            reason: http::reason(error, &self.url),
        })?;
        if reply.status >= 400 {
            return Err(Error::Status {
                status: reply.status,
                path: path.to_string(),
                body: reply.body,
            });
        }
        match serde_json::from_str(&reply.body) {
            Ok(Value::Array(items)) => Ok(items),
            Ok(_) => Err(Error::Body("the reply is not a list".to_string())),
            Err(_) => Err(Error::Body("the reply is not JSON".to_string())),
        }
    }
}

fn flow_run(run: &Value) -> FlowRun {
    FlowRun {
        id: text(&run["id"]),
        name: text(&run["name"]),
        device: optional(&run["parameters"]["device"]),
        peer: optional(&run["parameters"]["peer_address"]),
        start_time: text(&run["start_time"]),
        state: run["state"]["type"]
            .as_str()
            .unwrap_or(UNKNOWN_STATE)
            .to_string(),
        parent_task_run_id: optional(&run["parent_task_run_id"]),
    }
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

fn optional(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;
    use crate::http::stub::Stub;

    const FLOW_RUNS: &str = r#"[{"id":"q1","name":"quarantine_bgp | srl1:10.1.99.2","parameters":{"device":"srl1","peer_address":"10.1.99.2"},
  "start_time":"2026-11-06T10:15:40.512345Z","state":{"type":"COMPLETED"},"parent_task_run_id":"t0"},
 {"id":"r0","name":"alert_receiver | BgpSessionNotUp:firing","parameters":{"alertname":"BgpSessionNotUp","status":"firing","alert_group":{}},
  "start_time":"2026-11-06T10:15:39Z","state":{"type":"COMPLETED"},"parent_task_run_id":null}]"#;
    const TASK_RUNS: &str = r#"[{"id":"t0","flow_run_id":"r0"},{"id":"t1","flow_run_id":"q1"}]"#;

    fn closed_port_url() -> String {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        format!("http://127.0.0.1:{port}")
    }

    fn with_api(stub: &Stub) -> Prefect {
        Prefect::new(format!("{}/api/", stub.url()))
    }

    #[test]
    fn new_drops_a_trailing_slash_and_then_a_trailing_api() {
        for url in [
            "http://localhost:4200/api/",
            "http://localhost:4200/api",
            "http://localhost:4200/",
            "http://localhost:4200",
        ] {
            assert_eq!(Prefect::new(url).url(), "http://localhost:4200", "{url}");
        }
    }

    #[test]
    fn flow_runs_sends_one_post_with_the_sort_and_a_numeric_limit_and_the_content_type() {
        let stub = Stub::start(&[("POST", "/api/flow_runs/filter", 200, "[]")]);

        with_api(&stub).flow_runs(100).unwrap();

        let requests = stub.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/api/flow_runs/filter");
        assert_eq!(
            String::from_utf8_lossy(&requests[0].body),
            r#"{"sort":"START_TIME_DESC","limit":100}"#
        );
        assert_eq!(requests[0].header("content-type"), Some("application/json"));
    }

    #[test]
    fn flow_runs_reads_the_fields_in_the_order_the_server_sent_them() {
        let stub = Stub::start(&[("POST", "/api/flow_runs/filter", 200, FLOW_RUNS)]);

        let runs = with_api(&stub).flow_runs(100).unwrap();

        assert_eq!(
            runs,
            [
                FlowRun {
                    id: "q1".to_string(),
                    name: "quarantine_bgp | srl1:10.1.99.2".to_string(),
                    device: Some("srl1".to_string()),
                    peer: Some("10.1.99.2".to_string()),
                    start_time: "2026-11-06T10:15:40.512345Z".to_string(),
                    state: "COMPLETED".to_string(),
                    parent_task_run_id: Some("t0".to_string()),
                },
                FlowRun {
                    id: "r0".to_string(),
                    name: "alert_receiver | BgpSessionNotUp:firing".to_string(),
                    device: None,
                    peer: None,
                    start_time: "2026-11-06T10:15:39Z".to_string(),
                    state: "COMPLETED".to_string(),
                    parent_task_run_id: None,
                },
            ]
        );
    }

    #[test]
    fn a_run_without_a_name_a_start_or_a_state_gets_the_empty_and_question_mark_defaults() {
        let stub = Stub::start(&[(
            "POST",
            "/api/flow_runs/filter",
            200,
            r#"[{"id":"x","start_time":null,"state":null}]"#,
        )]);

        let runs = with_api(&stub).flow_runs(1).unwrap();

        assert_eq!(runs[0].name, "");
        assert_eq!(runs[0].start_time, "");
        assert_eq!(runs[0].state, "?");
    }

    #[test]
    fn task_runs_sends_one_post_with_the_any_filter_and_returns_the_pairs() {
        let stub = Stub::start(&[("POST", "/api/task_runs/filter", 200, TASK_RUNS)]);
        let ids = ["t0".to_string(), "t1".to_string()];

        let runs = with_api(&stub).task_runs(&ids).unwrap();

        let requests = stub.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/api/task_runs/filter");
        assert_eq!(
            String::from_utf8_lossy(&requests[0].body),
            r#"{"task_runs":{"id":{"any_":["t0","t1"]}},"limit":2}"#
        );
        assert_eq!(requests[0].header("content-type"), Some("application/json"));
        assert_eq!(
            runs,
            [
                TaskRun {
                    id: "t0".to_string(),
                    flow_run_id: "r0".to_string()
                },
                TaskRun {
                    id: "t1".to_string(),
                    flow_run_id: "q1".to_string()
                },
            ]
        );
    }

    #[test]
    fn a_status_of_400_or_more_is_a_status_error_with_the_path_and_the_body() {
        let stub = Stub::start(&[
            ("POST", "/api/flow_runs/filter", 500, "boom"),
            ("POST", "/api/task_runs/filter", 422, "bad ids"),
        ]);
        let prefect = with_api(&stub);

        assert_eq!(
            prefect.flow_runs(100),
            Err(Error::Status {
                status: 500,
                path: "/api/flow_runs/filter".to_string(),
                body: "boom".to_string()
            })
        );
        assert_eq!(
            prefect.task_runs(&["t0".to_string()]),
            Err(Error::Status {
                status: 422,
                path: "/api/task_runs/filter".to_string(),
                body: "bad ids".to_string()
            })
        );
    }

    #[test]
    fn a_reply_that_is_not_a_list_is_a_body_error() {
        let stub = Stub::start(&[
            ("POST", "/api/flow_runs/filter", 200, "not json"),
            ("POST", "/api/flow_runs/filter", 200, r#"{"detail":"x"}"#),
        ]);
        let prefect = with_api(&stub);

        for _ in 0..2 {
            let result = prefect.flow_runs(100);
            assert!(matches!(result, Err(Error::Body(_))), "{result:?}");
        }
    }

    #[test]
    fn no_listener_is_a_reach_error_with_the_url_and_a_reason_without_the_prefix() {
        let url = closed_port_url();

        match Prefect::new(&url).flow_runs(100) {
            Err(Error::Reach { url: seen, reason }) => {
                assert_eq!(seen, url);
                assert!(!reason.is_empty());
                assert!(!reason.starts_with("cannot reach"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_error_texts_are_the_ones_the_commands_print() {
        assert_eq!(
            Error::Reach {
                url: "http://localhost:4200".to_string(),
                reason: "connection refused".to_string()
            }
            .to_string(),
            "cannot reach http://localhost:4200: connection refused"
        );
        assert_eq!(
            Error::Status {
                status: 500,
                path: "/api/flow_runs/filter".to_string(),
                body: "x".repeat(300)
            }
            .to_string(),
            format!(
                "Prefect answered 500 on /api/flow_runs/filter: {}",
                "x".repeat(200)
            )
        );
        assert_eq!(
            Error::Body("the reply is not a list".to_string()).to_string(),
            "Prefect answered with text nobs cannot read: the reply is not a list"
        );
    }
}
