use std::time::Duration;

use crate::http;
use crate::infrahub::host_address;
use crate::sonda::Sonda;
use crate::workshop::URLS;

pub const TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Urls {
    pub grafana: String,
    pub prometheus: String,
    pub loki: String,
    pub alertmanager: String,
    pub infrahub: String,
    pub prefect: String,
    pub sonda: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub name: &'static str,
    pub base: String,
    pub path: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Ok,
    Http(u16),
    Down(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub target: Target,
    pub state: State,
}

impl Urls {
    pub fn defaults() -> Self {
        Self {
            grafana: default_url("Grafana"),
            prometheus: default_url("Prometheus"),
            loki: default_url("Loki"),
            alertmanager: default_url("Alertmanager"),
            infrahub: default_url("Infrahub"),
            prefect: default_url("Prefect"),
            sonda: default_url("Sonda"),
        }
    }

    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let defaults = Self::defaults();
        let pick = |name: &str, default: String| {
            lookup(name)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(default)
        };
        Self {
            prometheus: pick("PROMETHEUS_URL", defaults.prometheus),
            loki: pick("LOKI_URL", defaults.loki),
            alertmanager: pick("ALERTMANAGER_URL", defaults.alertmanager),
            infrahub: pick("INFRAHUB_ADDRESS", defaults.infrahub),
            ..defaults
        }
    }
}

fn default_url(name: &str) -> String {
    URLS.iter()
        .find(|(service, _)| *service == name)
        .map(|(_, url)| url.to_string())
        .expect("workshop::URLS lists every service")
}

pub fn targets(urls: &Urls) -> Vec<Target> {
    let infrahub = host_address(&urls.infrahub);
    [
        ("Grafana", urls.grafana.as_str(), "/api/health"),
        ("Prometheus", urls.prometheus.as_str(), "/-/ready"),
        ("Loki", urls.loki.as_str(), "/ready"),
        ("Alertmanager", urls.alertmanager.as_str(), "/-/ready"),
        ("Infrahub", infrahub.as_str(), "/health"),
        ("Prefect", urls.prefect.as_str(), "/api/health"),
        ("Sonda", urls.sonda.as_str(), "/health"),
    ]
    .into_iter()
    .map(|(name, base, path)| Target {
        name,
        base: base.trim_end_matches('/').to_string(),
        path,
    })
    .collect()
}

pub fn probe(target: &Target) -> Probe {
    let (url, result) = if target.name == "Sonda" {
        let result = Sonda::new(&target.base)
            .timeout(TIMEOUT)
            .health()
            .map(|reply| reply.status);
        (target.base.clone(), result)
    } else {
        let url = format!("{}{}", target.base, target.path);
        let result = http::get(&url, TIMEOUT).map(|reply| reply.status);
        (url, result)
    };
    let state = match result {
        Ok(status) if status < 400 => State::Ok,
        Ok(status) => State::Http(status),
        Err(error) => State::Down(http::reason(error, &url)),
    };
    Probe {
        target: target.clone(),
        state,
    }
}

pub fn probe_all(urls: &Urls) -> Vec<Probe> {
    targets(urls).iter().map(probe).collect()
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::time::Instant;

    use super::*;
    use crate::http::stub::Stub;

    fn target(name: &'static str, base: &str, path: &'static str) -> Target {
        Target {
            name,
            base: base.to_string(),
            path,
        }
    }

    fn closed_port_url() -> String {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        format!("http://127.0.0.1:{port}")
    }

    fn urls_all_at(base: &str) -> Urls {
        Urls {
            grafana: base.to_string(),
            prometheus: base.to_string(),
            loki: base.to_string(),
            alertmanager: base.to_string(),
            infrahub: base.to_string(),
            prefect: base.to_string(),
            sonda: base.to_string(),
        }
    }

    #[test]
    fn the_default_urls_are_the_workshop_addresses() {
        assert_eq!(
            Urls::defaults(),
            Urls {
                grafana: "http://localhost:3000".to_string(),
                prometheus: "http://localhost:9090".to_string(),
                loki: "http://localhost:3001".to_string(),
                alertmanager: "http://localhost:9093".to_string(),
                infrahub: "http://localhost:8000".to_string(),
                prefect: "http://localhost:4200".to_string(),
                sonda: "http://localhost:8085".to_string(),
            }
        );
    }

    #[test]
    fn from_lookup_takes_a_set_address_and_falls_back_to_the_default_for_a_blank_one() {
        let urls = Urls::from_lookup(|name| match name {
            "LOKI_URL" => Some("http://loki:3100".to_string()),
            "INFRAHUB_ADDRESS" => Some("  ".to_string()),
            _ => None,
        });
        assert_eq!(urls.loki, "http://loki:3100");
        assert_eq!(urls.infrahub, Urls::defaults().infrahub);
    }

    #[test]
    fn targets_are_the_seven_services_in_order_with_their_paths_and_default_bases() {
        let seen: Vec<(&str, String, &str)> = targets(&Urls::defaults())
            .into_iter()
            .map(|t| (t.name, t.base, t.path))
            .collect();
        let expected = [
            ("Grafana", "http://localhost:3000", "/api/health"),
            ("Prometheus", "http://localhost:9090", "/-/ready"),
            ("Loki", "http://localhost:3001", "/ready"),
            ("Alertmanager", "http://localhost:9093", "/-/ready"),
            ("Infrahub", "http://localhost:8000", "/health"),
            ("Prefect", "http://localhost:4200", "/api/health"),
            ("Sonda", "http://localhost:8085", "/health"),
        ];
        let expected: Vec<(&str, String, &str)> = expected
            .into_iter()
            .map(|(name, base, path)| (name, base.to_string(), path))
            .collect();
        assert_eq!(seen, expected);
    }

    #[test]
    fn targets_drop_every_trailing_slash_from_a_base() {
        let urls = Urls {
            grafana: "http://g:3000/".to_string(),
            sonda: "http://s:8085//".to_string(),
            ..Urls::defaults()
        };
        let found = targets(&urls);
        assert_eq!(found[0].base, "http://g:3000");
        assert_eq!(found[6].base, "http://s:8085");
    }

    #[test]
    fn targets_rewrite_the_container_infrahub_address_and_drop_its_slash() {
        let urls = Urls {
            infrahub: "http://infrahub-server:8000".to_string(),
            ..Urls::defaults()
        };
        assert_eq!(targets(&urls)[4].base, "http://localhost:8000");

        let urls = Urls {
            infrahub: "http://10.0.0.5:8000/".to_string(),
            ..Urls::defaults()
        };
        assert_eq!(targets(&urls)[4].base, "http://10.0.0.5:8000");
    }

    #[test]
    fn the_timeout_is_three_seconds() {
        assert_eq!(TIMEOUT, Duration::from_secs(3));
    }

    #[test]
    fn a_200_reply_is_ok() {
        let stub = Stub::start(&[("GET", "/api/health", 200, "")]);
        let probe = probe(&target("Grafana", stub.url(), "/api/health"));
        assert_eq!(probe.state, State::Ok);
    }

    #[test]
    fn a_503_reply_is_http_503() {
        let stub = Stub::start(&[("GET", "/-/ready", 503, "")]);
        let probe = probe(&target("Prometheus", stub.url(), "/-/ready"));
        assert_eq!(probe.state, State::Http(503));
    }

    #[test]
    fn a_2xx_status_is_ok_and_400_is_not() {
        let stub = Stub::start(&[("GET", "/a", 299, ""), ("GET", "/b", 400, "")]);
        assert_eq!(probe(&target("Loki", stub.url(), "/a")).state, State::Ok);
        assert_eq!(
            probe(&target("Loki", stub.url(), "/b")).state,
            State::Http(400)
        );
    }

    #[test]
    fn a_probe_keeps_its_target() {
        let stub = Stub::start(&[]);
        let wanted = target("Loki", stub.url(), "/ready");
        assert_eq!(probe(&wanted).target, wanted);
    }

    #[test]
    fn a_probe_sends_one_get_to_the_base_plus_the_path() {
        let stub = Stub::start(&[]);
        probe(&target("Alertmanager", stub.url(), "/-/ready"));

        let requests = stub.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path, "/-/ready");
    }

    #[test]
    fn a_closed_port_is_down_with_a_reason_that_has_no_url_prefix() {
        let url = closed_port_url();
        let probe = probe(&target("Grafana", &url, "/api/health"));
        match probe.state {
            State::Down(reason) => assert!(!reason.starts_with("cannot reach"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_sonda_target_is_probed_at_health() {
        let stub = Stub::start(&[("GET", "/health", 200, "")]);
        let probe = probe(&target("Sonda", stub.url(), "/health"));

        assert_eq!(probe.state, State::Ok);
        let requests = stub.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path, "/health");
    }

    #[test]
    fn a_sonda_503_is_http_503() {
        let stub = Stub::start(&[("GET", "/health", 503, "")]);
        let probe = probe(&target("Sonda", stub.url(), "/health"));
        assert_eq!(probe.state, State::Http(503));
    }

    #[test]
    fn a_closed_sonda_port_is_down() {
        let url = closed_port_url();
        let probe = probe(&target("Sonda", &url, "/health"));
        match probe.state {
            State::Down(reason) => assert!(!reason.starts_with("cannot reach"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_silent_server_is_down_after_the_timeout_not_after_ten_seconds() {
        let stub = Stub::start_silent(Duration::from_secs(20));
        let started = Instant::now();

        let probe = probe(&target("Grafana", stub.url(), "/api/health"));

        assert!(matches!(probe.state, State::Down(_)), "{:?}", probe.state);
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_silent_sonda_is_down_after_the_timeout_not_after_ten_seconds() {
        let stub = Stub::start_silent(Duration::from_secs(20));
        let started = Instant::now();

        let probe = probe(&target("Sonda", stub.url(), "/health"));

        assert!(matches!(probe.state, State::Down(_)), "{:?}", probe.state);
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn probe_all_probes_the_seven_targets_one_after_another_in_order() {
        let stub = Stub::start(&[
            ("GET", "/api/health", 200, ""),
            ("GET", "/-/ready", 503, ""),
            ("GET", "/ready", 200, ""),
        ]);

        let probes = probe_all(&urls_all_at(stub.url()));

        let seen: Vec<(&str, &State)> = probes.iter().map(|p| (p.target.name, &p.state)).collect();
        assert_eq!(
            seen,
            [
                ("Grafana", &State::Ok),
                ("Prometheus", &State::Http(503)),
                ("Loki", &State::Ok),
                ("Alertmanager", &State::Http(503)),
                ("Infrahub", &State::Http(404)),
                ("Prefect", &State::Ok),
                ("Sonda", &State::Http(404)),
            ]
        );
        let paths: Vec<String> = stub.requests().into_iter().map(|r| r.path).collect();
        assert_eq!(
            paths,
            [
                "/api/health",
                "/-/ready",
                "/ready",
                "/-/ready",
                "/health",
                "/api/health",
                "/health"
            ]
        );
    }
}
