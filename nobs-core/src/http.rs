use std::time::Duration;

use ureq::Agent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub body: String,
}

pub fn get(url: &str, timeout: Duration) -> Result<Reply, String> {
    let config = Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .build();
    Agent::new_with_config(config)
        .get(url)
        .call()
        .and_then(|mut response| {
            let status = response.status().as_u16();
            let body = response.body_mut().read_to_string()?;
            Ok(Reply { status, body })
        })
        .map_err(|error| format!("cannot reach {url}: {error}"))
}

#[cfg(test)]
#[path = "../../tests/support/stub.rs"]
pub(crate) mod stub;

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::time::Instant;

    use super::stub::Stub;
    use super::*;

    const WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn get_returns_the_status_of_a_200_reply() {
        let stub = Stub::start(&[("GET", "/up", 200, "ready")]);
        assert_eq!(
            get(&format!("{}/up", stub.url()), WAIT),
            Ok(Reply {
                status: 200,
                body: "ready".to_string()
            })
        );
    }

    #[test]
    fn get_returns_a_status_of_400_or_more_as_a_reply_not_an_error() {
        let stub = Stub::start(&[("GET", "/up", 503, "down")]);
        assert_eq!(
            get(&format!("{}/up", stub.url()), WAIT),
            Ok(Reply {
                status: 503,
                body: "down".to_string()
            })
        );
    }

    #[test]
    fn get_sends_a_get_for_the_path_it_was_given() {
        let stub = Stub::start(&[]);
        get(&format!("{}/a/b?c=1", stub.url()), WAIT).unwrap();

        let requests = stub.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path, "/a/b?c=1");
    }

    #[test]
    fn get_on_a_closed_port_is_an_error_that_starts_with_cannot_reach() {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let url = format!("http://127.0.0.1:{port}/up");

        let error = get(&url, WAIT).unwrap_err();

        assert!(
            error.starts_with(&format!("cannot reach {url}: ")),
            "{error}"
        );
    }

    #[test]
    fn get_gives_up_on_a_silent_server_after_the_timeout_it_was_given() {
        let stub = Stub::start_silent(Duration::from_secs(20));
        let started = Instant::now();

        let error = get(stub.url(), Duration::from_millis(300)).unwrap_err();

        assert!(error.starts_with("cannot reach "), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }
}
