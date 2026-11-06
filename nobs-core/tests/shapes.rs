use std::collections::BTreeSet;
use std::fs;
use std::time::Duration;

use nobs_core::infrahub::{Infrahub, KINDS};
use nobs_core::lab_vars;
use serde_json::{json, Map, Value};

#[path = "../../tests/support/stub.rs"]
mod stub;

use stub::{Seen, Stub};

const FILES: [&str; 2] = ["infrahub-1.1.jsonl", "infrahub-1.11.2.jsonl"];
const TOKEN: &str = "<token>";
const GRAPHQL: &str = "/graphql/main";
const LAB: &str = include_str!("../../tests/fixtures/workshop-one/lab_vars.yml");
const SCHEMA: &str = include_str!("../../tests/fixtures/workshop-one/infrahub/schema.yml");

struct Group {
    call: Vec<String>,
    sent: Vec<Seen>,
    captured: Vec<Value>,
}

#[derive(Debug)]
struct Operation {
    kind: String,
    field: String,
    arguments: Map<String, Value>,
    selection: BTreeSet<String>,
}

struct Text<'a> {
    rest: &'a str,
    variables: &'a Value,
}

impl Text<'_> {
    fn skip(&mut self) {
        self.rest = self
            .rest
            .trim_start_matches(|c: char| c.is_whitespace() || c == ',');
    }

    fn eat(&mut self, expected: char) -> bool {
        self.skip();
        match self.rest.strip_prefix(expected) {
            Some(rest) => {
                self.rest = rest;
                true
            }
            None => false,
        }
    }

    fn expect(&mut self, expected: char) {
        assert!(
            self.eat(expected),
            "expected {expected} before {:.60}",
            self.rest
        );
    }

    fn name(&mut self) -> String {
        self.skip();
        let end = self
            .rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(self.rest.len());
        let (name, rest) = self.rest.split_at(end);
        assert!(!name.is_empty(), "expected a name before {:.60}", self.rest);
        self.rest = rest;
        name.to_string()
    }

    fn value(&mut self) -> Value {
        if self.eat('$') {
            let name = self.name();
            return self.variables[name.as_str()].clone();
        }
        if self.eat('{') {
            let mut map = Map::new();
            while !self.eat('}') {
                let key = self.name();
                self.expect(':');
                map.insert(key, self.value());
            }
            return Value::Object(map);
        }
        if self.eat('[') {
            let mut items = Vec::new();
            while !self.eat(']') {
                items.push(self.value());
            }
            return Value::Array(items);
        }
        let end = if self.rest.starts_with('"') {
            let mut stream = serde_json::Deserializer::from_str(self.rest).into_iter::<String>();
            stream.next().expect("a string").expect("a plain string");
            stream.byte_offset()
        } else {
            self.rest
                .find(|c: char| c.is_whitespace() || ",)}]".contains(c))
                .unwrap_or(self.rest.len())
        };
        let (token, rest) = self.rest.split_at(end);
        self.rest = rest;
        serde_json::from_str(token).expect("a plain value")
    }

    fn selection(&mut self, names: &mut BTreeSet<String>) {
        self.expect('{');
        while !self.eat('}') {
            names.insert(self.name());
            self.skip();
            if self.rest.starts_with('{') {
                self.selection(names);
            }
        }
    }
}

fn operation(body: &Value) -> Operation {
    let no_variables = Value::Null;
    let mut text = Text {
        rest: body["query"].as_str().expect("a query text"),
        variables: body.get("variables").unwrap_or(&no_variables),
    };
    let kind = text.name();
    text.skip();
    if !text.rest.starts_with('{') {
        if !text.rest.starts_with('(') {
            text.name();
        }
        if text.eat('(') {
            text.rest = &text.rest[text.rest.find(')').expect("a closing )") + 1..];
        }
    }
    text.expect('{');
    let field = text.name();
    let mut arguments = Map::new();
    if text.eat('(') {
        while !text.eat(')') {
            let name = text.name();
            text.expect(':');
            arguments.insert(name, text.value());
        }
    }
    let mut selection = BTreeSet::new();
    text.selection(&mut selection);
    text.expect('}');
    Operation {
        kind,
        field,
        arguments,
        selection,
    }
}

fn capture(file: &str) -> Vec<Value> {
    let path = format!("{}/../captures/{file}", env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{path}: {error}"))
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn graphql_groups(file: &str) -> Vec<(Vec<String>, Vec<Value>)> {
    let mut groups: Vec<(Vec<String>, Vec<Value>)> = Vec::new();
    for line in capture(file) {
        if let Some(call) = line.get("call") {
            let words = call
                .as_array()
                .unwrap()
                .iter()
                .map(|word| word.as_str().unwrap().to_string())
                .collect();
            groups.push((words, Vec::new()));
        } else if line["url"].as_str().unwrap().ends_with(GRAPHQL) {
            groups.last_mut().unwrap().1.push(line);
        }
    }
    groups.retain(|(_, requests)| !requests.is_empty());
    groups
}

fn replay(file: &str) -> Vec<Group> {
    let lab = lab_vars::read(LAB).unwrap();
    graphql_groups(file)
        .into_iter()
        .map(|(call, captured)| {
            let answers: Vec<String> = captured
                .iter()
                .map(|line| line["response_body"].to_string())
                .collect();
            let routes: Vec<(&str, &str, u16, &str)> = answers
                .iter()
                .map(|answer| ("POST", GRAPHQL, 200, answer.as_str()))
                .collect();
            let stub = Stub::start(&routes);
            let infrahub = Infrahub::new(stub.url(), TOKEN);
            match call[0].as_str() {
                "seed" => {
                    infrahub.seed(&lab, &mut |_, _| {}).unwrap();
                }
                "maintenance" => {
                    infrahub
                        .maintenance(&call[4], &call[1], call[2] == "true")
                        .unwrap();
                }
                other => panic!("{file}: a call this test cannot replay: {other}"),
            }
            Group {
                call,
                sent: stub.requests(),
                captured,
            }
        })
        .collect()
}

fn body(seen: &Seen) -> Value {
    serde_json::from_slice(&seen.body).unwrap()
}

#[test]
fn the_schema_body_equals_the_one_the_1_11_2_capture_sent() {
    let lines = capture("infrahub-1.11.2.jsonl");
    let load = lines
        .iter()
        .find(|line| {
            line["url"]
                .as_str()
                .is_some_and(|url| url.contains("/api/schema/load"))
        })
        .unwrap();
    let schema: Value = serde_saphyr::from_str(SCHEMA).unwrap();
    let stub = Stub::start(&[(
        "POST",
        "/api/schema/load?branch=main",
        200,
        r#"{"schema_updated":true}"#,
    )]);

    Infrahub::new(stub.url(), TOKEN)
        .load_schema(&schema)
        .unwrap();

    let sent = &stub.requests()[0];
    assert!(load["url"].as_str().unwrap().ends_with(&sent.path));
    assert_eq!(body(sent), load["request_body"]);
}

#[test]
fn the_kind_wait_accepts_the_schema_answer_of_each_capture() {
    for file in FILES {
        let lines = capture(file);
        let fetch = lines.iter().find(|line| line["method"] == "GET").unwrap();
        let answer = fetch["response_body"].to_string();
        let stub = Stub::start(&[("GET", "/api/schema?branch=main", 200, &answer)]);

        let result = Infrahub::new(stub.url(), TOKEN).wait_for_kinds(&KINDS, Duration::ZERO);

        assert_eq!(result, Ok(()), "{file}");
        assert!(fetch["url"]
            .as_str()
            .unwrap()
            .ends_with(&stub.requests()[0].path));
    }
}

#[test]
fn nobs_sends_as_many_queries_and_mutations_as_each_capture() {
    for file in FILES {
        let groups = replay(file);
        let count = |requests: Vec<&Value>, kind: &str| {
            requests
                .into_iter()
                .filter(|request| operation(&request["request_body"]).kind == kind)
                .count()
        };

        for group in &groups {
            assert_eq!(
                group.sent.len(),
                group.captured.len(),
                "{file}: {:?}",
                group.call
            );
        }
        let captured: Vec<&Value> = groups.iter().flat_map(|group| &group.captured).collect();
        assert_eq!(count(captured.clone(), "query"), 34, "{file}");
        assert_eq!(count(captured, "mutation"), 34, "{file}");
        let sent: Vec<Value> = groups
            .iter()
            .flat_map(|group| &group.sent)
            .map(body)
            .collect();
        let kinds = |kind: &str| {
            sent.iter()
                .filter(|request| operation(request).kind == kind)
                .count()
        };
        assert_eq!((kinds("query"), kinds("mutation")), (34, 34), "{file}");
    }
}

#[test]
fn every_request_has_the_operation_root_field_arguments_and_data_of_the_captured_one() {
    for file in FILES {
        for group in replay(file) {
            for (n, (sent, captured)) in group.sent.iter().zip(&group.captured).enumerate() {
                let label = format!("{file} {:?} request {n}", group.call);
                let (mine, theirs) = (operation(&body(sent)), operation(&captured["request_body"]));

                assert_eq!(mine.kind, theirs.kind, "{label}");
                assert_eq!(mine.field, theirs.field, "{label}");
                assert_eq!(mine.arguments, theirs.arguments, "{label}");
                assert!(
                    mine.selection.is_subset(&theirs.selection),
                    "{label}: {:?}",
                    mine.selection
                );
            }
        }
    }
}

#[test]
fn every_request_goes_to_the_captured_method_and_path_with_the_captured_headers() {
    for file in FILES {
        for group in replay(file) {
            for (sent, captured) in group.sent.iter().zip(&group.captured) {
                assert_eq!(sent.method, captured["method"].as_str().unwrap(), "{file}");
                assert!(
                    captured["url"].as_str().unwrap().ends_with(&sent.path),
                    "{file}"
                );
                for name in ["X-INFRAHUB-KEY", "content-type"] {
                    assert_eq!(
                        sent.header(name),
                        captured["request_headers"][name].as_str(),
                        "{file}: {name}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_parser_reads_a_query_with_variables_and_a_mutation_with_a_data_object() {
    let query = operation(&json!({
        "query": "\nquery Filters_X ($offset: Int!, $limit: Int!) {\n  X(a__value: \"s\", ids: [\"1\", \"2\"], offset: $offset, limit: $limit) {\n    count\n    edges { node { id } }\n  }\n}\n",
        "variables": {"offset": 0, "limit": 50}
    }));
    assert_eq!((query.kind.as_str(), query.field.as_str()), ("query", "X"));
    assert_eq!(
        Value::Object(query.arguments),
        json!({"a__value": "s", "ids": ["1", "2"], "offset": 0, "limit": 50})
    );
    let names = ["count", "edges", "node", "id"].map(String::from);
    assert_eq!(query.selection, BTreeSet::from(names));

    let mutation = operation(&json!({
        "query": "mutation { XCreate(data: {a: {value: 1}, b: {value: null}, c: {id: \"7\"}}) { ok object { id } } }"
    }));
    assert_eq!(
        (mutation.kind.as_str(), mutation.field.as_str()),
        ("mutation", "XCreate")
    );
    assert_eq!(
        Value::Object(mutation.arguments),
        json!({"data": {"a": {"value": 1}, "b": {"value": null}, "c": {"id": "7"}}})
    );
}
