use std::net::TcpListener;

use nobs_core::sonda::{Reply, Sonda};

#[path = "../../tests/support/stub.rs"]
mod stub;

use stub::Stub;

fn call_all(sonda: &Sonda) {
    sonda.health().unwrap();
    sonda.ready().unwrap();
    sonda.list().unwrap();
    sonda.show("s1").unwrap();
    sonda.stats("s1").unwrap();
    sonda.delete("s1").unwrap();
    sonda.load(b"kind: runnable\n").unwrap();
    sonda.event(b"{}").unwrap();
    sonda.metrics().unwrap();
}

#[test]
fn every_call_uses_its_method_and_path() {
    let stub = Stub::start(&[]);
    call_all(&Sonda::new(stub.url()));

    let requests = stub.requests();
    let seen: Vec<(&str, &str)> = requests
        .iter()
        .map(|request| (request.method.as_str(), request.path.as_str()))
        .collect();
    assert_eq!(
        seen,
        [
            ("GET", "/health"),
            ("GET", "/ready"),
            ("GET", "/scenarios"),
            ("GET", "/scenarios/s1"),
            ("GET", "/scenarios/s1/stats"),
            ("DELETE", "/scenarios/s1"),
            ("POST", "/scenarios"),
            ("POST", "/events"),
            ("GET", "/metrics"),
        ]
    );
}

#[test]
fn a_key_is_sent_as_a_bearer_header_on_every_call_but_health_and_ready() {
    let stub = Stub::start(&[]);
    call_all(&Sonda::new(stub.url()).api_key(Some("k".to_string())));

    for request in stub
        .requests()
        .iter()
        .filter(|r| r.path != "/health" && r.path != "/ready")
    {
        assert_eq!(
            request.header("Authorization"),
            Some("Bearer k"),
            "{} {}",
            request.method,
            request.path
        );
    }
}

#[test]
fn health_and_ready_carry_no_authorization_even_with_a_key() {
    let stub = Stub::start(&[]);
    let sonda = Sonda::new(stub.url()).api_key(Some("k".to_string()));
    sonda.health().unwrap();
    sonda.ready().unwrap();

    let requests = stub.requests();
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert_eq!(request.header("authorization"), None, "{}", request.path);
    }
}

#[test]
fn without_a_key_no_call_carries_authorization() {
    let stub = Stub::start(&[]);
    call_all(&Sonda::new(stub.url()));

    let requests = stub.requests();
    assert_eq!(requests.len(), 9);
    for request in requests {
        assert_eq!(request.header("authorization"), None, "{}", request.path);
    }
}

#[test]
fn load_sends_the_bytes_unchanged_as_yaml() {
    let stub = Stub::start(&[]);
    let file = b"kind: runnable\r\nname: caf\xc3\xa9\n\n  ";
    Sonda::new(stub.url()).load(file).unwrap();

    let request = &stub.requests()[0];
    assert_eq!(request.header("Content-Type"), Some("application/x-yaml"));
    assert_eq!(request.body, file);
}

#[test]
fn event_sends_the_bytes_unchanged_as_json() {
    let stub = Stub::start(&[]);
    let file = b"{\n  \"signal_type\": \"metrics\"\n}";
    Sonda::new(stub.url()).event(file).unwrap();

    let request = &stub.requests()[0];
    assert_eq!(request.header("Content-Type"), Some("application/json"));
    assert_eq!(request.body, file);
}

#[test]
fn a_reply_carries_its_status_and_body() {
    let stub = Stub::start(&[("GET", "/health", 200, "{\"status\":\"ok\"}")]);

    assert_eq!(
        Sonda::new(stub.url()).health().unwrap(),
        Reply {
            status: 200,
            body: "{\"status\":\"ok\"}".to_string()
        }
    );
}

#[test]
fn a_409_is_a_reply_with_its_body_not_an_error() {
    let stub = Stub::start(&[("POST", "/scenarios", 409, "{\"error\":\"conflict\"}")]);

    assert_eq!(
        Sonda::new(stub.url()).load(b"kind: runnable\n"),
        Ok(Reply {
            status: 409,
            body: "{\"error\":\"conflict\"}".to_string()
        })
    );
}

#[test]
fn a_closed_port_is_an_error_that_starts_with_cannot_reach() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = format!("http://127.0.0.1:{port}");

    let error = Sonda::new(&url).health().unwrap_err();

    assert!(
        error.starts_with(&format!("cannot reach {url}: ")),
        "{error}"
    );
}

#[test]
fn trailing_slashes_on_the_url_do_not_double_the_slash_in_the_path() {
    let stub = Stub::start(&[]);
    let sonda = Sonda::new(format!("{}//", stub.url()));
    sonda.list().unwrap();

    assert_eq!(sonda.url(), stub.url());
    assert_eq!(stub.requests()[0].path, "/scenarios");
}
