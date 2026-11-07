use std::time::Duration;

use serde_json::Value;
use ureq::{Agent, RequestBuilder};

use crate::http;
pub use crate::http::Reply;

const TIMEOUT: Duration = Duration::from_secs(10);
const EVENT_TIMEOUT: Duration = Duration::from_secs(35);

pub struct Sonda {
    url: String,
    key: Option<String>,
    agent: Agent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioRow {
    pub id: String,
    pub name: String,
    pub state: String,
    pub elapsed_secs: String,
    pub degraded: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInfo {
    pub version: String,
    pub git_sha: String,
}

impl Sonda {
    pub const DEFAULT_URL: &'static str = "http://localhost:8080";

    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into().trim_end_matches('/').to_string(),
            key: None,
            agent: http::agent(TIMEOUT),
        }
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.agent = http::agent(timeout);
        self
    }

    pub fn api_key(mut self, key: Option<String>) -> Self {
        self.key = key;
        self
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn health(&self) -> Result<Reply, String> {
        self.get("/health", false)
    }

    pub fn ready(&self) -> Result<Reply, String> {
        self.get("/ready", false)
    }

    pub fn list(&self) -> Result<Reply, String> {
        self.get("/scenarios", true)
    }

    pub fn show(&self, id: &str) -> Result<Reply, String> {
        self.get(&format!("/scenarios/{id}"), true)
    }

    pub fn stats(&self, id: &str) -> Result<Reply, String> {
        self.get(&format!("/scenarios/{id}/stats"), true)
    }

    pub fn delete(&self, id: &str) -> Result<Reply, String> {
        let request = self
            .agent
            .delete(self.endpoint(&format!("/scenarios/{id}")));
        http::read(&self.url, self.with_key(request, true).call())
    }

    pub fn load(&self, yaml: &[u8]) -> Result<Reply, String> {
        self.post("/scenarios", "application/x-yaml", yaml, TIMEOUT)
    }

    pub fn post_scenario(&self, json: &[u8]) -> Result<Reply, String> {
        self.post("/scenarios", "application/json", json, TIMEOUT)
    }

    pub fn event(&self, json: &[u8]) -> Result<Reply, String> {
        self.post("/events", "application/json", json, EVENT_TIMEOUT)
    }

    pub fn metrics(&self) -> Result<Reply, String> {
        self.get("/metrics", true)
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.url)
    }

    fn with_key<B>(&self, request: RequestBuilder<B>, send_key: bool) -> RequestBuilder<B> {
        match (&self.key, send_key) {
            (Some(key), true) => request.header("Authorization", format!("Bearer {key}")),
            _ => request,
        }
    }

    fn get(&self, path: &str, send_key: bool) -> Result<Reply, String> {
        let request = self.agent.get(self.endpoint(path));
        http::read(&self.url, self.with_key(request, send_key).call())
    }

    fn post(
        &self,
        path: &str,
        content_type: &str,
        body: &[u8],
        timeout: Duration,
    ) -> Result<Reply, String> {
        let request = self
            .agent
            .post(self.endpoint(path))
            .config()
            .timeout_global(Some(timeout))
            .build()
            .content_type(content_type);
        http::read(&self.url, self.with_key(request, true).send(body))
    }
}

pub fn scenario_rows(list_body: &str) -> Result<Vec<ScenarioRow>, String> {
    let value: Value =
        serde_json::from_str(list_body).map_err(|_| "the reply is not JSON".to_string())?;
    let rows = value
        .get("scenarios")
        .ok_or("the reply has no \"scenarios\" list")?
        .as_array()
        .ok_or("\"scenarios\" is not a list")?;
    Ok(rows
        .iter()
        .map(|row| ScenarioRow {
            id: field(row, "id"),
            name: field(row, "name"),
            state: field(row, "state"),
            elapsed_secs: field(row, "elapsed_secs"),
            degraded: field(row, "degraded"),
        })
        .collect())
}

fn field(row: &Value, name: &str) -> String {
    match row.get(name) {
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => "-".to_string(),
    }
}

pub fn build_info(metrics_text: &str) -> Option<BuildInfo> {
    let labels = metrics_text
        .lines()
        .find_map(|line| line.strip_prefix("sonda_server_build_info{"))?
        .split_once('}')?
        .0;
    Some(BuildInfo {
        version: label(labels, "version")?,
        git_sha: label(labels, "git_sha")?,
    })
}

fn label(labels: &str, name: &str) -> Option<String> {
    labels.split(',').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key.trim() == name).then(|| value.trim().trim_matches('"').to_string())
    })
}

pub fn check_runnable(file_text: &str) -> Result<(), String> {
    const ONLY: &str = "nobs sends only a file with kind: runnable";
    if matches!(file_text.trim_start().chars().next(), Some('{' | '[')) {
        return Err(format!("this is a JSON file; {ONLY}"));
    }
    let Some(rest) = file_text
        .lines()
        .find_map(|line| line.strip_prefix("kind:"))
    else {
        return Err(format!(
            "the file has no kind: line at the start of a line; {ONLY}"
        ));
    };
    let value = unquote(rest.split('#').next().unwrap_or("").trim());
    match value {
        "runnable" => Ok(()),
        "composable" => Err(format!(
            "kind is composable, so this file is a pack; {ONLY}"
        )),
        other => Err(format!("kind is \"{other}\"; {ONLY}")),
    }
}

fn unquote(value: &str) -> &str {
    ['"', '\'']
        .into_iter()
        .find_map(|quote| value.strip_prefix(quote)?.strip_suffix(quote))
        .unwrap_or(value)
}

pub fn pretty(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reason(text: &str) -> String {
        check_runnable(text).unwrap_err()
    }

    #[test]
    fn check_runnable_accepts_a_plain_runnable_kind() {
        assert_eq!(check_runnable("version: 2\nkind: runnable\n"), Ok(()));
    }

    #[test]
    fn check_runnable_accepts_a_double_quoted_runnable_kind() {
        assert_eq!(check_runnable("kind: \"runnable\"\n"), Ok(()));
    }

    #[test]
    fn check_runnable_accepts_a_single_quoted_runnable_kind() {
        assert_eq!(check_runnable("kind: 'runnable'\n"), Ok(()));
    }

    #[test]
    fn check_runnable_accepts_a_kind_with_a_trailing_comment() {
        assert_eq!(check_runnable("kind: runnable # note\n"), Ok(()));
    }

    #[test]
    fn check_runnable_refuses_a_composable_file_and_says_pack() {
        assert!(reason("version: 2\nkind: composable\n").contains("pack"));
    }

    #[test]
    fn check_runnable_refuses_a_file_without_a_kind_line() {
        assert!(reason("version: 2\nscenarios: []\n").contains("no kind: line"));
    }

    #[test]
    fn check_runnable_does_not_count_an_indented_kind() {
        assert!(reason("scenarios:\n  kind: runnable\n").contains("no kind: line"));
    }

    #[test]
    fn check_runnable_refuses_a_json_object_file() {
        assert!(reason("\n  {\"kind\": \"runnable\"}\n").contains("JSON"));
    }

    #[test]
    fn check_runnable_refuses_a_json_list_file() {
        assert!(reason("[1]").contains("JSON"));
    }

    #[test]
    fn check_runnable_refuses_any_other_kind_and_names_it() {
        assert!(reason("kind: other\n").contains("\"other\""));
    }

    #[test]
    fn build_info_reads_version_first() {
        let text = "# HELP sonda_server_build_info Build.\n# TYPE sonda_server_build_info gauge\nsonda_server_build_info{version=\"1.29.0\",git_sha=\"abc\"} 1\n";
        assert_eq!(
            build_info(text),
            Some(BuildInfo {
                version: "1.29.0".to_string(),
                git_sha: "abc".to_string()
            })
        );
    }

    #[test]
    fn build_info_reads_git_sha_first() {
        let text = "sonda_server_build_info{git_sha=\"abc\",version=\"1.29.0\"} 1\n";
        assert_eq!(
            build_info(text),
            Some(BuildInfo {
                version: "1.29.0".to_string(),
                git_sha: "abc".to_string()
            })
        );
    }

    #[test]
    fn build_info_is_none_without_the_series() {
        assert_eq!(build_info("# HELP up Up.\nup 1\n"), None);
    }

    #[test]
    fn build_info_is_none_when_a_label_is_missing() {
        assert_eq!(
            build_info("sonda_server_build_info{version=\"1.29.0\"} 1\n"),
            None
        );
        assert_eq!(
            build_info("sonda_server_build_info{git_sha=\"abc\"} 1\n"),
            None
        );
    }

    #[test]
    fn scenario_rows_reads_strings_numbers_and_booleans() {
        let body = r#"{"scenarios":[
            {"id":"a1","name":"cpu","state":"running","elapsed_secs":12.5,"degraded":false},
            {"id":"b2","name":"mem","state":"finished","elapsed_secs":3,"degraded":true}
        ]}"#;
        assert_eq!(
            scenario_rows(body).unwrap(),
            vec![
                ScenarioRow {
                    id: "a1".to_string(),
                    name: "cpu".to_string(),
                    state: "running".to_string(),
                    elapsed_secs: "12.5".to_string(),
                    degraded: "false".to_string(),
                },
                ScenarioRow {
                    id: "b2".to_string(),
                    name: "mem".to_string(),
                    state: "finished".to_string(),
                    elapsed_secs: "3".to_string(),
                    degraded: "true".to_string(),
                },
            ]
        );
    }

    #[test]
    fn scenario_rows_shows_a_missing_field_as_a_dash() {
        let body = r#"{"scenarios":[{"id":"a1","name":"cpu","state":"running","elapsed_secs":1}]}"#;
        assert_eq!(scenario_rows(body).unwrap()[0].degraded, "-");
    }

    #[test]
    fn scenario_rows_gives_no_rows_for_an_empty_list() {
        assert_eq!(scenario_rows(r#"{"scenarios":[]}"#).unwrap(), vec![]);
    }

    #[test]
    fn scenario_rows_refuses_a_body_that_is_not_json() {
        assert!(scenario_rows("not json").is_err());
    }

    #[test]
    fn scenario_rows_refuses_a_body_without_the_scenarios_key() {
        let reason = scenario_rows(r#"{"items":[]}"#).unwrap_err();
        assert!(reason.contains("has no"), "{reason}");
    }

    #[test]
    fn scenario_rows_refuses_scenarios_that_is_not_a_list() {
        assert!(scenario_rows(r#"{"scenarios":"none"}"#).is_err());
    }

    #[test]
    fn pretty_puts_one_field_per_line_with_keys_sorted() {
        assert_eq!(pretty(r#"{"b":1,"a":2}"#), "{\n  \"a\": 2,\n  \"b\": 1\n}");
    }

    #[test]
    fn pretty_returns_a_body_that_is_not_json_unchanged() {
        let text = "# TYPE up gauge\nup 1\n";
        assert_eq!(pretty(text), text);
    }

    #[test]
    fn post_scenario_sends_the_bytes_given_as_json_to_scenarios() {
        let stub = crate::http::stub::Stub::start(&[("POST", "/scenarios", 201, "{}")]);

        let reply = Sonda::new(stub.url()).post_scenario(b"{\"a\":1}").unwrap();

        assert_eq!(reply.status, 201);
        let requests = stub.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/scenarios");
        assert_eq!(requests[0].header("content-type"), Some("application/json"));
        assert_eq!(requests[0].body, b"{\"a\":1}");
        assert_eq!(requests[0].header("authorization"), None);
    }

    #[test]
    fn post_scenario_sends_the_key_as_a_bearer_token() {
        let stub = crate::http::stub::Stub::start(&[("POST", "/scenarios", 201, "{}")]);

        Sonda::new(stub.url())
            .api_key(Some("secret".to_string()))
            .post_scenario(b"{}")
            .unwrap();

        assert_eq!(
            stub.requests()[0].header("authorization"),
            Some("Bearer secret")
        );
    }

    #[test]
    fn the_event_timeout_is_longer_than_the_server_own_30_seconds() {
        assert!(EVENT_TIMEOUT > Duration::from_secs(30), "{EVENT_TIMEOUT:?}");
    }

    #[test]
    fn timeout_makes_a_call_give_up_on_a_silent_server_sooner_than_ten_seconds() {
        let stub = crate::http::stub::Stub::start_silent(Duration::from_secs(20));
        let started = std::time::Instant::now();

        let error = Sonda::new(stub.url())
            .timeout(Duration::from_millis(300))
            .health()
            .unwrap_err();

        assert!(error.starts_with("cannot reach "), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }
}
