use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::Value;
use ureq::Agent;

use crate::http::{self, Reply};

const TIMEOUT: Duration = Duration::from_secs(15);
const PATH: &str = "/api/ds/query";

pub struct Grafana {
    url: String,
    authorization: String,
    agent: Agent,
}

impl Grafana {
    pub fn new(url: impl Into<String>, user: &str, password: &str) -> Self {
        Self {
            url: url.into().trim_end_matches('/').to_string(),
            authorization: format!("Basic {}", STANDARD.encode(format!("{user}:{password}"))),
            agent: http::agent(TIMEOUT),
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn ds_query(&self, body: &Value) -> Result<Reply, String> {
        let sent = self
            .agent
            .post(format!("{}{PATH}", self.url))
            .header("authorization", self.authorization.as_str())
            .header("content-type", "application/json")
            .send(body.to_string().as_bytes());
        http::read(&self.url, sent)
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use serde_json::json;

    use super::*;
    use crate::http::stub::Stub;

    #[test]
    fn new_drops_every_trailing_slash() {
        assert_eq!(
            Grafana::new("http://grafana:3000//", "admin", "admin").url(),
            "http://grafana:3000"
        );
    }

    #[test]
    fn ds_query_posts_the_body_with_basic_auth_and_json_content_type() {
        let stub = Stub::start(&[("POST", PATH, 200, r#"{"results":{}}"#)]);
        let body = json!({"queries": [{"refId": "A"}], "from": "1", "to": "2"});

        let reply = Grafana::new(stub.url(), "admin", "admin").ds_query(&body);

        assert_eq!(
            reply,
            Ok(Reply {
                status: 200,
                body: r#"{"results":{}}"#.to_string()
            })
        );
        let requests = stub.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, PATH);
        assert_eq!(
            requests[0].header("authorization"),
            Some("Basic YWRtaW46YWRtaW4=")
        );
        assert_eq!(requests[0].header("content-type"), Some("application/json"));
        let sent: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(sent, body);
    }

    #[test]
    fn a_status_of_500_comes_back_as_a_reply() {
        let stub = Stub::start(&[("POST", PATH, 500, "boom")]);

        let reply = Grafana::new(stub.url(), "admin", "admin").ds_query(&json!({}));

        assert_eq!(
            reply,
            Ok(Reply {
                status: 500,
                body: "boom".to_string()
            })
        );
    }

    #[test]
    fn a_closed_port_is_cannot_reach_with_the_url() {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let url = format!("http://127.0.0.1:{port}");

        let error = Grafana::new(&url, "admin", "admin")
            .ds_query(&json!({}))
            .unwrap_err();

        assert!(
            error.starts_with(&format!("cannot reach {url}: ")),
            "{error}"
        );
    }
}
