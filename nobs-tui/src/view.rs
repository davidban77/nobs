use std::time::Instant;

use nobs_core::sonda::pretty;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::data::{Model, Prompt};

const KEYS: &str = "q quit  j/k select  r refresh  l load  d delete";

pub fn draw(frame: &mut Frame, model: &Model, now: Instant) {
    let [top, middle, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(middle);
    frame.render_widget(Paragraph::new(top_line(model, now)), top);
    let mut state = TableState::default().with_selected(model.selected_index());
    frame.render_stateful_widget(table(model), left, &mut state);
    frame.render_widget(
        Paragraph::new(details(model)).block(Block::bordered()),
        right,
    );
    frame.render_widget(Paragraph::new(bottom_line(model, now)), bottom);
}

fn top_line(model: &Model, now: Instant) -> Line<'static> {
    let age = match model.refreshed_at() {
        Some(at) => format!(
            "refreshed {}s ago",
            now.saturating_duration_since(at).as_secs()
        ),
        None => "no answer yet".to_string(),
    };
    let mut spans = vec![Span::raw(format!("{}  {age}", model.server()))];
    if let Some(error) = model.error() {
        spans.push(Span::styled(
            format!("  {error}"),
            Style::new().fg(Color::Red),
        ));
    }
    Line::from(spans)
}

fn table(model: &Model) -> Table<'_> {
    let rows = model.rows().iter().map(|row| {
        Row::new([
            row.id.chars().take(8).collect::<String>(),
            row.name.clone(),
            row.state.clone(),
            row.elapsed_secs.clone(),
            row.degraded.clone(),
        ])
    });
    Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Min(4),
            Constraint::Length(10),
            Constraint::Length(7),
            Constraint::Length(8),
        ],
    )
    .header(Row::new(["ID", "NAME", "STATE", "ELAPSED", "DEGRADED"]))
    .block(Block::bordered())
    .highlight_symbol("> ")
    .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
}

fn details(model: &Model) -> Vec<Line<'static>> {
    let mut lines = model.stats_body().map(stats_lines).unwrap_or_default();
    if let Some(show) = model.show_body() {
        lines.extend(
            pretty(show)
                .lines()
                .map(|line| Line::from(line.to_string())),
        );
    }
    lines
}

fn stats_lines(body: &str) -> Vec<Line<'static>> {
    let text = pretty(body);
    match text
        .strip_prefix("{\n")
        .and_then(|inner| inner.strip_suffix("\n}"))
    {
        Some(fields) => fields.lines().map(|line| Line::from(field(line))).collect(),
        None => text
            .lines()
            .map(|line| Line::from(line.to_string()))
            .collect(),
    }
}

fn field(line: &str) -> String {
    let line = line.trim_start();
    let line = line.strip_suffix(',').unwrap_or(line);
    match line
        .strip_prefix('"')
        .and_then(|rest| rest.split_once("\": "))
    {
        Some((key, value)) => format!("{key}  {value}"),
        None => line.to_string(),
    }
}

fn bottom_line(model: &Model, now: Instant) -> String {
    match model.prompt() {
        Prompt::Path(path) => format!("load: {path}"),
        Prompt::Delete(id) => {
            let name = model
                .rows()
                .iter()
                .find(|row| &row.id == id)
                .map_or(id.as_str(), |row| row.name.as_str());
            format!("delete {name}? y/n")
        }
        Prompt::None => model.status(now).unwrap_or(KEYS).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
    use crate::data::Update;

    const SERVER: &str = "http://127.0.0.1:18122";
    const STATES: [&str; 7] = [
        "pending",
        "running",
        "paused",
        "held",
        "unresolved",
        "finished",
        "stopped",
    ];
    const STATS: &str = r#"{"total_events":3359,"current_rate":1.0,"state":"running","errors":0}"#;
    const SHOW: &str = r#"{"config":{"rate":1}}"#;

    fn model(at: Instant) -> Model {
        let rows: Vec<String> = STATES
            .iter()
            .enumerate()
            .map(|(index, state)| {
                format!(
                    r#"{{"id":"{index}abcdefghij","name":"name-{index}","state":"{state}","elapsed_secs":{index},"degraded":false}}"#
                )
            })
            .collect();
        let mut model = Model::new(SERVER.to_string());
        model.apply(Update {
            at,
            list: Ok(format!(r#"{{"scenarios":[{}]}}"#, rows.join(","))),
            selected: Some((
                "0abcdefghij".to_string(),
                Ok(SHOW.to_string()),
                Ok(STATS.to_string()),
            )),
        });
        model
    }

    fn screen(model: &Model, now: Instant, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, model, now)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    fn press(model: &mut Model, code: KeyCode) {
        model.handle(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn line_of(lines: &[String], text: &str) -> usize {
        lines
            .iter()
            .position(|line| line.contains(text))
            .unwrap_or_else(|| panic!("{text} not in\n{}", lines.join("\n")))
    }

    #[test]
    fn the_top_line_holds_the_server_and_the_age_of_the_last_answer() {
        let at = Instant::now();

        let lines = screen(&model(at), at + Duration::from_secs(2), 100, 30);

        assert!(lines[0].contains(SERVER), "{}", lines[0]);
        assert!(lines[0].contains("refreshed 2s ago"), "{}", lines[0]);
    }

    #[test]
    fn the_top_line_says_no_answer_yet_before_the_first_answer() {
        let lines = screen(&Model::new(SERVER.to_string()), Instant::now(), 100, 30);

        assert!(lines[0].contains("no answer yet"), "{}", lines[0]);
    }

    #[test]
    fn the_table_holds_the_header_and_one_row_per_scenario_with_its_state() {
        let now = Instant::now();

        let lines = screen(&model(now), now, 100, 30);

        let header = &lines[line_of(&lines, "ID")];
        for column in ["NAME", "STATE", "ELAPSED", "DEGRADED"] {
            assert!(header.contains(column), "{header}");
        }
        for (index, state) in STATES.iter().enumerate() {
            let row = &lines[line_of(&lines, &format!("name-{index} "))];
            assert!(row.contains(&format!("{index}abcdefg ")), "{row}");
            assert!(row.contains(state), "{row}");
        }
    }

    #[test]
    fn the_selected_row_is_marked() {
        let now = Instant::now();

        let lines = screen(&model(now), now, 100, 30);

        assert!(lines[line_of(&lines, "name-0")].contains("> 0abcdefg"));
        assert!(!lines[line_of(&lines, "name-1")].contains("> "));
    }

    #[test]
    fn the_right_panel_holds_the_stats_lines_then_the_show_lines() {
        let now = Instant::now();

        let lines = screen(&model(now), now, 100, 30);

        let total = line_of(&lines, "total_events  3359");
        for text in ["current_rate  1.0", "errors  0", "state  \"running\""] {
            line_of(&lines, text);
        }
        assert!(total < line_of(&lines, "\"config\": {"));
    }

    #[test]
    fn the_bottom_line_holds_the_keys() {
        let now = Instant::now();

        let lines = screen(&model(now), now, 100, 30);

        assert!(lines[29].starts_with(KEYS), "{}", lines[29]);
    }

    #[test]
    fn the_bottom_line_holds_the_path_prompt_while_it_is_open() {
        let now = Instant::now();
        let mut model = model(now);
        press(&mut model, KeyCode::Char('l'));
        for typed in "a.yaml".chars() {
            press(&mut model, KeyCode::Char(typed));
        }

        let lines = screen(&model, now, 100, 30);

        assert!(lines[29].starts_with("load: a.yaml "), "{}", lines[29]);
    }

    #[test]
    fn the_bottom_line_holds_the_delete_question_with_the_name() {
        let now = Instant::now();
        let mut model = model(now);
        press(&mut model, KeyCode::Char('d'));

        let lines = screen(&model, now, 100, 30);

        assert!(lines[29].starts_with("delete name-0? y/n"), "{}", lines[29]);
    }

    #[test]
    fn the_bottom_line_holds_a_note_for_three_seconds() {
        let now = Instant::now();
        let mut model = model(now);
        model.note("loaded x1".to_string(), now);

        let within = screen(&model, now + Duration::from_secs(2), 100, 30);
        let after = screen(&model, now + Duration::from_secs(3), 100, 30);

        assert!(within[29].starts_with("loaded x1"), "{}", within[29]);
        assert!(after[29].starts_with(KEYS), "{}", after[29]);
    }

    #[test]
    fn a_60_by_15_terminal_draws_without_a_panic() {
        let now = Instant::now();

        let lines = screen(&model(now), now, 60, 15);

        assert_eq!(lines.len(), 15);
    }
}
