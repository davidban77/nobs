use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::{http, time};

pub const DEFAULT_URL: &str = "http://localhost:9997/v1/api/webhook";

const TIMEOUT: Duration = Duration::from_secs(15);
const BODY_CUT: usize = 200;
const ALERTNAME: &str = "BgpSessionNotUp";
const NO_END: &str = "0001-01-01T00:00:00Z";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Reach { url: String, reason: String },
    Status { status: u16, body: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Reach { url, reason } => write!(f, "cannot reach {url}: {reason}"),
            Error::Status { status, body } => {
                let body: String = body.chars().take(BODY_CUT).collect();
                write!(f, "the webhook answered {status}: {body}")
            }
        }
    }
}

#[derive(Serialize)]
struct Payload<'a> {
    version: &'static str,
    #[serde(rename = "groupKey")]
    group_key: String,
    #[serde(rename = "truncatedAlerts")]
    truncated_alerts: u8,
    status: &'a str,
    receiver: &'static str,
    #[serde(rename = "groupLabels")]
    group_labels: GroupLabels<'a>,
    #[serde(rename = "commonLabels")]
    common_labels: CommonLabels,
    #[serde(rename = "commonAnnotations")]
    common_annotations: BTreeMap<String, String>,
    #[serde(rename = "externalURL")]
    external_url: &'static str,
    alerts: [PayloadAlert<'a>; 1],
}

#[derive(Serialize)]
struct GroupLabels<'a> {
    alertname: &'static str,
    device: &'a str,
    peer_address: &'a str,
}

#[derive(Serialize)]
struct CommonLabels {
    alertname: &'static str,
}

#[derive(Serialize)]
struct PayloadAlert<'a> {
    status: &'a str,
    labels: AlertLabels<'a>,
    annotations: BTreeMap<String, String>,
    #[serde(rename = "startsAt")]
    starts_at: String,
    #[serde(rename = "endsAt")]
    ends_at: String,
    #[serde(rename = "generatorURL")]
    generator_url: &'static str,
    fingerprint: String,
}

#[derive(Serialize)]
struct AlertLabels<'a> {
    alertname: &'static str,
    device: &'a str,
    peer_address: &'a str,
    afi_safi_name: &'static str,
    name: &'static str,
}

pub fn payload(device: &str, peer: &str, status: &str, now: SystemTime) -> String {
    let key = format!("cycle-{device}-{peer}");
    payload_keyed(device, peer, status, now, &key, &key)
}

pub fn payload_keyed(
    device: &str,
    peer: &str,
    status: &str,
    now: SystemTime,
    group_key: &str,
    fingerprint: &str,
) -> String {
    let seconds = now.duration_since(UNIX_EPOCH).map_or(0, |elapsed| {
        i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
    });
    let starts_at = time::rfc3339_utc(seconds);
    let ends_at = if status == "resolved" {
        starts_at.clone()
    } else {
        NO_END.to_string()
    };
    let payload = Payload {
        version: "4",
        group_key: group_key.to_string(),
        truncated_alerts: 0,
        status,
        receiver: "webhook-receiver",
        group_labels: GroupLabels {
            alertname: ALERTNAME,
            device,
            peer_address: peer,
        },
        common_labels: CommonLabels {
            alertname: ALERTNAME,
        },
        common_annotations: BTreeMap::new(),
        external_url: "http://localhost:9093",
        alerts: [PayloadAlert {
            status,
            labels: AlertLabels {
                alertname: ALERTNAME,
                device,
                peer_address: peer,
                afi_safi_name: "ipv4-unicast",
                name: "default",
            },
            annotations: BTreeMap::new(),
            starts_at,
            ends_at,
            generator_url: "",
            fingerprint: fingerprint.to_string(),
        }],
    };
    serde_json::to_string(&payload).expect("the payload is always valid JSON")
}

pub fn post(url: &str, body: &str) -> Result<(), Error> {
    let sent = http::agent(TIMEOUT)
        .post(url)
        .header("content-type", "application/json")
        .send(body.as_bytes());
    let reply = http::read(url, sent).map_err(|error| Error::Reach {
        url: url.to_string(),
        reason: http::reason(error, url),
    })?;
    if reply.status >= 400 {
        return Err(Error::Status {
            status: reply.status,
            body: reply.body,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;
    use crate::http::stub::Stub;

    const NOW_SECONDS: u64 = 1_793_960_400;

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(NOW_SECONDS)
    }

    fn firing_text(ends_at: &str) -> String {
        format!(
            concat!(
                r#"{{"version":"4","groupKey":"cycle-srl1-10.1.99.2","truncatedAlerts":0,"status":"firing","#,
                r#""receiver":"webhook-receiver","#,
                r#""groupLabels":{{"alertname":"BgpSessionNotUp","device":"srl1","peer_address":"10.1.99.2"}},"#,
                r#""commonLabels":{{"alertname":"BgpSessionNotUp"}},"commonAnnotations":{{}},"externalURL":"http://localhost:9093","#,
                r#""alerts":[{{"status":"firing","labels":{{"alertname":"BgpSessionNotUp","device":"srl1","#,
                r#""peer_address":"10.1.99.2","afi_safi_name":"ipv4-unicast","name":"default"}},"annotations":{{}},"#,
                r#""startsAt":"2026-11-06T10:20:00Z","endsAt":"{}","generatorURL":"","#,
                r#""fingerprint":"cycle-srl1-10.1.99.2"}}]}}"#
            ),
            ends_at
        )
    }

    fn closed_port_url() -> String {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        format!("http://127.0.0.1:{port}/v1/api/webhook")
    }

    #[test]
    fn a_firing_payload_is_the_exact_text_with_no_end_time() {
        assert_eq!(
            payload("srl1", "10.1.99.2", "firing", now()),
            firing_text("0001-01-01T00:00:00Z")
        );
    }

    #[test]
    fn a_resolved_payload_ends_when_it_starts_and_says_resolved_twice() {
        let resolved = payload("srl1", "10.1.99.2", "resolved", now());

        assert_eq!(
            resolved,
            firing_text("2026-11-06T10:20:00Z").replace("\"firing\"", "\"resolved\"")
        );
    }

    #[test]
    fn a_keyed_payload_carries_the_group_key_and_the_fingerprint_it_was_given() {
        assert_eq!(
            payload_keyed(
                "srl1",
                "10.1.99.2",
                "firing",
                now(),
                "try-it-try-it-1",
                "try-it-1"
            ),
            firing_text("0001-01-01T00:00:00Z")
                .replace(
                    r#""groupKey":"cycle-srl1-10.1.99.2""#,
                    r#""groupKey":"try-it-try-it-1""#
                )
                .replace(
                    r#""fingerprint":"cycle-srl1-10.1.99.2""#,
                    r#""fingerprint":"try-it-1""#
                )
        );
    }

    #[test]
    fn post_sends_the_body_as_given_to_the_url_as_given_with_the_content_type() {
        let stub = Stub::start(&[("POST", "/v1/api/webhook/", 204, "")]);
        let url = format!("{}/v1/api/webhook/", stub.url());

        assert_eq!(post(&url, r#"{"a":1}"#), Ok(()));

        let requests = stub.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/v1/api/webhook/");
        assert_eq!(String::from_utf8_lossy(&requests[0].body), r#"{"a":1}"#);
        assert_eq!(requests[0].header("content-type"), Some("application/json"));
    }

    #[test]
    fn a_status_of_400_or_more_is_a_status_error_with_the_body() {
        let stub = Stub::start(&[("POST", "/hook", 422, r#"{"detail":"bad status"}"#)]);

        assert_eq!(
            post(&format!("{}/hook", stub.url()), "{}"),
            Err(Error::Status {
                status: 422,
                body: r#"{"detail":"bad status"}"#.to_string()
            })
        );
    }

    #[test]
    fn no_listener_is_a_reach_error_with_the_url_and_a_reason_without_the_prefix() {
        let url = closed_port_url();

        match post(&url, "{}") {
            Err(Error::Reach { url: seen, reason }) => {
                assert_eq!(seen, url);
                assert!(!reason.is_empty());
                assert!(!reason.starts_with("cannot reach"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_error_texts_are_the_ones_the_command_prints() {
        assert_eq!(
            Error::Reach {
                url: "http://localhost:9997/v1/api/webhook".to_string(),
                reason: "connection refused".to_string()
            }
            .to_string(),
            "cannot reach http://localhost:9997/v1/api/webhook: connection refused"
        );
        assert_eq!(
            Error::Status {
                status: 422,
                body: "x".repeat(300)
            }
            .to_string(),
            format!("the webhook answered 422: {}", "x".repeat(200))
        );
    }
}
