use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use serde_json::Value;

use crate::http;

const TIMEOUT: Duration = Duration::from_secs(10);

pub struct Alertmanager {
    url: String,
    timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Unreachable { url: String, reason: String },
    Status { url: String, status: u16 },
    Body { url: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub labels: BTreeMap<String, String>,
    pub state: String,
    pub silenced: bool,
    pub inhibited: bool,
    pub starts_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Silence {
    pub id: String,
    pub state: String,
    pub starts_at: String,
    pub ends_at: String,
    pub matchers: Vec<(String, String)>,
    pub created_by: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unreachable { url, reason } => {
                write!(f, "cannot reach Alertmanager at {url}: {reason}")
            }
            Error::Status { url, status } => write!(f, "Alertmanager at {url} answered {status}"),
            Error::Body { url, reason } => write!(
                f,
                "Alertmanager at {url} answered with a body nobs cannot read: {reason}"
            ),
        }
    }
}

impl Alertmanager {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into().trim_end_matches('/').to_string(),
            timeout: TIMEOUT,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn alerts(&self, silenced: bool, inhibited: bool) -> Result<Vec<Alert>, Error> {
        let body = self.get(&format!(
            "/api/v2/alerts?active=true&silenced={silenced}&inhibited={inhibited}"
        ))?;
        parse_alerts(&body).map_err(|reason| self.body_error(reason))
    }

    pub fn silences(&self) -> Result<Vec<Silence>, Error> {
        let body = self.get("/api/v2/silences")?;
        parse_silences(&body).map_err(|reason| self.body_error(reason))
    }

    fn get(&self, target: &str) -> Result<String, Error> {
        let url = format!("{}{target}", self.url);
        let reply = http::get(&url, self.timeout).map_err(|error| Error::Unreachable {
            url: self.url.clone(),
            reason: match error.strip_prefix(&format!("cannot reach {url}: ")) {
                Some(rest) => rest.to_string(),
                None => error,
            },
        })?;
        if reply.status >= 400 {
            return Err(Error::Status {
                url: self.url.clone(),
                status: reply.status,
            });
        }
        Ok(reply.body)
    }

    fn body_error(&self, reason: String) -> Error {
        Error::Body {
            url: self.url.clone(),
            reason,
        }
    }
}

pub fn parse_alerts(body: &str) -> Result<Vec<Alert>, String> {
    Ok(list(body)?
        .iter()
        .map(|alert| Alert {
            labels: alert["labels"]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_string())))
                .collect(),
            state: text(&alert["status"]["state"]),
            silenced: has_items(&alert["status"]["silencedBy"]),
            inhibited: has_items(&alert["status"]["inhibitedBy"]),
            starts_at: text(&alert["startsAt"]),
        })
        .collect())
}

pub fn parse_silences(body: &str) -> Result<Vec<Silence>, String> {
    Ok(list(body)?
        .iter()
        .map(|silence| Silence {
            id: text(&silence["id"]),
            state: text(&silence["status"]["state"]),
            starts_at: text(&silence["startsAt"]),
            ends_at: text(&silence["endsAt"]),
            matchers: silence["matchers"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|matcher| (text(&matcher["name"]), text(&matcher["value"])))
                .collect(),
            created_by: text(&silence["createdBy"]),
        })
        .collect())
}

fn list(body: &str) -> Result<Vec<Value>, String> {
    match serde_json::from_str(body) {
        Ok(Value::Array(items)) => Ok(items),
        Ok(_) => Err("the reply is not a list".to_string()),
        Err(_) => Err("the reply is not JSON".to_string()),
    }
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

fn has_items(value: &Value) -> bool {
    value.as_array().is_some_and(|items| !items.is_empty())
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;
    use crate::http::stub::Stub;

    const ALERTS_PATH: &str = "/api/v2/alerts?active=true&silenced=true&inhibited=false";
    const ONE_ALERT: &str = r#"[{"labels":{"alertname":"A"},"status":{"state":"active","silencedBy":[],"inhibitedBy":[]},"startsAt":"2026-11-06T10:15:30Z"}]"#;

    fn closed_port_url() -> String {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        format!("http://127.0.0.1:{port}")
    }

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn parse_alerts_reads_labels_state_start_and_the_silenced_and_inhibited_flags() {
        let body = r#"[
            {"labels":{"alertname":"BgpSessionNotUp","device":"srl1"},"status":{"state":"active","silencedBy":["s1"],"inhibitedBy":[]},"startsAt":"2026-11-06T10:15:30.123Z"},
            {"labels":{"alertname":"InterfaceDown"},"status":{"state":"active","silencedBy":[],"inhibitedBy":["a1"]},"startsAt":"2026-11-06T10:20:00Z"}
        ]"#;
        let alerts = parse_alerts(body).unwrap();
        assert_eq!(
            alerts,
            vec![
                Alert {
                    labels: BTreeMap::from([
                        ("alertname".to_string(), "BgpSessionNotUp".to_string()),
                        ("device".to_string(), "srl1".to_string()),
                    ]),
                    state: "active".to_string(),
                    silenced: true,
                    inhibited: false,
                    starts_at: "2026-11-06T10:15:30.123Z".to_string(),
                },
                Alert {
                    labels: BTreeMap::from([(
                        "alertname".to_string(),
                        "InterfaceDown".to_string()
                    )]),
                    state: "active".to_string(),
                    silenced: false,
                    inhibited: true,
                    starts_at: "2026-11-06T10:20:00Z".to_string(),
                },
            ]
        );
    }

    #[test]
    fn a_suppressed_alert_with_silenced_by_is_silenced_and_keeps_its_state() {
        let body = r#"[{"labels":{},"status":{"state":"suppressed","silencedBy":["s1"],"inhibitedBy":[]},"startsAt":"2026-11-06T10:15:30Z"}]"#;
        let alert = &parse_alerts(body).unwrap()[0];
        assert!(alert.silenced);
        assert_eq!(alert.state, "suppressed");
    }

    #[test]
    fn parse_alerts_gives_empty_values_for_missing_fields() {
        let alerts = parse_alerts("[{}]").unwrap();
        assert_eq!(
            alerts,
            vec![Alert {
                labels: BTreeMap::new(),
                state: String::new(),
                silenced: false,
                inhibited: false,
                starts_at: String::new(),
            }]
        );
    }

    #[test]
    fn parse_alerts_refuses_a_body_that_is_not_a_list() {
        assert!(parse_alerts(r#"{"alerts":[]}"#).is_err());
        assert!(parse_alerts("not json").is_err());
    }

    #[test]
    fn parse_silences_reads_the_fields_and_gives_matchers_as_pairs() {
        let body = r#"[
            {"id":"0a1b2c3d-4e5f","status":{"state":"active"},"startsAt":"2026-11-06T10:00:00.000Z","endsAt":"2026-11-06T12:00:00.000Z","matchers":[{"name":"alertname","value":"BgpSessionNotUp","isRegex":false},{"name":"device","value":"srl1"}],"createdBy":"alice"},
            {}
        ]"#;
        assert_eq!(
            parse_silences(body).unwrap(),
            vec![
                Silence {
                    id: "0a1b2c3d-4e5f".to_string(),
                    state: "active".to_string(),
                    starts_at: "2026-11-06T10:00:00.000Z".to_string(),
                    ends_at: "2026-11-06T12:00:00.000Z".to_string(),
                    matchers: pairs(&[("alertname", "BgpSessionNotUp"), ("device", "srl1")]),
                    created_by: "alice".to_string(),
                },
                Silence {
                    id: String::new(),
                    state: String::new(),
                    starts_at: String::new(),
                    ends_at: String::new(),
                    matchers: Vec::new(),
                    created_by: String::new(),
                },
            ]
        );
    }

    #[test]
    fn parse_silences_refuses_a_body_that_is_not_a_list() {
        assert!(parse_silences(r#"{"silences":[]}"#).is_err());
        assert!(parse_silences("not json").is_err());
    }

    #[test]
    fn new_drops_every_trailing_slash() {
        assert_eq!(
            Alertmanager::new("http://am:9093//").url(),
            "http://am:9093"
        );
    }

    #[test]
    fn alerts_sends_one_get_with_the_query_and_returns_the_alerts() {
        let stub = Stub::start(&[("GET", ALERTS_PATH, 200, ONE_ALERT)]);

        let alerts = Alertmanager::new(stub.url()).alerts(true, false).unwrap();

        assert_eq!(alerts.len(), 1);
        let requests = stub.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path, ALERTS_PATH);
    }

    #[test]
    fn silences_sends_one_get_and_returns_the_silences() {
        let stub = Stub::start(&[("GET", "/api/v2/silences", 200, r#"[{"id":"s1"}]"#)]);

        let silences = Alertmanager::new(stub.url()).silences().unwrap();

        assert_eq!(silences[0].id, "s1");
        let requests = stub.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/api/v2/silences");
    }

    #[test]
    fn a_status_of_400_or_more_is_a_status_error_with_the_base_url() {
        let stub = Stub::start(&[
            ("GET", "/api/v2/silences", 500, "[]"),
            ("GET", ALERTS_PATH, 400, "[]"),
        ]);
        let client = Alertmanager::new(stub.url());

        assert_eq!(
            client.silences(),
            Err(Error::Status {
                url: stub.url().to_string(),
                status: 500
            })
        );
        assert_eq!(
            client.alerts(true, false),
            Err(Error::Status {
                url: stub.url().to_string(),
                status: 400
            })
        );
    }

    #[test]
    fn a_closed_port_is_unreachable_with_a_reason_that_has_no_cannot_reach_prefix() {
        let url = closed_port_url();

        match Alertmanager::new(&url).silences() {
            Err(Error::Unreachable { url: seen, reason }) => {
                assert_eq!(seen, url);
                assert!(!reason.is_empty());
                assert!(!reason.starts_with("cannot reach"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_body_that_is_not_a_list_is_a_body_error_with_the_base_url() {
        let stub = Stub::start(&[
            ("GET", "/api/v2/silences", 200, "not json"),
            ("GET", ALERTS_PATH, 200, "{}"),
        ]);
        let client = Alertmanager::new(stub.url());

        for result in [
            client.silences().map(|_| ()),
            client.alerts(true, false).map(|_| ()),
        ] {
            match result {
                Err(Error::Body { url, reason }) => {
                    assert_eq!(url, stub.url());
                    assert!(!reason.is_empty());
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn the_errors_display_as_the_texts_the_command_prints() {
        let url = "http://am:9093".to_string();
        assert_eq!(
            Error::Unreachable {
                url: url.clone(),
                reason: "connection refused".to_string()
            }
            .to_string(),
            "cannot reach Alertmanager at http://am:9093: connection refused"
        );
        assert_eq!(
            Error::Status {
                url: url.clone(),
                status: 503
            }
            .to_string(),
            "Alertmanager at http://am:9093 answered 503"
        );
        assert_eq!(
            Error::Body {
                url,
                reason: "the reply is not a list".to_string()
            }
            .to_string(),
            "Alertmanager at http://am:9093 answered with a body nobs cannot read: the reply is not a list"
        );
    }
}
