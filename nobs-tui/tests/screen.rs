use std::time::{Duration, Instant};

use nobs_core::sonda::Sonda;
use nobs_tui::data::{Model, Poller};
use nobs_tui::view::draw;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[path = "../../tests/support/stub.rs"]
mod stub;

use stub::Stub;

const ID: &str = "5f2c9e1a-77b0-4c1e-9d3a-2b6f0c8e4d11";

#[test]
fn the_first_update_of_the_poller_draws_the_fixture_scenario_and_its_stats() {
    let list = format!(
        r#"{{"scenarios":[{{"id":"{ID}","name":"nobs-t-hello","state":"running","elapsed_secs":12,"degraded":false}}]}}"#
    );
    let stub = Stub::start(&[
        ("GET", "/scenarios", 200, &list),
        (
            "GET",
            &format!("/scenarios/{ID}"),
            200,
            r#"{"name":"nobs-t-hello"}"#,
        ),
        (
            "GET",
            &format!("/scenarios/{ID}/stats"),
            200,
            r#"{"total_events":12,"current_rate":1.0}"#,
        ),
    ]);
    let (poller, updates, _notes) = Poller::start(Sonda::new(stub.url()), Duration::from_secs(1));
    let update = updates.recv_timeout(Duration::from_secs(5)).unwrap();
    poller.stop();
    let mut model = Model::new(stub.url().to_string());
    model.apply(update);

    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let now = Instant::now();
    terminal.draw(|frame| draw(frame, &model, now)).unwrap();

    let buffer = terminal.backend().buffer();
    let lines: Vec<String> = (0..30)
        .map(|y| (0..100).map(|x| buffer[(x, y)].symbol()).collect())
        .collect();
    let screen = lines.join("\n");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("5f2c9e1a ") && line.contains("nobs-t-hello")),
        "{screen}"
    );
    assert!(screen.contains("total_events  12"), "{screen}");
    assert!(screen.contains("current_rate  1.0"), "{screen}");
}
