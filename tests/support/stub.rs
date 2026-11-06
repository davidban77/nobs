#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Clone)]
struct Route {
    method: String,
    path: String,
    status: u16,
    body: String,
}

pub struct Stub {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Stub {
    pub fn start(routes: &[(&str, &str, u16, &str)]) -> Self {
        Self::start_after(Duration::ZERO, routes)
    }

    pub fn start_after(delay: Duration, routes: &[(&str, &str, u16, &str)]) -> Self {
        let routes: Vec<Route> = routes
            .iter()
            .map(|(method, path, status, body)| Route {
                method: method.to_string(),
                path: path.to_string(),
                status: *status,
                body: body.to_string(),
            })
            .collect();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stub");
        let url = format!("http://{}", listener.local_addr().expect("stub address"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_by_thread = Arc::clone(&seen);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = serve(stream, &routes, &seen_by_thread, delay);
            }
        });
        Self { url, seen }
    }

    pub fn start_silent(hold: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stub");
        let url = format!("http://{}", listener.local_addr().expect("stub address"));
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                thread::sleep(hold);
                drop(stream);
            }
        });
        Self {
            url,
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn requests(&self) -> Vec<Seen> {
        self.seen.lock().expect("stub requests").clone()
    }
}

fn matches(pattern: &str, path: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => path.starts_with(prefix),
        None => pattern == path,
    }
}

fn serve(
    mut stream: TcpStream,
    routes: &[Route],
    seen: &Mutex<Vec<Seen>>,
    delay: Duration,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    let mut headers = Vec::new();
    loop {
        line.clear();
        reader.read_line(&mut line)?;
        let text = line.trim_end();
        if text.is_empty() {
            break;
        }
        if let Some((name, value)) = text.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }

    let length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;

    let (status, reply) = {
        let mut seen = seen.lock().expect("stub requests");
        let pattern = routes
            .iter()
            .find(|route| route.method == method && matches(&route.path, &path))
            .map_or("", |route| route.path.as_str());
        let earlier = seen
            .iter()
            .filter(|request| request.method == method && matches(pattern, &request.path))
            .count();
        let matching: Vec<&Route> = routes
            .iter()
            .filter(|route| route.method == method && route.path == pattern)
            .collect();
        let answer = matching
            .get(earlier)
            .or(matching.last())
            .map(|route| (route.status, route.body.clone()))
            .unwrap_or((404, String::new()));
        seen.push(Seen {
            method,
            path,
            headers,
            body,
        });
        answer
    };

    thread::sleep(delay);
    write!(
        stream,
        "HTTP/1.1 {status} Stub\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    )?;
    stream.flush()
}
