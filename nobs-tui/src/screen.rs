use std::time::{Instant, UNIX_EPOCH};

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Cell, Clear, HighlightSpacing, Paragraph, Row, Table, TableState, Wrap,
};
use ratatui::Frame;

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
const HELP: [(&str, &str); 13] = [
    ("1 2 3", "switch workbench"),
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
const HELP_DEVICES: [(&str, &str); 13] = [
    ("1 2 3", "switch workbench"),
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
const HELP_LAB: [(&str, &str); 7] = [
    ("1 2 3", "switch workbench"),
    ("j k ↑ ↓", "move"),
    ("r", "refresh now"),
    ("o", "run pane"),
    ("x", "dismiss the error"),
    ("?", "close"),
    ("Esc", "close/fold"),
];
const HELP_MORE: &str = "j/k more";
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
        let line = format!(" {source}: {text}  {}s ago", seconds(model, at));
        frame.render_widget(
            Paragraph::new(line).style(Style::new().fg(Color::Red)),
            error_row,
        );
    }
    workbench(frame, body, model);
    if !model.chooser() {
        if model.run_pane() {
            run_pane(frame, bottom, model);
        } else {
            run_line(frame, bottom, model);
        }
    }
    frame.render_widget(Paragraph::new(keys.join("\n")), key_row);
    if model.help() {
        help(frame, body, model);
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
    let bottom_height = if model.chooser() {
        0
    } else if model.run_pane() {
        let width = usize::from(middle.width.saturating_sub(2));
        pane_height(model.run(), width, middle.height, workbench_least(model))
    } else {
        1
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
    let essential = rows(command.min(1) + last);
    let third = middle.saturating_sub(1) / 3 + 1;
    third
        .max(needed.min(middle / 2))
        .max(essential.min(middle.saturating_sub(least)))
}

fn workbench_least(model: &Model) -> u16 {
    let row = match model.workbench() {
        Workbench::Devices => return 7,
        Workbench::Services => model
            .visible_stacks()
            .iter()
            .position(|index| *index == model.selected_stack()),
        Workbench::Lab => model
            .selected_chapter()
            .and_then(|chapter| model.chapters().iter().position(|index| *index == chapter)),
    };
    u16::try_from(row.unwrap_or(0) + 3).unwrap_or(u16::MAX)
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
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars
        .chunks(width.max(1))
        .map(|row| row.iter().collect())
        .collect()
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
    let shown = if model.book_lab() { 3 } else { 2 };
    for (index, (workbench, name)) in entries.into_iter().take(shown).enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(if workbench == model.workbench() {
            Span::styled(format!("[{name}]"), REVERSE)
        } else {
            Span::raw(name)
        });
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    right(frame, area, Line::from("? keys  q quit"), 1);
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
    let lines: Vec<Line> = indices
        .iter()
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
        format!(" {name}  {} services  {age} ", model.services().len())
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
    let logs = if on_devices {
        model
            .device_logs()
            .map(|(device, _, following, _)| (device, following))
    } else {
        model
            .logs()
            .map(|(service, _, following, _)| (service, following))
    };
    if let (Tab::Logs, Some((service, following))) = (model.tab(), logs) {
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
    let alerts_height = (inner.height / 4).max(2);
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
    let summary = format!("{} interfaces  {differs} differs", interfaces.len());
    frame.render_widget(
        Paragraph::new(fitted(vec![(summary, Style::new())], width)),
        footer,
    );
}

fn device_overview(frame: &mut Frame, area: Rect, model: &Model) {
    let Some(device) = model.selected_device() else {
        return;
    };
    let width = usize::from(area.width);
    let plain = Style::new();
    let line = |text: String| fitted(vec![(text, plain)], width);
    let states = model.session_states();
    let established = states.iter().filter(|(_, state)| state == "up").count();
    let seen = model.devices_at().map_or("—".to_string(), |at| {
        format!("{}s ago", seconds(model, at))
    });
    let mut lines = vec![
        line(format!(
            "kind {}   site {}   role {}",
            devices::kind_short(device.kind.as_deref()),
            device.site.as_deref().unwrap_or("—"),
            device.role.as_deref().unwrap_or("—")
        )),
        line(format!(
            "maintenance {}   sot {}   seen {seen}",
            if device.maintenance { "yes" } else { "no" },
            model.sot()
        )),
        line(format!(
            "bgp sessions  {} intended  {established} established  {} other",
            states.len(),
            states.len() - established
        )),
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
    lines.push(line(match latest {
        _ if !model.alerts_ok() => "alerts   ?".to_string(),
        Some(alert) => format!(
            "alerts   {}  {}  {}",
            alert.labels.get("alertname").map_or("—", String::as_str),
            alert.labels.get("severity").map_or("—", String::as_str),
            time::age(&alert.starts_at, wall_seconds(model))
        ),
        None => "alerts   none".to_string(),
    }));
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
    let summary = format!(
        "last {}   alerts {alerts}  flows {flows}  rca {rcas}  logs {logs}",
        devices::window_text(model.window())
    );
    frame.render_widget(
        Paragraph::new(fitted(
            vec![(summary, Style::new())],
            usize::from(area.width),
        )),
        counts,
    );
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
    let words = if run.words.chars().count() > room {
        fit(&run.words, room)
    } else {
        run.words.clone()
    };
    vec![
        Span::raw(format!(" run  {words}  ")),
        Span::styled(state, style),
    ]
}

fn run_line(frame: &mut Frame, area: Rect, model: &Model) {
    let hint = Line::from("o open");
    let before_hint = usize::from(area.width).saturating_sub(hint.width() + 1);
    frame.render_widget(
        Paragraph::new(Line::from(run_title(model, before_hint))),
        area,
    );
    right(frame, area, hint, 1);
}

fn run_pane(frame: &mut Frame, area: Rect, model: &Model) {
    let hint = Line::from(" Esc fold ").right_aligned();
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
    let mut command = command_rows(run, width);
    let output: usize = run
        .lines
        .iter()
        .map(|line| wrapped(line, width).len())
        .sum();
    let keep = height.saturating_sub(output).clamp(1, command.len());
    if keep < command.len() {
        let rest = command.split_off(keep - 1).concat();
        command.push(fit(&rest, width));
    }
    let mut lines: Vec<Line> = command
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
    let [top, bottom] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(chapters_height)]).areas(left);
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
    let last = model
        .run()
        .filter(|run| run.target == Target::Chapter(chapter));
    let (last, end) = match last {
        Some(run) => {
            let (end, style) = run_end(run);
            (format!("{}  ", run.words), (end, style))
        }
        None => ("—".to_string(), (String::new(), Style::new())),
    };
    let width = usize::from(content.width);
    let row = |label: &str, value: String| format!(" {label:<8}  {value}");
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
        return vec![fit(
            &format!(" filter: {text}  Enter keep  Esc clear"),
            width,
        )];
    }
    if let Some(question) = model.question() {
        return wrapped(&format!(" {}", question.text()), width);
    }
    if let Some(notice) = model.notice() {
        return vec![fit(&format!(" {notice}"), width)];
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

fn whole_hints(hints: &[String], width: usize) -> String {
    let mut line = String::new();
    for hint in hints {
        let gap = if line.is_empty() { " " } else { "  " };
        if line.chars().count() + gap.len() + hint.chars().count() > width {
            break;
        }
        line.push_str(gap);
        line.push_str(hint);
    }
    line
}

pub(crate) fn help_entries(workbench: Workbench) -> Vec<(String, &'static str)> {
    let fixed: &[(&str, &str)] = match workbench {
        Workbench::Devices => &HELP_DEVICES,
        Workbench::Services => &HELP,
        Workbench::Lab => &HELP_LAB,
    };
    let actions = ACTIONS
        .iter()
        .filter(|action| action.workbench == workbench)
        .map(|action| (action.key.to_string(), action.hint));
    let owned = |&(key, meaning): &(&str, &'static str)| (key.to_string(), meaning);
    fixed
        .iter()
        .map(owned)
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
    let workbench = model.workbench();
    let entries = help_entries(workbench);
    let note = match workbench {
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

pub(crate) fn help_overflow(model: &Model) -> usize {
    let (width, height) = model.size();
    let (_, [_, _, _, body, _, _]) = regions(model, Rect::new(0, 0, width, height));
    let layout = help_layout(model, body);
    usize::from(layout.rows - layout.room)
}

fn help(frame: &mut Frame, body: Rect, model: &Model) {
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
    } = help_layout(model, body);
    frame.render_widget(Clear, body);
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
    use super::*;

    const REFUSAL: &str = "nobs: lab deploy: node containers already exist (clab-nobs-t-book-nobs-t-srl1, clab-nobs-t-book-nobs-t-srl2); run nobs lab destroy first";

    #[test]
    fn a_long_command_line_gives_its_rows_to_the_output_down_to_one() {
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
        let short = rows(4);
        assert_eq!(short[0], format!("{}…", &command[0][..57]));
        assert_eq!(short[1..], refusal[..]);
        let taller = rows(6);
        assert_eq!(taller[..2], command[..2]);
        assert_eq!(taller[2], format!("{}…", &command[2][..57]));
        assert_eq!(taller[3..], refusal[..]);
    }

    #[test]
    fn the_run_pane_grows_past_half_only_to_keep_the_first_command_row_and_the_last_line() {
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
        assert_eq!(pane_height(Some(&refused), 58, 11, 3), 6);
        assert_eq!(pane_height(Some(&refused), 58, 12, 3), 6);
        assert_eq!(pane_height(Some(&running), 58, 11, 3), 5);
        assert_eq!(pane_height(Some(&refused), 98, 26, 3), 9);
        assert_eq!(pane_height(Some(&refused), 98, 27, 3), 9);
        assert_eq!(pane_height(Some(&last_line(5)), 58, 11, 3), 8);
        assert_eq!(pane_height(Some(&last_line(6)), 58, 11, 3), 8);
        assert_eq!(pane_height(Some(&last_line(6)), 58, 12, 3), 9);
        assert_eq!(pane_height(Some(&last_line(7)), 58, 12, 3), 9);
        let newest: Vec<String> = run_rows(&last_line(7), 58, 7)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(newest.len(), 7);
        assert!(newest[0].ends_with('…'));
        let newest_six: Vec<String> = (1..7).map(|row| row.to_string().repeat(58)).collect();
        assert_eq!(newest[1..], newest_six[..]);
    }
}
