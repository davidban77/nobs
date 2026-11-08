use std::time::Instant;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Clear, HighlightSpacing, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::shell::{kind_text, Focus, Model, StackKind, StackState, Tab, Workbench};
use crate::view;

const WIDE: u16 = 100;
const LEFT_NARROW: u16 = 23;
const LEFT_WIDE: u16 = 32;
const TOO_SMALL: &str = "terminal too small: 60x15 needed";
const HELP_HEIGHT: u16 = 11;
const HELP_PLACEHOLDER_HEIGHT: u16 = 8;
const HELP_PLACEHOLDER_NOTE: &str = "the workbench comes with slice 24";
const HELP_SONDA: &str = "sonda tab: l load  d delete  r refresh";
const HELP: [(&str, &str); 14] = [
    ("1 2 3", "switch workbench"),
    ("Tab", "focus list/tabs"),
    ("j k ↑ ↓", "move"),
    ("[ ]", "previous/next tab"),
    ("Enter", "open the row"),
    ("/", "filter"),
    ("r", "refresh now"),
    ("l", "service logs"),
    ("f", "follow/pause logs"),
    ("o", "run pane"),
    ("x", "dismiss the error"),
    ("?", "close"),
    ("q Ctrl-C", "quit"),
    ("Esc", "close/clear/back"),
];
const HELP_PLACEHOLDER: [(&str, &str); 4] = [
    ("1 2 3", "switch workbench"),
    ("?", "close"),
    ("q Ctrl-C", "quit"),
    ("Esc", "close"),
];
const REVERSE: Style = Style::new().add_modifier(Modifier::REVERSED);
const DIM: Style = Style::new().add_modifier(Modifier::DIM);

pub fn draw(frame: &mut Frame, model: &Model) {
    let area = frame.area();
    if model.too_small() {
        too_small(frame, area);
        return;
    }
    let error = model.error();
    let [header_row, switcher_row, error_row, middle, key_row] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(u16::from(error.is_some())),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    let bottom_height = if model.chooser() {
        0
    } else if model.run_pane() {
        middle.height.saturating_sub(1) / 3 + 1
    } else {
        1
    };
    let [body, bottom] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(bottom_height)]).areas(middle);
    header(frame, header_row, model);
    switcher(frame, switcher_row, model);
    if let Some((source, text, at)) = error {
        let line = format!(" {source}: {text}  {}s ago", seconds(model, at));
        frame.render_widget(
            Paragraph::new(line).style(Style::new().fg(Color::Red)),
            error_row,
        );
    }
    workbench(frame, body, model);
    if !model.chooser() {
        if model.run_pane() {
            let block = Block::bordered()
                .title_top(" run  none ")
                .title_top(Line::from(" Esc fold ").right_aligned());
            frame.render_widget(block, bottom);
        } else {
            run_line(frame, bottom);
        }
    }
    frame.render_widget(Paragraph::new(key_line(model)), key_row);
    if model.help() {
        help(frame, body, model.workbench());
    }
}

fn too_small(frame: &mut Frame, area: Rect) {
    frame.render_widget(Clear, area);
    let row = Rect::new(area.x, area.y + area.height / 2, area.width, 1).intersection(area);
    frame.render_widget(Paragraph::new(Line::from(TOO_SMALL).centered()), row);
}

fn seconds(model: &Model, at: Instant) -> u64 {
    model.now().saturating_duration_since(at).as_secs()
}

fn right(frame: &mut Frame, area: Rect, line: Line<'_>, margin: u16) {
    let width = u16::try_from(line.width())
        .unwrap_or(u16::MAX)
        .min(area.width.saturating_sub(margin));
    let x = area.right().saturating_sub(margin + width);
    frame.render_widget(Paragraph::new(line), Rect::new(x, area.y, width, 1));
}

fn marker(selected: bool) -> &'static str {
    if selected {
        ">"
    } else {
        " "
    }
}

fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return format!("{text:<width$}");
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn highlighted(line: Line<'static>, width: u16) -> Line<'static> {
    let padding = usize::from(width).saturating_sub(line.width());
    let mut spans = line.spans;
    spans.push(Span::raw(" ".repeat(padding)));
    Line::from(spans).style(REVERSE)
}

fn header(frame: &mut Frame, area: Rect, model: &Model) {
    let mut text = format!(" {}", model.header());
    if area.width >= WIDE && !model.chooser() {
        text = format!("{text}  {} services", model.services().len());
    }
    frame.render_widget(Paragraph::new(text), area);
    right(frame, area, Line::from(model.clock()), 1);
}

fn switcher(frame: &mut Frame, area: Rect, model: &Model) {
    if model.chooser() {
        frame.render_widget(Paragraph::new(" choose a lab, or q to quit"), area);
        return;
    }
    let mut spans = vec![Span::raw(" ")];
    let entries = [
        (Workbench::Devices, "1 devices"),
        (Workbench::Services, "2 services"),
        (Workbench::Lab, "3 lab"),
    ];
    for (index, (workbench, name)) in entries.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(if workbench == model.workbench() {
            Span::styled(format!("[{name}]"), REVERSE)
        } else if workbench == Workbench::Services {
            Span::raw(name)
        } else {
            Span::styled(name, DIM)
        });
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    right(frame, area, Line::from("? keys  q"), 1);
}

fn workbench(frame: &mut Frame, area: Rect, model: &Model) {
    if model.chooser() {
        chooser(frame, area, model);
        return;
    }
    match model.workbench() {
        Workbench::Devices => frame.render_widget(
            Paragraph::new(" the device workbench comes with slice 24"),
            area,
        ),
        Workbench::Lab => frame.render_widget(
            Paragraph::new(" the lab workbench comes with slice 24"),
            area,
        ),
        Workbench::Services => services(frame, area, model),
    }
}

fn state_style(state: &StackState) -> Style {
    match state {
        StackState::Up { running, listed } if running == listed => Style::new().fg(Color::Green),
        StackState::Up { .. } => Style::new().fg(Color::Yellow),
        StackState::Down { listed: 0 } | StackState::Unknown | StackState::Failed => {
            Style::new().fg(Color::Red)
        }
        StackState::Down { .. } => DIM,
    }
}

fn tone(text: &str) -> Style {
    match text {
        "running" | "ok" | "established" => Style::new().fg(Color::Green),
        "starting" | "pending" | "paused" | "waiting" | "restarting" => {
            Style::new().fg(Color::Yellow)
        }
        _ if text.starts_with("http") => Style::new().fg(Color::Yellow),
        "down" | "failed" | "firing" | "exited" | "dead" => Style::new().fg(Color::Red),
        "finished" | "stopped" | "created" => DIM,
        _ => Style::new(),
    }
}

fn services(frame: &mut Frame, area: Rect, model: &Model) {
    let wide = area.width >= WIDE;
    let left_width = if wide { LEFT_WIDE } else { LEFT_NARROW };
    let [left, right] =
        Layout::horizontal([Constraint::Length(left_width), Constraint::Min(0)]).areas(area);
    stacks(frame, left, model);
    detail(frame, right, model, wide);
}

fn stacks(frame: &mut Frame, area: Rect, model: &Model) {
    let visible = model.visible_stacks();
    let block = Block::bordered().title_top(format!(" stacks {} ", visible.len()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let longest = visible
        .iter()
        .map(|index| model.stacks()[*index].name.chars().count())
        .max()
        .unwrap_or(0);
    let name_width = longest.min(usize::from(inner.width).saturating_sub(11));
    let lines: Vec<Line> = visible
        .iter()
        .map(|&index| {
            let state = model.stack_state(index);
            let selected = index == model.selected_stack();
            let line = Line::from(vec![
                Span::raw(format!(
                    "{}{}  ",
                    marker(selected),
                    fit(&model.stacks()[index].name, name_width)
                )),
                Span::styled(state.text(), state_style(state)),
            ]);
            if selected {
                highlighted(line, inner.width)
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn detail(frame: &mut Frame, area: Rect, model: &Model, wide: bool) {
    let name = model
        .stacks()
        .get(model.selected_stack())
        .map_or("", |stack| stack.name.as_str());
    let age = match model.services_at() {
        Some(at) => format!("refreshed {}s ago", seconds(model, at)),
        None => "no answer yet".to_string(),
    };
    let title = if wide {
        format!(" {name}  {} services  {age} ", model.services().len())
    } else {
        format!(" {name}  {age} ")
    };
    let block = Block::bordered().title_top(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [tab_row, content] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    tabs(frame, tab_row, model);
    match model.tab() {
        Tab::Overview => overview(frame, content, model, wide),
        Tab::Logs => logs(frame, content, model),
        Tab::Sonda => view::draw_in(frame, content, model.sonda(), model.now()),
    }
}

fn tabs(frame: &mut Frame, area: Rect, model: &Model) {
    let mut spans = Vec::new();
    let entries = [
        (Tab::Overview, "overview"),
        (Tab::Logs, "logs"),
        (Tab::Sonda, "sonda"),
    ];
    for (index, (tab, name)) in entries.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(if tab == model.tab() {
            Span::styled(name.to_uppercase(), REVERSE)
        } else {
            Span::raw(name)
        });
    }
    let line = Line::from(spans);
    let room = usize::from(area.width).saturating_sub(line.width() + 2);
    frame.render_widget(Paragraph::new(line), area);
    if let (Tab::Logs, Some((service, _, following, _))) = (model.tab(), model.logs()) {
        let mode = if following { "following" } else { "paused" };
        let status = format!("{service}  {mode}");
        let status = if status.chars().count() <= room {
            status
        } else {
            mode.to_string()
        };
        right(frame, area, Line::from(status), 0);
    }
}

fn overview(frame: &mut Frame, area: Rect, model: &Model, wide: bool) {
    let dash = || "—".to_string();
    let rows: Vec<[String; 5]> = model
        .services()
        .iter()
        .map(|service| {
            [
                service.name.clone(),
                service.state.clone(),
                service.health.clone().unwrap_or_else(dash),
                service.port.clone().unwrap_or_else(dash),
                service.age.clone().unwrap_or_else(dash),
            ]
        })
        .collect();
    let columns = if wide { 5 } else { 4 };
    let titles = [
        "SERVICE",
        "STATE",
        if wide { "HEALTH" } else { "HLTH" },
        "PORT",
        "AGE",
    ];
    let mut widths = titles.map(|title| title.chars().count());
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let fixed = 1 + widths[1..columns]
        .iter()
        .map(|width| width + 2)
        .sum::<usize>();
    widths[0] = widths[0].min(usize::from(area.width).saturating_sub(fixed));
    let on_tabs = model.focus() == Focus::Tabs;
    let selected = (!rows.is_empty()).then(|| model.selected_service());
    let body_rows = rows.iter().map(|row| {
        let cells = [
            Cell::from(fit(&row[0], widths[0])),
            Cell::from(row[1].as_str()).style(tone(&row[1])),
            Cell::from(row[2].as_str()).style(tone(&row[2])),
            Cell::from(row[3].as_str()),
            Cell::from(row[4].as_str()).style(DIM),
        ];
        Row::new(cells.into_iter().take(columns))
    });
    let constraints = widths[..columns]
        .iter()
        .map(|width| Constraint::Length(u16::try_from(*width).unwrap_or(u16::MAX)));
    let table = Table::new(body_rows, constraints)
        .header(Row::new(titles[..columns].iter().copied()))
        .column_spacing(2)
        .highlight_spacing(HighlightSpacing::Always)
        .highlight_symbol(if on_tabs { ">" } else { " " })
        .row_highlight_style(if on_tabs { REVERSE } else { Style::new() });
    let mut state = TableState::default().with_selected(selected);
    frame.render_stateful_widget(table, area, &mut state);
}

fn logs(frame: &mut Frame, area: Rect, model: &Model) {
    let Some((_, lines, following, scroll)) = model.logs() else {
        return;
    };
    let end = if following {
        lines.len()
    } else {
        lines.len().saturating_sub(scroll)
    };
    let start = end.saturating_sub(usize::from(area.height));
    let shown: Vec<Line> = lines[start..end]
        .iter()
        .map(|line| Line::from(*line))
        .collect();
    frame.render_widget(Paragraph::new(shown), area);
}

fn chooser(frame: &mut Frame, area: Rect, model: &Model) {
    let block = Block::bordered().title_top(" labs found ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let stacks = model.stacks();
    let paths: Vec<String> = stacks
        .iter()
        .map(|stack| stack.path.display().to_string())
        .collect();
    let name_width = stacks
        .iter()
        .map(|stack| stack.name.chars().count())
        .max()
        .unwrap_or(0);
    let workshops = || stacks.iter().filter(|stack| stack.kind != StackKind::Sonda);
    let kind_width = workshops()
        .map(|stack| kind_text(stack.kind).len())
        .max()
        .unwrap_or(0);
    let path_width = workshops()
        .map(|stack| stack.path.display().to_string().chars().count())
        .max()
        .unwrap_or(0);
    let lines: Vec<Line> = stacks
        .iter()
        .zip(&paths)
        .enumerate()
        .map(|(index, (stack, path))| {
            let selected = index == model.selected_stack();
            let spans = if stack.kind == StackKind::Sonda {
                vec![Span::raw(format!(
                    "{}{}  {}",
                    marker(selected),
                    fit(&stack.name, name_width),
                    model.sonda_address(index)
                ))]
            } else {
                let state = model.stack_state(index);
                vec![
                    Span::raw(format!(
                        "{}{}  {}  {}  ",
                        marker(selected),
                        fit(&stack.name, name_width),
                        fit(kind_text(stack.kind), kind_width),
                        fit(path, path_width)
                    )),
                    Span::styled(state.text(), state_style(state)),
                ]
            };
            let line = Line::from(spans);
            if selected {
                highlighted(line, inner.width)
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn run_line(frame: &mut Frame, area: Rect) {
    frame.render_widget(Paragraph::new(" run  none"), area);
    right(frame, area, Line::from("o open"), 1);
}

fn key_line(model: &Model) -> String {
    if let Some(text) = model.typed_filter() {
        return format!(" filter: {text}  Enter keep  Esc clear");
    }
    let keys = if model.chooser() {
        " j/k select  Enter open  q quit"
    } else if model.workbench() != Workbench::Services {
        " 1 2 3 switch  ? keys  q quit"
    } else {
        match (model.focus(), model.tab()) {
            (Focus::List, _) => " Tab focus  j/k move  / filter  l logs  r refresh",
            (Focus::Tabs, Tab::Overview) => " Tab focus  j/k move  [ ] tab  Enter logs  Esc back",
            (Focus::Tabs, Tab::Logs) => match model.logs() {
                Some((_, _, false, _)) => {
                    " f follow  / filter  j/k scroll  [ ] tab  Esc back to list"
                }
                _ => " f pause  / filter  j/k scroll  [ ] tab  Esc back to list",
            },
            (Focus::Tabs, Tab::Sonda) => " Tab focus  [ ] tab  Esc back",
        }
    };
    keys.to_string()
}

fn help(frame: &mut Frame, body: Rect, workbench: Workbench) {
    let placeholder = workbench != Workbench::Services;
    let width = body.width.saturating_sub(4);
    let height = body.height.min(if placeholder {
        HELP_PLACEHOLDER_HEIGHT
    } else {
        HELP_HEIGHT
    });
    let popup = Rect::new(
        body.x + (body.width - width) / 2,
        body.y + (body.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, body);
    let name = match workbench {
        Workbench::Devices => "devices",
        Workbench::Services => "services",
        Workbench::Lab => "lab",
    };
    let block = Block::bordered().title_top(format!(" keys  {name} workbench "));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let column = |x: u16, y: u16, width: u16, height: u16| {
        Rect::new(inner.x + x, inner.y + y, width, height).intersection(inner)
    };
    if placeholder {
        let rows = u16::try_from(HELP_PLACEHOLDER.len()).unwrap_or(u16::MAX);
        frame.render_widget(
            Paragraph::new(help_column(&HELP_PLACEHOLDER)),
            column(0, 0, inner.width, rows),
        );
        frame.render_widget(
            Paragraph::new(HELP_PLACEHOLDER_NOTE),
            column(0, rows + 1, inner.width, 1),
        );
        return;
    }
    let (first, second) = HELP.split_at(HELP.len() / 2);
    let rows = u16::try_from(first.len()).unwrap_or(u16::MAX);
    let middle = inner.width / 2;
    frame.render_widget(
        Paragraph::new(help_column(first)),
        column(0, 0, middle, rows),
    );
    frame.render_widget(
        Paragraph::new(help_column(second)),
        column(middle, 0, inner.width - middle, rows),
    );
    frame.render_widget(
        Paragraph::new(HELP_SONDA),
        column(0, rows + 1, inner.width, 1),
    );
}

fn help_column(entries: &[(&str, &str)]) -> String {
    let key_width = entries
        .iter()
        .map(|(key, _)| key.chars().count())
        .max()
        .unwrap_or(0)
        + 2;
    let lines: Vec<String> = entries
        .iter()
        .map(|(key, meaning)| format!("{key:<key_width$}{meaning}"))
        .collect();
    lines.join("\n")
}
