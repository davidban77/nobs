use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use serde_json::Value;
use ureq::Agent;

use crate::http;

pub const DEFAULT_URL: &str = "http://localhost:9090";

const TIMEOUT: Duration = Duration::from_secs(10);
const PATH: &str = "/api/v1/query";
const BODY_CUT: usize = 200;

pub struct Prometheus {
    url: String,
    agent: Agent,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub labels: BTreeMap<String, String>,
    pub value: f64,
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
                write!(f, "Prometheus answered {status} on {path}: {body}")
            }
            Error::Body(reason) => {
                write!(
                    f,
                    "Prometheus answered with text nobs cannot read: {reason}"
                )
            }
        }
    }
}

impl Prometheus {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into().trim_end_matches('/').to_string(),
            agent: http::agent(TIMEOUT),
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn instant(&self, query: &str) -> Result<Option<f64>, Error> {
        let sent = self
            .agent
            .get(format!("{}{PATH}", self.url))
            .query("query", query)
            .call();
        let reply = http::read(&self.url, sent).map_err(|error| Error::Reach {
            url: self.url.clone(),
            reason: http::reason(error, &self.url),
        })?;
        if reply.status >= 400 {
            return Err(Error::Status {
                status: reply.status,
                path: PATH.to_string(),
                body: reply.body,
            });
        }
        parse(&reply.body).map_err(Error::Body)
    }

    pub fn samples(&self, query: &str) -> Result<Vec<Sample>, Error> {
        let sent = self
            .agent
            .get(format!("{}{PATH}", self.url))
            .query("query", query)
            .call();
        let reply = http::read(&self.url, sent).map_err(|error| Error::Reach {
            url: self.url.clone(),
            reason: http::reason(error, &self.url),
        })?;
        if reply.status >= 400 {
            return Err(Error::Status {
                status: reply.status,
                path: PATH.to_string(),
                body: reply.body,
            });
        }
        parse_samples(&reply.body).map_err(Error::Body)
    }
}

fn parse(body: &str) -> Result<Option<f64>, String> {
    let answer: Value =
        serde_json::from_str(body).map_err(|_| "the reply is not JSON".to_string())?;
    let result = answer
        .pointer("/data/result")
        .and_then(Value::as_array)
        .ok_or("the reply has no data.result list")?;
    let Some(first) = result.first() else {
        return Ok(None);
    };
    first
        .pointer("/value/1")
        .and_then(Value::as_str)
        .and_then(|text| text.parse().ok())
        .map(Some)
        .ok_or_else(|| "the first result has no number in value".to_string())
}

fn parse_samples(body: &str) -> Result<Vec<Sample>, String> {
    let answer: Value =
        serde_json::from_str(body).map_err(|_| "the reply is not JSON".to_string())?;
    let result = answer
        .pointer("/data/result")
        .and_then(Value::as_array)
        .ok_or("the reply has no data.result list")?;
    Ok(result.iter().filter_map(sample).collect())
}

fn sample(entry: &Value) -> Option<Sample> {
    let value = entry.pointer("/value/1")?.as_str()?.parse().ok()?;
    let labels = entry
        .get("metric")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, label)| Some((name.clone(), label.as_str()?.to_string())))
        .collect();
    Some(Sample { labels, value })
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;
    use crate::http::stub::Stub;

    const ROUTE: &str = "/api/v1/query*";
    const QUERY: &str = r#"bgp_admin_state{device="srl1",peer_address="10.1.99.2",afi_safi_name="ipv4-unicast",name="default"}"#;
    const ONE_SERIES: &str = r#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{},"value":[1793960400,"1"]}]}}"#;
    const NO_SERIES: &str = r#"{"status":"success","data":{"resultType":"vector","result":[]}}"#;

    fn closed_port_url() -> String {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        format!("http://127.0.0.1:{port}")
    }

    #[test]
    fn new_drops_every_trailing_slash() {
        assert_eq!(
            Prometheus::new("http://prom:9090//").url(),
            "http://prom:9090"
        );
    }

    #[test]
    fn instant_sends_one_get_with_the_query_percent_encoded_and_returns_the_value() {
        let stub = Stub::start(&[("GET", ROUTE, 200, ONE_SERIES)]);

        let value = Prometheus::new(stub.url()).instant(QUERY);

        assert_eq!(value, Ok(Some(1.0)));
        let requests = stub.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        let path = &requests[0].path;
        assert!(
            path.starts_with("/api/v1/query?query=bgp_admin_state%7Bdevice%3D%22srl1%22"),
            "{path}"
        );
        assert!(!path.contains([' ', '{', '"']), "{path}");
    }

    #[test]
    fn a_number_that_is_not_whole_comes_back_as_it_is() {
        let stub = Stub::start(&[(
            "GET",
            ROUTE,
            200,
            r#"{"data":{"result":[{"value":[1793960400,"0.5"]}]}}"#,
        )]);

        assert_eq!(Prometheus::new(stub.url()).instant("up"), Ok(Some(0.5)));
    }

    #[test]
    fn an_empty_result_is_none() {
        let stub = Stub::start(&[("GET", ROUTE, 200, NO_SERIES)]);

        assert_eq!(Prometheus::new(stub.url()).instant(QUERY), Ok(None));
    }

    #[test]
    fn a_status_of_400_or_more_is_a_status_error_with_the_path_and_the_body() {
        let stub = Stub::start(&[("GET", ROUTE, 400, "bad query")]);

        assert_eq!(
            Prometheus::new(stub.url()).instant(QUERY),
            Err(Error::Status {
                status: 400,
                path: "/api/v1/query".to_string(),
                body: "bad query".to_string()
            })
        );
    }

    #[test]
    fn a_reply_that_is_not_the_prometheus_shape_is_a_body_error() {
        let bodies = [
            "not json",
            "{}",
            r#"{"data":{"result":[{"value":[1793960400,"soon"]}]}}"#,
            r#"{"data":{"result":[{"value":[1793960400,1]}]}}"#,
        ];
        let routes: Vec<(&str, &str, u16, &str)> = bodies
            .iter()
            .map(|body| ("GET", ROUTE, 200, *body))
            .collect();
        let stub = Stub::start(&routes);
        let prometheus = Prometheus::new(stub.url());

        for body in bodies {
            let result = prometheus.instant(QUERY);
            assert!(matches!(result, Err(Error::Body(_))), "{body}: {result:?}");
        }
    }

    #[test]
    fn samples_keeps_every_result_with_its_labels() {
        let stub = Stub::start(&[(
            "GET",
            ROUTE,
            200,
            r#"{"status":"success","data":{"resultType":"vector","result":[
                {"metric":{"__name__":"bgp_oper_state","device":"srl1","peer_address":"10.1.2.2"},"value":[1793960400,"1"]},
                {"metric":{"__name__":"interface_oper_state","device":"srl2","name":"ethernet-1/11"},"value":[1793960400,"2"]},
                {"metric":{"__name__":"bad"},"value":[1793960400,"soon"]}
            ]}}"#,
        )]);

        let samples = Prometheus::new(stub.url())
            .samples(r#"{__name__=~"bgp_oper_state|interface_oper_state"}"#)
            .unwrap();

        let labels = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect()
        };
        assert_eq!(
            samples,
            [
                Sample {
                    labels: labels(&[
                        ("__name__", "bgp_oper_state"),
                        ("device", "srl1"),
                        ("peer_address", "10.1.2.2")
                    ]),
                    value: 1.0
                },
                Sample {
                    labels: labels(&[
                        ("__name__", "interface_oper_state"),
                        ("device", "srl2"),
                        ("name", "ethernet-1/11")
                    ]),
                    value: 2.0
                },
            ]
        );
        let requests = stub.requests();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0]
                .path
                .starts_with("/api/v1/query?query=%7B__name__"),
            "{}",
            requests[0].path
        );
    }

    #[test]
    fn no_listener_is_a_reach_error_with_the_url_and_a_reason_without_the_prefix() {
        let url = closed_port_url();

        match Prometheus::new(&url).instant(QUERY) {
            Err(Error::Reach { url: seen, reason }) => {
                assert_eq!(seen, url);
                assert!(!reason.is_empty());
                assert!(!reason.starts_with("cannot reach"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_call_gives_up_on_an_answer_that_comes_after_ten_seconds() {
        let stub = Stub::start_after(Duration::from_secs(12), &[("GET", ROUTE, 200, ONE_SERIES)]);

        let result = Prometheus::new(stub.url()).instant(QUERY);

        assert!(matches!(result, Err(Error::Reach { .. })), "{result:?}");
    }

    #[test]
    fn the_error_texts_are_the_ones_the_commands_print() {
        assert_eq!(
            Error::Reach {
                url: "http://localhost:9090".to_string(),
                reason: "connection refused".to_string()
            }
            .to_string(),
            "cannot reach http://localhost:9090: connection refused"
        );
        assert_eq!(
            Error::Status {
                status: 400,
                path: "/api/v1/query".to_string(),
                body: "x".repeat(300)
            }
            .to_string(),
            format!(
                "Prometheus answered 400 on /api/v1/query: {}",
                "x".repeat(200)
            )
        );
        assert_eq!(
            Error::Body("the reply is not JSON".to_string()).to_string(),
            "Prometheus answered with text nobs cannot read: the reply is not JSON"
        );
    }
}
