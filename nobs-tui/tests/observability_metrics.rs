use std::time::Duration;

use nobs_core::lab::services;
use nobs_tui::{Command, Tab, Update, Workbench};

#[path = "../../tests/support/stub.rs"]
mod stub;
use stub::Stub;

mod pollers;
use pollers::{press, pump_until, send, shell, start_pollers};

const PROMETHEUS_BODY: &str = r#"{"status":"success","data":{"resultType":"vector","result":[
    {"metric":{"__name__":"bgp_oper_state","device":"srl1","peer_address":"10.1.2.2"},"value":[1793960400,"1"]}
]}}"#;
const QUICK: Duration = Duration::from_millis(500);

fn prometheus_listed() -> Update {
    Update::Services(
        0,
        Ok(vec!["prometheus".to_string()]),
        Ok(services(r#"{"Service":"prometheus","State":"running"}"#).unwrap()),
    )
}

fn queries(stub: &Stub) -> usize {
    stub.requests()
        .iter()
        .filter(|request| request.path.starts_with("/api/v1/query"))
        .count()
}

#[test]
fn opening_and_leaving_the_metrics_view_sends_no_new_request() {
    let stub = Stub::start(&[("GET", "/api/v1/query*", 200, PROMETHEUS_BODY)]);
    let (pollers, updates) = start_pollers(&stub);
    let mut model = shell(Workbench::Devices);
    send(&pollers, model.apply(prometheus_listed()));
    let metrics = |update: &Update| matches!(update, Update::Metrics(Ok(_)));
    assert!(pump_until(
        &mut model,
        &pollers,
        &updates,
        Duration::from_secs(5),
        metrics
    ));
    let before = queries(&stub);

    let mut sent = Vec::new();
    for typed in "3[][".chars() {
        let commands = press(&mut model, typed);
        sent.extend(commands.clone());
        send(&pollers, commands);
    }
    pump_until(&mut model, &pollers, &updates, QUICK, |_| false);
    let after_the_keys = queries(&stub);
    send(&pollers, press(&mut model, 'r'));
    pump_until(&mut model, &pollers, &updates, QUICK, |_| false);
    let after_refresh = queries(&stub);
    pollers.send(Command::Quit);

    assert_eq!(model.tab(), Tab::Metrics);
    assert!(
        !sent
            .iter()
            .any(|command| matches!(command, Command::Ask(..))),
        "{sent:?}"
    );
    assert_eq!(after_the_keys, before);
    assert!(after_refresh > after_the_keys);
}
