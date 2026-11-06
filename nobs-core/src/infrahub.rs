use std::fmt;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use ureq::{Agent, Body, RequestBuilder};

use crate::http::{self, Reply};
use crate::lab_vars::{BgpSession, Device, Interface, LabVars};

pub const DEFAULT_ADDRESS: &str = "http://localhost:8000";
pub const KINDS: [&str; 3] = [DEVICE, INTERFACE, SESSION];

const DEVICE: &str = "WorkshopDevice";
const INTERFACE: &str = "WorkshopInterface";
const SESSION: &str = "WorkshopBgpSession";
const CONTAINER_HOST: &str = "infrahub-server";
const TIMEOUT: Duration = Duration::from_secs(10);
const SCHEMA_LOAD_TIMEOUT: Duration = Duration::from_secs(120);
const RETRY: Duration = Duration::from_secs(1);
const SCHEMA_LOAD_PATH: &str = "/api/schema/load?branch=main";
const SCHEMA_PATH: &str = "/api/schema?branch=main";
const GRAPHQL_PATH: &str = "/graphql/main";
const BODY_CUT: usize = 200;
const INTENT_QUERY: &str = "query DeviceIntent($name: String!) { WorkshopDevice(name__value: $name) { edges { node { name { value } maintenance { value } site_name { value } role { value } bgp_sessions { edges { node { peer_address { value } afi_safi { value } expected_state { value } remote_as { value } expected_prefixes_received { value } reason { value } } } } } } } }";

type Field = (&'static str, Option<Value>);

pub fn host_address(infrahub: &str) -> String {
    if infrahub.contains(CONTAINER_HOST) {
        DEFAULT_ADDRESS.to_string()
    } else {
        infrahub.to_string()
    }
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
    Graphql(String),
    NotReady(Vec<String>),
    NotFound {
        kind: String,
        device: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Reach { url, reason } => write!(f, "cannot reach {url}: {reason}"),
            Error::Status { status, path, body } => {
                let body: String = body.chars().take(BODY_CUT).collect();
                write!(f, "Infrahub answered {status} on {path}: {body}")
            }
            Error::Graphql(message) => write!(f, "Infrahub: {message}"),
            Error::NotReady(kinds) => write!(
                f,
                "workshop schema kinds {} not visible to Infrahub after 30 s",
                kinds.join(", ")
            ),
            Error::NotFound { kind, device } => write!(f, "{kind} {device} not found in Infrahub."),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seed {
    pub rows: Vec<(String, u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Maintenance {
    pub previous: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIntent {
    pub name: String,
    pub maintenance: bool,
    pub site: Option<String>,
    pub role: Option<String>,
    pub sessions: Vec<IntentSession>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentSession {
    pub peer: String,
    pub afi_safi: Option<String>,
    pub expected_state: Option<String>,
    pub remote_as: Option<i64>,
    pub expected_prefixes: Option<i64>,
    pub reason: Option<String>,
}

pub struct Infrahub {
    address: String,
    token: String,
    agent: Agent,
}

struct Upsert {
    kind: &'static str,
    filters: Vec<(&'static str, Value)>,
    create_only: Vec<Field>,
    fields: Vec<Field>,
}

impl Infrahub {
    pub fn new(address: &str, token: &str) -> Self {
        Self {
            address: host_address(address).trim_end_matches('/').to_string(),
            token: token.to_string(),
            agent: http::agent(TIMEOUT),
        }
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn load_schema(&self, schema: &Value) -> Result<bool, Error> {
        let reply = self.post(
            SCHEMA_LOAD_PATH,
            &json!({ "schemas": [schema] }),
            SCHEMA_LOAD_TIMEOUT,
        )?;
        let answer = parse(&reply)?;
        Ok(at(&answer, "/schema_updated")?.as_bool().unwrap_or(false))
    }

    pub fn wait_for_kinds(&self, kinds: &[&str], limit: Duration) -> Result<(), Error> {
        let started = Instant::now();
        loop {
            let schema = parse(&self.get(SCHEMA_PATH)?)?;
            let missing = missing_kinds(&schema, kinds);
            if missing.is_empty() {
                return Ok(());
            }
            let remaining = limit.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err(Error::NotReady(missing));
            }
            thread::sleep(remaining.min(RETRY));
        }
    }

    pub fn seed(&self, lab: &LabVars, progress: &mut dyn FnMut(&str, bool)) -> Result<Seed, Error> {
        let mut rows = [(0, 0); 3];
        for device in &lab.devices {
            let (id, created) = self.upsert(device_row(device))?;
            tally(&mut rows[0], created);
            progress(&device.name, created);
            for interface in &device.interfaces {
                let (_, created) = self.upsert(interface_row(&id, interface))?;
                tally(&mut rows[1], created);
            }
            for session in &device.sessions {
                let (_, created) = self.upsert(session_row(&id, session))?;
                tally(&mut rows[2], created);
            }
        }
        Ok(Seed {
            rows: KINDS
                .iter()
                .zip(rows)
                .map(|(kind, (created, updated))| (kind.to_string(), created, updated))
                .collect(),
        })
    }

    pub fn maintenance(&self, kind: &str, device: &str, state: bool) -> Result<Maintenance, Error> {
        let filters = [("name__value", json!(device))];
        let node = self
            .find(kind, &filters, " maintenance { value }")?
            .ok_or_else(|| Error::NotFound {
                kind: kind.to_string(),
                device: device.to_string(),
            })?;
        let id = id_at(&node, "/id")?;
        self.mutate(
            &format!("{kind}Update"),
            &format!(
                "maintenance: {{value: {state}}}, id: {}",
                literal(&json!(id))
            ),
        )?;
        Ok(Maintenance {
            previous: node
                .pointer("/maintenance/value")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    pub fn device_intent(&self, device: &str) -> Result<Option<DeviceIntent>, Error> {
        let answer = self.graphql_with(INTENT_QUERY, &json!({ "name": device }))?;
        let edges = at(&answer, "/data/WorkshopDevice/edges")?;
        let Some(node) = edges.pointer("/0/node") else {
            return Ok(None);
        };
        Ok(Some(DeviceIntent {
            name: attribute_text(node, "name").unwrap_or_else(|| device.to_string()),
            maintenance: attribute_value(node, "maintenance")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            site: attribute_text(node, "site_name"),
            role: attribute_text(node, "role"),
            sessions: node
                .pointer("/bgp_sessions/edges")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|edge| intent_session(&edge["node"]))
                .collect(),
        }))
    }

    fn upsert(&self, row: Upsert) -> Result<(String, bool), Error> {
        match self.find(row.kind, &row.filters, "")? {
            Some(node) => {
                let id = id_at(&node, "/id")?;
                self.mutate(&format!("{}Update", row.kind), &update_data(&row, &id))?;
                Ok((id, false))
            }
            None => {
                let answer = self.mutate(&format!("{}Create", row.kind), &create_data(&row))?;
                let id = id_at(&answer, &format!("/data/{}Create/object/id", row.kind))?;
                Ok((id, true))
            }
        }
    }

    fn find(
        &self,
        kind: &str,
        filters: &[(&str, Value)],
        extra: &str,
    ) -> Result<Option<Value>, Error> {
        let arguments: String = filters
            .iter()
            .map(|(name, value)| format!("{name}: {}, ", literal(value)))
            .collect();
        let answer = self.graphql(&format!(
            "query {{ {kind}({arguments}offset: 0, limit: 50) {{ count edges {{ node {{ id{extra} }} }} }} }}"
        ))?;
        if at(&answer, &format!("/data/{kind}/count"))?.as_u64() == Some(0) {
            return Ok(None);
        }
        at(&answer, &format!("/data/{kind}/edges/0/node")).map(|node| Some(node.clone()))
    }

    fn mutate(&self, operation: &str, data: &str) -> Result<Value, Error> {
        self.graphql(&format!(
            "mutation {{ {operation}(data: {{{data}}}) {{ ok object {{ id }} }} }}"
        ))
    }

    fn graphql(&self, query: &str) -> Result<Value, Error> {
        self.graphql_body(&json!({ "query": query }))
    }

    fn graphql_with(&self, query: &str, variables: &Value) -> Result<Value, Error> {
        self.graphql_body(&json!({ "query": query, "variables": variables }))
    }

    fn graphql_body(&self, body: &Value) -> Result<Value, Error> {
        let answer = parse(&self.post(GRAPHQL_PATH, body, TIMEOUT)?)?;
        match answer
            .get("errors")
            .and_then(Value::as_array)
            .and_then(|e| e.first())
        {
            Some(first) => Err(Error::Graphql(
                first["message"]
                    .as_str()
                    .map_or_else(|| first.to_string(), str::to_string),
            )),
            None => Ok(answer),
        }
    }

    fn get(&self, path: &str) -> Result<Reply, Error> {
        let request = self.agent.get(format!("{}{path}", self.address));
        self.finish(path, self.with_headers(request).call())
    }

    fn post(&self, path: &str, body: &Value, timeout: Duration) -> Result<Reply, Error> {
        let request = self
            .agent
            .post(format!("{}{path}", self.address))
            .config()
            .timeout_global(Some(timeout))
            .build();
        self.finish(
            path,
            self.with_headers(request).send(body.to_string().as_bytes()),
        )
    }

    fn with_headers<B>(&self, request: RequestBuilder<B>) -> RequestBuilder<B> {
        request
            .header("X-INFRAHUB-KEY", self.token.as_str())
            .header("content-type", "application/json")
    }

    fn finish(
        &self,
        path: &str,
        sent: Result<ureq::http::Response<Body>, ureq::Error>,
    ) -> Result<Reply, Error> {
        let reply = http::read(&self.address, sent).map_err(|error| Error::Reach {
            url: self.address.clone(),
            reason: http::reason(error, &self.address),
        })?;
        if reply.status == 200 {
            Ok(reply)
        } else {
            Err(Error::Status {
                status: reply.status,
                path: path.to_string(),
                body: reply.body,
            })
        }
    }
}

fn tally(row: &mut (u32, u32), created: bool) {
    if created {
        row.0 += 1;
    } else {
        row.1 += 1;
    }
}

fn parse(reply: &Reply) -> Result<Value, Error> {
    serde_json::from_str(&reply.body)
        .map_err(|_| Error::Graphql("the reply is not JSON".to_string()))
}

fn at<'a>(answer: &'a Value, pointer: &str) -> Result<&'a Value, Error> {
    answer
        .pointer(pointer)
        .ok_or_else(|| Error::Graphql(format!("the reply has no {pointer}")))
}

fn id_at(answer: &Value, pointer: &str) -> Result<String, Error> {
    Ok(at(answer, pointer)?
        .as_str()
        .unwrap_or_default()
        .to_string())
}

fn attribute_value<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node.get(name)?.get("value")
}

fn attribute_text(node: &Value, name: &str) -> Option<String> {
    attribute_value(node, name)?.as_str().map(str::to_string)
}

fn intent_session(node: &Value) -> IntentSession {
    IntentSession {
        peer: attribute_text(node, "peer_address").unwrap_or_default(),
        afi_safi: attribute_text(node, "afi_safi"),
        expected_state: attribute_text(node, "expected_state"),
        remote_as: attribute_value(node, "remote_as").and_then(Value::as_i64),
        expected_prefixes: attribute_value(node, "expected_prefixes_received")
            .and_then(Value::as_i64),
        reason: attribute_text(node, "reason"),
    }
}

fn missing_kinds(schema: &Value, kinds: &[&str]) -> Vec<String> {
    let present: Vec<String> = schema["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|node| {
            format!(
                "{}{}",
                node["namespace"].as_str().unwrap_or_default(),
                node["name"].as_str().unwrap_or_default()
            )
        })
        .collect();
    kinds
        .iter()
        .filter(|kind| !present.iter().any(|name| name == *kind))
        .map(|kind| kind.to_string())
        .collect()
}

fn attribute(name: &'static str, value: impl Into<Option<Value>>) -> Field {
    (name, value.into().map(|value| json!({ "value": value })))
}

fn parent(device_id: &str) -> Field {
    ("device", Some(json!({ "id": device_id })))
}

fn device_row(device: &Device) -> Upsert {
    Upsert {
        kind: DEVICE,
        filters: vec![("name__value", json!(device.name))],
        create_only: vec![attribute("name", json!(device.name))],
        fields: vec![
            attribute("site_name", json!(device.site)),
            attribute("role", json!(device.role)),
            attribute("asn", device.asn.map(Value::from)),
            attribute("maintenance", json!(device.maintenance)),
        ],
    }
}

fn interface_row(device_id: &str, interface: &Interface) -> Upsert {
    Upsert {
        kind: INTERFACE,
        filters: vec![
            ("name__value", json!(interface.name)),
            ("device__ids", json!([device_id])),
        ],
        create_only: vec![parent(device_id)],
        fields: vec![
            attribute("name", json!(interface.name)),
            attribute("role", json!(interface.role)),
            attribute("ip_address", interface.ip.clone().map(Value::from)),
            attribute("expected_state", json!(interface.expected_state)),
        ],
    }
}

fn session_row(device_id: &str, session: &BgpSession) -> Upsert {
    Upsert {
        kind: SESSION,
        filters: vec![
            ("peer_address__value", json!(session.peer)),
            ("device__ids", json!([device_id])),
        ],
        create_only: vec![parent(device_id)],
        fields: vec![
            attribute("peer_address", json!(session.peer)),
            attribute("remote_as", session.remote_as.map(Value::from)),
            attribute("afi_safi", json!(session.afi_safi)),
            attribute("expected_state", json!(session.expected_state)),
            attribute(
                "expected_prefixes_received",
                session.expected_prefixes.map(Value::from),
            ),
            attribute("reason", session.reason.clone().map(Value::from)),
        ],
    }
}

fn create_data(row: &Upsert) -> String {
    row.create_only
        .iter()
        .chain(&row.fields)
        .filter_map(|(name, value)| Some(format!("{name}: {}", literal(value.as_ref()?))))
        .collect::<Vec<_>>()
        .join(", ")
}

fn update_data(row: &Upsert, id: &str) -> String {
    row.fields
        .iter()
        .map(|(name, value)| {
            let value = value
                .clone()
                .unwrap_or_else(|| json!({ "value": Value::Null }));
            format!("{name}: {}", literal(&value))
        })
        .chain([format!("id: {}", literal(&json!(id)))])
        .collect::<Vec<_>>()
        .join(", ")
}

fn literal(value: &Value) -> String {
    match value {
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(key, value)| format!("{key}: {}", literal(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(literal).collect::<Vec<_>>().join(", ")
        ),
        scalar => scalar.to_string(),
    }
}

fn quoted(text: &str) -> String {
    let mut escaped = String::new();
    for character in Value::String(text.to_string()).to_string().chars() {
        if character.is_ascii() && character != '\u{7f}' {
            escaped.push(character);
            continue;
        }
        for unit in character.encode_utf16(&mut [0; 2]) {
            escaped.push_str(&format!("\\u{unit:04x}"));
        }
    }
    escaped
}

pub fn read_schema(text: &str) -> Result<Value, String> {
    serde_saphyr::from_str(text).map_err(|error| error.to_string())
}

pub fn annotation(device: &str, state: bool, sink: &str) -> String {
    let flag = if state { "True" } else { "False" };
    let message = quoted(&format!(
        "Configured from CLI: {device}.maintenance = {flag}"
    ));
    let device = quoted(device);
    let sink = quoted(sink.trim_end_matches('/'));
    format!(
        r#"{{"signal_type": "logs", "labels": {{"device": {device}, "source": "workshop-trigger", "event": "config-push", "level": "info"}}, "log": {{"severity": "info", "message": {message}, "fields": {{}}}}, "encoder": {{"type": "json_lines"}}, "sink": {{"type": "loki", "url": {sink}}}}}"#
    )
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;
    use crate::http::stub::Stub;
    use crate::lab_vars;

    const TOKEN: &str = "nobs-t-token";
    const LAB: &str = include_str!("../../tests/fixtures/workshop-one/lab_vars.yml");
    const FOUND_KINDS: &str = r#"{"nodes":[{"namespace":"Workshop","name":"Device"},{"namespace":"Workshop","name":"Interface"},{"namespace":"Workshop","name":"BgpSession"},{"namespace":"Core","name":"Account"}]}"#;
    const WITHOUT_SESSIONS: &str = r#"{"nodes":[{"namespace":"Workshop","name":"Device"},{"namespace":"Workshop","name":"Interface"}]}"#;

    fn lab() -> LabVars {
        lab_vars::read(LAB).unwrap()
    }

    fn closed_port_url() -> String {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        format!("http://127.0.0.1:{port}")
    }

    fn graphql_stub(answers: &[String]) -> Stub {
        let routes: Vec<(&str, &str, u16, &str)> = answers
            .iter()
            .map(|answer| ("POST", GRAPHQL_PATH, 200, answer.as_str()))
            .collect();
        Stub::start(&routes)
    }

    fn filter_answer(kind: &str, id: Option<&str>) -> String {
        match id {
            Some(id) => format!(
                r#"{{"data":{{"{kind}":{{"count":1,"edges":[{{"node":{{"id":"{id}","maintenance":{{"value":false}}}}}}]}}}}}}"#
            ),
            None => format!(r#"{{"data":{{"{kind}":{{"count":0,"edges":[]}}}}}}"#),
        }
    }

    fn mutation_answer(operation: &str, id: &str) -> String {
        format!(r#"{{"data":{{"{operation}":{{"ok":true,"object":{{"id":"{id}"}}}}}}}}"#)
    }

    fn seed_answers(found: bool) -> Vec<String> {
        let mut answers = Vec::new();
        for device in 0..2 {
            for (kind, rows) in [(DEVICE, 1), (INTERFACE, 4), (SESSION, 3)] {
                for row in 0..rows {
                    let id = if kind == DEVICE {
                        format!("dev-{device}")
                    } else {
                        format!("{kind}-{device}-{row}")
                    };
                    let (operation, existing) = if found {
                        (format!("{kind}Update"), Some(id.as_str()))
                    } else {
                        (format!("{kind}Create"), None)
                    };
                    answers.push(filter_answer(kind, existing));
                    answers.push(mutation_answer(&operation, &id));
                }
            }
        }
        answers
    }

    fn queries(stub: &Stub) -> Vec<String> {
        stub.requests()
            .iter()
            .filter(|request| request.path == GRAPHQL_PATH)
            .map(|request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                body["query"].as_str().unwrap().to_string()
            })
            .collect()
    }

    fn mutations(stub: &Stub) -> Vec<String> {
        queries(stub)
            .into_iter()
            .filter(|query| query.starts_with("mutation"))
            .collect()
    }

    fn root(query: &str) -> &str {
        query.split(['{', '(']).nth(1).unwrap().trim()
    }

    fn seeded(found: bool) -> (Stub, Seed, Vec<(String, bool)>) {
        let stub = graphql_stub(&seed_answers(found));
        let mut progress = Vec::new();
        let seed = Infrahub::new(stub.url(), TOKEN)
            .seed(&lab(), &mut |name, created| {
                progress.push((name.to_string(), created))
            })
            .unwrap();
        (stub, seed, progress)
    }

    #[test]
    fn host_address_swaps_the_container_name_for_localhost() {
        assert_eq!(
            host_address("http://infrahub-server:8000"),
            "http://localhost:8000"
        );
        assert_eq!(
            host_address("http://infrahub-server:8000/"),
            "http://localhost:8000"
        );
    }

    #[test]
    fn host_address_gives_localhost_8000_whatever_port_the_container_name_had() {
        assert_eq!(
            host_address("http://infrahub-server:9999"),
            "http://localhost:8000"
        );
    }

    #[test]
    fn host_address_returns_any_other_address_unchanged() {
        assert_eq!(host_address("http://10.0.0.5:8000"), "http://10.0.0.5:8000");
        assert_eq!(
            host_address("http://localhost:8000"),
            "http://localhost:8000"
        );
    }

    #[test]
    fn new_keeps_the_host_form_of_the_address_without_a_trailing_slash() {
        assert_eq!(
            Infrahub::new("http://infrahub-server:8000", TOKEN).address(),
            "http://localhost:8000"
        );
        assert_eq!(
            Infrahub::new("http://10.0.0.5:8000//", TOKEN).address(),
            "http://10.0.0.5:8000"
        );
    }

    #[test]
    fn every_request_carries_the_key_and_the_json_content_type() {
        let stub = Stub::start(&[
            ("POST", SCHEMA_LOAD_PATH, 200, r#"{"schema_updated":true}"#),
            ("GET", SCHEMA_PATH, 200, FOUND_KINDS),
            (
                "POST",
                GRAPHQL_PATH,
                200,
                &filter_answer(DEVICE, Some("d1")),
            ),
            (
                "POST",
                GRAPHQL_PATH,
                200,
                &mutation_answer("WorkshopDeviceUpdate", "d1"),
            ),
        ]);
        let infrahub = Infrahub::new(stub.url(), TOKEN);

        infrahub.load_schema(&json!({})).unwrap();
        infrahub.wait_for_kinds(&KINDS, Duration::ZERO).unwrap();
        infrahub.maintenance(DEVICE, "srl1", true).unwrap();

        let requests = stub.requests();
        assert_eq!(requests.len(), 4);
        for request in requests {
            assert_eq!(request.header("X-INFRAHUB-KEY"), Some(TOKEN));
            assert_eq!(request.header("content-type"), Some("application/json"));
        }
    }

    #[test]
    fn load_schema_posts_the_schema_in_a_list_and_returns_schema_updated() {
        let stub = Stub::start(&[
            ("POST", SCHEMA_LOAD_PATH, 200, r#"{"schema_updated":true}"#),
            ("POST", SCHEMA_LOAD_PATH, 200, r#"{"schema_updated":false}"#),
        ]);
        let infrahub = Infrahub::new(stub.url(), TOKEN);
        let schema = json!({"version": "1.0", "nodes": []});

        assert_eq!(infrahub.load_schema(&schema), Ok(true));
        assert_eq!(infrahub.load_schema(&schema), Ok(false));

        let requests = stub.requests();
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/api/schema/load?branch=main");
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body, json!({"schemas": [schema]}));
    }

    #[test]
    fn wait_for_kinds_returns_at_once_when_the_kinds_are_in_nodes() {
        let stub = Stub::start(&[("GET", SCHEMA_PATH, 200, FOUND_KINDS)]);

        let result =
            Infrahub::new(stub.url(), TOKEN).wait_for_kinds(&KINDS, Duration::from_secs(5));

        assert_eq!(result, Ok(()));
        assert_eq!(stub.requests().len(), 1);
    }

    #[test]
    fn wait_for_kinds_polls_again_while_a_kind_is_missing() {
        let stub = Stub::start(&[
            ("GET", SCHEMA_PATH, 200, WITHOUT_SESSIONS),
            ("GET", SCHEMA_PATH, 200, FOUND_KINDS),
        ]);

        let result =
            Infrahub::new(stub.url(), TOKEN).wait_for_kinds(&KINDS, Duration::from_secs(5));

        assert_eq!(result, Ok(()));
        assert_eq!(stub.requests().len(), 2);
    }

    #[test]
    fn wait_for_kinds_names_the_missing_kinds_after_the_limit() {
        let stub = Stub::start(&[("GET", SCHEMA_PATH, 200, WITHOUT_SESSIONS)]);

        let result =
            Infrahub::new(stub.url(), TOKEN).wait_for_kinds(&KINDS, Duration::from_millis(100));

        assert_eq!(result, Err(Error::NotReady(vec![SESSION.to_string()])));
        assert!(stub.requests().len() >= 2);
    }

    #[test]
    fn wait_for_kinds_gives_up_at_once_on_a_status_that_is_not_200() {
        let stub = Stub::start(&[("GET", SCHEMA_PATH, 401, "no")]);

        let result =
            Infrahub::new(stub.url(), TOKEN).wait_for_kinds(&KINDS, Duration::from_secs(5));

        assert!(matches!(result, Err(Error::Status { status: 401, .. })));
        assert_eq!(stub.requests().len(), 1);
    }

    #[test]
    fn seed_on_an_empty_infrahub_creates_everything_in_the_order_of_the_file() {
        let (stub, seed, _) = seeded(false);

        let roots: Vec<String> = mutations(&stub)
            .iter()
            .map(|query| root(query).to_string())
            .collect();
        let one_device = [
            vec!["WorkshopDeviceCreate"],
            vec!["WorkshopInterfaceCreate"; 4],
            vec!["WorkshopBgpSessionCreate"; 3],
        ]
        .concat();
        assert_eq!(roots, [one_device.clone(), one_device].concat());
        assert_eq!(
            seed.rows,
            vec![
                (DEVICE.to_string(), 2, 0),
                (INTERFACE.to_string(), 8, 0),
                (SESSION.to_string(), 6, 0),
            ]
        );
    }

    #[test]
    fn a_create_carries_the_device_id_the_stub_answered_and_leaves_out_an_absent_field() {
        let (stub, _, _) = seeded(false);
        let created = mutations(&stub);

        assert!(!created[0].contains("device:"), "{}", created[0]);
        assert!(created[0].contains(r#"name: {value: "srl1"}"#));
        for query in &created[1..8] {
            assert!(query.contains(r#"device: {id: "dev-0"}"#), "{query}");
        }
        for query in &created[9..16] {
            assert!(query.contains(r#"device: {id: "dev-1"}"#), "{query}");
        }
        let third_session = &created[7];
        assert!(third_session.contains(r#"reason: {value: "ip-mismatch-demo"}"#));
        assert!(!third_session.contains("expected_prefixes_received"));
        assert!(created[5].contains("expected_prefixes_received: {value: 10}"));
        assert!(!created[5].contains("reason"));
    }

    #[test]
    fn the_filter_queries_use_the_name_the_peer_address_and_the_device_ids() {
        let (stub, _, _) = seeded(false);
        let filters: Vec<String> = queries(&stub)
            .into_iter()
            .filter(|query| query.starts_with("query"))
            .collect();

        assert!(filters[0].contains(r#"WorkshopDevice(name__value: "srl1", offset: 0, limit: 50)"#));
        assert!(filters[1].contains(
            r#"WorkshopInterface(name__value: "ethernet-1/1", device__ids: ["dev-0"], offset: 0, limit: 50)"#
        ));
        assert!(filters[5].contains(
            r#"WorkshopBgpSession(peer_address__value: "10.1.2.2", device__ids: ["dev-0"], offset: 0, limit: 50)"#
        ));
        assert!(filters[0].contains("count edges { node { id } }"));
    }

    #[test]
    fn seed_on_a_filled_infrahub_updates_with_the_found_id_and_never_sends_device() {
        let (stub, seed, _) = seeded(true);
        let updated = mutations(&stub);

        assert_eq!(updated.len(), 16);
        for query in &updated {
            assert!(root(query).ends_with("Update"), "{query}");
            assert!(!query.contains("device:"), "{query}");
        }
        assert!(updated[0].contains(r#"id: "dev-0""#));
        assert!(
            !updated[0].contains(r#"name: {value: "srl1"}"#),
            "{}",
            updated[0]
        );
        assert!(updated[1].contains(r#"id: "WorkshopInterface-0-0""#));
        assert!(updated[1].contains(r#"name: {value: "ethernet-1/1"}"#));
        assert_eq!(
            seed.rows,
            vec![
                (DEVICE.to_string(), 0, 2),
                (INTERFACE.to_string(), 0, 8),
                (SESSION.to_string(), 0, 6),
            ]
        );
    }

    #[test]
    fn an_update_sends_null_for_an_absent_field() {
        let (stub, _, _) = seeded(true);
        let updated = mutations(&stub);

        assert!(updated[7].contains(r#"reason: {value: "ip-mismatch-demo"}"#));
        assert!(updated[7].contains("expected_prefixes_received: {value: null}"));
        assert!(updated[5].contains("reason: {value: null}"));
    }

    #[test]
    fn the_progress_callback_sees_each_device_right_after_its_own_upsert() {
        let stub = graphql_stub(&seed_answers(false));
        let mut seen = Vec::new();

        Infrahub::new(stub.url(), TOKEN)
            .seed(&lab(), &mut |name, created| {
                seen.push((name.to_string(), created, stub.requests().len()))
            })
            .unwrap();

        assert_eq!(
            seen,
            [
                ("srl1".to_string(), true, 2),
                ("srl2".to_string(), true, 18)
            ]
        );
    }

    #[test]
    fn maintenance_reads_the_previous_value_and_updates_with_the_id_and_the_new_state() {
        let stub = graphql_stub(&[
            filter_answer(DEVICE, Some("dev-9")),
            mutation_answer("WorkshopDeviceUpdate", "dev-9"),
        ]);

        let result = Infrahub::new(stub.url(), TOKEN).maintenance(DEVICE, "srl1", true);

        assert_eq!(result, Ok(Maintenance { previous: false }));
        let sent = queries(&stub);
        assert!(sent[0].contains(r#"WorkshopDevice(name__value: "srl1""#));
        assert!(sent[0].contains("maintenance { value }"));
        assert_eq!(
            sent[1],
            r#"mutation { WorkshopDeviceUpdate(data: {maintenance: {value: true}, id: "dev-9"}) { ok object { id } } }"#
        );
    }

    #[test]
    fn maintenance_returns_the_stored_state_as_previous_and_counts_null_as_false() {
        for (stored, previous) in [("true", true), ("false", false), ("null", false)] {
            let found = format!(
                r#"{{"data":{{"Other":{{"count":1,"edges":[{{"node":{{"id":"x","maintenance":{{"value":{stored}}}}}}}]}}}}}}"#
            );
            let stub = graphql_stub(&[found, mutation_answer("OtherUpdate", "x")]);

            let result = Infrahub::new(stub.url(), TOKEN).maintenance("Other", "srl1", false);

            assert_eq!(result, Ok(Maintenance { previous }), "{stored}");
            assert_eq!(root(&queries(&stub)[1]), "OtherUpdate");
        }
    }

    #[test]
    fn maintenance_for_a_device_infrahub_does_not_have_is_not_found_and_updates_nothing() {
        let stub = graphql_stub(&[filter_answer(DEVICE, None)]);

        let result = Infrahub::new(stub.url(), TOKEN).maintenance(DEVICE, "srl9", true);

        assert_eq!(
            result,
            Err(Error::NotFound {
                kind: DEVICE.to_string(),
                device: "srl9".to_string()
            })
        );
        assert_eq!(stub.requests().len(), 1);
    }

    #[test]
    fn a_status_that_is_not_200_is_a_status_error_with_the_path_and_the_body() {
        let stub = Stub::start(&[("POST", SCHEMA_LOAD_PATH, 401, "bad token")]);

        let result = Infrahub::new(stub.url(), TOKEN).load_schema(&json!({}));

        assert_eq!(
            result,
            Err(Error::Status {
                status: 401,
                path: "/api/schema/load?branch=main".to_string(),
                body: "bad token".to_string()
            })
        );
    }

    #[test]
    fn a_200_with_errors_is_a_graphql_error_with_the_first_message() {
        let stub = graphql_stub(&[
            r#"{"errors":[{"message":"Cannot query field"},{"message":"second"}]}"#.to_string(),
        ]);

        let result = Infrahub::new(stub.url(), TOKEN).maintenance(DEVICE, "srl1", true);

        assert_eq!(
            result,
            Err(Error::Graphql("Cannot query field".to_string()))
        );
    }

    #[test]
    fn a_reply_that_is_not_json_or_lacks_the_data_is_a_graphql_error() {
        let stub = graphql_stub(&["not json".to_string(), r#"{"data":{}}"#.to_string()]);
        let infrahub = Infrahub::new(stub.url(), TOKEN);

        assert_eq!(
            infrahub.maintenance(DEVICE, "srl1", true),
            Err(Error::Graphql("the reply is not JSON".to_string()))
        );
        assert_eq!(
            infrahub.maintenance(DEVICE, "srl1", true),
            Err(Error::Graphql(
                "the reply has no /data/WorkshopDevice/count".to_string()
            ))
        );
    }

    #[test]
    fn no_listener_is_a_reach_error_with_the_address_and_a_reason_without_the_prefix() {
        let url = closed_port_url();

        match Infrahub::new(&url, TOKEN).load_schema(&json!({})) {
            Err(Error::Reach { url: seen, reason }) => {
                assert_eq!(seen, url);
                assert!(!reason.is_empty());
                assert!(!reason.starts_with("cannot reach"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn load_schema_waits_for_an_answer_that_comes_after_eleven_seconds() {
        let stub = Stub::start_after(
            Duration::from_secs(11),
            &[("POST", SCHEMA_LOAD_PATH, 200, r#"{"schema_updated":true}"#)],
        );

        let result = Infrahub::new(stub.url(), TOKEN).load_schema(&json!({}));

        assert_eq!(result, Ok(true));
    }

    #[test]
    fn a_graphql_call_gives_up_on_an_answer_that_comes_after_ten_seconds() {
        let stub = Stub::start_after(
            Duration::from_secs(12),
            &[("POST", GRAPHQL_PATH, 200, "{}")],
        );

        let result = Infrahub::new(stub.url(), TOKEN).maintenance(DEVICE, "srl1", true);

        assert!(matches!(result, Err(Error::Reach { .. })), "{result:?}");
    }

    #[test]
    fn wait_for_kinds_gives_up_on_a_schema_answer_that_comes_after_ten_seconds() {
        let stub = Stub::start_after(
            Duration::from_secs(11),
            &[("GET", SCHEMA_PATH, 200, FOUND_KINDS)],
        );

        let result =
            Infrahub::new(stub.url(), TOKEN).wait_for_kinds(&KINDS, Duration::from_secs(30));

        assert!(matches!(result, Err(Error::Reach { .. })), "{result:?}");
    }

    #[test]
    fn the_error_texts_are_the_ones_the_commands_print() {
        assert_eq!(
            Error::Reach {
                url: "http://localhost:8000".to_string(),
                reason: "connection refused".to_string()
            }
            .to_string(),
            "cannot reach http://localhost:8000: connection refused"
        );
        assert_eq!(
            Error::Status {
                status: 401,
                path: "/api/schema/load?branch=main".to_string(),
                body: "no".to_string()
            }
            .to_string(),
            "Infrahub answered 401 on /api/schema/load?branch=main: no"
        );
        assert_eq!(
            Error::Graphql("boom".to_string()).to_string(),
            "Infrahub: boom"
        );
        assert_eq!(
            Error::NotReady(vec!["A".to_string(), "B".to_string()]).to_string(),
            "workshop schema kinds A, B not visible to Infrahub after 30 s"
        );
        assert_eq!(
            Error::NotFound {
                kind: DEVICE.to_string(),
                device: "srl1".to_string()
            }
            .to_string(),
            "WorkshopDevice srl1 not found in Infrahub."
        );
    }

    #[test]
    fn the_status_text_cuts_the_body_at_200_characters() {
        let text = Error::Status {
            status: 500,
            path: "/p".to_string(),
            body: "x".repeat(300),
        }
        .to_string();

        assert_eq!(
            text,
            format!("Infrahub answered 500 on /p: {}", "x".repeat(200))
        );
    }

    fn python_body(state: &str) -> Value {
        json!({
            "signal_type": "logs",
            "labels": {
                "device": "srl1",
                "source": "workshop-trigger",
                "event": "config-push",
                "level": "info"
            },
            "log": {
                "severity": "info",
                "message": format!("Configured from CLI: srl1.maintenance = {state}"),
                "fields": {}
            },
            "encoder": {"type": "json_lines"},
            "sink": {"type": "loki", "url": "http://loki:3001"}
        })
    }

    #[test]
    fn annotation_is_the_body_the_python_sends() {
        let text = annotation("srl1", true, "http://loki:3001");
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            python_body("True")
        );
        assert_eq!(
            text,
            r#"{"signal_type": "logs", "labels": {"device": "srl1", "source": "workshop-trigger", "event": "config-push", "level": "info"}, "log": {"severity": "info", "message": "Configured from CLI: srl1.maintenance = True", "fields": {}}, "encoder": {"type": "json_lines"}, "sink": {"type": "loki", "url": "http://loki:3001"}}"#
        );
    }

    #[test]
    fn annotation_for_false_says_false() {
        let text = annotation("srl1", false, "http://loki:3001");
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            python_body("False")
        );
        assert!(text.contains("srl1.maintenance = False"));
    }

    #[test]
    fn annotation_drops_the_trailing_slash_of_the_sink() {
        let text = annotation("srl1", true, "http://loki:3001/");
        assert!(text.ends_with(r#""url": "http://loki:3001"}}"#), "{text}");
    }

    #[test]
    fn annotation_writes_every_non_ascii_character_as_utf16_escapes_like_python() {
        let text = annotation("é😀\u{7f}", true, "http://loki:3001");

        assert!(text.is_ascii(), "{text}");
        let escaped: String = [0xe9_u16, 0xd83d, 0xde00, 0x7f]
            .iter()
            .map(|unit| format!("{}u{unit:04x}", '\\'))
            .collect();
        assert!(
            text.contains(&format!(r#""device": "{escaped}""#)),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                r#""message": "Configured from CLI: {escaped}.maintenance = True""#
            )),
            "{text}"
        );
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["labels"]["device"], "é😀\u{7f}");
    }

    #[test]
    fn annotation_escapes_the_device_name_wherever_it_appears() {
        let text = annotation("a\"b", true, "http://loki:3001");
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["labels"]["device"], "a\"b");
        assert_eq!(
            value["log"]["message"],
            "Configured from CLI: a\"b.maintenance = True"
        );
    }

    const INTENT_REPLY: &str = r#"{"data":{"WorkshopDevice":{"edges":[{"node":{"name":{"value":"srl1"},"maintenance":{"value":false},"site_name":{"value":"lab"},"role":{"value":"edge"},"bgp_sessions":{"edges":[{"node":{"peer_address":{"value":"10.1.99.2"},"afi_safi":{"value":"ipv4-unicast"},"expected_state":{"value":"established"},"remote_as":{"value":65102},"reason":{"value":"ip-mismatch-demo"}}}]}}}]}}}"#;

    fn sent_bodies(stub: &Stub) -> Vec<Value> {
        stub.requests()
            .iter()
            .map(|request| serde_json::from_slice(&request.body).unwrap())
            .collect()
    }

    #[test]
    fn device_intent_sends_one_post_with_the_key_the_fields_and_the_name_as_a_variable() {
        let stub = graphql_stub(&[INTENT_REPLY.to_string()]);

        Infrahub::new(stub.url(), TOKEN)
            .device_intent("srl1")
            .unwrap();

        let requests = stub.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, GRAPHQL_PATH);
        assert_eq!(requests[0].header("X-INFRAHUB-KEY"), Some(TOKEN));
        let body = &sent_bodies(&stub)[0];
        let query = body["query"].as_str().unwrap();
        for word in [
            "$name: String!",
            "WorkshopDevice(name__value: $name)",
            "maintenance { value }",
            "site_name { value }",
            "role { value }",
            "bgp_sessions",
            "peer_address { value }",
            "afi_safi { value }",
            "expected_state { value }",
            "remote_as { value }",
            "expected_prefixes_received { value }",
            "reason { value }",
        ] {
            assert!(query.contains(word), "{word}");
        }
        assert_eq!(body["variables"], json!({"name": "srl1"}));
    }

    #[test]
    fn device_intent_reads_the_device_and_its_session_from_the_reply() {
        let stub = graphql_stub(&[INTENT_REPLY.to_string()]);

        let intent = Infrahub::new(stub.url(), TOKEN).device_intent("srl1");

        assert_eq!(
            intent,
            Ok(Some(DeviceIntent {
                name: "srl1".to_string(),
                maintenance: false,
                site: Some("lab".to_string()),
                role: Some("edge".to_string()),
                sessions: vec![IntentSession {
                    peer: "10.1.99.2".to_string(),
                    afi_safi: Some("ipv4-unicast".to_string()),
                    expected_state: Some("established".to_string()),
                    remote_as: Some(65102),
                    expected_prefixes: None,
                    reason: Some("ip-mismatch-demo".to_string()),
                }],
            }))
        );
    }

    #[test]
    fn device_intent_reads_maintenance_as_it_is_stored_and_null_as_false() {
        for (stored, maintenance) in [("true", true), ("false", false), ("null", false)] {
            let reply = format!(
                r#"{{"data":{{"WorkshopDevice":{{"edges":[{{"node":{{"name":{{"value":"srl1"}},"maintenance":{{"value":{stored}}}}}}}]}}}}}}"#
            );
            let stub = graphql_stub(&[reply]);

            let intent = Infrahub::new(stub.url(), TOKEN)
                .device_intent("srl1")
                .unwrap()
                .unwrap();

            assert_eq!(intent.maintenance, maintenance, "{stored}");
        }
    }

    #[test]
    fn device_intent_gives_none_for_every_null_or_missing_field() {
        let reply = r#"{"data":{"WorkshopDevice":{"edges":[{"node":{"name":{"value":"srl1"},"maintenance":{"value":false},"site_name":{"value":null},"bgp_sessions":{"edges":[{"node":{"peer_address":{"value":"10.1.99.2"},"afi_safi":{"value":null},"expected_state":{"value":null},"remote_as":{"value":null},"expected_prefixes_received":{"value":10},"reason":{"value":null}}}]}}}]}}}"#;
        let stub = graphql_stub(&[reply.to_string()]);

        let intent = Infrahub::new(stub.url(), TOKEN)
            .device_intent("srl1")
            .unwrap()
            .unwrap();

        assert_eq!((intent.site, intent.role), (None, None));
        assert_eq!(
            intent.sessions,
            vec![IntentSession {
                peer: "10.1.99.2".to_string(),
                afi_safi: None,
                expected_state: None,
                remote_as: None,
                expected_prefixes: Some(10),
                reason: None,
            }]
        );
    }

    #[test]
    fn device_intent_for_a_device_with_no_edges_is_none() {
        let stub = graphql_stub(&[r#"{"data":{"WorkshopDevice":{"edges":[]}}}"#.to_string()]);

        assert_eq!(
            Infrahub::new(stub.url(), TOKEN).device_intent("srl9"),
            Ok(None)
        );
    }

    #[test]
    fn device_intent_with_a_200_that_has_errors_is_a_graphql_error() {
        let stub = graphql_stub(&[r#"{"errors":[{"message":"Cannot query field"}]}"#.to_string()]);

        assert_eq!(
            Infrahub::new(stub.url(), TOKEN).device_intent("srl1"),
            Err(Error::Graphql("Cannot query field".to_string()))
        );
    }

    #[test]
    fn device_intent_with_a_reply_that_has_no_edges_list_is_a_graphql_error() {
        let stub = graphql_stub(&[r#"{"data":{}}"#.to_string()]);

        assert_eq!(
            Infrahub::new(stub.url(), TOKEN).device_intent("srl1"),
            Err(Error::Graphql(
                "the reply has no /data/WorkshopDevice/edges".to_string()
            ))
        );
    }

    #[test]
    fn device_intent_with_a_status_that_is_not_200_is_a_status_error() {
        let stub = Stub::start(&[("POST", GRAPHQL_PATH, 500, "down")]);

        assert_eq!(
            Infrahub::new(stub.url(), TOKEN).device_intent("srl1"),
            Err(Error::Status {
                status: 500,
                path: GRAPHQL_PATH.to_string(),
                body: "down".to_string()
            })
        );
    }

    #[test]
    fn device_intent_on_a_closed_port_is_a_reach_error() {
        let url = closed_port_url();

        assert!(matches!(
            Infrahub::new(&url, TOKEN).device_intent("srl1"),
            Err(Error::Reach { .. })
        ));
    }

    #[test]
    fn read_schema_gives_the_yaml_as_json_and_an_error_for_text_that_is_not_yaml() {
        assert_eq!(
            read_schema("version: '1.0'\nnodes:\n  - name: Device\n    namespace: Workshop\n"),
            Ok(json!({"version": "1.0", "nodes": [{"name": "Device", "namespace": "Workshop"}]}))
        );
        assert!(read_schema("nodes: [").is_err());
    }
}
