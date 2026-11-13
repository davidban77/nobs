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
const HELP_LAB: [(&str, &str); 9] = [
    ("1 2 3", "switch workbench"),
    ("j k ↑ ↓", "move"),
    ("D", "deploy chapter"),
    ("X", "destroy chapter"),
    ("r", "refresh now"),
    ("o", "run pane"),
    ("x", "dismiss the error"),
    ("?", "close"),
    ("Esc", "close/fold"),
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
            run_pane(frame, bottom, model);
        } else {
            run_line(frame, bottom, model);
        }
    }
    let keys = fit(&key_line(model), usize::from(key_row.width));
    frame.render_widget(Paragraph::new(keys), key_row);
    if model.help() {
        help(frame, body, model);
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

fn run_title(model: &Model) -> Vec<Span<'static>> {
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
    vec![
        Span::raw(format!(" run  {}  ", run.words)),
        Span::styled(state, style),
    ]
}

fn run_line(frame: &mut Frame, area: Rect, model: &Model) {
    frame.render_widget(Paragraph::new(Line::from(run_title(model))), area);
    right(frame, area, Line::from("o open"), 1);
}

fn run_pane(frame: &mut Frame, area: Rect, model: &Model) {
    let mut title = run_title(model);
    title.push(Span::raw(" "));
    let block = Block::bordered()
        .title_top(Line::from(title))
        .title_top(Line::from(" Esc fold ").right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(run) = model.run() else {
        return;
    };
    let width = usize::from(inner.width);
    let room = usize::from(inner.height).saturating_sub(1);
    let tail = &run.lines[run.lines.len().saturating_sub(room)..];
    let mut lines = vec![fitted(vec![(format!("$ nobs {}", run.words), DIM)], width)];
    lines.extend(tail.iter().map(|line| {
        let style = if line.starts_with("nobs:") {
            Style::new().fg(Color::Red)
        } else {
            Style::new()
        };
        fitted(vec![(line.clone(), style)], width)
    }));
    frame.render_widget(Paragraph::new(lines), inner);
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
    let (rows, _) = model.nodes();
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
    let (nodes, _) = model.nodes();
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

fn offers(model: &Model, key: char, target: Target) -> bool {
    ACTIONS
        .iter()
        .find(|action| action.key == key)
        .is_some_and(|action| arguments(action, &target, model).is_some())
}

fn offers_up_down(model: &Model) -> bool {
    offers(model, 'u', Target::Stack(model.selected_stack()))
}

fn offers_maintenance(model: &Model) -> bool {
    model
        .selected_device()
        .is_some_and(|device| offers(model, 'm', Target::Device(device.name.clone())))
}

fn key_line(model: &Model) -> String {
    if let Some(text) = model.typed_filter() {
        return format!(" filter: {text}  Enter keep  Esc clear");
    }
    if let Some(question) = model.question() {
        return format!(" {}", question.text());
    }
    if let Some(notice) = model.notice() {
        return format!(" {notice}");
    }
    let keys = if model.chooser() {
        " j/k select  Enter open  q quit"
    } else if model.workbench() == Workbench::Lab {
        " j/k move  D deploy  X destroy  r refresh  o pane"
    } else if model.workbench() == Workbench::Devices {
        return device_keys(model);
    } else {
        let up_down = if offers_up_down(model) {
            "  u up  d down"
        } else {
            ""
        };
        match (model.focus(), model.tab()) {
            (Focus::List, _) => {
                return format!(" Tab focus  j/k move  / filter  l logs  r refresh{up_down}");
            }
            (Focus::Tabs, Tab::Overview | Tab::Interfaces | Tab::Timeline) => {
                return format!(" Tab focus  j/k move  [ ] tab  Enter logs  Esc back{up_down}");
            }
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

fn device_keys(model: &Model) -> String {
    let maintenance = if offers_maintenance(model) {
        "  m maintenance"
    } else {
        ""
    };
    let keys = match (model.focus(), model.tab()) {
        (Focus::List, _) => {
            return format!(" Tab focus  j/k move  / filter  Enter open  r refresh{maintenance}");
        }
        (Focus::Tabs, Tab::Timeline) if model.popup() => " Esc close",
        (Focus::Tabs, Tab::Timeline) => " j/k move  Enter full text  - + window  [ ] tab  Esc back",
        (Focus::Tabs, Tab::Logs) => match model.device_logs() {
            Some((_, _, false, _)) => " f follow  / filter  j/k scroll  [ ] tab  Esc back to list",
            _ => " f pause  / filter  j/k scroll  [ ] tab  Esc back to list",
        },
        (Focus::Tabs, _) => {
            return format!(" Tab focus  [ ] tab  Esc back{maintenance}");
        }
    };
    keys.to_string()
}

fn help(frame: &mut Frame, body: Rect, model: &Model) {
    let workbench = model.workbench();
    let (mut entries, note): (Vec<(&str, &str)>, Option<&str>) = match workbench {
        Workbench::Devices => (HELP_DEVICES.to_vec(), Some(HELP_DEVICES_NOTE)),
        Workbench::Services => (HELP.to_vec(), Some(HELP_SONDA)),
        Workbench::Lab => (HELP_LAB.to_vec(), None),
    };
    if workbench == Workbench::Services && offers_up_down(model) {
        entries.push(("u d", "stack up / down"));
    }
    if workbench == Workbench::Devices && offers_maintenance(model) {
        entries.push(("m", "maintenance on/off"));
    }
    entries.extend(HELP_QUIT);
    let width = body.width.saturating_sub(4);
    let note_rows = note.map_or(0, |note| {
        let note = u16::try_from(note.chars().count()).unwrap_or(u16::MAX);
        note.div_ceil(width.saturating_sub(2).max(1)).max(1)
    });
    let rows = u16::try_from(entries.len().div_ceil(2)).unwrap_or(u16::MAX);
    let note_height = if note.is_some() { note_rows + 1 } else { 0 };
    let height = body.height.min(rows + 2 + note_height);
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
    let (first, second) = entries.split_at(usize::from(rows));
    let middle = inner.width / 2;
    frame.render_widget(
        Paragraph::new(help_column(first)),
        column(0, 0, middle, rows),
    );
    frame.render_widget(
        Paragraph::new(help_column(second)),
        column(middle, 0, inner.width - middle, rows),
    );
    if let Some(note) = note {
        let note_row = (rows + 1)
            .min(inner.height.saturating_sub(note_rows))
            .max(rows);
        frame.render_widget(
            Paragraph::new(note).wrap(Wrap { trim: false }),
            column(0, note_row, inner.width, note_rows),
        );
    }
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
