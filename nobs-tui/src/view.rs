use std::time::Instant;

use nobs_core::sonda::pretty;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::data::{Model, Prompt};
use crate::screen::{
    border_below, cut_words, first_row, fit, rows_mark, whole_hints, widest, wrapped, Keep,
};

const SELECT: &str = "j/k select";
const KEYS: [&str; 5] = ["q quit", SELECT, "r refresh", "l load", "d delete"];

pub fn draw(frame: &mut Frame, model: &Model, now: Instant) {
    draw_in(frame, frame.area(), model, now, false);
}

pub fn draw_in(frame: &mut Frame, area: Rect, model: &Model, now: Instant, details_focused: bool) {
    let [top, left, right, bottom] = panes(area);
    frame.render_widget(
        Paragraph::new(top_line(model, now, usize::from(top.width))),
        top,
    );
    let rows = Block::bordered().inner(left);
    let room = usize::from(rows.height).saturating_sub(1);
    let first = first_row(model.selected_index().unwrap_or(0), room);
    let mut state = TableState::default()
        .with_selected(model.selected_index())
        .with_offset(first);
    frame.render_stateful_widget(table(model, left.width), left, &mut state);
    rows_mark(frame, border_below(rows), first, room, model.rows().len());
    let border = if details_focused {
        BorderType::Double
    } else {
        BorderType::Plain
    };
    let block = Block::bordered().border_type(border);
    let inner = block.inner(right);
    frame.render_widget(block, right);
    let lines = details_lines(model, inner.width);
    let (room, total) = (usize::from(inner.height), lines.len());
    let first = model.details_scroll().min(total.saturating_sub(room));
    let shown: Vec<Line> = lines.into_iter().skip(first).collect();
    frame.render_widget(Paragraph::new(shown), inner);
    rows_mark(frame, border_below(inner), first, room, total);
    frame.render_widget(
        Paragraph::new(bottom_line(
            model,
            now,
            usize::from(bottom.width),
            details_focused,
        )),
        bottom,
    );
}

fn panes(area: Rect) -> [Rect; 4] {
    let [top, middle, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(middle);
    [top, left, right, bottom]
}

pub(crate) fn details_overflow(model: &Model, area: Rect) -> usize {
    let [_, _, right, _] = panes(area);
    let inner = Block::bordered().inner(right);
    details_lines(model, inner.width)
        .len()
        .saturating_sub(usize::from(inner.height))
}

fn top_line(model: &Model, now: Instant, width: usize) -> Line<'static> {
    let age = match model.refreshed_at() {
        Some(at) => format!("refreshed {}s ago", seconds(now, at)),
        None => "no answer yet".to_string(),
    };
    let server = model.server();
    let error = model
        .error()
        .zip(model.error_at())
        .map(|(text, at)| (text, format!(" ({}s ago)", seconds(now, at))));
    let whole = error.as_ref().map(|(text, at)| format!("{text}{at}"));
    let red = Style::new().fg(Color::Red);
    let plain = [vec![server, age.as_str()], vec![server], Vec::new()]
        .into_iter()
        .filter(|plain| !plain.is_empty() || whole.is_some())
        .find(|plain| {
            let parts: Vec<&str> = plain.iter().copied().chain(whole.as_deref()).collect();
            parts.join("  ").chars().count() <= width
        });
    let Some(plain) = plain else {
        return match error {
            Some((text, at)) => {
                let room = width.saturating_sub(at.chars().count());
                Line::from(Span::styled(
                    format!("{}{at}", cut_words(text, room, Keep::End)),
                    red,
                ))
            }
            None => Line::from(cut_words(server, width, Keep::Start)),
        };
    };
    let gap = if plain.is_empty() { "" } else { "  " };
    let mut spans: Vec<Span<'static>> = (!plain.is_empty())
        .then(|| Span::raw(plain.join("  ")))
        .into_iter()
        .collect();
    spans.extend(whole.map(|whole| Span::styled(format!("{gap}{whole}"), red)));
    Line::from(spans)
}

fn seconds(now: Instant, at: Instant) -> u64 {
    now.saturating_duration_since(at).as_secs()
}

fn table(model: &Model, width: u16) -> Table<'_> {
    const TITLES: [&str; 5] = ["ID", "NAME", "STATE", "ELAPSED", "DEGRADED"];
    let cells: Vec<[String; 5]> = model
        .rows()
        .iter()
        .map(|row| {
            [
                row.id.chars().take(8).collect::<String>(),
                row.name.clone(),
                row.state.clone(),
                row.elapsed_secs.clone(),
                row.degraded.clone(),
            ]
        })
        .collect();
    let mut widths = widest(TITLES, &cells);
    let room = usize::from(width).saturating_sub(2 + "> ".len());
    let mut kept = [true; 5];
    for leaving in [0, 4, 3, 2] {
        let used = (0..TITLES.len())
            .filter(|column| kept[*column])
            .map(|column| widths[column] + 1)
            .sum::<usize>()
            .saturating_sub(1);
        if used <= room {
            break;
        }
        kept[leaving] = false;
    }
    let columns: Vec<usize> = (0..TITLES.len()).filter(|column| kept[*column]).collect();
    widths[1] = widths[1].min(room);
    let rows = cells.into_iter().map(|row| {
        Row::new(columns.iter().map(|column| match column {
            1 => fit(&row[1], widths[1]),
            _ => row[*column].clone(),
        }))
    });
    let constraints = columns.iter().map(|column| {
        let least = u16::try_from(widths[*column]).unwrap_or(u16::MAX);
        if *column == 1 {
            Constraint::Min(least)
        } else {
            Constraint::Length(least)
        }
    });
    Table::new(rows, constraints)
        .header(Row::new(columns.iter().map(|column| TITLES[*column])))
        .block(Block::bordered())
        .highlight_symbol("> ")
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
}

fn details_lines(model: &Model, width: u16) -> Vec<Line<'static>> {
    let mut lines = model.stats_body().map(stats_lines).unwrap_or_default();
    if let Some(show) = model.show_body() {
        lines.extend(pretty(show).lines().map(String::from));
    }
    lines
        .iter()
        .flat_map(|line| wrapped(line, usize::from(width)))
        .map(Line::from)
        .collect()
}

fn stats_lines(body: &str) -> Vec<String> {
    let text = pretty(body);
    match text
        .strip_prefix("{\n")
        .and_then(|inner| inner.strip_suffix("\n}"))
    {
        Some(fields) => fields.lines().map(field).collect(),
        None => text.lines().map(String::from).collect(),
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

fn bottom_line(model: &Model, now: Instant, width: usize, details_focused: bool) -> String {
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
        Prompt::None => match model.status(now) {
            Some(status) => status.to_string(),
            None => {
                let keys: Vec<String> = KEYS
                    .into_iter()
                    .filter(|key| !details_focused || *key != SELECT)
                    .map(String::from)
                    .collect();
                whole_hints(&keys, width).trim_start().to_string()
            }
        },
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
    fn the_top_line_holds_the_error_and_its_age_after_the_age_of_the_last_answer() {
        let at = Instant::now();
        let mut model = model(at);
        model.apply(Update {
            at: at + Duration::from_secs(1),
            list: Err("cannot reach http://sonda: refused".to_string()),
            selected: None,
        });

        let lines = screen(&model, at + Duration::from_secs(3), 100, 30);

        assert!(
            lines[0].contains("refreshed 3s ago  cannot reach http://sonda: refused (2s ago)"),
            "{}",
            lines[0]
        );
    }

    #[test]
    fn a_long_sonda_error_keeps_its_reason_and_its_age() {
        let at = Instant::now();
        let mut model = model(at);
        model.apply(Update {
            at: at + Duration::from_secs(1),
            list: Err(
                "cannot reach http://127.0.0.1:18122/scenarios: io: Connection refused (os error 61)"
                    .to_string(),
            ),
            selected: None,
        });

        let line = top_line(&model, at + Duration::from_secs(3), 60);

        assert_eq!(
            line,
            Line::from(Span::styled(
                "… io: Connection refused (os error 61) (2s ago)",
                Style::new().fg(Color::Red)
            ))
        );
    }

    #[test]
    fn a_top_line_too_short_for_the_age_keeps_the_server_and_a_short_error() {
        let at = Instant::now();
        let mut model = model(at);
        model.apply(Update {
            at: at + Duration::from_secs(1),
            list: Err("refused".to_string()),
            selected: None,
        });

        let line = top_line(&model, at + Duration::from_secs(3), 40);

        assert_eq!(
            line,
            Line::from(vec![
                Span::raw("http://127.0.0.1:18122"),
                Span::styled("  refused (2s ago)", Style::new().fg(Color::Red)),
            ])
        );
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
    fn every_sonda_title_and_cell_is_whole_at_60_and_100_columns() {
        let now = Instant::now();
        let titles = ["ID", "NAME", "STATE", "ELAPSED", "DEGRADED"];
        for width in 60..=100 {
            let lines = screen(&model(now), now, width, 15);
            let table = |line: &str| line.split('│').nth(1).unwrap_or_default().to_string();
            let top = line_of(&lines, "NAME");
            let header: Vec<String> = table(&lines[top])
                .split_whitespace()
                .map(String::from)
                .collect();
            let gone: Vec<&str> = titles
                .into_iter()
                .filter(|title| !header.iter().any(|kept| kept == title))
                .collect();
            assert!(
                ["ID", "DEGRADED", "ELAPSED"].starts_with(&gone),
                "{width}: {header:?}"
            );
            let kept: Vec<usize> = (0..5)
                .filter(|column| !gone.contains(&titles[*column]))
                .collect();
            assert_eq!(
                header,
                kept.iter()
                    .map(|column| titles[*column])
                    .collect::<Vec<_>>(),
                "{width}"
            );
            for (index, state) in STATES.iter().enumerate() {
                let cells = [
                    format!("{index}abcdefg"),
                    format!("name-{index}"),
                    (*state).to_string(),
                    index.to_string(),
                    "false".to_string(),
                ];
                let row = table(&lines[top + 1 + index]);
                let shown: Vec<&str> = row.trim_start_matches('>').split_whitespace().collect();
                let expected: Vec<&str> =
                    kept.iter().map(|column| cells[*column].as_str()).collect();
                assert_eq!(shown, expected, "{width}");
            }
            if width == 100 {
                assert!(gone.is_empty(), "{header:?}");
            }
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

        assert!(
            lines[29].starts_with("q quit  j/k select  r refresh  l load  d delete"),
            "{}",
            lines[29]
        );
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
        assert!(
            after[29].starts_with("q quit  j/k select  r refresh  l load  d delete"),
            "{}",
            after[29]
        );
    }

    #[test]
    fn a_60_by_15_terminal_draws_without_a_panic() {
        let now = Instant::now();

        let lines = screen(&model(now), now, 60, 15);

        assert_eq!(lines.len(), 15);
    }
}
