use std::cmp::Reverse;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use ureq::Agent;

use crate::http;

pub const DEFAULT_URL: &str = "http://localhost:3001";

const TIMEOUT: Duration = Duration::from_secs(10);
const PATH: &str = "/loki/api/v1/query_range";
const BODY_CUT: usize = 200;
const NANOSECONDS_PER_MINUTE: u128 = 60_000_000_000;

pub struct Loki {
    url: String,
    agent: Agent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub nanoseconds: i64,
    pub text: String,
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
                write!(f, "Loki answered {status} on {path}: {body}")
            }
            Error::Body(reason) => {
                write!(f, "Loki answered with text nobs cannot read: {reason}")
            }
        }
    }
}

impl Loki {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into().trim_end_matches('/').to_string(),
            agent: http::agent(TIMEOUT),
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn query_range(
        &self,
        query: &str,
        minutes: u64,
        limit: u64,
        now: SystemTime,
    ) -> Result<Vec<Line>, Error> {
        let end = nanoseconds_since_1970(now);
        let window =
            i64::try_from(u128::from(minutes) * NANOSECONDS_PER_MINUTE).unwrap_or(i64::MAX);
        let start = end.saturating_sub(window).max(0);
        let sent = self
            .agent
            .get(format!("{}{PATH}", self.url))
            .query("query", query)
            .query("start", start.to_string())
            .query("end", end.to_string())
            .query("limit", limit.to_string())
            .query("direction", "BACKWARD")
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
        let mut lines = parse(&reply.body).map_err(Error::Body)?;
        lines.sort_by_key(|line| Reverse(line.nanoseconds));
        lines.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(lines)
    }
}

fn nanoseconds_since_1970(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |elapsed| {
        i64::try_from(elapsed.as_nanos()).unwrap_or(i64::MAX)
    })
}

fn parse(body: &str) -> Result<Vec<Line>, String> {
    let answer: Value =
        serde_json::from_str(body).map_err(|_| "the reply is not JSON".to_string())?;
    let streams = answer
        .pointer("/data/result")
        .and_then(Value::as_array)
        .ok_or("the reply has no data.result list")?;
    let mut lines = Vec::new();
    for pair in streams
        .iter()
        .flat_map(|stream| stream["values"].as_array().into_iter().flatten())
    {
        let (Some(nanoseconds), Some(text)) = (
            pair[0].as_str().and_then(|text| text.parse().ok()),
            pair[1].as_str(),
        ) else {
            return Err("a line of the reply is not a pair of nanoseconds and text".to_string());
        };
        lines.push(Line {
            nanoseconds,
            text: text.to_string(),
        });
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;
    use crate::http::stub::Stub;

    const ROUTE: &str = "/loki/api/v1/query_range*";
    const NOW_SECONDS: u64 = 1_793_960_400;
    const TWO_STREAMS: &str = r#"{"status":"success","data":{"resultType":"streams","result":[
        {"stream":{"device":"srl1"},"values":[["1793960390000000000","a-new"],["1793960300000000000","a-old"]]},
        {"stream":{"device":"srl2"},"values":[["1793960395000000000","b-newest"],["1793960350000000000","b-mid"]]}
    ]}}"#;

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(NOW_SECONDS)
    }

    fn closed_port_url() -> String {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        format!("http://127.0.0.1:{port}")
    }

    fn texts(lines: &[Line]) -> Vec<&str> {
        lines.iter().map(|line| line.text.as_str()).collect()
    }

    #[test]
    fn new_drops_every_trailing_slash() {
        assert_eq!(Loki::new("http://loki:3001//").url(), "http://loki:3001");
    }

    #[test]
    fn query_range_sends_one_get_with_the_query_the_window_the_limit_and_backward() {
        let stub = Stub::start(&[("GET", ROUTE, 200, TWO_STREAMS)]);

        Loki::new(stub.url())
            .query_range(r#"{source="prefect"}"#, 5, 2, now())
            .unwrap();

        let requests = stub.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        let (path, parameters) = requests[0].path.split_once('?').unwrap();
        assert_eq!(path, "/loki/api/v1/query_range");
        assert_eq!(
            parameters.split('&').collect::<Vec<_>>(),
            [
                "query=%7Bsource%3D%22prefect%22%7D",
                "start=1793960100000000000",
                "end=1793960400000000000",
                "limit=2",
                "direction=BACKWARD",
            ]
        );
    }

    #[test]
    fn a_query_with_spaces_and_regex_characters_is_sent_without_a_raw_space() {
        let stub = Stub::start(&[("GET", ROUTE, 200, TWO_STREAMS)]);

        Loki::new(stub.url())
            .query_range(
                r#"{device="srl1"} != "license" |~ "(bgp|BGP)""#,
                10,
                20,
                now(),
            )
            .unwrap();

        let path = &stub.requests()[0].path;
        assert!(!path.contains(' '), "{path}");
        assert!(!path.contains('"'), "{path}");
        assert!(!path.contains('{'), "{path}");
    }

    #[test]
    fn the_values_of_every_stream_come_back_newest_first_and_cut_to_the_limit() {
        let stub = Stub::start(&[
            ("GET", ROUTE, 200, TWO_STREAMS),
            ("GET", ROUTE, 200, TWO_STREAMS),
        ]);
        let loki = Loki::new(stub.url());

        let all = loki.query_range("q", 10, 10, now()).unwrap();
        let cut = loki.query_range("q", 10, 3, now()).unwrap();

        assert_eq!(texts(&all), ["b-newest", "a-new", "b-mid", "a-old"]);
        assert_eq!(all[0].nanoseconds, 1_793_960_395_000_000_000);
        assert_eq!(texts(&cut), ["b-newest", "a-new", "b-mid"]);
    }

    #[test]
    fn an_empty_result_is_an_empty_list() {
        let stub = Stub::start(&[(
            "GET",
            ROUTE,
            200,
            r#"{"status":"success","data":{"resultType":"streams","result":[]}}"#,
        )]);

        assert_eq!(
            Loki::new(stub.url()).query_range("q", 10, 20, now()),
            Ok(vec![])
        );
    }

    #[test]
    fn a_huge_number_of_minutes_starts_at_zero_and_does_not_panic() {
        let stub = Stub::start(&[("GET", ROUTE, 200, TWO_STREAMS)]);

        Loki::new(stub.url())
            .query_range("q", u64::MAX, u64::MAX, now())
            .unwrap();

        let path = &stub.requests()[0].path;
        assert!(path.contains("&start=0&"), "{path}");
        assert!(path.contains(&format!("&limit={}&", u64::MAX)), "{path}");
    }

    #[test]
    fn a_status_of_400_or_more_is_a_status_error_with_the_path_and_the_body() {
        let stub = Stub::start(&[("GET", ROUTE, 500, "boom")]);

        assert_eq!(
            Loki::new(stub.url()).query_range("q", 10, 20, now()),
            Err(Error::Status {
                status: 500,
                path: "/loki/api/v1/query_range".to_string(),
                body: "boom".to_string()
            })
        );
    }

    #[test]
    fn a_reply_that_is_not_the_loki_shape_is_a_body_error() {
        let bodies = [
            "not json",
            "{}",
            r#"{"data":{"result":[{"values":[["soon","x"]]}]}}"#,
            r#"{"data":{"result":[{"values":[["1793960390000000000"]]}]}}"#,
        ];
        let routes: Vec<(&str, &str, u16, &str)> = bodies
            .iter()
            .map(|body| ("GET", ROUTE, 200, *body))
            .collect();
        let stub = Stub::start(&routes);
        let loki = Loki::new(stub.url());

        for body in bodies {
            let result = loki.query_range("q", 10, 20, now());
            assert!(matches!(result, Err(Error::Body(_))), "{body}: {result:?}");
        }
    }

    #[test]
    fn no_listener_is_a_reach_error_with_the_url_and_a_reason_without_the_prefix() {
        let url = closed_port_url();

        match Loki::new(&url).query_range("q", 10, 20, now()) {
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
        let stub = Stub::start_after(Duration::from_secs(12), &[("GET", ROUTE, 200, TWO_STREAMS)]);

        let result = Loki::new(stub.url()).query_range("q", 10, 20, now());

        assert!(matches!(result, Err(Error::Reach { .. })), "{result:?}");
    }

    #[test]
    fn the_error_texts_are_the_ones_the_commands_print() {
        assert_eq!(
            Error::Reach {
                url: "http://localhost:3001".to_string(),
                reason: "connection refused".to_string()
            }
            .to_string(),
            "cannot reach http://localhost:3001: connection refused"
        );
        assert_eq!(
            Error::Status {
                status: 500,
                path: "/loki/api/v1/query_range".to_string(),
                body: "x".repeat(300)
            }
            .to_string(),
            format!(
                "Loki answered 500 on /loki/api/v1/query_range: {}",
                "x".repeat(200)
            )
        );
        assert_eq!(
            Error::Body("the reply is not JSON".to_string()).to_string(),
            "Loki answered with text nobs cannot read: the reply is not JSON"
        );
    }

    #[test]
    fn a_prefix_route_moves_through_its_replies_for_requests_with_different_paths() {
        let stub = Stub::start(&[
            ("GET", "/q/*", 200, "first"),
            ("GET", "/q/*", 200, "second"),
            ("GET", "/exact", 200, "exact"),
        ]);
        let url = stub.url();
        let wait = Duration::from_secs(5);

        let answers: Vec<String> = ["/q/a?x=1", "/q/b?x=2", "/exact", "/other"]
            .iter()
            .map(|path| http::get(&format!("{url}{path}"), wait).unwrap())
            .map(|reply| format!("{} {}", reply.status, reply.body))
            .collect();

        assert_eq!(answers, ["200 first", "200 second", "200 exact", "404 "]);
        assert_eq!(stub.requests()[1].path, "/q/b?x=2");
    }
}
