use std::time::{Instant, UNIX_EPOCH};

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Cell, Clear, HighlightSpacing, Paragraph, Row, Table, TableState, Wrap,
};
use ratatui::Frame;

use nobs_core::count::counted;
use nobs_core::locate::Located;
use nobs_core::time;

use crate::actions::{arguments, Target, ACTIONS};
use crate::devices;
use crate::run::End;
use crate::shell::{
    kind_text, Cursor, Focus, Model, Run, RunState, StackKind, StackState, Tab, Workbench,
    DEVICE_TABS, SERVICES_TABS,
};
use crate::view;

const WIDE: u16 = 100;
const LEFT_NARROW: u16 = 23;
const LEFT_WIDE: u16 = 32;
const LEFT_DEVICES: u16 = 42;
const TOO_SMALL: &str = "terminal too small: needs 60 columns and 15 rows";
const HELP_DEVICES_NOTE: &str = "alerts box: Enter picks the device, opens its timeline";
const HELP_SONDA: &str = "sonda tab: l load  d delete  r refresh";
const HELP_QUIT: [(&str, &str); 2] = [
    ("q", "quit; asks in a run"),
    ("Ctrl-C", "stop a run, or quit"),
];
const HELP: [(&str, &str); 12] = [
    ("Tab", "focus list/tabs"),
    ("j k ↑ ↓", "move"),
    ("[ ]", "other tab"),
    ("Enter", "open the row"),
    ("/", "filter"),
    ("r", "refresh now"),
    ("l", "service logs"),
    ("f", "follow/pause logs"),
    ("o", "run pane"),
    ("x", "dismiss the error"),
    ("?", "close"),
    ("Esc", "close/clear/back"),
];
const HELP_DEVICES: [(&str, &str); 12] = [
    ("Tab", "focus list/tabs"),
    ("j k ↑ ↓", "move"),
    ("[ ]", "other tab"),
    ("- +", "timeline window"),
    ("x", "dismiss the error"),
    ("Esc", "close/clear/back"),
    ("Enter", "open / pick alert"),
    ("/", "filter"),
    ("r", "refresh now"),
    ("f", "follow/pause logs"),
    ("o", "run pane"),
    ("?", "close"),
];
const HELP_LAB: [(&str, &str); 6] = [
    ("j k ↑ ↓", "move"),
    ("r", "refresh now"),
    ("o", "run pane"),
    ("x", "dismiss the error"),
    ("?", "close"),
    ("Esc", "close/fold"),
];
const WORKBENCHES: [(Workbench, &str, &str); 3] = [
    (Workbench::Devices, "1", "devices"),
    (Workbench::Services, "2", "services"),
    (Workbench::Lab, "3", "lab"),
];
const HELP_MORE: &str = "j/k more";
const FOLD: &str = "Esc fold";
const PANE_LEAST: u16 = 3;
const NODES_LEAST: u16 = 4;
const ALERTS_LEAST: u16 = 2;
const BORDERS: u16 = 2;
const REVERSE: Style = Style::new().add_modifier(Modifier::REVERSED);
const DIM: Style = Style::new().add_modifier(Modifier::DIM);

pub fn draw(frame: &mut Frame, model: &Model) {
    let area = frame.area();
    if model.too_small() {
        too_small(frame, area);
        return;
    }
    let (keys, [header_row, switcher_row, error_row, body, bottom, key_row]) = regions(model, area);
    header(frame, header_row, model);
    switcher(frame, switcher_row, model);
    if let Some((source, text, at)) = model.error() {
        let age = format!("  {}s ago", seconds(model, at));
        let room = usize::from(error_row.width).saturating_sub(1 + age.chars().count());
        let line = format!(" {}{age}", cut_words(&format!("{source}: {text}"), room));
        frame.render_widget(
            Paragraph::new(line).style(Style::new().fg(Color::Red)),
            error_row,
        );
    }
    workbench(frame, body, model);
    match bottom.height {
        0 => {}
        1 => run_line(frame, bottom, model),
        _ => run_pane(frame, bottom, model),
    }
    frame.render_widget(Paragraph::new(keys.join("\n")), key_row);
    if model.help() {
        help(frame, body.union(bottom), model);
    }
}

fn regions(model: &Model, area: Rect) -> (Vec<String>, [Rect; 6]) {
    let keys = key_line(model, usize::from(area.width));
    let key_height = u16::try_from(keys.len()).unwrap_or(u16::MAX);
    let [header_row, switcher_row, error_row, middle, key_row] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(u16::from(model.error().is_some())),
        Constraint::Min(0),
        Constraint::Length(key_height),
    ])
    .areas(area);
    let least = workbench_least(model);
    let run_line = middle.height.saturating_sub(least).min(1);
    let bottom_height = if model.chooser() {
        0
    } else if model.run_pane() {
        let width = usize::from(middle.width.saturating_sub(2));
        let pane = pane_height(model.run(), width, middle.height, least);
        if pane < PANE_LEAST {
            run_line
        } else {
            pane
        }
    } else {
        run_line
    };
    let [body, bottom] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(bottom_height)]).areas(middle);
    (
        keys,
        [header_row, switcher_row, error_row, body, bottom, key_row],
    )
}

fn pane_height(run: Option<&Run>, width: usize, middle: u16, least: u16) -> u16 {
    let rows = |count: usize| u16::try_from(count + 2).unwrap_or(u16::MAX);
    let command = run.map_or(0, |run| command_rows(run, width).len());
    let last = run
        .and_then(|run| run.lines.last())
        .map_or(0, |line| wrapped(line, width).len());
    let needed = rows(command + last);
    let essential = rows(last);
    let third = middle.saturating_sub(1) / 3 + 1;
    third
        .max(needed.min(middle / 2))
        .max(essential)
        .min(middle.saturating_sub(least))
}

fn workbench_least(model: &Model) -> u16 {
    let list = match model.workbench() {
        Workbench::Devices => BORDERS + 3 + ALERTS_LEAST,
        Workbench::Services => BORDERS + 1,
        Workbench::Lab => NODES_LEAST + BORDERS + 1,
    };
    let tab_rows = match (model.workbench(), model.tab()) {
        (Workbench::Lab, _) => 1,
        (Workbench::Services, Tab::Overview) | (_, Tab::Timeline) => 2,
        (_, Tab::Overview | Tab::Logs) => 1,
        (_, Tab::Interfaces) => 3,
        (_, Tab::Sonda) => 6,
    };
    list.max(BORDERS + 1 + tab_rows)
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

fn row_marker(model: &Model, selected: bool, target: &Target) -> &'static str {
    if model.failed(target) {
        "!"
    } else {
        marker(selected)
    }
}

fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return format!("{text:<width$}");
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn wrapped(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row = String::new();
    for (index, word) in text.split(' ').enumerate() {
        let gap = if index == 0 { "" } else { " " };
        let used = row.chars().count() + gap.len();
        let length = word.chars().count();
        if used + length <= width {
            row.push_str(gap);
            row.push_str(word);
            continue;
        }
        let mut letters: Vec<char> = word.chars().collect();
        if length > width && used < width {
            row.push_str(gap);
            row.extend(letters.drain(..width - used));
        }
        if !row.is_empty() {
            rows.push(std::mem::take(&mut row));
        }
        let mut pieces: Vec<String> = letters.chunks(width).map(String::from_iter).collect();
        row = pieces.pop().unwrap_or_default();
        rows.extend(pieces);
    }
    rows.push(row);
    rows
}

pub(crate) fn cut_words(text: &str, width: usize) -> String {
    let words: Vec<&str> = text.split(' ').collect();
    cut_whole(&words, " ", width)
}

fn cut_whole(pieces: &[&str], gap: &str, width: usize) -> String {
    let text = pieces.join(gap);
    if text.chars().count() <= width {
        return text;
    }
    let mut kept = String::new();
    for piece in pieces {
        let longer = if kept.is_empty() {
            piece.to_string()
        } else {
            format!("{kept}{gap}{piece}")
        };
        if longer.chars().count() + " …".chars().count() > width {
            break;
        }
        kept = longer;
    }
    if kept.is_empty() {
        "…".chars().take(width).collect()
    } else {
        format!("{kept} …")
    }
}

fn highlighted(line: Line<'static>, width: u16) -> Line<'static> {
    let padding = usize::from(width).saturating_sub(line.width());
    let mut spans = line.spans;
    spans.push(Span::raw(" ".repeat(padding)));
    Line::from(spans).style(REVERSE)
}

fn header(frame: &mut Frame, area: Rect, model: &Model) {
    frame.render_widget(Paragraph::new(format!(" {}", model.header())), area);
    right(frame, area, Line::from(model.clock()), 1);
}

fn switcher(frame: &mut Frame, area: Rect, model: &Model) {
    if model.chooser() {
        frame.render_widget(Paragraph::new(" choose a lab, or q to quit"), area);
        return;
    }
    let mut spans = vec![Span::raw(" ")];
    for (index, (workbench, key, name)) in workbenches(model).iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        let name = format!("{key} {name}");
        spans.push(if *workbench == model.workbench() {
            Span::styled(format!("[{name}]"), REVERSE)
        } else {
            Span::raw(name)
        });
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    right(frame, area, Line::from("? keys  q quit"), 1);
}

fn workbenches(model: &Model) -> &'static [(Workbench, &'static str, &'static str)] {
    let shown = if model.book_lab() { 3 } else { 2 };
    &WORKBENCHES[..shown]
}

fn workbench(frame: &mut Frame, area: Rect, model: &Model) {
    if model.chooser() {
        chooser(frame, area, model);
        return;
    }
    match model.workbench() {
        Workbench::Devices => devices(frame, area, model),
        Workbench::Lab => lab(frame, area, model),
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
        "running" | "ok" | "established" | "up" => Style::new().fg(Color::Green),
        "starting" | "pending" | "paused" | "waiting" | "restarting" | "warn" | "active"
        | "idle" | "connect" => Style::new().fg(Color::Yellow),
        _ if text.starts_with("http") => Style::new().fg(Color::Yellow),
        "down" | "failed" | "firing" | "exited" | "dead" => Style::new().fg(Color::Red),
        "finished" | "stopped" | "created" | "?" | "—" => DIM,
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
    let selected = Some(model.selected_stack());
    stack_list(
        frame,
        area,
        model,
        "stacks",
        &visible,
        selected,
        Target::Stack,
    );
}

fn stack_list(
    frame: &mut Frame,
    area: Rect,
    model: &Model,
    title: &str,
    indices: &[usize],
    selected: Option<usize>,
    target: fn(usize) -> Target,
) {
    let block = Block::bordered().title_top(format!(" {title} {} ", indices.len()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let longest = indices
        .iter()
        .map(|index| model.stacks()[*index].name.chars().count())
        .max()
        .unwrap_or(0);
    let name_width = longest.min(usize::from(inner.width).saturating_sub(11));
    let at = indices
        .iter()
        .position(|index| Some(*index) == selected)
        .unwrap_or(0);
    let lines: Vec<Line> = indices
        .iter()
        .skip(first_row(at, usize::from(inner.height)))
        .map(|&index| {
            let state = model.stack_state(index);
            let selected = Some(index) == selected;
            let line = Line::from(vec![
                Span::raw(format!(
                    "{}{}  ",
                    row_marker(model, selected, &target(index)),
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
        let count = counted(model.services().len(), "service");
        format!(" {name}  {count}  {age} ")
    } else {
        format!(" {name}  {age} ")
    };
    let block = Block::bordered().title_top(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [tab_row, content] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    tabs(frame, tab_row, model, wide);
    match model.tab() {
        Tab::Overview | Tab::Interfaces | Tab::Timeline => overview(frame, content, model, wide),
        Tab::Logs => logs(frame, content, model),
        Tab::Sonda => {
            view::draw_in(frame, content, model.sonda(), model.now());
            if model.focus() == Focus::List {
                let key_row = Rect {
                    y: content.bottom().saturating_sub(1),
                    height: 1,
                    ..content
                };
                frame.render_widget(Clear, key_row.intersection(content));
            }
        }
    }
}

fn tabs(frame: &mut Frame, area: Rect, model: &Model, wide: bool) {
    let on_devices = model.workbench() == Workbench::Devices;
    let shown: &[Tab] = if on_devices {
        &DEVICE_TABS
    } else {
        &SERVICES_TABS
    };
    let mut spans = Vec::new();
    for (index, tab) in shown.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        let name = tab.title(wide);
        spans.push(if *tab == model.tab() {
            Span::styled(name.to_uppercase(), REVERSE)
        } else {
            Span::raw(name)
        });
    }
    let line = Line::from(spans);
    let room = usize::from(area.width).saturating_sub(line.width() + 2);
    frame.render_widget(Paragraph::new(line), area);
    let logs = if model.tab() != Tab::Logs {
        None
    } else if on_devices {
        model
            .device_logs()
            .map(|(device, _, following, _)| (device, following))
    } else {
        model
            .logs()
            .map(|(service, _, following, _)| (service, following))
    };
    if let Some((service, following)) = logs {
        let mode = if following { "following" } else { "paused" };
        let both = format!("{service}  {mode}");
        let status = [both, mode.to_string()]
            .into_iter()
            .find(|status| status.chars().count() <= room);
        if let Some(status) = status {
            right(frame, area, Line::from(status), 0);
        }
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

fn first_row(selected: usize, room: usize) -> usize {
    (selected + 1).saturating_sub(room)
}

fn fitted(parts: Vec<(String, Style)>, width: usize) -> Line<'static> {
    let total: usize = parts.iter().map(|(text, _)| text.chars().count()).sum();
    let cut = total > width;
    let mut left = if cut { width.saturating_sub(1) } else { total };
    let mut spans = Vec::new();
    for (text, style) in parts {
        let kept: String = text.chars().take(left).collect();
        left -= kept.chars().count();
        spans.push(Span::styled(kept, style));
    }
    if cut {
        spans.push(Span::raw("…"));
    }
    Line::from(spans)
}

fn wall_seconds(model: &Model) -> i64 {
    model
        .wall()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

fn devices(frame: &mut Frame, area: Rect, model: &Model) {
    let wide = area.width >= WIDE;
    let left_width = if wide { LEFT_DEVICES } else { LEFT_NARROW };
    let [left, right] =
        Layout::horizontal([Constraint::Length(left_width), Constraint::Min(0)]).areas(area);
    device_list(frame, left, model, wide);
    device_detail(frame, right, model, wide);
}

fn device_list(frame: &mut Frame, area: Rect, model: &Model, wide: bool) {
    let visible = model.visible_devices();
    let block = Block::bordered().title_top(format!(" devices {} ", visible.len()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let alerts_height = (inner.height / 4).max(ALERTS_LEAST);
    let [list, separator, alerts] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(alerts_height),
    ])
    .areas(inner);
    match model.source_note() {
        Some(note) => {
            let line = fitted(vec![(note.to_string(), DIM)], usize::from(list.width));
            frame.render_widget(Paragraph::new(line), list);
        }
        None => device_rows(frame, list, model, &visible, wide),
    }
    let count = if model.alerts_ok() {
        model.alerts().len().to_string()
    } else {
        "?".to_string()
    };
    let head = format!("├ alerts {count} ");
    let fill = usize::from(area.width).saturating_sub(head.chars().count() + 1);
    let line = format!("{head}{}┤", "─".repeat(fill));
    let row = Rect::new(area.x, separator.y, area.width, 1);
    frame.render_widget(Paragraph::new(line), row);
    alert_rows(frame, alerts, model, wide);
}

fn device_rows(frame: &mut Frame, area: Rect, model: &Model, visible: &[usize], wide: bool) {
    let rows = model.device_rows();
    let fixed = if wide { 27 } else { 15 };
    let name_width = rows
        .iter()
        .map(|row| row.name.chars().count())
        .max()
        .unwrap_or(0)
        .max(4)
        .min(usize::from(area.width).saturating_sub(fixed));
    let mut header = format!("   {}", fit("NAME", name_width));
    if wide {
        header.push_str("  KIND");
    }
    header.push_str("  HLTH");
    if wide {
        header.push_str("  SYNC");
    }
    header.push_str("  ALRT");
    let cursor = model.cursor();
    let at = visible
        .iter()
        .position(|index| cursor == Cursor::Device(*index))
        .unwrap_or(0);
    let start = first_row(at, usize::from(area.height).saturating_sub(1));
    let mut lines = vec![Line::from(header)];
    for (row, index) in rows.iter().zip(visible).skip(start) {
        let selected = cursor == Cursor::Device(*index);
        let mark = if row.maintenance {
            Span::styled("M", Style::new().fg(Color::Yellow))
        } else {
            Span::raw(" ")
        };
        let target = Target::Device(row.name.clone());
        let mut spans = vec![
            Span::raw(row_marker(model, selected, &target)),
            mark,
            Span::raw(format!(" {}", fit(&row.name, name_width))),
        ];
        if wide {
            spans.push(Span::styled(
                format!("  {}", fit(&row.kind, 4)),
                tone(&row.kind),
            ));
        }
        let health = row.health.text();
        spans.push(Span::styled(format!("  {health:<4}"), tone(health)));
        if wide {
            spans.push(Span::styled("  —   ", DIM));
        }
        let (count, style) = match row.alerts {
            None => ("?".to_string(), DIM),
            Some((0, _)) => (String::new(), Style::new()),
            Some((count, true)) => (count.to_string(), Style::new().fg(Color::Red)),
            Some((count, false)) => (count.to_string(), Style::new()),
        };
        spans.push(Span::styled(format!("  {count:>4}"), style));
        let line = Line::from(spans);
        lines.push(if selected {
            highlighted(line, area.width)
        } else {
            line
        });
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn alert_rows(frame: &mut Frame, area: Rect, model: &Model, wide: bool) {
    let room = usize::from(area.height);
    let cursor = model.cursor();
    let start = match cursor {
        Cursor::Alert(at) => first_row(at, room),
        Cursor::Device(_) => 0,
    };
    let wall = wall_seconds(model);
    let lines: Vec<Line> = model
        .alerts()
        .iter()
        .enumerate()
        .skip(start)
        .take(room)
        .map(|(index, alert)| {
            let label = |name: &str| alert.labels.get(name).map_or("—", String::as_str);
            let (name, device) = (label("alertname"), label("device"));
            let base = if alert.silenced || alert.inhibited {
                DIM
            } else {
                Style::new()
            };
            let parts = if wide {
                let state = alert.state_word();
                let style = base.patch(tone(&state));
                vec![
                    (format!("{name}  {device}  "), base),
                    (state, style),
                    (format!("  {}", time::age(&alert.starts_at, wall)), base),
                ]
            } else {
                vec![(format!("{device}  {name}"), base)]
            };
            let line = fitted(parts, usize::from(area.width));
            if cursor == Cursor::Alert(index) {
                highlighted(line, area.width)
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn device_detail(frame: &mut Frame, area: Rect, model: &Model, wide: bool) {
    let title = model.selected_device().map_or_else(
        || " devices ".to_string(),
        |device| format!(" {} ", device.name),
    );
    let block = Block::bordered().title_top(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [tab_row, content] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    tabs(frame, tab_row, model, wide);
    if let Some(note) = model.source_note() {
        let line = fitted(vec![(note.to_string(), DIM)], usize::from(content.width));
        frame.render_widget(Paragraph::new(line), content);
        return;
    }
    match model.tab() {
        Tab::Overview => device_overview(frame, content, model),
        Tab::Timeline => timeline(frame, content, model),
        Tab::Logs => device_logs(frame, content, model),
        Tab::Interfaces => device_interfaces(frame, content, model, wide),
        Tab::Sonda => {}
    }
}

fn device_interfaces(frame: &mut Frame, area: Rect, model: &Model, wide: bool) {
    let interfaces = model.interfaces();
    let columns: &[usize] = if wide {
        &[0, 1, 2, 3, 4, 5]
    } else {
        &[0, 2, 3, 5]
    };
    let titles = ["NAME", "INTENT", "ADMIN", "OPER", "PEER", "BGP"];
    let cells: Vec<[&str; 6]> = interfaces
        .iter()
        .map(|row| {
            [
                row.name.as_str(),
                row.intent.as_str(),
                row.admin.as_str(),
                row.oper.as_str(),
                row.peer.as_str(),
                row.bgp.as_str(),
            ]
        })
        .collect();
    let mut widths = titles.map(|title| title.chars().count());
    for row in &cells {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let width = usize::from(area.width);
    let line = |row: &[&str; 6], yellow: bool, states: bool| {
        let parts = columns
            .iter()
            .map(|&column| {
                let text = if column == 5 {
                    row[column].to_string()
                } else {
                    format!("{:<1$}  ", row[column], widths[column])
                };
                let style = if yellow {
                    Style::new().fg(Color::Yellow)
                } else if states && matches!(column, 2 | 3 | 5) {
                    tone(row[column])
                } else {
                    Style::new()
                };
                (text, style)
            })
            .collect();
        fitted(parts, width)
    };
    let [table, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
    let mut lines = vec![line(&titles, false, false)];
    lines.extend(
        cells
            .iter()
            .zip(&interfaces)
            .map(|(row, source)| line(row, source.differs, true)),
    );
    frame.render_widget(Paragraph::new(lines), table);
    let differs = interfaces.iter().filter(|row| row.differs).count();
    let count = counted(interfaces.len(), "interface");
    let differs = format!("{differs} differs");
    frame.render_widget(
        Paragraph::new(cut_whole(&[&count, &differs], "  ", width)),
        footer,
    );
}

fn device_overview(frame: &mut Frame, area: Rect, model: &Model) {
    let Some(device) = model.selected_device() else {
        return;
    };
    let width = usize::from(area.width);
    let plain = Style::new();
    let facts = |pieces: &[String], gap: &str| {
        let pieces: Vec<&str> = pieces.iter().map(String::as_str).collect();
        Line::from(cut_whole(&pieces, gap, width))
    };
    let states = model.session_states();
    let established = states.iter().filter(|(_, state)| state == "up").count();
    let seen = model.devices_at().map_or("—".to_string(), |at| {
        format!("{}s ago", seconds(model, at))
    });
    let mut lines = vec![
        facts(
            &[
                format!("kind {}", devices::kind_short(device.kind.as_deref())),
                format!("site {}", device.site.as_deref().unwrap_or("—")),
                format!("role {}", device.role.as_deref().unwrap_or("—")),
            ],
            "   ",
        ),
        facts(
            &[
                format!(
                    "maintenance {}",
                    if device.maintenance { "yes" } else { "no" }
                ),
                format!("sot {}", model.sot()),
                format!("seen {seen}"),
            ],
            "   ",
        ),
        facts(
            &[
                "bgp sessions".to_string(),
                format!("{} intended", states.len()),
                format!("{established} established"),
                format!("{} other", states.len() - established),
            ],
            "  ",
        ),
    ];
    for (session, state) in &states {
        let mut parts = vec![
            (
                format!(
                    " {:<9}  {:<12}  ",
                    session.peer,
                    session.afi_safi.as_deref().unwrap_or("—")
                ),
                plain,
            ),
            (format!("{state:<6}"), tone(state)),
        ];
        let unmet = session.expected_state.as_deref().filter(|expected| {
            let wanted = if *expected == "established" {
                "up"
            } else {
                expected
            };
            wanted != state && state != "?" && state != "—"
        });
        if let Some(expected) = unmet {
            parts.push((
                format!("  ! expected {expected}"),
                Style::new().fg(Color::Yellow),
            ));
        }
        lines.push(fitted(parts, width));
    }
    let latest = model.alerts().iter().find(|alert| {
        alert.labels.get("device") == Some(&device.name) && alert.state_word() == "firing"
    });
    let label = |name: &str| format!("alerts   {name}");
    lines.push(match latest {
        _ if !model.alerts_ok() => facts(&[label("?")], ""),
        Some(alert) => facts(
            &[
                label(alert.labels.get("alertname").map_or("—", String::as_str)),
                alert
                    .labels
                    .get("severity")
                    .map_or("—", String::as_str)
                    .to_string(),
                time::age(&alert.starts_at, wall_seconds(model)),
            ],
            "  ",
        ),
        None => facts(&[label("none")], ""),
    });
    frame.render_widget(Paragraph::new(lines), area);
}

fn kind_style(kind: devices::Kind) -> Style {
    match kind {
        devices::Kind::Alert => Style::new().fg(Color::Red),
        devices::Kind::Flow => Style::new().fg(Color::Blue),
        devices::Kind::Rca => Style::new().fg(Color::Magenta),
        devices::Kind::Log => Style::new(),
    }
}

fn timeline(frame: &mut Frame, area: Rect, model: &Model) {
    let entries = model.timeline();
    let [counts, rows] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    let [alerts, flows, rcas, logs] = devices::counts(&entries);
    let last = format!("last {}", devices::window_text(model.window()));
    let alerts = format!("   alerts {alerts}");
    let flows = format!("  flows {flows}");
    let rcas = format!("  rca {rcas}");
    let logs = format!("  logs {logs}");
    let summary = cut_whole(
        &[&last, &alerts, &flows, &rcas, &logs],
        "",
        usize::from(area.width),
    );
    frame.render_widget(Paragraph::new(summary), counts);
    let on_tabs = model.focus() == Focus::Tabs;
    let selected = model.timeline_row();
    let start = first_row(selected, usize::from(rows.height));
    let lines: Vec<Line> = entries
        .iter()
        .enumerate()
        .skip(start)
        .take(usize::from(rows.height))
        .map(|(index, entry)| {
            let here = on_tabs && index == selected;
            let first = entry.text.lines().next().unwrap_or("");
            let parts = vec![
                (
                    format!("{}{}  ", marker(here), devices::clock_of(entry.nanoseconds)),
                    Style::new(),
                ),
                (format!("{:<5}", entry.kind.name()), kind_style(entry.kind)),
                (format!("  {first}"), Style::new()),
            ];
            let line = fitted(parts, usize::from(rows.width));
            if here {
                highlighted(line, rows.width)
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), rows);
    if model.popup() {
        if let Some(entry) = entries.get(selected) {
            timeline_popup(frame, area, entry);
        }
    }
}

fn timeline_popup(frame: &mut Frame, area: Rect, entry: &devices::Entry) {
    let popup = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    );
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title_top(format!(
            " {}  {} ",
            entry.kind.name(),
            devices::clock_of(entry.nanoseconds)
        ))
        .title_top(Line::from(" Esc close ").right_aligned());
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(entry.text.as_str()).wrap(Wrap { trim: false }),
        inner,
    );
}

fn device_logs(frame: &mut Frame, area: Rect, model: &Model) {
    let Some((_, lines, following, scroll)) = model.device_logs() else {
        return;
    };
    let start = if following { 0 } else { scroll };
    let shown: Vec<Line> = lines
        .iter()
        .skip(start)
        .take(usize::from(area.height))
        .map(|(nanoseconds, text)| {
            let text = format!("{}  {text}", devices::clock_of(*nanoseconds));
            Line::from(fit(&text, usize::from(area.width)))
        })
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

fn run_end(run: &Run) -> (String, Style) {
    match run.state {
        RunState::Running => ("running".to_string(), Style::new()),
        RunState::Stopping => ("stopping".to_string(), Style::new()),
        RunState::Ended(End::Exited(0)) => ("exit 0".to_string(), Style::new().fg(Color::Green)),
        RunState::Ended(End::Exited(code)) => (format!("exit {code}"), Style::new().fg(Color::Red)),
        RunState::Ended(End::Stopped) => ("stopped".to_string(), Style::new().fg(Color::Yellow)),
        RunState::Ended(End::NotStopped { .. }) => {
            ("not stopped".to_string(), Style::new().fg(Color::Red))
        }
    }
}

fn run_title(model: &Model, before_hint: usize) -> Vec<Span<'static>> {
    let Some(run) = model.run() else {
        return vec![Span::raw(" run  none")];
    };
    let (end, style) = run_end(run);
    let state = match (run.state, run.ended) {
        (RunState::Stopping, _) => end,
        (_, Some(ended)) => {
            let took = ended.saturating_duration_since(run.started).as_secs();
            format!("{end}  {took}s")
        }
        (_, None) => format!("{end} {}s", seconds(model, run.started)),
    };
    let room = before_hint.saturating_sub(" run  ".len() + "  ".len() + state.chars().count() + 1);
    let words = cut_words(&run.words, room);
    vec![
        Span::raw(format!(" run  {words}  ")),
        Span::styled(state, style),
    ]
}

fn run_line(frame: &mut Frame, area: Rect, model: &Model) {
    let hint = Line::from(if model.run_pane() { FOLD } else { "o open" });
    let before_hint = usize::from(area.width).saturating_sub(hint.width() + 1);
    frame.render_widget(
        Paragraph::new(Line::from(run_title(model, before_hint))),
        area,
    );
    right(frame, area, hint, 1);
}

fn run_pane(frame: &mut Frame, area: Rect, model: &Model) {
    let hint = Line::from(format!(" {FOLD} ")).right_aligned();
    let before_hint = usize::from(area.width).saturating_sub(hint.width() + 2);
    let mut title = run_title(model, before_hint);
    title.push(Span::raw(" "));
    let block = Block::bordered()
        .title_top(Line::from(title))
        .title_top(hint);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(run) = model.run() else {
        return;
    };
    let lines = run_rows(run, usize::from(inner.width), usize::from(inner.height));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn run_rows(run: &Run, width: usize, height: usize) -> Vec<Line<'static>> {
    let command = command_rows(run, width);
    let last = run
        .lines
        .last()
        .map_or(0, |line| wrapped(line, width).len());
    let shown = if command.len() + last <= height {
        command
    } else {
        Vec::new()
    };
    let mut lines: Vec<Line> = shown
        .into_iter()
        .map(|row| Line::styled(row, DIM))
        .collect();
    let room = height.saturating_sub(lines.len());
    let mut tail: Vec<Line> = Vec::new();
    for line in run.lines.iter().rev() {
        if tail.len() >= room {
            break;
        }
        let style = if line.starts_with("nobs:") {
            Style::new().fg(Color::Red)
        } else {
            Style::new()
        };
        let rows = wrapped(line, width);
        tail.extend(rows.into_iter().rev().map(|row| Line::styled(row, style)));
    }
    tail.truncate(room);
    tail.reverse();
    lines.extend(tail);
    lines
}

fn command_rows(run: &Run, width: usize) -> Vec<String> {
    wrapped(&format!("$ nobs {}", run.command), width)
}

fn lab(frame: &mut Frame, area: Rect, model: &Model) {
    let wide = area.width >= WIDE;
    let left_width = if wide { LEFT_DEVICES } else { LEFT_NARROW };
    let [left, right] =
        Layout::horizontal([Constraint::Length(left_width), Constraint::Min(0)]).areas(area);
    let chapters = model.chapters();
    let chapters_height = u16::try_from(chapters.len() + 2).unwrap_or(u16::MAX);
    let [top, bottom] = Layout::vertical([
        Constraint::Min(NODES_LEAST),
        Constraint::Length(chapters_height),
    ])
    .areas(left);
    node_list(frame, top, model, wide);
    let selected = model.selected_chapter();
    stack_list(
        frame,
        bottom,
        model,
        "chapters",
        &chapters,
        selected,
        Target::Chapter,
    );
    lab_overview(frame, right, model);
}

fn node_list(frame: &mut Frame, area: Rect, model: &Model, wide: bool) {
    let rows = model.nodes();
    let block = Block::bordered().title_top(format!(" nodes {} ", rows.len()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut sorted: Vec<_> = rows.iter().collect();
    sorted.sort_by(|one, other| one.name.cmp(&other.name));
    let width = |texts: Vec<&str>| {
        texts
            .iter()
            .map(|text| text.chars().count())
            .max()
            .unwrap_or(0)
            .max(4)
    };
    let name_width = width(sorted.iter().map(|row| row.name.as_str()).collect());
    let kind_width = width(sorted.iter().map(|row| row.kind.as_str()).collect());
    let line = |name: &str, kind: &str, state: (&str, Style)| {
        let mut parts = vec![(format!(" {}  ", fit(name, name_width)), Style::new())];
        if wide {
            parts.push((format!("{}  ", fit(kind, kind_width)), Style::new()));
        }
        parts.push((state.0.to_string(), state.1));
        fitted(parts, usize::from(inner.width))
    };
    let mut lines = vec![line("NAME", "KIND", ("STATE", Style::new()))];
    for row in sorted {
        let state = match row.state.as_deref() {
            _ if !model.nodes_ok() => ("?", tone("?")),
            None => ("—", tone("—")),
            Some("running") => ("running", tone("running")),
            Some(other) => (other, Style::new().fg(Color::Red)),
        };
        lines.push(line(&row.name, &row.kind, state));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn lab_overview(frame: &mut Frame, area: Rect, model: &Model) {
    let Some(chapter) = model.selected_chapter() else {
        return;
    };
    let block = Block::bordered().title_top(format!(" {} ", model.stacks()[chapter].name));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [tab_row, content] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    frame.render_widget(Paragraph::new(Span::styled("OVERVIEW", REVERSE)), tab_row);
    let lab = model.located();
    let topology = lab.map_or("—".to_string(), |lab| {
        let path = lab.topology();
        path.strip_prefix(&lab.dir)
            .unwrap_or(&path)
            .display()
            .to_string()
    });
    let nodes = model.nodes();
    let running = nodes
        .iter()
        .filter(|node| node.state.as_deref() == Some("running"))
        .count();
    let nodes = if model.nodes_ok() {
        format!("{running} of {} running", nodes.len())
    } else {
        "?".to_string()
    };
    let services = match model.stack_state(chapter) {
        StackState::Up { running, listed } => format!("{running} of {listed} running"),
        StackState::Down { listed } => format!("0 of {listed} running"),
        StackState::Unknown | StackState::Failed => "?".to_string(),
    };
    let width = usize::from(content.width);
    let row = |label: &str, value: String| format!(" {label:<8}  {value}");
    let last = model
        .run()
        .filter(|run| run.target == Target::Chapter(chapter));
    let (last, end) = match last {
        Some(run) => {
            let (end, style) = run_end(run);
            let used = row("last", String::new()).chars().count() + "  ".len();
            let room = width.saturating_sub(used + end.chars().count());
            (format!("{}  ", cut_words(&run.words, room)), (end, style))
        }
        None => ("—".to_string(), (String::new(), Style::new())),
    };
    let plain = |label: &str, value: String| fitted(vec![(row(label, value), Style::new())], width);
    let lines = vec![
        plain("topology", topology),
        plain(
            "project",
            lab.and_then(Located::project).unwrap_or("—").to_string(),
        ),
        plain("nodes", nodes),
        plain("services", services),
        fitted(vec![(row("last", last), Style::new()), end], width),
    ];
    frame.render_widget(Paragraph::new(lines), content);
}

fn action_hints(model: &Model) -> Vec<String> {
    ACTIONS
        .iter()
        .filter(|action| action.workbench == model.workbench())
        .filter(|action| {
            model
                .action_target(action.row)
                .is_some_and(|target| arguments(action, &target, model).is_some())
        })
        .map(|action| format!("{} {}", action.key, action.hint))
        .collect()
}

pub(crate) fn key_line(model: &Model, width: usize) -> Vec<String> {
    if let Some(text) = model.typed_filter() {
        return filter_line(text, width);
    }
    if let Some(question) = model.question() {
        return wrapped(&format!(" {}", question.text()), width);
    }
    if let Some(notice) = model.notice() {
        return wrapped(&format!(" {notice}"), width);
    }
    vec![whole_hints(&key_hints(model), width)]
}

fn key_hints(model: &Model) -> Vec<String> {
    let listed = |fixed: Vec<&'static str>| fixed.into_iter().map(ToString::to_string);
    if model.chooser() {
        return listed(vec!["j/k select", "Enter open", "q quit"]).collect();
    }
    let actions = action_hints(model);
    let fixed = match (model.workbench(), model.focus(), model.tab()) {
        (Workbench::Lab, ..) => {
            return listed(vec!["j/k move"])
                .chain(actions)
                .chain(listed(vec!["r refresh", "o pane"]))
                .collect();
        }
        (Workbench::Services, Focus::List, _) => {
            vec!["Tab focus", "j/k move", "/ filter", "l logs", "r refresh"]
        }
        (Workbench::Services, Focus::Tabs, Tab::Overview | Tab::Interfaces | Tab::Timeline) => {
            vec!["Tab focus", "j/k move", "[ ] tab", "Enter logs", "Esc back"]
        }
        (Workbench::Services, Focus::Tabs, Tab::Logs) => {
            log_hints(matches!(model.logs(), Some((_, _, false, _))))
        }
        (Workbench::Services, Focus::Tabs, Tab::Sonda) => vec!["Tab focus", "[ ] tab", "Esc back"],
        (Workbench::Devices, Focus::List, _) => {
            vec![
                "Tab focus",
                "j/k move",
                "/ filter",
                "Enter open",
                "r refresh",
            ]
        }
        (Workbench::Devices, Focus::Tabs, Tab::Timeline) if model.popup() => vec!["Esc close"],
        (Workbench::Devices, Focus::Tabs, Tab::Timeline) => vec![
            "j/k move",
            "Enter full text",
            "- + window",
            "[ ] tab",
            "Esc back",
        ],
        (Workbench::Devices, Focus::Tabs, Tab::Logs) => {
            log_hints(matches!(model.device_logs(), Some((_, _, false, _))))
        }
        (Workbench::Devices, Focus::Tabs, _) => vec!["Tab focus", "[ ] tab", "Esc back"],
    };
    listed(fixed).chain(actions).collect()
}

fn log_hints(paused: bool) -> Vec<&'static str> {
    let follow = if paused { "f follow" } else { "f pause" };
    vec![
        follow,
        "/ filter",
        "j/k scroll",
        "[ ] tab",
        "Esc back to list",
    ]
}

pub(crate) fn whole_hints(hints: &[String], width: usize) -> String {
    let mut line = String::new();
    for hint in hints {
        if !push_whole(&mut line, hint, width) {
            break;
        }
    }
    line
}

fn push_whole(line: &mut String, hint: &str, width: usize) -> bool {
    let gap = if line.is_empty() { " " } else { "  " };
    if line.chars().count() + gap.len() + hint.chars().count() > width {
        return false;
    }
    line.push_str(gap);
    line.push_str(hint);
    true
}

fn filter_line(text: &str, width: usize) -> Vec<String> {
    let mut rows = wrapped(&format!(" filter: {text}"), width);
    for hint in ["Enter keep", "Esc clear"] {
        let fits = rows
            .last_mut()
            .is_some_and(|row| push_whole(row, hint, width));
        if !fits {
            let mut row = String::new();
            push_whole(&mut row, hint, width);
            rows.push(row);
        }
    }
    rows
}

pub(crate) fn help_entries(model: &Model) -> Vec<(String, &'static str)> {
    let workbench = model.workbench();
    let fixed: &[(&str, &str)] = match workbench {
        Workbench::Devices => &HELP_DEVICES,
        Workbench::Services => &HELP,
        Workbench::Lab => &HELP_LAB,
    };
    let switch: Vec<&str> = workbenches(model).iter().map(|(_, key, _)| *key).collect();
    let actions = ACTIONS
        .iter()
        .filter(|action| action.workbench == workbench)
        .map(|action| (action.key.to_string(), action.hint));
    let owned = |&(key, meaning): &(&str, &'static str)| (key.to_string(), meaning);
    [(switch.join(" "), "switch workbench")]
        .into_iter()
        .chain(fixed.iter().map(owned))
        .chain(actions)
        .chain(HELP_QUIT.iter().map(owned))
        .collect()
}

struct HelpLayout {
    entries: Vec<(String, &'static str)>,
    note: Option<&'static str>,
    popup: Rect,
    inner: Rect,
    rows: u16,
    room: u16,
    note_rows: u16,
    note_row: u16,
    fits: bool,
}

fn help_layout(model: &Model, body: Rect) -> HelpLayout {
    let entries = help_entries(model);
    let note = match model.workbench() {
        Workbench::Devices => Some(HELP_DEVICES_NOTE),
        Workbench::Services => Some(HELP_SONDA),
        Workbench::Lab => None,
    };
    let width = body.width.saturating_sub(4);
    let note_rows = note.map_or(0, |note| {
        let note = u16::try_from(note.chars().count()).unwrap_or(u16::MAX);
        note.div_ceil(width.saturating_sub(2).max(1)).max(1)
    });
    let rows = u16::try_from(entries.len().div_ceil(2)).unwrap_or(u16::MAX);
    let note_height = if note.is_some() { note_rows + 1 } else { 0 };
    let fits = rows + note_rows + 2 <= body.height;
    let height = body.height.min(rows + 2 + note_height);
    let popup = Rect::new(
        body.x + (body.width - width) / 2,
        body.y + (body.height - height) / 2,
        width,
        height,
    );
    let inner = Block::bordered().inner(popup);
    let (room, note_row) = if fits {
        let note_row = (rows + 1)
            .min(inner.height.saturating_sub(note_rows))
            .max(rows);
        (rows, note_row)
    } else {
        let room = inner.height.saturating_sub(note_rows + 1);
        (room, room)
    };
    HelpLayout {
        entries,
        note,
        popup,
        inner,
        rows,
        room,
        note_rows,
        note_row,
        fits,
    }
}

pub(crate) fn help_overflow(model: &Model, area: Rect) -> usize {
    if !model.help() {
        return 0;
    }
    let (_, [_, _, _, body, bottom, _]) = regions(model, area);
    let layout = help_layout(model, body.union(bottom));
    usize::from(layout.rows - layout.room)
}

fn help(frame: &mut Frame, area: Rect, model: &Model) {
    let HelpLayout {
        entries,
        note,
        popup,
        inner,
        rows,
        room,
        note_rows,
        note_row,
        fits,
    } = help_layout(model, area);
    frame.render_widget(Clear, area);
    let name = match model.workbench() {
        Workbench::Devices => "devices",
        Workbench::Services => "services",
        Workbench::Lab => "lab",
    };
    let block = Block::bordered().title_top(format!(" keys  {name} workbench "));
    frame.render_widget(block, popup);
    let column = |x: u16, y: u16, width: u16, height: u16| {
        Rect::new(inner.x + x, inner.y + y, width, height).intersection(inner)
    };
    let top = u16::try_from(model.help_scroll())
        .unwrap_or(u16::MAX)
        .min(rows - room);
    let (first, second) = entries.split_at(usize::from(rows));
    let middle = inner.width / 2;
    frame.render_widget(
        Paragraph::new(help_column(first)).scroll((top, 0)),
        column(0, 0, middle, room),
    );
    frame.render_widget(
        Paragraph::new(help_column(second)).scroll((top, 0)),
        column(middle, 0, inner.width - middle, room),
    );
    if let Some(note) = note {
        frame.render_widget(
            Paragraph::new(note).wrap(Wrap { trim: false }),
            column(0, note_row, inner.width, note_rows),
        );
    }
    if !fits {
        frame.render_widget(
            Paragraph::new(HELP_MORE),
            column(0, inner.height.saturating_sub(1), inner.width, 1),
        );
    }
}

fn help_column(entries: &[(String, &str)]) -> String {
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use nobs_core::locate::locate;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
    use crate::book::NodeRow;
    use crate::shell::tests::{
        alert, devices_model, fixture, row, sonda_rows, stack, start, workshop_located,
    };
    use crate::shell::{handle, stacks, Stack, Start, Update};

    const REFUSAL: &str = "nobs: lab deploy: node containers already exist (clab-nobs-t-book-nobs-t-srl1, clab-nobs-t-book-nobs-t-srl2); run nobs lab destroy first";
    const LAB_DIR: &str = "/home/me/My Labs/network observability/the book lab/second edition/chapter files/copy for the screen";
    const LONG_ERROR: &str = "cannot reach the Docker daemon at unix:///var/run/docker.sock while it asks for the newest lines of loki";
    const SIZES: [(u16, u16); 3] = [(60, 15), (80, 24), (100, 30)];

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Keys {
        Hints,
        Filter,
        Question,
    }

    struct Bench {
        build: fn() -> Model,
        selected: &'static str,
        run: char,
        question: char,
        filter: Option<&'static str>,
        detail: &'static str,
        tabs: &'static [(Tab, &'static str)],
        other: Option<(&'static str, &'static str)>,
        fill: fn(&mut Model),
    }

    struct Walked {
        label: String,
        selected: &'static str,
        shown: Vec<(&'static str, &'static str)>,
        model: Model,
        width: u16,
        height: u16,
        rows: Vec<String>,
    }

    fn press(model: &mut Model, key: char) {
        handle(model, KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
    }

    fn devices_bench() -> Model {
        let mut lab = workshop_located("infrahub");
        lab.dir = PathBuf::from(LAB_DIR);
        let mut model = devices_model(lab);
        model.apply(Update::DeviceLogs(
            "srl1".to_string(),
            Ok(vec![nobs_core::loki::Line {
                nanoseconds: 0,
                text: "link flap".to_string(),
            }]),
        ));
        model
    }

    fn services_bench() -> Model {
        let stacks = (1..=12)
            .map(|index| Stack {
                path: PathBuf::from(LAB_DIR),
                ..stack(&format!("stack-{index:02}"), &[])
            })
            .collect();
        let mut model = Model::new(start(stacks));
        handle(
            &mut model,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        for _ in 1..12 {
            press(&mut model, 'j');
        }
        model.apply(Update::Services(11, Ok(vec![row("grafana", "running")])));
        sonda_rows(&mut model);
        model
    }

    fn services_logs(model: &mut Model) {
        let lines = vec!["2026-11-09T13:53:32Z ready to serve".to_string()];
        model.apply(Update::Logs(11, "grafana".to_string(), Ok(lines)));
    }

    fn open_tab(model: &mut Model, tab: Tab) {
        let focus = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        if model.tab() == tab {
            return;
        }
        handle(model, focus);
        while model.tab() != tab {
            press(model, ']');
        }
        handle(model, focus);
    }

    fn lab_bench(count: usize) -> Model {
        let dir = fixture("book-lab");
        let located = locate(&dir, Some(&dir)).unwrap();
        let first = stacks(Some(&located), Vec::new()).unwrap().remove(0);
        let chapters = (1..=count)
            .map(|index| Stack {
                name: format!("ch{index}"),
                path: PathBuf::from(LAB_DIR),
                ..first.clone()
            })
            .collect();
        let mut model = Model::new(Start {
            lab: Some(located),
            home: Workbench::Lab,
            chooser: false,
            ..start(chapters)
        });
        for _ in 1..count {
            press(&mut model, 'j');
        }
        model.apply(Update::Nodes(Ok(vec![NodeRow {
            name: "nobs-t-srl1".to_string(),
            kind: "nokia_srlinux".to_string(),
            state: Some("running".to_string()),
        }])));
        model
    }

    fn benches() -> [Bench; 4] {
        let lab = |build: fn() -> Model, selected, detail| Bench {
            build,
            selected,
            run: 'X',
            question: 'U',
            filter: None,
            detail,
            tabs: &[(Tab::Overview, "topology")],
            other: Some((" nodes ", "nobs-t-srl1")),
            fill: |_| {},
        };
        [
            Bench {
                build: devices_bench,
                selected: ">  srl1",
                run: 'm',
                question: 'm',
                filter: Some("srl"),
                detail: " srl1 ",
                tabs: &[
                    (Tab::Overview, "kind "),
                    (Tab::Interfaces, "ethernet-1/1"),
                    (Tab::Timeline, "link flap"),
                    (Tab::Logs, "link flap"),
                ],
                other: Some((" devices ", "BgpSessionNotUp")),
                fill: |_| {},
            },
            Bench {
                build: services_bench,
                selected: ">stack-12",
                run: 'u',
                question: 'p',
                filter: Some("stack"),
                detail: " stack-12 ",
                tabs: &[
                    (Tab::Overview, "grafana"),
                    (Tab::Logs, "ready to serve"),
                    (Tab::Sonda, "cpu"),
                ],
                other: None,
                fill: services_logs,
            },
            lab(|| lab_bench(2), ">ch2", " ch2 "),
            lab(|| lab_bench(10), ">ch10", " ch10 "),
        ]
    }

    fn staged(bench: &Bench, tab: Tab, error: bool, keys: Keys, pane: bool) -> Model {
        let mut model = (bench.build)();
        open_tab(&mut model, tab);
        (bench.fill)(&mut model);
        press(&mut model, bench.run);
        press(&mut model, 'y');
        for line in ["pulling", "created", REFUSAL] {
            model.apply(Update::ActionLine(line.to_string()));
        }
        model.apply(Update::ActionEnded(End::Exited(0)));
        if !pane {
            press(&mut model, 'o');
        }
        if error {
            model.apply(Update::Logs(
                0,
                "loki".to_string(),
                Err(LONG_ERROR.to_string()),
            ));
        }
        match (keys, bench.filter) {
            (Keys::Hints, _) => {}
            (Keys::Filter, Some(filter)) => {
                press(&mut model, '/');
                filter.chars().for_each(|typed| press(&mut model, typed));
            }
            (Keys::Filter, None) => unreachable!(),
            (Keys::Question, _) => press(&mut model, bench.question),
        }
        model
    }

    fn drawn(model: &mut Model, width: u16, height: u16) -> Vec<String> {
        model.tick(model.now(), UNIX_EPOCH, (width, height));
        painted(width, height, |frame| draw(frame, model))
    }

    fn painted(width: u16, height: u16, paint: impl FnOnce(&mut Frame)) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(paint).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                let row: String = (0..width).map(|x| buffer[(x, y)].symbol()).collect();
                row.trim_end().to_string()
            })
            .collect()
    }

    fn walk() -> Vec<Walked> {
        let mut walked = Vec::new();
        for bench in benches() {
            for &(tab, row) in bench.tabs {
                for (width, height) in SIZES {
                    for error in [false, true] {
                        for keys in [Keys::Hints, Keys::Filter, Keys::Question] {
                            if keys == Keys::Filter && bench.filter.is_none() {
                                continue;
                            }
                            for pane in [false, true] {
                                let mut model = staged(&bench, tab, error, keys, pane);
                                let rows = drawn(&mut model, width, height);
                                walked.push(Walked {
                                    label: format!(
                                        "{} {tab:?} {width}x{height} error {error} {keys:?} pane {pane}",
                                        bench.selected
                                    ),
                                    selected: bench.selected,
                                    shown: [(bench.detail, row)].into_iter().chain(bench.other).collect(),
                                    model,
                                    width,
                                    height,
                                    rows,
                                });
                            }
                        }
                    }
                }
            }
        }
        walked
    }

    fn boxed(rows: &[String], title: &str) -> Vec<String> {
        let grid: Vec<Vec<char>> = rows.iter().map(|row| row.chars().collect()).collect();
        let top: Vec<char> = format!("┌{title}").chars().collect();
        for (y, row) in grid.iter().enumerate() {
            let Some(left) = row.windows(top.len()).position(|seen| seen == top) else {
                continue;
            };
            let right = row[left..]
                .iter()
                .position(|cell| *cell == '┐')
                .map_or(row.len(), |at| left + at);
            return grid[y + 1..]
                .iter()
                .take_while(|row| row.get(left) != Some(&'└'))
                .map(|row| {
                    let end = right.min(row.len());
                    row.get(left + 1..end).unwrap_or_default().iter().collect()
                })
                .collect();
        }
        Vec::new()
    }

    fn gave_way(model: &Model, width: u16, height: u16) -> bool {
        let (_, [_, _, _, body, _, _]) = regions(model, Rect::new(0, 0, width, height));
        body.height < workbench_least(model)
    }

    fn pane_inside(rows: &[String]) -> Option<Vec<String>> {
        let top = rows.iter().position(|row| row.starts_with("┌ run"))?;
        let inside = rows[top + 1..]
            .iter()
            .take_while(|row| !row.starts_with('└'))
            .map(|row| {
                let row = row.strip_prefix('│').unwrap_or(row);
                row.strip_suffix('│').unwrap_or(row).trim_end().to_string()
            })
            .collect();
        Some(inside)
    }

    fn cut_at_a_word(shown: &str, full: &str) -> bool {
        shown == full
            || shown
                .strip_suffix(" …")
                .is_some_and(|kept| full.starts_with(&format!("{kept} ")))
    }

    #[test]
    fn the_walk_runs_a_three_row_command_and_a_three_row_last_line_at_60_columns() {
        for bench in benches() {
            let model = staged(&bench, bench.tabs[0].0, false, Keys::Hints, true);
            let run = model.run().unwrap();

            assert_eq!(command_rows(run, 58).len(), 3, "{}", bench.selected);
            assert_eq!(wrapped(REFUSAL, 58).len(), 3, "{}", bench.selected);
        }
    }

    #[test]
    fn every_frame_of_the_walk_shows_the_selected_row_a_row_of_every_other_list_and_the_key_line_or_gives_way_to_a_question(
    ) {
        let mut given_way = Vec::new();
        for walked in walk() {
            let Walked {
                label,
                selected,
                shown,
                model,
                width,
                height,
                rows,
            } = &walked;
            let screen = rows.join("\n");
            let keys: Vec<String> = key_line(model, usize::from(*width))
                .iter()
                .map(|row| row.trim_end().to_string())
                .collect();

            assert_eq!(
                rows[rows.len() - keys.len()..],
                keys[..],
                "{label}\n{screen}"
            );
            if gave_way(model, *width, *height) {
                assert!(model.question().is_some(), "{label}\n{screen}");
                assert!(model.error().is_some(), "{label}\n{screen}");
                given_way.push(label.clone());
                continue;
            }
            assert!(
                rows.iter().any(|row| row.contains(selected)),
                "{label}\n{screen}"
            );
            for (title, row) in shown {
                let inside = boxed(rows, title);
                assert!(
                    inside.iter().any(|line| line.contains(row)),
                    "{label}: {row} in{title}\n{screen}"
                );
            }
        }
        assert_eq!(
            given_way,
            [
                ">stack-12 Sonda 60x15 error true Question pane false",
                ">stack-12 Sonda 60x15 error true Question pane true",
            ]
        );
    }

    #[test]
    fn a_row_of_several_facts_shows_the_facts_that_fit_whole_then_an_ellipsis_at_60_columns() {
        let mut wide = Vec::new();
        let mut narrow = Vec::new();
        for tab in [Tab::Overview, Tab::Timeline] {
            let mut model = devices_bench();
            open_tab(&mut model, tab);
            wide.extend(boxed(&drawn(&mut model, 100, 30), " srl1 "));
            narrow.extend(boxed(&drawn(&mut model, 60, 15), " srl1 "));
        }
        let mut cut = 0;
        for start in ["kind ", "maintenance ", "bgp sessions", "last "] {
            let find = |rows: &[String]| {
                let row = rows.iter().find(|row| row.starts_with(start)).unwrap();
                row.trim_end().to_string()
            };
            let (whole, shown) = (find(&wide), find(&narrow));
            if shown == whole {
                continue;
            }
            cut += 1;
            let kept = shown.strip_suffix(" …").unwrap_or(&shown);
            assert!(
                whole.starts_with(&format!("{kept}  ")),
                "{shown:?} is not whole facts of {whole:?}"
            );
        }
        assert!(cut > 0);
    }

    #[test]
    fn every_question_of_the_walk_shows_whole_with_y_n_in_one_row() {
        for Walked {
            label,
            model,
            width,
            ..
        } in walk()
        {
            let Some(question) = model.question() else {
                continue;
            };
            let rows = key_line(&model, usize::from(width));
            let joined: Vec<&str> = rows.iter().map(|row| row.trim()).collect();

            assert_eq!(joined.join(" "), question.text(), "{label}");
            assert!(rows.iter().any(|row| row.contains("y/n")), "{label}");
        }
    }

    #[test]
    fn the_run_pane_shows_the_whole_command_above_the_newest_output_or_only_the_newest_output() {
        for Walked {
            label,
            model,
            width,
            height,
            rows,
            ..
        } in walk()
        {
            let large = width >= 80 && height >= 24;
            let screen = rows.join("\n");
            let Some(inside) = pane_inside(&rows) else {
                assert!(!(large && model.run_pane()), "{label}\n{screen}");
                continue;
            };
            let run = model.run().unwrap();
            let inner = usize::from(width) - 2;
            let command = format!("$ nobs {}", run.command);
            let command_rows = wrapped(&command, inner).len();
            let last = wrapped(REFUSAL, inner).len();
            let both_fit = command_rows + last <= inside.len();
            let output: Vec<String> = run
                .lines
                .iter()
                .flat_map(|line| wrapped(line, inner))
                .collect();

            assert!(both_fit || !large, "{label}\n{screen}");
            if both_fit {
                assert_eq!(
                    inside[..command_rows].join(" "),
                    command,
                    "{label}\n{screen}"
                );
            } else {
                assert_eq!(
                    inside[..],
                    output[output.len() - inside.len()..],
                    "{label}\n{screen}"
                );
            }
        }
    }

    #[test]
    fn the_run_title_the_error_row_and_the_last_row_end_at_a_whole_word() {
        for Walked {
            label, model, rows, ..
        } in walk()
        {
            let screen = rows.join("\n");
            let words = &model.run().unwrap().words;
            let shown_after = |anchor: &str| {
                rows.iter()
                    .find_map(|row| row.split_once(anchor))
                    .and_then(|(_, rest)| rest.split("  ").next())
            };
            if let Some(title) = shown_after("run  ") {
                assert!(cut_at_a_word(title, words), "{label}\n{screen}");
            }
            if let Some(last) = shown_after("│ last      ") {
                assert!(cut_at_a_word(last, words), "{label}\n{screen}");
            }
            if let Some((source, text, _)) = model.error() {
                let (shown, age) = rows[2].rsplit_once("  ").unwrap();
                let seconds = age.strip_suffix("s ago").unwrap_or(age);
                assert!(seconds.parse::<u64>().is_ok(), "{label}\n{screen}");
                let full = format!("{source}: {text}");
                assert!(
                    cut_at_a_word(shown.trim_start(), &full),
                    "{label}\n{screen}"
                );
            }
        }
    }

    #[test]
    fn the_switcher_row_ends_with_its_keys_and_quit_hint_at_every_width() {
        for Walked { label, rows, .. } in walk() {
            assert!(rows[1].ends_with("? keys  q quit"), "{label}\n{}", rows[1]);
        }
    }

    #[test]
    fn the_services_title_counts_one_service_in_the_singular_and_the_header_holds_no_count() {
        for (count, title) in [
            (0, " a  0 services  "),
            (1, " a  1 service  "),
            (2, " a  2 services  "),
        ] {
            let mut model = Model::new(start(vec![stack("a", &[])]));
            handle(
                &mut model,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            );
            let rows = (0..count)
                .map(|index| row(&format!("s{index}"), "running"))
                .collect();
            model.apply(Update::Services(0, Ok(rows)));
            for (width, height) in SIZES {
                let screen = drawn(&mut model, width, height);

                assert!(!screen[0].contains("services"), "{}", screen.join("\n"));
                if width >= WIDE {
                    assert!(screen[2].contains(title), "{}", screen.join("\n"));
                }
            }
        }
    }

    #[test]
    fn a_closed_help_has_no_overflow_at_a_size_where_the_open_help_overflows() {
        let area = Rect::new(0, 0, 60, 15);
        let mut model = services_bench();

        assert_eq!(help_overflow(&model, area), 0);
        press(&mut model, '?');
        assert!(help_overflow(&model, area) > 0);
    }

    #[test]
    fn the_help_names_the_lab_switch_key_only_in_the_book_lab() {
        let mut book_services = lab_bench(2);
        press(&mut book_services, '2');
        for (model, switch) in [
            (devices_bench(), "1 2"),
            (services_bench(), "1 2"),
            (lab_bench(2), "1 2 3"),
            (book_services, "1 2 3"),
        ] {
            let entries = help_entries(&model);

            assert_eq!(
                entries[0],
                (switch.to_string(), "switch workbench"),
                "{:?}",
                model.workbench()
            );
        }
    }

    #[test]
    fn the_run_line_hint_says_what_o_does_now() {
        let mut folded_open = 0;
        for Walked {
            label, model, rows, ..
        } in walk()
        {
            let run_line = rows.iter().find(|row| row.starts_with(" run  "));
            let Some(run_line) = run_line.filter(|_| pane_inside(&rows).is_none()) else {
                continue;
            };
            let screen = rows.join("\n");
            let hint = if model.run_pane() {
                "Esc fold"
            } else {
                "o open"
            };
            folded_open += usize::from(model.run_pane());

            assert!(run_line.ends_with(hint), "{label}\n{screen}");
        }
        assert!(folded_open > 0);
    }

    #[test]
    fn the_filter_line_moves_a_hint_that_does_not_fit_whole_to_the_next_row() {
        let mut model = services_bench();
        press(&mut model, '/');
        let text = "s".repeat(44);
        text.chars().for_each(|typed| press(&mut model, typed));

        assert_eq!(
            key_line(&model, 60),
            [
                format!(" filter: {text}"),
                " Enter keep  Esc clear".to_string()
            ]
        );
    }

    #[test]
    fn a_long_notice_wraps_whole_onto_the_key_rows() {
        for (width, height) in SIZES {
            let mut model = lab_bench(2);
            press(&mut model, 'X');
            press(&mut model, 'y');
            press(&mut model, 'U');
            let notice = model.notice().unwrap().to_string();

            let rows = drawn(&mut model, width, height);

            let keys = key_line(&model, usize::from(width));
            let shown: Vec<&str> = rows[rows.len() - keys.len()..]
                .iter()
                .map(|row| row.trim())
                .collect();
            assert_eq!(shown.join(" "), notice, "{width}x{height}");
        }
    }

    #[test]
    fn a_long_command_line_shows_whole_above_the_last_line_or_leaves_the_pane_to_the_output() {
        let run = Run {
            words: "lab deploy -s ch1".to_string(),
            command: format!(
                "--dir /home/me/{}book-lab lab deploy -s ch1",
                "labs/".repeat(40)
            ),
            target: Target::Chapter(0),
            lines: vec![REFUSAL.to_string()],
            started: Instant::now(),
            ended: None,
            state: RunState::Running,
        };
        let command = command_rows(&run, 58);
        let refusal = wrapped(REFUSAL, 58);
        let rows = |height| -> Vec<String> {
            run_rows(&run, 58, height)
                .iter()
                .map(ToString::to_string)
                .collect()
        };

        assert!(command.len() >= 4);
        assert_eq!(refusal.len(), 3);
        let short = rows(command.len() + 2);
        assert_eq!(short[..], refusal[..]);
        let tall = rows(command.len() + 3);
        assert_eq!(tall[..command.len()], command[..]);
        assert_eq!(tall[command.len()..], refusal[..]);
    }

    #[test]
    fn the_run_pane_grows_past_half_only_to_keep_the_last_line_and_leaves_the_workbench_its_least()
    {
        let run = |lines: Vec<String>, state| Run {
            words: "lab deploy -s ch1".to_string(),
            command: "--dir /home/me/labs/network-observability/book-lab lab deploy -s ch1"
                .to_string(),
            target: Target::Chapter(0),
            lines,
            started: Instant::now(),
            ended: None,
            state,
        };
        let refused = run(vec![REFUSAL.to_string()], RunState::Ended(End::Exited(1)));
        let running = run(
            (0..10).map(|line| format!("line {line}")).collect(),
            RunState::Running,
        );
        let last_line = |rows: usize| {
            let line = (0..rows).map(|row| row.to_string().repeat(58)).collect();
            run(vec![line], RunState::Running)
        };

        assert_eq!(command_rows(&refused, 58).len(), 2);
        assert_eq!(pane_height(Some(&refused), 58, 11, 3), 5);
        assert_eq!(pane_height(Some(&refused), 58, 12, 3), 6);
        assert_eq!(pane_height(Some(&running), 58, 11, 3), 5);
        assert_eq!(pane_height(Some(&refused), 98, 26, 3), 9);
        assert_eq!(pane_height(Some(&refused), 98, 27, 3), 9);
        assert_eq!(pane_height(Some(&last_line(5)), 58, 11, 3), 7);
        assert_eq!(pane_height(Some(&last_line(6)), 58, 11, 3), 8);
        assert_eq!(pane_height(Some(&last_line(6)), 58, 12, 3), 8);
        assert_eq!(pane_height(Some(&last_line(7)), 58, 12, 3), 9);
        assert_eq!(pane_height(Some(&refused), 58, 11, 7), 4);
        assert_eq!(pane_height(Some(&running), 98, 26, 7), 9);
        assert_eq!(pane_height(Some(&last_line(7)), 58, 12, 7), 5);
        assert_eq!(pane_height(None, 58, 11, 9), 2);
        let newest: Vec<String> = run_rows(&last_line(7), 58, 7)
            .iter()
            .map(ToString::to_string)
            .collect();
        let seven: Vec<String> = (0..7).map(|row| row.to_string().repeat(58)).collect();
        assert_eq!(newest, seven);
    }

    #[test]
    fn the_device_alerts_row_with_a_firing_alert_ends_at_a_whole_fact() {
        let mut model = devices_model(workshop_located("infrahub"));
        model.apply(Update::Alerts(Ok(vec![alert(
            "srl1",
            "BgpSessionNotUp",
            "2026-11-09T13:53:32Z",
        )])));

        let rows = painted(28, 10, |frame| device_overview(frame, frame.area(), &model));

        let alerts = rows.iter().find(|row| row.starts_with("alerts")).unwrap();
        assert_eq!(alerts, "alerts   BgpSessionNotUp …");
    }

    #[test]
    fn the_interfaces_summary_row_ends_at_a_whole_fact() {
        let model = devices_model(workshop_located("infrahub"));

        let rows = painted(20, 10, |frame| {
            device_interfaces(frame, frame.area(), &model, false)
        });

        assert_eq!(rows[9], "4 interfaces …");
    }
}
