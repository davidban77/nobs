use std::time::{Instant, UNIX_EPOCH};

use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::line;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Cell, Clear, HighlightSpacing, Paragraph, Row, Table, TableState, Wrap,
};
use ratatui::Frame;
use tui_term::widget::{Cursor as TermCursor, PseudoTerminal};

use nobs_core::alertmanager::Alert;
use nobs_core::count::counted;
use nobs_core::locate::{Located, Nodes};
use nobs_core::project::Rendered;
use nobs_core::status::{row, Service};
use nobs_core::time;

use crate::actions::{self, arguments, fits, Target, ACTIONS};
use crate::devices;
use crate::run::End;
use crate::shell::{
    absent, kind_text, Agreement, Cursor, Focus, IntendedView, LoginEntry, Model, Reach, Run,
    RunMeasure, RunState, StackKind, StackState, Tab, Workbench, NO_BREAK, OBSERVABILITY_TABS,
    SERVICES_TABS,
};
use crate::terminal::SHELL;
use crate::view;

const WIDE: u16 = 100;
const LEFT_NARROW: u16 = 23;
const LEFT_WIDE: u16 = 32;
const LEFT_DEVICES: u16 = 42;
const TOO_SMALL: &str = "terminal too small: needs 60 columns and 15 rows";
const CONFIG_HINT: &str = "press g to get the running config";
const INTENDED_HINT: &str = "press g to compare with the device";
const INTENDED_ERROR: &str = "intended: error";
const VIEW_RUNNING: &str = "v running";
const VIEW_INTENDED: &str = "v intended";
const DIFFERENCE_NARROW: &str = "n N difference";
const DIFFERENCE_WIDE: &str = "n N next/previous difference";
const HELP_CONFIG: [(&str, &str); 2] = [
    ("v", "intended or running config"),
    ("n N", "next/previous difference"),
];
const TERMINAL_HINT: &str = "press t to open a shell on";
const HELP_QUIT_SESSION: &str = "quit; asks if busy";
const HELP_TERMINAL: &str = "type in terminal";
const PANE_HINTS: [&str; 4] = ["/ filter", "j/k scroll", "[ ] tab", "Esc back to list"];
const HELP_DEVICES_NOTE: &str = "alerts box: Enter picks the device, opens its timeline";
const HELP_SONDA: &str = "sonda tab: Tab details  l load  d delete  r refresh";
const HELP_RUN: &str = "run pane, after Tab: j/k scroll  f follow  Esc fold";
const HELP_MARK: &str = "↑3 ↓13   3 rows hidden above, 13 below";
const HELP_PAGE: &str = "PgUp PgDn  move a page where j and k move";
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
const HELP_LAB: [(&str, &str); 7] = [
    ("Tab", "focus next pane"),
    ("j k ↑ ↓", "move"),
    ("r", "refresh now"),
    ("o", "run pane"),
    ("x", "dismiss the error"),
    ("?", "close"),
    ("Esc", "close/fold/back"),
];
const HELP_OBSERVABILITY: [(&str, &str); 10] = [
    ("Tab", "focus"),
    ("j k ↑ ↓", "move"),
    ("[ ]", "other view"),
    ("/", "filter"),
    ("Enter", "open the device"),
    ("r", "refresh now"),
    ("o", "run pane"),
    ("x", "dismiss the error"),
    ("Esc", "close/clear/back"),
    ("?", "close"),
];
const KEYS_HINT: &str = "? keys  q quit";
const STATE_WIDTH: usize = 9;
const COUNT_WIDTH: usize = 11;
const HELP_MORE: &str = "j/k more";
const FOLD: &str = "Esc fold";
const PANE_LEAST: u16 = 3;
const TERMINAL_LEAST: u16 = 2;
const NODES_LEAST: u16 = 4;
const ALERTS_LEAST: u16 = 2;
const BORDERS: u16 = 2;
const REVERSE: Style = Style::new().add_modifier(Modifier::REVERSED);
const DIM: Style = Style::new().add_modifier(Modifier::DIM);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Holder {
    Chooser,
    Keys,
    Entry,
    Stacks,
    Chapters,
    Nodes,
    Overview,
    Pane,
    Devices,
    Sonda(view::Part),
    Run,
    Alerts,
}

fn holder(model: &Model) -> Holder {
    if model.chooser() {
        return Holder::Chooser;
    }
    if model.help() {
        return Holder::Keys;
    }
    match (model.workbench(), model.focus()) {
        (_, Focus::Run) => Holder::Run,
        (Workbench::Services, Focus::List) => Holder::Stacks,
        (Workbench::Services, Focus::Tabs) if model.tab() == Tab::Sonda => {
            Holder::Sonda(view::Part::Scenarios)
        }
        (Workbench::Services, Focus::Tabs) => Holder::Pane,
        (Workbench::Services, Focus::Lines) => Holder::Sonda(view::Part::Details),
        (Workbench::Devices, Focus::Tabs)
            if model.tab() == Tab::Timeline && model.popup_entry().is_some() =>
        {
            Holder::Entry
        }
        (Workbench::Devices, Focus::Tabs) => Holder::Pane,
        (Workbench::Devices, _) => Holder::Devices,
        (Workbench::Lab, Focus::List) => Holder::Chapters,
        (Workbench::Lab, Focus::Tabs) => Holder::Nodes,
        (Workbench::Lab, Focus::Lines) => Holder::Overview,
        (Workbench::Observability, _) => Holder::Alerts,
    }
}

pub(crate) fn border(focused: bool) -> BorderType {
    if focused {
        BorderType::Double
    } else {
        BorderType::Plain
    }
}

fn border_of(model: &Model, which: Holder) -> BorderType {
    border(holder(model) == which)
}

fn line_set(border: BorderType) -> line::Set<'static> {
    if border == BorderType::Plain {
        line::NORMAL
    } else {
        line::DOUBLE
    }
}

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
        let head = format!(" {source}: ");
        let room =
            usize::from(error_row.width).saturating_sub(head.chars().count() + age.chars().count());
        let line = format!("{head}{}{age}", cut_words(text, room, Keep::End));
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
        let grown = model.focus() == Focus::Run;
        let pane = pane_height(model.run(), width, middle.height, least, grown);
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

fn pane_height(run: Option<&Run>, width: usize, middle: u16, least: u16, grown: bool) -> u16 {
    let rows = |count: usize| u16::try_from(count + 2).unwrap_or(u16::MAX);
    let command = run.map_or(0, |run| command_rows(run, width).len());
    let last = run
        .and_then(|run| run.lines.last())
        .map_or(0, |line| wrapped(line, width).len());
    let needed = rows(command + last);
    let essential = rows(last);
    let third = middle.saturating_sub(1) / 3 + 1;
    let ungrown = third
        .max(needed.min(middle / 2))
        .max(essential)
        .min(middle.saturating_sub(least));
    if !grown {
        return ungrown;
    }
    let want = rows(run.map_or(0, |run| run_all_rows(run, width).len()));
    let height = ungrown.max(want.min(middle));
    if middle - height < least {
        middle
    } else {
        height
    }
}

pub(crate) fn run_view(model: &Model, area: Rect) -> Option<RunMeasure> {
    if model.too_small() {
        return None;
    }
    let (_, [_, _, _, _, bottom, _]) = regions(model, area);
    if bottom.height < 2 {
        return None;
    }
    let inner = Block::bordered().inner(bottom);
    let width = usize::from(inner.width);
    let total = model.run().map_or(0, |run| run_all_rows(run, width).len());
    Some(RunMeasure {
        overflow: total.saturating_sub(usize::from(inner.height)),
        cut_rows: model
            .run_cut()
            .iter()
            .map(|line| wrapped(line, width).len())
            .sum(),
    })
}

pub(crate) fn terminal_size(model: &Model, area: Rect) -> (u16, u16) {
    let (_, [_, _, _, body, _, _]) = regions(model, area);
    let [_, inside] = tab_content(body);
    (
        inside.height.max(TERMINAL_LEAST),
        inside.width.max(TERMINAL_LEAST),
    )
}

pub(crate) fn shown_rows(model: &Model, area: Rect) -> usize {
    let (_, [_, _, _, body, bottom, _]) = regions(model, area);
    let inside = |area: Rect| usize::from(Block::bordered().inner(area).height);
    match holder(model) {
        Holder::Chooser => inside(body),
        Holder::Keys => usize::from(help_layout(model, body.union(bottom)).room),
        Holder::Entry => inside(popup_area(device_content(body))),
        Holder::Stacks => inside(service_panes(body)[0]),
        Holder::Chapters => inside(lab_panes(model, device_panes(body)[0])[1]),
        Holder::Nodes => {
            let [nodes, _] = lab_panes(model, device_panes(body)[0]);
            table_room(Block::bordered().inner(nodes))
        }
        Holder::Overview => usize::from(device_content(body).height),
        Holder::Pane => {
            let services = model.workbench() == Workbench::Services;
            let content = if services {
                service_content(body)
            } else {
                device_content(body)
            };
            match model.tab() {
                Tab::Overview if services => table_room(content),
                Tab::Overview | Tab::Logs => usize::from(content.height),
                Tab::Timeline => usize::from(timeline_layout(model, content).1[1].height),
                Tab::Interfaces => table_room(table_and_footer(content)[0]),
                Tab::Config => usize::from(config_layout(model, content)[2].height),
                Tab::Terminal => usize::from(terminal_size(model, area).0),
                Tab::Sonda | Tab::Alerts | Tab::Metrics => 0,
            }
        }
        Holder::Devices => {
            let [list, _, alerts] = list_and_box(Block::bordered().inner(device_panes(body)[0]));
            match model.cursor() {
                Cursor::Device(_) => table_room(list),
                Cursor::Alert(_) => usize::from(alerts.height),
            }
        }
        Holder::Sonda(part) => view::shown_rows(service_content(body), part),
        Holder::Run => inside(bottom),
        Holder::Alerts => {
            let inner = Block::bordered().inner(body);
            match model.tab() {
                Tab::Logs => usize::from(header_and_rows(inner)[1].height),
                Tab::Metrics => table_room(inner),
                _ if model.selected_alert().is_some() => table_room(list_and_box(inner)[0]),
                _ => table_room(inner),
            }
        }
    }
}

fn workbench_least(model: &Model) -> u16 {
    let list = match model.workbench() {
        Workbench::Devices | Workbench::Observability => BORDERS + 3 + ALERTS_LEAST,
        Workbench::Services => BORDERS + 1,
        Workbench::Lab => NODES_LEAST + BORDERS + 1,
    };
    let tab_rows = match (model.workbench(), model.tab()) {
        (Workbench::Lab, _) => 1,
        (Workbench::Services, Tab::Overview) | (_, Tab::Timeline | Tab::Config) => 2,
        (_, Tab::Overview | Tab::Logs | Tab::Alerts | Tab::Metrics | Tab::Terminal) => 1,
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

pub(crate) fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return format!("{text:<width$}");
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

pub(crate) fn widest<const N: usize, S: AsRef<str>>(
    titles: [&str; N],
    rows: &[[S; N]],
) -> [usize; N] {
    let mut widths = titles.map(|title| title.chars().count());
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.as_ref().chars().count());
        }
    }
    widths
}

pub(crate) fn wrapped(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row = String::new();
    let words = text.split(' ').flat_map(|piece| {
        let whole = piece.chars().count() <= width;
        piece.split(move |letter| letter == NO_BREAK && !whole)
    });
    for (index, word) in words.enumerate() {
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
    rows.iter().map(|row| row.replace(NO_BREAK, " ")).collect()
}

#[derive(Clone, Copy)]
pub(crate) enum Keep {
    Start,
    End,
}

pub(crate) fn cut_words(text: &str, width: usize, keep: Keep) -> String {
    let words: Vec<&str> = text.split(' ').collect();
    cut_whole(&words, " ", width, keep).replace(NO_BREAK, " ")
}

fn cut_whole(pieces: &[&str], gap: &str, width: usize, keep: Keep) -> String {
    let text = pieces.join(gap);
    if text.chars().count() <= width {
        return text;
    }
    let shown = |count: usize| match keep {
        Keep::Start => format!("{} …", pieces[..count].join(gap)),
        Keep::End => format!("… {}", pieces[pieces.len() - count..].join(gap)),
    };
    (1..pieces.len())
        .rev()
        .map(shown)
        .find(|cut| cut.chars().count() <= width)
        .unwrap_or_else(|| "…".chars().take(width).collect())
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
    let keys = Line::from(KEYS_HINT);
    let full = workbench_line(model, true);
    let fits = full.width() + 2 + keys.width() < usize::from(area.width);
    let line = if fits {
        full
    } else {
        workbench_line(model, false)
    };
    frame.render_widget(Paragraph::new(line), area);
    right(frame, area, keys, 1);
}

fn workbench_line(model: &Model, wide: bool) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (key, workbench) in workbench_keys(model) {
        if key > 1 {
            spans.push(Span::raw("  "));
        }
        let name = format!("{key} {}", workbench.title(wide));
        spans.push(if workbench == model.workbench() {
            Span::styled(format!("[{name}]"), REVERSE)
        } else {
            Span::raw(name)
        });
    }
    Line::from(spans)
}

fn workbench_keys(model: &Model) -> impl Iterator<Item = (usize, Workbench)> {
    (1..).zip(model.workbenches())
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
        Workbench::Observability => observability(frame, area, model),
    }
}

fn state_style(state: &StackState) -> Style {
    match state {
        StackState::Up { running, total } if running == total => Style::new().fg(Color::Green),
        StackState::Up { .. } => Style::new().fg(Color::Yellow),
        StackState::Unknown | StackState::Failed => Style::new().fg(Color::Red),
        StackState::Down { .. } => DIM,
    }
}

fn state_width(model: &Model, indices: impl Iterator<Item = usize>) -> usize {
    indices
        .map(|index| model.stack_state(index).text().chars().count())
        .max()
        .unwrap_or(0)
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
    let [left, right] = service_panes(area);
    stacks(frame, left, model);
    detail(frame, right, model, area.width >= WIDE);
}

fn service_panes(area: Rect) -> [Rect; 2] {
    let left_width = if area.width >= WIDE {
        LEFT_WIDE
    } else {
        LEFT_NARROW
    };
    Layout::horizontal([Constraint::Length(left_width), Constraint::Min(0)]).areas(area)
}

pub(crate) fn sonda_overflow(model: &Model, area: Rect) -> usize {
    let (_, [_, _, _, body, _, _]) = regions(model, area);
    view::details_overflow(model.sonda(), service_content(body))
}

fn service_content(body: Rect) -> Rect {
    let [_, right] = service_panes(body);
    let [_, content] = tab_content(right);
    content
}

fn stacks(frame: &mut Frame, area: Rect, model: &Model) {
    let visible = model.visible_stacks();
    let selected = Some(model.selected_stack());
    let block = Block::bordered()
        .border_type(border_of(model, Holder::Stacks))
        .title_top(format!(" stacks {} ", visible.len()));
    if !visible.is_empty()
        && visible
            .iter()
            .all(|index| model.stack_state(*index).total() == Some(0))
    {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        dim_line(frame, inner, "no services in this lab".to_string());
        return;
    }
    stack_list(frame, area, model, block, &visible, selected, Target::Stack);
}

fn stack_list(
    frame: &mut Frame,
    area: Rect,
    model: &Model,
    block: Block,
    indices: &[usize],
    selected: Option<usize>,
    target: fn(usize) -> Target,
) {
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let longest = indices
        .iter()
        .map(|index| model.stacks()[*index].name.chars().count())
        .max()
        .unwrap_or(0);
    let state_width = state_width(model, indices.iter().copied());
    let name_width = longest.min(usize::from(inner.width).saturating_sub(3 + state_width));
    let at = indices
        .iter()
        .position(|index| Some(*index) == selected)
        .unwrap_or(0);
    let room = usize::from(inner.height);
    let first = first_row(at, room);
    let lines: Vec<Line> = indices
        .iter()
        .skip(first)
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
    rows_mark(frame, border_below(inner), first, room, indices.len());
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
        let count = model
            .stack_state(model.selected_stack())
            .total()
            .map_or_else(|| "?".to_string(), |total| counted(total, "service"));
        let width = usize::from(area.width).saturating_sub(4);
        format!(
            " {} ",
            cut_whole(
                &[name, count.as_str(), age.as_str()],
                "  ",
                width,
                Keep::Start
            )
        )
    } else {
        format!(" {name}  {age} ")
    };
    frame.render_widget(
        Block::bordered()
            .border_type(border_of(model, Holder::Pane))
            .title_top(title),
        area,
    );
    let [tab_row, content] = tab_content(area);
    tabs(frame, tab_row, model, wide);
    match model.tab() {
        Tab::Overview
        | Tab::Interfaces
        | Tab::Config
        | Tab::Timeline
        | Tab::Alerts
        | Tab::Metrics => overview(frame, content, model, wide),
        Tab::Logs => logs(frame, content, model),
        Tab::Terminal => {}
        Tab::Sonda => {
            view::draw_in(
                frame,
                content,
                model.sonda(),
                model.now(),
                match holder(model) {
                    Holder::Sonda(part) => Some(part),
                    _ => None,
                },
            );
            if let Some(sentence) = model.absent_sentence(Service::Sonda) {
                let top_row = Rect {
                    height: content.height.min(1),
                    ..content
                };
                frame.render_widget(Clear, top_row);
                dim_line(frame, top_row, sentence);
            }
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
        model.device_tabs()
    } else {
        &SERVICES_TABS
    };
    let word = model.drawn_session().map(|session| session.state.word());
    let width = usize::from(area.width)
        .saturating_sub(word.as_ref().map_or(0, |word| word.chars().count() + 2));
    let line = tab_line(shown, model.tab(), wide, width);
    let room = usize::from(area.width).saturating_sub(line.width() + 2);
    frame.render_widget(Paragraph::new(line), area);
    if let Some(word) = word {
        right(frame, area, Line::from(word), 0);
    }
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

fn tab_line(tabs: &[Tab], open: Tab, wide: bool, width: usize) -> Line<'static> {
    let titles: Vec<&str> = tabs.iter().map(|tab| tab.title(wide)).collect();
    let at = tabs.iter().position(|tab| *tab == open).unwrap_or(0);
    let (first, end) = tab_window(&titles, at, width);
    let mut spans = Vec::new();
    if first > 0 {
        spans.push(Span::raw("‹ "));
    }
    for (index, tab) in tabs.iter().enumerate().take(end).skip(first) {
        if index > first {
            spans.push(Span::raw("  "));
        }
        let name = tab.title(wide);
        spans.push(if *tab == open {
            Span::styled(name.to_uppercase(), REVERSE)
        } else {
            Span::raw(name)
        });
    }
    if end < tabs.len() {
        spans.push(Span::raw(" ›"));
    }
    Line::from(spans)
}

fn tab_window(titles: &[&str], open: usize, width: usize) -> (usize, usize) {
    let fits = |first: usize, end: usize| {
        let names: usize = titles[first..end]
            .iter()
            .map(|title| title.chars().count() + 2)
            .sum();
        let marks = 2 * (usize::from(first > 0) + usize::from(end < titles.len()));
        names - 2 + marks <= width
    };
    let mut first = 0;
    loop {
        let end = (first + 2..=titles.len())
            .rev()
            .find(|end| fits(first, *end))
            .unwrap_or(first + 1);
        if open < end || first >= open {
            return (first, end);
        }
        first += 1;
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
    let mut widths = widest(titles, &rows);
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
    let room = table_room(area);
    let first = first_row(selected.unwrap_or(0), room);
    let mut state = TableState::default()
        .with_selected(selected)
        .with_offset(first);
    frame.render_stateful_widget(table, area, &mut state);
    rows_mark(frame, border_below(area), first, room, rows.len());
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
        .map(|line| Line::from(fit(line, usize::from(area.width))))
        .collect();
    frame.render_widget(Paragraph::new(shown), area);
    rows_mark(frame, border_below(area), start, end - start, lines.len());
}

pub(crate) fn first_row(selected: usize, room: usize) -> usize {
    (selected + 1).saturating_sub(room)
}

pub(crate) fn table_room(area: Rect) -> usize {
    usize::from(area.height).saturating_sub(1)
}

pub(crate) fn border_below(inner: Rect) -> Rect {
    Rect::new(inner.x + 1, inner.bottom(), inner.width, 1)
}

pub(crate) fn rows_mark(frame: &mut Frame, free: Rect, first: usize, room: usize, total: usize) {
    let below = total.saturating_sub(first + room);
    let mark = match (first, below) {
        (0, 0) => return,
        (0, below) => format!("↓{below}"),
        (above, 0) => format!("↑{above}"),
        (above, below) => format!("↑{above} ↓{below}"),
    };
    let line = format!(" {mark} ");
    if line.chars().count() + 2 <= usize::from(free.width) {
        right(frame, free, Line::from(line), 2);
    }
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

fn device_panes(area: Rect) -> [Rect; 2] {
    let left_width = if area.width >= WIDE {
        LEFT_DEVICES
    } else {
        LEFT_NARROW
    };
    Layout::horizontal([Constraint::Length(left_width), Constraint::Min(0)]).areas(area)
}

fn tab_content(area: Rect) -> [Rect; 2] {
    Layout::vertical([Constraint::Length(1), Constraint::Min(0)])
        .areas(Block::bordered().inner(area))
}

fn device_content(body: Rect) -> Rect {
    let [_, right] = device_panes(body);
    let [_, content] = tab_content(right);
    content
}

fn devices(frame: &mut Frame, area: Rect, model: &Model) {
    let wide = area.width >= WIDE;
    if model.tab() == Tab::Terminal {
        device_detail(frame, area, model, wide);
        return;
    }
    let [left, right] = device_panes(area);
    device_list(frame, left, model, wide);
    device_detail(frame, right, model, wide);
}

fn device_list(frame: &mut Frame, area: Rect, model: &Model, wide: bool) {
    let visible = model.visible_devices();
    let border = border_of(model, Holder::Devices);
    let block = Block::bordered()
        .border_type(border)
        .title_top(format!(" devices {} ", visible.len()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [list, separator, alerts] = list_and_box(inner);
    let first = match model.device_note() {
        Some(note) => {
            dim_line(frame, list, note);
            None
        }
        None => Some(device_rows(frame, list, model, &visible, wide)),
    };
    let count = alerts_cell(model, &model.alerts().len().to_string());
    let row = Rect::new(area.x, separator.y, area.width, 1);
    let room = table_room(list);
    let window = first.map(|first| (first, room, visible.len()));
    alerts_separator(frame, row, &count, window, border);
    alert_rows(frame, alerts, model, wide);
}

fn list_and_box(inner: Rect) -> [Rect; 3] {
    let box_height = (inner.height / 4).max(ALERTS_LEAST);
    Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(box_height),
    ])
    .areas(inner)
}

fn alerts_answer(model: &Model) -> bool {
    has_answer(model.reach(Service::Alerts), model.alerts_ok())
}

fn has_answer(reach: Reach, answered: bool) -> bool {
    reach == Reach::Answers && answered
}

fn alerts_cell(model: &Model, shown: &str) -> String {
    answer_cell(model.reach(Service::Alerts), model.alerts_ok(), shown)
}

fn answer_cell(reach: Reach, answered: bool, shown: &str) -> String {
    if has_answer(reach, answered) {
        shown.to_string()
    } else {
        cell(reach, "?").to_string()
    }
}

fn cell(reach: Reach, shown: &str) -> &str {
    match reach {
        Reach::Answers => shown,
        Reach::NoAnswer => "?",
        Reach::Unknown | Reach::NotInLab | Reach::StackDown => "—",
    }
}

fn dim_line(frame: &mut Frame, area: Rect, text: String) {
    dim_lines(frame, area, vec![text]);
}

fn alerts_separator(
    frame: &mut Frame,
    row: Rect,
    count: &str,
    window: Option<(usize, usize, usize)>,
    border: BorderType,
) {
    separator_line(frame, row, &format!("alerts {count}"), window, border);
}

fn separator_line(
    frame: &mut Frame,
    row: Rect,
    title: &str,
    window: Option<(usize, usize, usize)>,
    border: BorderType,
) {
    let set = line_set(border);
    let head = format!("{} {title} ", set.vertical_right);
    let fill = usize::from(row.width).saturating_sub(head.chars().count() + 1);
    let line = format!("{head}{}{}", set.horizontal.repeat(fill), set.vertical_left);
    frame.render_widget(Paragraph::new(line), row);
    if let Some((first, room, total)) = window {
        let title = u16::try_from(head.chars().count() - 1).unwrap_or(u16::MAX);
        let free = Rect {
            x: row.x + title,
            width: row.width.saturating_sub(title),
            ..row
        };
        rows_mark(frame, free, first, room, total);
    }
}

fn device_rows(
    frame: &mut Frame,
    area: Rect,
    model: &Model,
    visible: &[usize],
    wide: bool,
) -> usize {
    let rows = model.device_rows();
    let metrics = model.reach(Service::Metrics);
    let alerts = model.reach(Service::Alerts);
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
    let start = first_row(at, table_room(area));
    let mut lines = vec![Line::from(header)];
    for (row, index) in rows.iter().zip(visible).skip(start) {
        let selected = cursor == Cursor::Device(*index);
        let open = model
            .session_of(&row.name)
            .is_some_and(|session| session.state.marked());
        let mark = if open {
            Span::raw("t")
        } else if row.maintenance {
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
        let health = cell(metrics, row.health.text());
        spans.push(Span::styled(format!("  {health:<4}"), tone(health)));
        if wide {
            let (text, style) = sync_cell(model.agreement(&row.name));
            spans.push(Span::styled(format!("  {text:<4}"), style));
        }
        let (count, style) = match (alerts, row.alerts) {
            (Reach::Answers, Some((0, _))) => (String::new(), Style::new()),
            (Reach::Answers, Some((count, true))) => {
                (count.to_string(), Style::new().fg(Color::Red))
            }
            (Reach::Answers, Some((count, false))) => (count.to_string(), Style::new()),
            (reach, _) => (cell(reach, "?").to_string(), DIM),
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
    start
}

fn alert_rows(frame: &mut Frame, area: Rect, model: &Model, wide: bool) {
    let reach = model.reach(Service::Alerts);
    if let Some(sentence) = absent(reach, row(Service::Alerts).word) {
        dim_line(frame, area, sentence);
        return;
    }
    let shown = model.shown_alerts();
    let room = usize::from(area.height);
    let cursor = model.cursor();
    let start = match cursor {
        Cursor::Alert(at) => first_row(at, room),
        Cursor::Device(_) => 0,
    };
    let wall = wall_seconds(model);
    let lines: Vec<Line> = shown
        .iter()
        .enumerate()
        .skip(start)
        .take(room)
        .map(|(index, alert)| {
            let (name, device) = (label(alert, "alertname"), label(alert, "device"));
            let base = alert_style(alert);
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
    rows_mark(frame, border_below(area), start, room, shown.len());
}

fn label<'a>(alert: &'a Alert, name: &str) -> &'a str {
    alert.labels.get(name).map_or("—", String::as_str)
}

fn alert_style(alert: &Alert) -> Style {
    if alert.silenced || alert.inhibited {
        DIM
    } else {
        Style::new()
    }
}

fn observability(frame: &mut Frame, area: Rect, model: &Model) {
    let visible = model.visible_alerts();
    let border = border_of(model, Holder::Alerts);
    let mut views = tab_line(
        &OBSERVABILITY_TABS,
        model.tab(),
        area.width >= WIDE,
        usize::from(area.width),
    )
    .spans;
    let count = format!(" {}", alerts_cell(model, &visible.len().to_string()));
    views.insert(1, Span::raw(count));
    let top: Vec<Span> = [Span::raw(" ")]
        .into_iter()
        .chain(views)
        .chain([Span::raw(" ")])
        .collect();
    let bottom = format!(
        " metrics {}  logs {}  alerts {} ",
        answer_cell(model.reach(Service::Metrics), model.metrics_ok(), "ok"),
        cell(model.reach(Service::Logs), "ok"),
        alerts_cell(model, "ok")
    );
    let block = Block::bordered()
        .border_type(border)
        .title_top(Line::from(top))
        .title_bottom(bottom);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    match model.tab() {
        Tab::Logs => logs_view(frame, inner, model),
        Tab::Metrics => metrics_view(frame, inner, model),
        _ => alerts_view(frame, area, inner, model, border, &visible),
    }
}

fn metrics_view(frame: &mut Frame, area: Rect, model: &Model) {
    let reach = model.reach(Service::Metrics);
    if let Some(sentence) = absent(reach, row(Service::Metrics).word) {
        dim_lines(frame, area, absent_lines(reach, sentence, Service::Metrics));
        return;
    }
    if let Some(note) = model.device_note().filter(|_| !model.metrics_listed()) {
        dim_line(frame, area, note);
        return;
    }
    let rows = model.metrics_rows();
    let cells: Vec<[&str; 3]> = rows
        .iter()
        .map(|row| {
            [
                row.name.as_str(),
                cell(reach, &row.interfaces),
                cell(reach, &row.sessions),
            ]
        })
        .collect();
    let titles = ["DEVICE", "INTERFACES", "BGP SESSIONS"];
    let mut widths = widest(titles, &cells);
    widths[1] = widths[1].max(COUNT_WIDTH);
    let body_rows = rows.iter().zip(&cells).map(|(row, cells)| {
        let style = if row.no_data {
            DIM
        } else if row.short {
            Style::new().fg(Color::Yellow)
        } else {
            Style::new()
        };
        Row::new(*cells).style(style)
    });
    let at = model.marked_metrics_row();
    marked_table(frame, area, Row::new(titles), &widths, body_rows, at);
}

fn logs_view(frame: &mut Frame, area: Rect, model: &Model) {
    let reach = model.reach(Service::Logs);
    if let Some(sentence) = absent(reach, row(Service::Logs).word) {
        dim_lines(frame, area, absent_lines(reach, sentence, Service::Logs));
        return;
    }
    if !model.lab_lines_shown() {
        return;
    }
    let (lines, following, marked, held) = model.lab_logs();
    let [header, rows_area] = header_and_rows(area);
    let count = match held {
        Some(held) => format!("{} of {}", lines.len(), counted(held, "line")),
        None => counted(lines.len(), "line"),
    };
    let width = usize::from(area.width);
    frame.render_widget(
        Paragraph::new(fit(&format!(" all devices   {count}"), width)),
        header,
    );
    let mode = if following { "following" } else { "paused" };
    right(frame, header, Line::from(mode), 1);
    let pad = lines
        .iter()
        .map(|(_, device, _)| device.chars().count())
        .max()
        .unwrap_or(0);
    let rows = lines
        .iter()
        .enumerate()
        .map(|(at, (nanoseconds, device, text))| {
            let gutter = if marked == Some(at) { "> " } else { "  " };
            let text = log_row(
                *nanoseconds,
                Some((device, pad)),
                text,
                width.saturating_sub(3),
            );
            Line::from(format!("{gutter}{text}"))
        });
    let start = first_row(marked.unwrap_or(0), usize::from(rows_area.height));
    pane_rows(frame, rows_area, rows, start, lines.len());
}

fn alerts_view(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    model: &Model,
    border: BorderType,
    visible: &[usize],
) {
    let reach = model.reach(Service::Alerts);
    if let Some(sentence) = absent(reach, row(Service::Alerts).word) {
        dim_lines(frame, inner, absent_lines(reach, sentence, Service::Alerts));
        return;
    }
    if model.shown_alerts().is_empty() {
        if alerts_answer(model) {
            dim_line(frame, inner, "none".to_string());
        }
        return;
    }
    let Some(selected) = model.selected_alert() else {
        alert_table(frame, inner, model, visible);
        return;
    };
    let [table, separator, lower] = list_and_box(inner);
    let first = alert_table(frame, table, model, visible);
    let room = table_room(table);
    let title = cut_whole(
        &[label(selected, "alertname"), label(selected, "device")],
        "  ",
        usize::from(area.width).saturating_sub(4),
        Keep::Start,
    );
    let row = Rect::new(area.x, separator.y, area.width, 1);
    separator_line(
        frame,
        row,
        &title,
        Some((first, room, visible.len())),
        border,
    );
    alert_box(frame, lower, selected);
}

fn absent_lines(reach: Reach, sentence: String, service: Service) -> Vec<String> {
    let service = row(service);
    let mut lines = vec![sentence];
    if reach == Reach::NotInLab {
        lines.extend([
            String::new(),
            "to add them, write the address in the lab's .env:".to_string(),
            format!(
                "  {}={}",
                service.setting.unwrap_or_default(),
                service.default
            ),
        ]);
    }
    lines
}

fn dim_lines(frame: &mut Frame, area: Rect, texts: Vec<String>) {
    let width = usize::from(area.width);
    let lines: Vec<Line> = texts
        .into_iter()
        .map(|text| fitted(vec![(text, DIM)], width))
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn alert_table(frame: &mut Frame, area: Rect, model: &Model, visible: &[usize]) -> usize {
    let alerts = model.shown_alerts();
    let wall = wall_seconds(model);
    let rows: Vec<[String; 5]> = visible
        .iter()
        .map(|&index| {
            let alert = &alerts[index];
            [
                label(alert, "alertname").to_string(),
                label(alert, "device").to_string(),
                label(alert, "severity").to_string(),
                alert.state_word(),
                time::age(&alert.starts_at, wall),
            ]
        })
        .collect();
    let titles = ["NAME", "DEVICE", "SEVERITY", "STATE", "SINCE"];
    let mut widths = widest(titles, &rows);
    widths[3] = widths[3].max(STATE_WIDTH);
    let fixed = 2 + widths[1..].iter().map(|width| width + 2).sum::<usize>();
    widths[0] = widths[0].min(usize::from(area.width).saturating_sub(fixed + 1));
    let at = model.selected_alert_row();
    let body_rows = visible.iter().zip(&rows).map(|(&index, row)| {
        Row::new([
            Cell::from(fit(&row[0], widths[0])),
            Cell::from(row[1].as_str()),
            Cell::from(row[2].as_str()),
            Cell::from(row[3].as_str()).style(tone(&row[3])),
            Cell::from(row[4].as_str()),
        ])
        .style(alert_style(&alerts[index]))
    });
    marked_table(frame, area, Row::new(titles), &widths, body_rows, at)
}

fn marked_table<'a>(
    frame: &mut Frame,
    area: Rect,
    header: Row<'a>,
    widths: &[usize],
    body_rows: impl IntoIterator<Item = Row<'a>>,
    at: Option<usize>,
) -> usize {
    let constraints = widths
        .iter()
        .map(|width| Constraint::Length(u16::try_from(*width).unwrap_or(u16::MAX)));
    let table = Table::new(body_rows, constraints)
        .header(header)
        .column_spacing(2)
        .highlight_spacing(HighlightSpacing::Always)
        .highlight_symbol("> ")
        .row_highlight_style(REVERSE);
    let first = first_row(at.unwrap_or(0), table_room(area));
    let mut state = TableState::default().with_selected(at).with_offset(first);
    frame.render_stateful_widget(table, area, &mut state);
    first
}

fn alert_box(frame: &mut Frame, area: Rect, alert: &Alert) {
    let width = usize::from(area.width);
    let text = alert
        .annotations
        .get("summary")
        .or_else(|| alert.annotations.get("description"))
        .cloned();
    let started = format!(
        "started {}   severity {}",
        time::clock(&alert.starts_at),
        label(alert, "severity")
    );
    let lines: Vec<Line> = text
        .into_iter()
        .chain([started])
        .map(|line| fitted(vec![(line, Style::new())], width))
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn device_detail(frame: &mut Frame, area: Rect, model: &Model, wide: bool) {
    let title = model.selected_device().map_or_else(
        || " devices ".to_string(),
        |device| format!(" {} ", device.name),
    );
    frame.render_widget(
        Block::bordered()
            .border_type(border_of(model, Holder::Pane))
            .title_top(title),
        area,
    );
    let [tab_row, content] = tab_content(area);
    tabs(frame, tab_row, model, wide);
    if let Some(note) = model.device_note() {
        dim_line(frame, content, note);
        return;
    }
    match model.tab() {
        Tab::Overview => device_overview(frame, content, model),
        Tab::Timeline => timeline(frame, content, model),
        Tab::Logs => device_logs(frame, content, model),
        Tab::Interfaces => device_interfaces(frame, content, model, wide),
        Tab::Config => config_pane(frame, content, model),
        Tab::Terminal => terminal_pane(frame, content, model),
        Tab::Sonda | Tab::Alerts | Tab::Metrics => {}
    }
}

fn terminal_pane(frame: &mut Frame, area: Rect, model: &Model) {
    let Some(device) = model.selected_device() else {
        return;
    };
    let Some(session) = model.drawn_session() else {
        let hint = format!("{TERMINAL_HINT} {}", device.name);
        let line = fitted(vec![(hint, DIM)], usize::from(area.width));
        frame.render_widget(Paragraph::new(line), area);
        return;
    };
    let screen = session.screen.screen();
    let hidden = TermCursor::default().visibility(false);
    frame.render_widget(PseudoTerminal::new(screen).cursor(hidden), area);
    let (row, col) = screen.cursor_position();
    let at = Position::new(area.x.saturating_add(col), area.y.saturating_add(row));
    if session.state.typed() && !screen.hide_cursor() && area.contains(at) {
        frame.set_cursor_position(at);
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
    let metrics = model.reach(Service::Metrics);
    let cells: Vec<[&str; 6]> = interfaces
        .iter()
        .map(|row| {
            [
                row.name.as_str(),
                row.intent.as_str(),
                cell(metrics, &row.admin),
                cell(metrics, &row.oper),
                row.peer.as_str(),
                if row.peer.is_empty() {
                    ""
                } else {
                    cell(metrics, &row.bgp)
                },
            ]
        })
        .collect();
    let widths = widest(titles, &cells);
    let width = usize::from(area.width);
    let line = |mark: &str, row: &[&str; 6], yellow: bool, states: bool| {
        let parts = std::iter::once((mark.to_string(), Style::new()))
            .chain(columns.iter().map(|&column| {
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
            }))
            .collect();
        fitted(parts, width)
    };
    let [table, footer] = table_and_footer(area);
    let room = table_room(table);
    let on_tabs = model.focus() == Focus::Tabs;
    let selected = model.interface_row();
    let first = first_row(selected, room);
    let mut lines = vec![line(" ", &titles, false, false)];
    lines.extend(cells.iter().zip(&interfaces).enumerate().skip(first).map(
        |(index, (row, source))| {
            let here = on_tabs && index == selected;
            let line = line(marker(here), row, source.differs, true);
            if here {
                highlighted(line, table.width)
            } else {
                line
            }
        },
    ));
    frame.render_widget(Paragraph::new(lines), table);
    rows_mark(frame, border_below(area), first, room, interfaces.len());
    let differs = interfaces.iter().filter(|row| row.differs).count();
    let count = counted(interfaces.len(), "interface");
    let differs = format!("{differs} differs");
    frame.render_widget(
        Paragraph::new(cut_whole(&[&count, &differs], "  ", width, Keep::Start)),
        footer,
    );
}

fn table_and_footer(area: Rect) -> [Rect; 2] {
    Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area)
}

pub(crate) fn overview_overflow(model: &Model, area: Rect) -> usize {
    let (_, [_, _, _, body, _, _]) = regions(model, area);
    let content = device_content(body);
    overview_lines(model, usize::from(content.width))
        .len()
        .saturating_sub(usize::from(content.height))
}

fn device_overview(frame: &mut Frame, area: Rect, model: &Model) {
    let lines = overview_lines(model, usize::from(area.width));
    let (room, total) = (usize::from(area.height), lines.len());
    let first = model.overview_scroll().min(total.saturating_sub(room));
    let shown: Vec<Line> = lines.into_iter().skip(first).collect();
    frame.render_widget(Paragraph::new(shown), area);
    rows_mark(frame, border_below(area), first, room, total);
}

fn overview_lines(model: &Model, width: usize) -> Vec<Line<'static>> {
    let Some(device) = model.selected_device() else {
        return Vec::new();
    };
    let plain = Style::new();
    let facts = |pieces: &[String], gap: &str| {
        let pieces: Vec<&str> = pieces.iter().map(String::as_str).collect();
        Line::from(cut_whole(&pieces, gap, width, Keep::Start))
    };
    let metrics = model.reach(Service::Metrics);
    let states: Vec<_> = model
        .session_states()
        .into_iter()
        .map(|(session, state)| {
            let shown = cell(metrics, &state).to_string();
            (session, shown)
        })
        .collect();
    let (established, intended) = devices::sessions_up(&states);
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
                format!("{intended} intended"),
                format!("{established} established"),
                format!("{} other", intended - established),
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
    lines.extend(
        absent(metrics, row(Service::Metrics).word)
            .map(|sentence| fitted(vec![(sentence, DIM)], width)),
    );
    let latest = model.alerts().iter().find(|alert| {
        alert.labels.get("device") == Some(&device.name) && alert.state_word() == "firing"
    });
    let label = |name: &str| format!("alerts   {name}");
    let alerts = model.reach(Service::Alerts);
    lines.push(match latest {
        _ if !alerts_answer(model) => facts(&[label(cell(alerts, "?"))], ""),
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
    lines
}

fn kind_style(kind: devices::Kind) -> Style {
    match kind {
        devices::Kind::Alert => Style::new().fg(Color::Red),
        devices::Kind::Flow => Style::new().fg(Color::Blue),
        devices::Kind::Rca => Style::new().fg(Color::Magenta),
        devices::Kind::Log => Style::new(),
    }
}

fn timeline_layout(model: &Model, area: Rect) -> (Vec<String>, [Rect; 3]) {
    let mut notes: Vec<String> = Vec::new();
    for sentence in [Service::Alerts, Service::Logs, Service::Flows]
        .into_iter()
        .filter_map(|service| model.absent_sentence(service))
    {
        if !notes.contains(&sentence) {
            notes.push(sentence);
        }
    }
    let parts = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(u16::try_from(notes.len()).unwrap_or(u16::MAX)),
    ])
    .areas(area);
    (notes, parts)
}

fn timeline(frame: &mut Frame, area: Rect, model: &Model) {
    let entries = model.timeline();
    let (notes, [counts, rows, absent_rows]) = timeline_layout(model, area);
    let width = usize::from(absent_rows.width);
    let notes: Vec<Line> = notes
        .into_iter()
        .map(|sentence| fitted(vec![(sentence, DIM)], width))
        .collect();
    frame.render_widget(Paragraph::new(notes), absent_rows);
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
        Keep::Start,
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
    rows_mark(
        frame,
        border_below(rows),
        start,
        usize::from(rows.height),
        entries.len(),
    );
    if let Some(entry) = model.popup_entry() {
        timeline_popup(
            frame,
            area,
            entry,
            model.popup_scroll(),
            border_of(model, Holder::Entry),
        );
    }
}

fn popup_area(area: Rect) -> Rect {
    Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    )
}

fn popup_lines(entry: &devices::Entry, width: u16) -> Vec<String> {
    entry
        .text
        .lines()
        .flat_map(|line| wrapped(line, usize::from(width)))
        .collect()
}

pub(crate) fn popup_overflow(model: &Model, area: Rect) -> usize {
    let Some(entry) = model.popup_entry() else {
        return 0;
    };
    let (_, [_, _, _, body, _, _]) = regions(model, area);
    let inner = Block::bordered().inner(popup_area(device_content(body)));
    popup_lines(entry, inner.width)
        .len()
        .saturating_sub(usize::from(inner.height))
}

fn timeline_popup(
    frame: &mut Frame,
    area: Rect,
    entry: &devices::Entry,
    scroll: usize,
    border: BorderType,
) {
    let popup = popup_area(area);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(border)
        .title_top(format!(
            " {}  {} ",
            entry.kind.name(),
            devices::clock_of(entry.nanoseconds)
        ))
        .title_top(Line::from(" Esc close ").right_aligned());
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let lines = popup_lines(entry, inner.width);
    let (room, total) = (usize::from(inner.height), lines.len());
    let first = scroll.min(total.saturating_sub(room));
    let shown: Vec<Line> = lines.into_iter().skip(first).map(Line::from).collect();
    frame.render_widget(Paragraph::new(shown), inner);
    rows_mark(frame, border_below(inner), first, room, total);
}

fn device_logs(frame: &mut Frame, area: Rect, model: &Model) {
    let reach = model.reach(Service::Logs);
    if let Some(sentence) = absent(reach, row(Service::Logs).word) {
        dim_line(frame, area, sentence);
        return;
    }
    if reach != Reach::Answers {
        return;
    }
    let Some((_, lines, following, scroll)) = model.device_logs() else {
        return;
    };
    let start = if following { 0 } else { scroll };
    let rows = lines.iter().map(|(nanoseconds, text)| {
        Line::from(log_row(*nanoseconds, None, text, usize::from(area.width)))
    });
    pane_rows(frame, area, rows, start, lines.len());
}

fn log_row(nanoseconds: i64, device: Option<(&str, usize)>, text: &str, width: usize) -> String {
    let device = device
        .map(|(name, pad)| format!("{name:<pad$}  "))
        .unwrap_or_default();
    fit(
        &format!("{}  {device}{text}", devices::clock_of(nanoseconds)),
        width,
    )
}

fn sync_cell(agreement: Agreement) -> (String, Style) {
    match agreement {
        Agreement::Unknown => ("—".to_string(), DIM),
        Agreement::Failed => ("!".to_string(), Style::new().fg(Color::Red)),
        Agreement::InSync => ("=".to_string(), Style::new()),
        Agreement::Missing(count) if count >= 1000 => ("999+".to_string(), Style::new()),
        Agreement::Missing(count) => (format!("+{count}"), Style::new()),
    }
}

fn selected_fetch_error(model: &Model) -> Option<&str> {
    model
        .config()
        .and_then(|(device, ..)| model.fetch_error(device))
}

fn fetch_failed(model: &Model, width: usize) -> Option<Line<'static>> {
    let words = selected_fetch_error(model)?;
    Some(Line::styled(
        fit(&format!("fetch failed: {words}"), width),
        Style::new().fg(Color::Red),
    ))
}

fn config_pane(frame: &mut Frame, area: Rect, model: &Model) {
    let [note, header, rows_area] = config_layout(model, area);
    let width = usize::from(area.width);
    let failed = fetch_failed(model, width);
    if let Some(view) = model.intended_view() {
        intended_pane(frame, [header, rows_area], &view, failed);
        return;
    }
    if let Some(words) = model.no_template() {
        frame.render_widget(Paragraph::new(fit(words, width)), note);
    }
    let Some((_, kept, lines, scroll)) = model.config() else {
        return;
    };
    let fetched = kept.map(|kept| {
        let said = format!(
            "fetched {}  {}",
            kept.fetched,
            counted(kept.lines.len(), "line")
        );
        Line::from(fit(&said, width))
    });
    let hint = || fitted(vec![(CONFIG_HINT.to_string(), DIM)], width);
    let first = fetched.or_else(|| failed.is_none().then(hint));
    let header_lines: Vec<Line> = first.into_iter().chain(failed).collect();
    frame.render_widget(Paragraph::new(header_lines), header);
    if kept.is_none() {
        return;
    }
    let rows = lines.iter().map(|line| Line::from(fit(line, width)));
    pane_rows(frame, rows_area, rows, scroll, lines.len());
}

fn config_layout(model: &Model, area: Rect) -> [Rect; 3] {
    let kept = model.config().is_some_and(|(_, kept, ..)| kept.is_some());
    let below = u16::from(kept && selected_fetch_error(model).is_some());
    let header = below
        + if model.intended_view().is_some() {
            2
        } else {
            1
        };
    Layout::vertical([
        Constraint::Length(u16::from(model.no_template().is_some())),
        Constraint::Length(header),
        Constraint::Fill(1),
    ])
    .areas(area)
}

fn intended_pane(
    frame: &mut Frame,
    [header, rows]: [Rect; 2],
    view: &IntendedView,
    failed: Option<Line<'static>>,
) {
    let width = usize::from(header.width);
    if let Some(error) = &view.error {
        let words = error.lines().flat_map(|line| wrapped(line, width));
        let lines: Vec<Line> = std::iter::once(Line::from(INTENDED_ERROR))
            .chain(failed)
            .chain(words.map(Line::from))
            .collect();
        frame.render_widget(Paragraph::new(lines), header.union(rows));
        return;
    }
    let first = format!("intended {}  {}", view.since, counted(view.lines, "line"));
    let fetched = view
        .fetched
        .zip(view.missing)
        .map(|(fetched, missing)| match missing {
            0 => Line::from(fit(&format!("fetched {fetched}  none missing"), width)),
            missing => Line::from(fit(&format!("fetched {fetched}  {missing} missing"), width)),
        });
    let hint = || fitted(vec![(INTENDED_HINT.to_string(), DIM)], width);
    let second = fetched.or_else(|| failed.is_none().then(hint));
    let lines: Vec<Line> = [Line::from(fit(&first, width))]
        .into_iter()
        .chain(second)
        .chain(failed)
        .collect();
    frame.render_widget(Paragraph::new(lines), header);
    let shown = view.rows.iter().map(|(mark, line)| {
        let style = match mark {
            '+' => Style::new().fg(Color::Green),
            '-' => Style::new().fg(Color::Red),
            _ => Style::new(),
        };
        Line::styled(
            format!("{mark}{}", cut_middle(line, width.saturating_sub(1))),
            style,
        )
    });
    pane_rows(frame, rows, shown, view.scroll, view.rows.len());
}

fn cut_middle(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return text.to_string();
    }
    let kept = width.saturating_sub(1);
    let head = kept / 2;
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(count - (kept - head)).collect();
    format!("{start}…{end}")
}

fn header_and_rows(area: Rect) -> [Rect; 2] {
    Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area)
}

fn pane_rows<'a>(
    frame: &mut Frame,
    area: Rect,
    rows: impl Iterator<Item = Line<'a>>,
    start: usize,
    total: usize,
) {
    let shown: Vec<Line> = rows.skip(start).take(usize::from(area.height)).collect();
    rows_mark(frame, border_below(area), start, shown.len(), total);
    frame.render_widget(Paragraph::new(shown), area);
}

fn chooser(frame: &mut Frame, area: Rect, model: &Model) {
    let block = Block::bordered()
        .border_type(border_of(model, Holder::Chooser))
        .title_top(" labs found ");
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
    let state_width = state_width(
        model,
        (0..stacks.len()).filter(|index| stacks[*index].kind != StackKind::Sonda),
    );
    let path_room = usize::from(inner.width)
        .saturating_sub(1 + name_width + 2 + kind_width + 2 + 2 + state_width);
    let path_width = workshops()
        .map(|stack| stack.path.display().to_string().chars().count())
        .max()
        .unwrap_or(0)
        .min(path_room);
    let room = usize::from(inner.height);
    let first = first_row(model.selected_stack(), room);
    let lines: Vec<Line> = stacks
        .iter()
        .zip(&paths)
        .enumerate()
        .skip(first)
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
    rows_mark(frame, border_below(inner), first, room, stacks.len());
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
        RunState::NotRun => ("not run".to_string(), Style::new().fg(Color::Yellow)),
    }
}

fn run_title(model: &Model, before_hint: usize) -> Vec<Span<'static>> {
    let Some(run) = model.run() else {
        return vec![Span::raw(" run  none")];
    };
    let (end, style) = run_end(run);
    let state = match (run.state, run.ended) {
        (RunState::Stopping | RunState::NotRun, _) => end,
        (_, Some(ended)) => {
            let took = ended.saturating_duration_since(run.started).as_secs();
            format!("{end}  {took}s")
        }
        (_, None) => format!("{end} {}s", seconds(model, run.started)),
    };
    let room = before_hint.saturating_sub(" run  ".len() + "  ".len() + state.chars().count() + 1);
    let words = cut_words(&run.words, room, Keep::Start);
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
        .border_type(border_of(model, Holder::Run))
        .title_top(Line::from(title))
        .title_top(hint);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(run) = model.run() else {
        return;
    };
    if model.focus() == Focus::Run {
        let rows = run_all_rows(run, usize::from(inner.width));
        let total = rows.len();
        pane_rows(frame, inner, rows.into_iter(), model.run_scroll(), total);
        return;
    }
    let lines = run_rows(run, usize::from(inner.width), usize::from(inner.height));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn run_all_rows(run: &Run, width: usize) -> Vec<Line<'static>> {
    let output = run.lines.iter().flat_map(|line| line_rows(line, width));
    command_lines(run, width)
        .into_iter()
        .chain(output)
        .collect()
}

fn command_lines(run: &Run, width: usize) -> Vec<Line<'static>> {
    command_rows(run, width)
        .into_iter()
        .map(|row| Line::styled(row, DIM))
        .collect()
}

fn line_rows(line: &str, width: usize) -> impl DoubleEndedIterator<Item = Line<'static>> {
    let style = line_style(line);
    wrapped(line, width)
        .into_iter()
        .map(move |row| Line::styled(row, style))
}

fn line_style(line: &str) -> Style {
    if line.starts_with("nobs:") {
        Style::new().fg(Color::Red)
    } else {
        Style::new()
    }
}

fn run_rows(run: &Run, width: usize, height: usize) -> Vec<Line<'static>> {
    let command = command_lines(run, width);
    let last = run
        .lines
        .last()
        .map_or(0, |line| wrapped(line, width).len());
    let mut lines = if command.len() + last <= height {
        command
    } else {
        Vec::new()
    };
    let room = height.saturating_sub(lines.len());
    let mut tail: Vec<Line> = Vec::new();
    for line in run.lines.iter().rev() {
        if tail.len() >= room {
            break;
        }
        tail.extend(line_rows(line, width).rev());
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
    let [left, right] = device_panes(area);
    let chapters = model.chapters();
    let [top, bottom] = lab_panes(model, left);
    node_list(frame, top, model, wide);
    if model.has_chapters_box() {
        let selected = model.selected_chapter();
        let block = Block::bordered()
            .border_type(border_of(model, Holder::Chapters))
            .title_top(format!(" chapters {} ", chapters.len()));
        stack_list(
            frame,
            bottom,
            model,
            block,
            &chapters,
            selected,
            Target::Chapter,
        );
    }
    lab_overview(frame, right, model);
}

fn lab_panes(model: &Model, left: Rect) -> [Rect; 2] {
    let chapters_height = if model.has_chapters_box() {
        u16::try_from(model.chapters().len() + 2).unwrap_or(u16::MAX)
    } else {
        0
    };
    Layout::vertical([
        Constraint::Min(NODES_LEAST),
        Constraint::Length(chapters_height),
    ])
    .areas(left)
}

fn node_list(frame: &mut Frame, area: Rect, model: &Model, wide: bool) {
    let rows = model.nodes();
    let block = Block::bordered()
        .border_type(border_of(model, Holder::Nodes))
        .title_top(format!(" nodes {} ", rows.len()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = |texts: Vec<&str>| {
        texts
            .iter()
            .map(|text| text.chars().count())
            .max()
            .unwrap_or(0)
            .max(4)
    };
    let states: Vec<(&str, Style)> = rows
        .iter()
        .map(|row| match row.state.as_deref() {
            _ if !model.nodes_ok() => ("?", tone("?")),
            None => ("—", tone("—")),
            Some("running") => ("running", tone("running")),
            Some(other) => (other, Style::new().fg(Color::Red)),
        })
        .collect();
    let kind_width = width(rows.iter().map(|row| row.kind.as_str()).collect());
    let state_width = width(
        states
            .iter()
            .map(|state| state.0)
            .chain(["STATE"])
            .collect(),
    );
    let fixed = 3 + if wide { kind_width + 2 } else { 0 } + state_width;
    let name_width = width(rows.iter().map(|row| row.name.as_str()).collect())
        .min(usize::from(inner.width).saturating_sub(fixed));
    let line = |mark: &str, name: &str, kind: &str, state: (&str, Style)| {
        let mut parts = vec![(format!("{mark}{}  ", fit(name, name_width)), Style::new())];
        if wide {
            parts.push((format!("{}  ", fit(kind, kind_width)), Style::new()));
        }
        parts.push((state.0.to_string(), state.1));
        fitted(parts, usize::from(inner.width))
    };
    let room = table_room(inner);
    let on_tabs = model.focus() == Focus::Tabs;
    let selected = model.node_row();
    let first = first_row(selected, room);
    let mut lines = vec![line(" ", "NAME", "KIND", ("STATE", Style::new()))];
    for (index, (row, state)) in rows.iter().zip(states).enumerate().skip(first) {
        let here = on_tabs && index == selected;
        let line = line(marker(here), &row.name, &row.kind, state);
        lines.push(if here {
            highlighted(line, inner.width)
        } else {
            line
        });
    }
    frame.render_widget(Paragraph::new(lines), inner);
    rows_mark(frame, border_below(inner), first, room, rows.len());
}

fn lab_overview(frame: &mut Frame, area: Rect, model: &Model) {
    let block = Block::bordered().border_type(border_of(model, Holder::Overview));
    let Some(title) = overview_title(model) else {
        frame.render_widget(block, area);
        return;
    };
    frame.render_widget(block.title_top(format!(" {title} ")), area);
    let [tab_row, content] = tab_content(area);
    frame.render_widget(Paragraph::new(Span::styled("OVERVIEW", REVERSE)), tab_row);
    let lines = lab_lines(model, usize::from(content.width));
    let (room, total) = (usize::from(content.height), lines.len());
    let first = model.lab_scroll().min(total.saturating_sub(room));
    let shown: Vec<Line> = lines.into_iter().skip(first).collect();
    frame.render_widget(Paragraph::new(shown), content);
    rows_mark(frame, border_below(content), first, room, total);
}

fn overview_title(model: &Model) -> Option<String> {
    match model.selected_chapter() {
        Some(chapter) => Some(model.stacks()[chapter].name.clone()),
        None if model.has_chapters_box() => None,
        None => model.lab_name(),
    }
}

pub(crate) fn lab_overflow(model: &Model, area: Rect) -> usize {
    if overview_title(model).is_none() {
        return 0;
    }
    let (_, [_, _, _, body, _, _]) = regions(model, area);
    let content = device_content(body);
    lab_lines(model, usize::from(content.width))
        .len()
        .saturating_sub(usize::from(content.height))
}

fn files_text(files: Option<&Rendered>) -> String {
    match files {
        None => "?".to_string(),
        Some(Rendered::NotRendered) => "not rendered".to_string(),
        Some(Rendered::AsRendered) => "as rendered".to_string(),
        Some(Rendered::Differ { count, of }) => format!("{count} of {of} differ"),
        Some(Rendered::Error(text)) => format!("error: {text}"),
    }
}

fn lab_lines(model: &Model, width: usize) -> Vec<Line<'static>> {
    let chapter = model.selected_chapter();
    let lab = model.located();
    let topology = lab
        .zip(model.topology())
        .map_or("—".to_string(), |(lab, path)| {
            path.strip_prefix(&lab.dir)
                .unwrap_or(path)
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
    let services = chapter.map(|chapter| match model.stack_state(chapter) {
        StackState::Up { running, total } => format!("{running} of {total} running"),
        StackState::Down { total } => format!("0 of {total} running"),
        StackState::Unknown | StackState::Failed => "?".to_string(),
    });
    let files = (model.nodes_from() == Some(Nodes::Project)).then(|| files_text(model.files()));
    let row = |label: &str, value: String| format!(" {label:<8}  {value}");
    let target = chapter.map_or(Target::Lab, Target::Chapter);
    let last = model.run().filter(|run| run.target == target);
    let (last, end) = match last {
        Some(run) => {
            let (end, style) = run_end(run);
            let used = row("last", String::new()).chars().count() + "  ".len();
            let room = width.saturating_sub(used + end.chars().count());
            (
                format!("{}  ", cut_words(&run.words, room, Keep::Start)),
                (end, style),
            )
        }
        None => ("—".to_string(), (String::new(), Style::new())),
    };
    let plain = |label: &str, value: String| fitted(vec![(row(label, value), Style::new())], width);
    let project = lab
        .and_then(Located::project)
        .map(|project| plain("project", project.to_string()));
    [plain("topology", topology)]
        .into_iter()
        .chain(project)
        .chain([plain("nodes", nodes)])
        .chain(services.map(|services| plain("services", services)))
        .chain(files.map(|files| plain("files", files)))
        .chain([fitted(vec![(row("last", last), Style::new()), end], width)])
        .collect()
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
    if let Some(entry) = model.login_entry() {
        let (label, text, hint) = match entry {
            LoginEntry::User { text, hint } => {
                ("user for the devices".to_string(), text.to_string(), hint)
            }
            LoginEntry::Password { user, dots, hint } => {
                (format!("password for {user}"), "•".repeat(dots), hint)
            }
        };
        return entry_line(&label, &text, [hint, "Esc cancel"], width);
    }
    if let Some((device, text)) = model.entry() {
        let label = format!("run on {device}");
        return entry_line(&label, text, ["Enter run", "Esc cancel"], width);
    }
    if let Some(text) = model.typed_filter() {
        return entry_line("filter", text, ["Enter keep", "Esc clear"], width);
    }
    if let Some(question) = model.question() {
        return question
            .text()
            .split('\n')
            .flat_map(|line| wrapped(&format!(" {line}"), width))
            .collect();
    }
    if let Some(notice) = model.notice() {
        return wrapped(&format!(" {notice}"), width);
    }
    vec![whole_hints(
        &key_hints(model, width >= usize::from(WIDE)),
        width,
    )]
}

fn key_hints(model: &Model, wide: bool) -> Vec<String> {
    let listed = |fixed: Vec<&'static str>| fixed.into_iter().map(ToString::to_string);
    if model.chooser() {
        return listed(vec!["j/k select", "Enter open", "q quit"]).collect();
    }
    let actions = action_hints(model);
    let fixed = match (model.workbench(), model.focus(), model.tab()) {
        (_, Focus::Run, _) => {
            let moved = model.run_scroll() < model.run_overflow();
            return listed(vec!["j/k scroll"])
                .chain(moved.then(|| "f follow".to_string()))
                .chain(listed(vec!["Tab focus", FOLD]))
                .collect();
        }
        (Workbench::Lab, ..) => {
            return listed(vec!["Tab focus", "j/k move"])
                .chain(actions)
                .chain(listed(vec!["r refresh", "o pane"]))
                .collect();
        }
        (Workbench::Observability, ..) => observability_hints(model),
        (Workbench::Services, Focus::List, _) => {
            vec!["Tab focus", "j/k move", "/ filter", "l logs", "r refresh"]
        }
        (
            Workbench::Services,
            Focus::Tabs,
            Tab::Overview
            | Tab::Interfaces
            | Tab::Config
            | Tab::Timeline
            | Tab::Alerts
            | Tab::Metrics
            | Tab::Terminal,
        ) => {
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
        (Workbench::Devices, Focus::Tabs, Tab::Timeline) if model.popup() => {
            vec!["j/k move", "Esc close"]
        }
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
        (Workbench::Devices, Focus::Tabs, Tab::Config) => {
            return listed(view_hints(model, wide))
                .chain(listed(PANE_HINTS.to_vec()))
                .chain(actions)
                .collect();
        }
        (Workbench::Devices, Focus::Tabs, Tab::Terminal) => {
            return match model.drawn_session() {
                Some(session) => listed(session.state.keys().to_vec()).collect(),
                None => vec![
                    format!("{} {}", SHELL.key, SHELL.hint),
                    PANE_HINTS[2].to_string(),
                ],
            };
        }
        (Workbench::Services, Focus::Lines, _) | (Workbench::Devices, _, _) => {
            vec!["Tab focus", "j/k move", "[ ] tab", "Esc back"]
        }
    };
    listed(fixed).chain(actions).collect()
}

fn view_hints(model: &Model, wide: bool) -> Vec<&'static str> {
    let difference = if wide {
        DIFFERENCE_WIDE
    } else {
        DIFFERENCE_NARROW
    };
    match model.intended_view() {
        Some(view) => [VIEW_RUNNING]
            .into_iter()
            .chain(view.error.is_none().then_some(difference))
            .collect(),
        None if model.has_template() => vec![VIEW_INTENDED],
        None => Vec::new(),
    }
}

fn observability_hints(model: &Model) -> Vec<&'static str> {
    let offered = if model.tab() == Tab::Logs {
        lab_hints(model)
    } else {
        let (listed, opens) = if model.tab() == Tab::Metrics {
            (
                model.metrics_listed(),
                model.marked_metrics_device().is_some(),
            )
        } else {
            (
                !model.shown_alerts().is_empty(),
                model
                    .selected_alert()
                    .is_some_and(|alert| model.alert_device(alert).is_some()),
            )
        };
        vec![
            (listed, "j/k move"),
            (listed, "/ filter"),
            (true, "[ ] view"),
            (opens, "Enter open the device"),
            (true, "r refresh"),
        ]
    };
    [(model.run_drawn(), "Tab focus")]
        .into_iter()
        .chain(offered)
        .filter_map(|(offered, hint)| offered.then_some(hint))
        .collect()
}

fn lab_hints(model: &Model) -> Vec<(bool, &'static str)> {
    if !model.lab_can_pause() {
        return vec![(true, "[ ] view"), (true, "r refresh")];
    }
    if model.lab_logs().1 {
        return vec![
            (true, "f pause"),
            (model.lab_can_filter(), "/ filter"),
            (true, "[ ] view"),
            (true, "r refresh"),
        ];
    }
    vec![
        (true, "f follow"),
        (model.lab_can_move(), "j/k move"),
        (model.lab_can_filter(), "/ filter"),
        (model.lab_enter_device().is_some(), "Enter open the device"),
    ]
}

fn log_hints(paused: bool) -> Vec<&'static str> {
    let follow = if paused { "f follow" } else { "f pause" };
    [follow].into_iter().chain(PANE_HINTS).collect()
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

fn entry_line(label: &str, text: &str, keys: [&str; 2], width: usize) -> Vec<String> {
    let mut rows = wrapped(&format!(" {label}: {text}"), width);
    for hint in keys {
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
        Workbench::Observability => &HELP_OBSERVABILITY,
    };
    let switch: Vec<String> = workbench_keys(model)
        .map(|(key, _)| key.to_string())
        .collect();
    let actions = ACTIONS
        .iter()
        .filter(|action| action.workbench == workbench && fits(action, model))
        .filter(|action| actions::in_its_lab(action.row, model))
        .map(|action| (action.key.to_string(), action.hint));
    let owned = |&(key, meaning): &(&str, &'static str)| (key.to_string(), meaning);
    let terminal = workbench == Workbench::Devices && model.lab_with_nodes();
    let fixed = fixed.iter().flat_map(|entry| {
        let after = terminal && entry.0 == "[ ]";
        let typed = after.then(|| (SHELL.key.to_string(), HELP_TERMINAL));
        let views = HELP_CONFIG
            .iter()
            .filter(move |_| after && model.has_template())
            .map(owned);
        std::iter::once(owned(entry)).chain(typed).chain(views)
    });
    let quit = HELP_QUIT.iter().map(|&(key, meaning)| match key {
        "q" if model.lab_with_nodes() => (key.to_string(), HELP_QUIT_SESSION),
        _ => (key.to_string(), meaning),
    });
    [(switch.join(" "), "switch workbench")]
        .into_iter()
        .chain(fixed)
        .chain(actions)
        .chain(quit)
        .collect()
}

struct HelpLayout {
    entries: Vec<(String, &'static str)>,
    notes: Vec<&'static str>,
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
    let mut notes = match model.workbench() {
        Workbench::Devices => vec![HELP_DEVICES_NOTE],
        Workbench::Services => vec![HELP_SONDA],
        Workbench::Lab | Workbench::Observability => vec![],
    };
    notes.insert(0, HELP_PAGE);
    if model.run_drawn() {
        notes.push(HELP_RUN);
    }
    notes.push(HELP_MARK);
    let width = body.width.saturating_sub(4);
    let note_rows = notes
        .iter()
        .map(|note| {
            let note = u16::try_from(note.chars().count()).unwrap_or(u16::MAX);
            note.div_ceil(width.saturating_sub(2).max(1)).max(1)
        })
        .sum();
    let rows = u16::try_from(entries.len().div_ceil(2)).unwrap_or(u16::MAX);
    let note_height = note_rows + 1;
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
        notes,
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
        notes,
        popup,
        inner,
        rows,
        room,
        note_rows,
        note_row,
        fits,
    } = help_layout(model, area);
    frame.render_widget(Clear, area);
    let name = model.workbench().title(true);
    let block = Block::bordered()
        .border_type(border_of(model, Holder::Keys))
        .title_top(format!(" keys  {name} workbench "));
    frame.render_widget(block, popup);
    let column = |x: u16, y: u16, width: u16, height: u16| {
        Rect::new(inner.x + x, inner.y + y, width, height).intersection(inner)
    };
    let top = u16::try_from(model.help_scroll())
        .unwrap_or(u16::MAX)
        .min(rows - room);
    rows_mark(
        frame,
        border_below(inner),
        usize::from(top),
        usize::from(room),
        usize::from(rows),
    );
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
    frame.render_widget(
        Paragraph::new(notes.join("\n")).wrap(Wrap { trim: false }),
        column(0, note_row, inner.width, note_rows),
    );
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
    use crate::data;
    use crate::shell::tests::{
        alert, defined, devices_model, every_service_up, fixture, lab_devices_model, listing, row,
        sonda_rows, stack, start, workshop_located,
    };
    use crate::shell::{handle, stacks, Command, Stack, Start, Update, DEVICE_TABS, WORKBENCHES};

    #[test]
    fn a_cell_is_four_wide_and_a_count_of_a_thousand_or_more_says_999_plus() {
        let cells = [
            Agreement::Unknown,
            Agreement::Failed,
            Agreement::InSync,
            Agreement::Missing(1),
            Agreement::Missing(999),
            Agreement::Missing(1000),
            Agreement::Missing(123_456),
        ]
        .map(|agreement| format!("  {:<4}", sync_cell(agreement).0));

        assert_eq!(
            cells,
            ["  —   ", "  !   ", "  =   ", "  +1  ", "  +999", "  999+", "  999+"]
        );
        assert_eq!(sync_cell(Agreement::Failed).1, Style::new().fg(Color::Red));
    }

    fn tab_text(tabs: &[Tab], open: Tab) -> String {
        tab_line(tabs, open, false, 35)
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn a_log_row_pads_the_device_name_between_the_time_and_the_text() {
        let at = 49_323_000_000_000;
        assert_eq!(
            log_row(at, Some(("srl1", 6)), "up", 24),
            "13:42:03  srl1    up    "
        );
        assert_eq!(log_row(at, None, "up", 12), "13:42:03  up");
    }

    #[test]
    fn the_tab_line_keeps_the_open_title_and_marks_hidden_ones() {
        let lab = lab_devices_model();
        let five = lab.device_tabs();
        let lab_lines: Vec<String> = five.iter().map(|tab| tab_text(five, *tab)).collect();
        assert_eq!(
            lab_lines,
            [
                "OVERVIEW  ifaces  config ›",
                "overview  IFACES  config ›",
                "overview  ifaces  CONFIG ›",
                "‹ ifaces  config  TIMELINE  logs ›",
                "‹ ifaces  config  timeline  LOGS ›",
                "‹ config  timeline  logs  TERMINAL",
            ]
        );

        let workshop_lines: Vec<String> = DEVICE_TABS
            .iter()
            .map(|tab| tab_text(&DEVICE_TABS, *tab))
            .collect();
        assert_eq!(
            workshop_lines,
            [
                "OVERVIEW  ifaces  timeline  logs",
                "overview  IFACES  timeline  logs",
                "overview  ifaces  TIMELINE  logs",
                "overview  ifaces  timeline  LOGS",
            ]
        );
    }

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
        other: Option<(&'static str, &'static str, BorderType)>,
        fill: fn(&mut Model),
    }

    struct Walked {
        label: String,
        selected: &'static str,
        shown: Vec<(&'static str, &'static str, BorderType)>,
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
                ..stack(
                    &format!("stack-{index:02}"),
                    &[("SONDA_SERVER_URL", "http://localhost:8085")],
                )
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
        model.apply(listing(11, Ok(vec![row("grafana", "running")])));
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
        while model.focus() != Focus::List && tab != Tab::Terminal {
            handle(model, focus);
        }
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
            nodes: located.nodes(),
            topology: Some(located.topology()),
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

    #[test]
    fn no_row_of_any_help_is_cut_at_60_columns() {
        let area = Rect::new(0, 0, 60, 15);
        let builds: [fn() -> Model; 3] = [|| lab_bench(2), devices_bench, services_bench];
        let mut cut = Vec::new();
        for build in builds {
            for key in ['1', '2', '3', '4'] {
                let mut model = build();
                press(&mut model, key);
                press(&mut model, '?');
                let (_, [_, _, _, body, bottom, _]) = regions(&model, area);
                let layout = help_layout(&model, body.union(bottom));
                let (first, second) = layout.entries.split_at(usize::from(layout.rows));
                let middle = layout.inner.width / 2;
                for (entries, width) in [(first, middle), (second, layout.inner.width - middle)] {
                    for line in help_column(entries).lines() {
                        if line.chars().count() > usize::from(width) {
                            cut.push(format!("{:?} {width}: {line}", model.workbench()));
                        }
                    }
                }
            }
        }

        assert!(cut.is_empty(), "{cut:#?}");
    }

    #[test]
    fn q_asks_if_busy_in_every_help_of_a_lab_with_nodes_and_asks_in_a_run_elsewhere() {
        let builds: [fn() -> Model; 3] = [|| lab_bench(2), devices_bench, services_bench];
        let mut seen = Vec::new();
        for build in builds {
            for key in ['1', '2', '3', '4'] {
                let mut model = build();
                press(&mut model, key);
                let quit = help_entries(&model)
                    .into_iter()
                    .find(|(key, _)| key == "q")
                    .map(|(_, meaning)| meaning);
                seen.push((model.lab_with_nodes(), model.workbench(), quit));
            }
        }

        let wanted: Vec<_> = seen
            .iter()
            .map(|&(nodes, workbench, _)| {
                let words = if nodes {
                    "quit; asks if busy"
                } else {
                    "quit; asks in a run"
                };
                (nodes, workbench, Some(words))
            })
            .collect();
        assert_eq!(seen, wanted);
        let mut with_nodes: Vec<String> = seen
            .iter()
            .filter(|seen| seen.0)
            .map(|seen| format!("{:?}", seen.1))
            .collect();
        with_nodes.sort();
        with_nodes.dedup();
        assert_eq!(with_nodes, ["Devices", "Lab", "Observability", "Services"]);
        assert!(seen.iter().any(|seen| !seen.0));
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
            other: Some((" nodes ", "nobs-t-srl1", border(false))),
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
                other: Some((" devices ", "BgpSessionNotUp", border(true))),
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
                                    shown: [(bench.detail, row, border(false))].into_iter().chain(bench.other).collect(),
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

    fn boxed(rows: &[String], title: &str, border: BorderType) -> Vec<String> {
        let set = border.to_border_set();
        let corner = |symbol: &str| symbol.chars().next();
        let grid: Vec<Vec<char>> = rows.iter().map(|row| row.chars().collect()).collect();
        let top: Vec<char> = format!("{}{title}", set.top_left).chars().collect();
        for (y, row) in grid.iter().enumerate() {
            let Some(left) = row.windows(top.len()).position(|seen| seen == top) else {
                continue;
            };
            let right = row[left..]
                .iter()
                .position(|cell| Some(*cell) == corner(set.top_right))
                .map_or(row.len(), |at| left + at);
            return grid[y + 1..]
                .iter()
                .take_while(|row| row.get(left).copied() != corner(set.bottom_left))
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

    fn cut_at_a_word(shown: &str, full: &str, keep: Keep) -> bool {
        shown == full
            || match keep {
                Keep::Start => shown
                    .strip_suffix(" …")
                    .is_some_and(|kept| full.starts_with(&format!("{kept} "))),
                Keep::End => shown
                    .strip_prefix("… ")
                    .is_some_and(|kept| full.ends_with(&format!(" {kept}"))),
            }
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
            for (title, row, border) in shown {
                let inside = boxed(rows, title, *border);
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
    fn the_first_shown_row_carries_the_mark_after_the_filter_hides_the_cursor_row() {
        let cases = [
            (devices_bench(), "2", " devices ", "srl2"),
            (services_bench(), "-0", " stacks ", "stack-01"),
        ];
        for (mut model, filter, list, name) in cases {
            press(&mut model, '/');
            filter.chars().for_each(|typed| press(&mut model, typed));

            let rows = drawn(&mut model, 60, 15);

            let screen = rows.join("\n");
            let marked: Vec<String> = boxed(&rows, list, border(true))
                .into_iter()
                .filter(|row| row.starts_with('>'))
                .collect();
            assert_eq!(marked.len(), 1, "{screen}");
            assert!(marked[0].contains(name), "{screen}");
            assert!(
                !boxed(&rows, &format!(" {name} "), border(false)).is_empty(),
                "{screen}"
            );
        }
    }

    #[test]
    fn a_row_of_several_facts_shows_the_facts_that_fit_whole_then_an_ellipsis_at_60_columns() {
        let mut wide = Vec::new();
        let mut narrow = Vec::new();
        for tab in [Tab::Overview, Tab::Timeline] {
            let mut model = devices_bench();
            open_tab(&mut model, tab);
            wide.extend(boxed(&drawn(&mut model, 100, 30), " srl1 ", border(false)));
            narrow.extend(boxed(&drawn(&mut model, 60, 15), " srl1 ", border(false)));
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

            assert_eq!(
                joined.join(" "),
                question.text().replace(NO_BREAK, " "),
                "{label}"
            );
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
                    command.replace(NO_BREAK, " "),
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
    fn a_flag_and_its_value_share_a_row_at_every_width_where_they_fit() {
        let lab_dir = format!("--dir {}", fixture("book-lab").display());
        let bench_dir = format!("--dir {LAB_DIR}");
        let cases = [
            ('D', vec![bench_dir.as_str(), "-s ch1"]),
            ('m', vec![bench_dir.as_str(), "-d srl1"]),
            ('P', vec![lab_dir.as_str()]),
        ];
        for (key, pairs) in cases {
            for width in 60..=100 {
                let mut model = if key == 'm' {
                    devices_bench()
                } else {
                    lab_bench(1)
                };
                press(&mut model, key);
                let question = key_line(&model, width);
                press(&mut model, 'y');
                let run = model.run().unwrap();
                for (rows, room) in [(question, width), (command_rows(run, width - 2), width - 2)] {
                    for pair in &pairs {
                        let whole_word = |word: &str| {
                            word.chars().count() > room
                                || rows
                                    .iter()
                                    .any(|row| row.split(' ').any(|seen| seen == word))
                        };
                        let kept = if pair.chars().count() <= room {
                            rows.iter().any(|row| row.contains(pair))
                        } else {
                            pair.split(' ').all(whole_word)
                        };
                        assert!(kept, "{key} {width} {pair}\n{}", rows.join("\n"));
                    }
                }
                for room in 0..run.words.chars().count() {
                    let cut = cut_words(&run.words, room, Keep::Start);
                    assert!(
                        !cut.ends_with("-s …") && !cut.ends_with("-d …"),
                        "{key} {room} {cut}"
                    );
                }
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
            let words = &model.run().unwrap().words.replace(NO_BREAK, " ");
            let shown_after = |anchor: &str| {
                rows.iter()
                    .find_map(|row| row.split_once(anchor))
                    .and_then(|(_, rest)| rest.split("  ").next())
            };
            if let Some(title) = shown_after("run  ") {
                assert!(
                    cut_at_a_word(title, words, Keep::Start),
                    "{label}\n{screen}"
                );
            }
            if let Some(last) = shown_after("│ last      ") {
                assert!(cut_at_a_word(last, words, Keep::Start), "{label}\n{screen}");
            }
            if let Some((source, text, _)) = model.error() {
                let (shown, age) = rows[2].rsplit_once("  ").unwrap();
                let seconds = age.strip_suffix("s ago").unwrap_or(age);
                assert!(seconds.parse::<u64>().is_ok(), "{label}\n{screen}");
                let shown = shown.strip_prefix(&format!(" {source}: ")).unwrap();
                assert!(cut_at_a_word(shown, text, Keep::End), "{label}\n{screen}");
            }
        }
    }

    #[test]
    fn a_long_error_keeps_its_source_its_reason_and_its_age() {
        let mut model = Model::new(start(vec![stack("a", &[])]));
        model.apply(every_service_up(0));
        model.apply(Update::Metrics(Err(
            "cannot reach http://localhost:9090: io: Connection refused (os error 61)".to_string(),
        )));

        let rows = drawn(&mut model, 60, 15);

        assert_eq!(
            rows[2],
            " metrics: … io: Connection refused (os error 61)  0s ago",
            "{}",
            rows.join("\n")
        );
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
            model.apply(Update::Services(0, names(count), Ok(Vec::new())));
            for (width, height) in SIZES {
                let screen = drawn(&mut model, width, height);

                assert!(!screen[0].contains("services"), "{}", screen.join("\n"));
                if width >= WIDE {
                    assert!(screen[2].contains(title), "{}", screen.join("\n"));
                }
            }
        }
    }

    fn names(count: usize) -> Result<Vec<String>, String> {
        Ok((1..=count).map(|index| format!("s{index:02}")).collect())
    }

    fn opened(name: &str) -> Model {
        let mut model = Model::new(start(vec![stack(name, &[])]));
        handle(
            &mut model,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        model
    }

    fn row_with<'a>(screen: &'a [String], text: &str) -> &'a str {
        screen
            .iter()
            .find(|row| row.contains(text))
            .unwrap_or_else(|| panic!("no row with {text}\n{}", screen.join("\n")))
    }

    #[test]
    fn a_chapter_with_no_container_whose_file_defines_one_service_reads_0_of_1_running() {
        let mut model = lab_bench(1);
        model.apply(Update::Services(0, defined(&["sleeper"]), Ok(Vec::new())));

        for (width, height) in [(60, 15), (100, 30)] {
            let screen = drawn(&mut model, width, height).join("\n");

            assert!(screen.contains(" services  0 of 1 running"), "{screen}");
            assert!(screen.contains(">ch1  down 0/1"), "{screen}");
        }
    }

    #[test]
    fn a_compose_that_cannot_answer_shows_a_question_mark_names_config_and_clears_when_it_answers()
    {
        let mut model = lab_bench(1);
        let listed = || Ok(vec![row("sleeper", "running")]);

        model.apply(Update::Services(
            0,
            Err("no such file".to_string()),
            listed(),
        ));
        let screen = drawn(&mut model, 100, 30).join("\n");

        assert_eq!(model.stack_state(0).text(), "?");
        assert_eq!(model.stack_state(0).total(), None);
        assert!(screen.contains(" services  ?"), "{screen}");
        let (source, text, _) = model.error().unwrap();
        assert_eq!((source, text), ("config ch1", "no such file"));

        model.apply(Update::Services(0, defined(&["sleeper"]), listed()));
        let screen = drawn(&mut model, 100, 30).join("\n");

        assert_eq!(model.stack_state(0).text(), "up 1/1");
        assert!(screen.contains(" services  1 of 1 running"), "{screen}");
        assert_eq!(model.error(), None);
    }

    #[test]
    fn every_place_reads_the_one_pair() {
        let mut model = opened("packt");
        model.apply(Update::Services(0, names(22), Ok(Vec::new())));
        let screen = drawn(&mut model, 100, 30);

        assert!(row_with(&screen, "║>packt").contains(">packt  down 0/22"));
        assert!(row_with(&screen, "╔ stacks").contains("┌ packt  22 services  "));

        let three = vec![row("a", "running"), row("b", "running"), row("c", "exited")];
        model.apply(Update::Services(
            0,
            Err("cannot read".to_string()),
            Ok(three),
        ));
        let screen = drawn(&mut model, 100, 30);

        assert!(row_with(&screen, "║>packt").contains(">packt  ?"));
        assert!(row_with(&screen, "╔ stacks").contains("┌ packt  ?  "));
    }

    #[test]
    fn a_long_stack_name_gives_way_so_the_state_stays_whole_at_60_columns() {
        let mut model = opened("nobs-t-packt");
        model.apply(Update::Services(0, names(22), Ok(Vec::new())));
        let screen = drawn(&mut model, 60, 15);

        assert!(row_with(&screen, "║>nobs").contains("  down 0/22║"));
    }

    #[test]
    fn a_long_path_gives_way_so_the_chooser_state_stays_whole_at_60_columns() {
        let packt = Stack {
            path: PathBuf::from(LAB_DIR),
            ..stack("packt", &[])
        };
        let mut model = Model::new(start(vec![packt]));
        model.apply(Update::Services(0, names(13), Ok(Vec::new())));
        let screen = drawn(&mut model, 60, 15);

        assert!(row_with(&screen, "║>packt").contains("…  down 0/13"));
    }

    #[test]
    fn a_long_stack_name_never_cuts_the_count_in_the_title() {
        for length in 40..=70 {
            let mut model = opened(&"a".repeat(length));
            model.apply(Update::Services(0, names(22), Ok(Vec::new())));
            let screen = drawn(&mut model, 100, 30);
            let top = row_with(&screen, "╔ stacks");
            let title = &top[top.find("╗┌").unwrap()..];

            assert!(
                title.contains(" 22 services") || !title.contains('2'),
                "{title}"
            );
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
    fn the_keys_box_marks_its_hidden_lines_on_its_bottom_border() {
        let mut model = services_bench();
        model.apply(Update::Logs(
            0,
            "loki".to_string(),
            Err(LONG_ERROR.to_string()),
        ));
        press(&mut model, '?');
        let bottom = |rows: &[String]| {
            rows.iter()
                .find(|row| row.trim_start().starts_with('╚'))
                .unwrap()
                .clone()
        };
        for mark in ["↓6", "↑1 ↓5", "↑2 ↓4"] {
            let rows = drawn(&mut model, 60, 15);
            model.set_help_overflow(help_overflow(&model, Rect::new(0, 0, 60, 15)));
            let screen = rows.join("\n");

            assert!(
                bottom(&rows).ends_with(&format!("═ {mark} ═╝")),
                "{mark}\n{screen}"
            );
            assert!(screen.contains(HELP_MORE), "{mark}\n{screen}");
            press(&mut model, 'j');
        }

        let rows = drawn(&mut model, 100, 30);

        assert!(
            bottom(&rows)
                .trim_start()
                .chars()
                .all(|c| "╚═╝".contains(c)),
            "{}",
            rows.join("\n")
        );
    }

    #[test]
    fn the_help_names_the_lab_switch_key_only_in_the_book_lab() {
        let mut book_services = lab_bench(2);
        press(&mut book_services, '2');
        for (model, switch) in [
            (devices_bench(), "1 2 3"),
            (services_bench(), "1 2 3"),
            (lab_bench(2), "1 2 3 4"),
            (book_services, "1 2 3 4"),
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
    fn in_a_book_lab_the_switcher_says_obs_up_to_64_columns_and_observability_from_65() {
        let mut model = lab_devices_model();

        let narrow = drawn(&mut model, 64, 15);
        let wide = drawn(&mut model, 65, 15);

        assert_eq!(
            narrow[1],
            " [1 devices]  2 services  3 lab  4 obs           ? keys  q quit"
        );
        assert_eq!(
            wide[1],
            " [1 devices]  2 services  3 lab  4 observability  ? keys  q quit"
        );
    }

    fn alert_names(rows: &[String], names: &[&'static str]) -> Vec<&'static str> {
        rows.iter()
            .filter_map(|row| names.iter().find(|name| row.contains(*name)).copied())
            .collect()
    }

    fn requests(commands: &[Command]) -> Vec<&Command> {
        commands
            .iter()
            .filter(|command| matches!(command, Command::Ask(..) | Command::Refresh))
            .collect()
    }

    #[test]
    fn the_table_lists_the_alerts_of_the_box_in_its_order_and_a_new_alert_needs_no_request() {
        let names = ["nobs-t-mike", "nobs-t-alpha", "nobs-t-zulu", "nobs-t-bravo"];
        let first = vec![
            alert("srl1", names[2], "2026-11-09T13:51:32Z"),
            alert("srl1", names[0], "2026-11-09T13:53:32Z"),
            alert("srl2", names[1], "2026-11-09T13:52:32Z"),
        ];
        let mut model = devices_model(workshop_located("infrahub"));
        let answered = model.apply(Update::Alerts(Ok(first.clone())));
        let screen = drawn(&mut model, 100, 30);
        let separator = screen
            .iter()
            .position(|row| row.contains(" alerts 3 "))
            .unwrap();
        let left: Vec<String> = screen[separator + 1..]
            .iter()
            .map(|row| row.chars().take(usize::from(LEFT_DEVICES)).collect())
            .collect();
        let in_the_box = alert_names(&left, &names);
        let table = |model: &mut Model| {
            let screen = drawn(model, 100, 30);
            let end = screen.iter().position(|row| row.starts_with('╠')).unwrap();
            alert_names(&screen[..end], &names)
        };

        let switched = handle(
            &mut model,
            KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE),
        );
        let in_the_table = table(&mut model);
        let newer = alert("srl2", names[3], "2026-11-09T13:54:32Z");
        let grown = model.apply(Update::Alerts(Ok([newer]
            .into_iter()
            .chain(first)
            .collect())));
        let moved = handle(
            &mut model,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        );

        assert_eq!(in_the_box, [names[0], names[1], names[2]]);
        assert_eq!(in_the_table, in_the_box);
        assert_eq!(table(&mut model), [names[3], names[0], names[1], names[2]]);
        assert_eq!(requests(&answered), Vec::<&Command>::new());
        assert_eq!(requests(&switched), Vec::<&Command>::new());
        assert_eq!(requests(&grown), Vec::<&Command>::new());
        assert_eq!(requests(&moved), Vec::<&Command>::new());
    }

    #[test]
    fn the_lower_box_follows_the_selection_and_the_selection_stays_on_its_alert_when_the_list_changes(
    ) {
        let older = [
            alert("srl1", "nobs-t-first", "2026-11-09T13:53:32Z"),
            alert("srl2", "nobs-t-second", "2026-11-09T13:52:32Z"),
        ];
        let mut model = devices_model(workshop_located("infrahub"));
        model.apply(Update::Alerts(Ok(older.to_vec())));
        press(&mut model, '3');

        let at_first = lower_box_title(&mut model);
        press(&mut model, 'j');
        let after_j = lower_box_title(&mut model);
        let newer = alert("srl1", "nobs-t-newest", "2026-11-09T13:54:32Z");
        model.apply(Update::Alerts(Ok([newer]
            .into_iter()
            .chain(older)
            .collect())));
        let after_newer = lower_box_title(&mut model);

        assert!(at_first.starts_with("╠ nobs-t-first  srl1 ═"), "{at_first}");
        assert!(after_j.starts_with("╠ nobs-t-second  srl2 ═"), "{after_j}");
        assert!(
            after_newer.starts_with("╠ nobs-t-second  srl2 ═"),
            "{after_newer}"
        );
    }

    fn lower_box_title(model: &mut Model) -> String {
        drawn(model, 100, 30)
            .into_iter()
            .find(|row| row.starts_with('╠'))
            .unwrap_or_default()
    }

    fn titles_across_a_failed_poll_and_a_newer_alert(filter: &str) -> (String, String) {
        let older = [
            alert("srl1", "nobs-t-first", "2026-11-09T13:53:32Z"),
            alert("srl2", "nobs-t-second", "2026-11-09T13:52:32Z"),
            alert("srl2", "nobs-t-third", "2026-11-09T13:51:32Z"),
        ];
        let mut model = devices_model(workshop_located("infrahub"));
        model.apply(Update::Alerts(Ok(older.to_vec())));
        press(&mut model, '3');
        if !filter.is_empty() {
            press(&mut model, '/');
            filter.chars().for_each(|letter| press(&mut model, letter));
            handle(
                &mut model,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            );
        }
        press(&mut model, 'j');
        let before = lower_box_title(&mut model);
        model.apply(Update::Alerts(Err("refused".to_string())));
        let newer = alert("srl2", "nobs-t-newest", "2026-11-09T13:54:32Z");
        model.apply(Update::Alerts(Ok([newer]
            .into_iter()
            .chain(older)
            .collect())));
        (before, lower_box_title(&mut model))
    }

    #[test]
    fn under_a_filter_the_lower_box_keeps_its_alert_across_a_failed_poll_and_a_newer_alert() {
        let (before, after) = titles_across_a_failed_poll_and_a_newer_alert("srl2");

        assert!(before.starts_with("╠ nobs-t-third  srl2 ═"), "{before}");
        assert_eq!(after, before);
    }

    #[test]
    fn the_lower_box_keeps_its_alert_across_a_failed_poll_and_a_newer_alert() {
        let (before, after) = titles_across_a_failed_poll_and_a_newer_alert("");

        assert!(before.starts_with("╠ nobs-t-second  srl2 ═"), "{before}");
        assert_eq!(after, before);
    }

    #[test]
    fn the_observability_key_line_offers_enter_only_on_an_alert_of_a_device_of_the_lab_and_tab_only_beside_a_drawn_run_pane(
    ) {
        let mut model = devices_model(workshop_located("infrahub"));
        model.apply(Update::Alerts(Ok(vec![
            alert(
                "nobs-t-elsewhere",
                "BgpSessionNotUp",
                "2026-11-09T13:53:32Z",
            ),
            alert("srl1", "BgpSessionNotUp", "2026-11-09T13:52:32Z"),
        ])));
        press(&mut model, '3');

        let elsewhere = key_line(&model, 100);
        press(&mut model, 'j');
        let in_the_lab = key_line(&model, 100);
        press(&mut model, 'o');
        model.set_run_view(Some(RunMeasure {
            overflow: 0,
            cut_rows: 0,
        }));
        let beside_the_run_pane = key_line(&model, 100);

        assert_eq!(elsewhere, [" j/k move  / filter  [ ] view  r refresh"]);
        assert_eq!(
            in_the_lab,
            [" j/k move  / filter  [ ] view  Enter open the device  r refresh"]
        );
        assert_eq!(
            beside_the_run_pane,
            [" Tab focus  j/k move  / filter  [ ] view  Enter open the device  r refresh"]
        );
    }

    #[test]
    fn the_logs_view_key_line_offers_enter_only_on_a_paused_line_of_a_device_of_the_lab() {
        let mut model = devices_model(workshop_located("infrahub"));
        let line = |device: &str, nanoseconds: i64, text: &str| {
            (
                [("device".to_string(), device.to_string())].into(),
                nobs_core::loki::Line {
                    nanoseconds,
                    text: text.to_string(),
                },
            )
        };
        model.apply(Update::LabLogs(Ok(vec![
            line("srl1", 2, "bgp up"),
            line("nobs-t-elsewhere", 1, "link down"),
        ])));
        press(&mut model, '3');
        press(&mut model, ']');

        let following = key_line(&model, 100);
        press(&mut model, 'f');
        let in_the_lab = key_line(&model, 100);
        press(&mut model, 'j');
        let elsewhere = key_line(&model, 100);

        assert_eq!(following, [" f pause  / filter  [ ] view  r refresh"]);
        assert_eq!(
            in_the_lab,
            [" f follow  j/k move  / filter  Enter open the device"]
        );
        assert_eq!(elsewhere, [" f follow  j/k move  / filter"]);
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
            let notice = model.notice().unwrap().replace(NO_BREAK, " ");

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
            output: None,
            started: Instant::now(),
            ended: None,
            state: RunState::Running,
            typed: false,
            asked: false,
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
            output: None,
            started: Instant::now(),
            ended: None,
            state,
            typed: false,
            asked: false,
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
        assert_eq!(pane_height(Some(&refused), 58, 11, 3, false), 5);
        assert_eq!(pane_height(Some(&refused), 58, 12, 3, false), 6);
        assert_eq!(pane_height(Some(&running), 58, 11, 3, false), 5);
        assert_eq!(pane_height(Some(&refused), 98, 26, 3, false), 9);
        assert_eq!(pane_height(Some(&refused), 98, 27, 3, false), 9);
        assert_eq!(pane_height(Some(&last_line(5)), 58, 11, 3, false), 7);
        assert_eq!(pane_height(Some(&last_line(6)), 58, 11, 3, false), 8);
        assert_eq!(pane_height(Some(&last_line(6)), 58, 12, 3, false), 8);
        assert_eq!(pane_height(Some(&last_line(7)), 58, 12, 3, false), 9);
        assert_eq!(pane_height(Some(&refused), 58, 11, 7, false), 4);
        assert_eq!(pane_height(Some(&running), 98, 26, 7, false), 9);
        assert_eq!(pane_height(Some(&last_line(7)), 58, 12, 7, false), 5);
        assert_eq!(pane_height(None, 58, 11, 9, false), 2);
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

    #[test]
    fn the_interfaces_footer_of_a_device_with_one_interface_says_1_interface() {
        let mut model = devices_model(workshop_located("infrahub"));
        let mut list = model.devices().to_vec();
        list[0].interfaces.truncate(1);
        model.apply(Update::Devices(Ok(list)));
        assert_eq!(model.interfaces().len(), 1);

        let rows = painted(30, 10, |frame| {
            device_interfaces(frame, frame.area(), &model, false)
        });

        assert_eq!(rows[9], "1 interface  0 differs");
    }

    type Build = fn() -> Model;

    fn many_stacks(count: usize) -> Vec<Stack> {
        (1..=count)
            .map(|index| stack(&format!("s{index:02}"), &[]))
            .collect()
    }

    fn moved(mut model: Model, key: char, times: usize) -> Model {
        for _ in 0..times {
            press(&mut model, key);
        }
        model
    }

    fn focus_tabs(model: &mut Model) {
        handle(model, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    }

    fn stacks_of(count: usize) -> Model {
        let mut model = Model::new(start(many_stacks(count)));
        handle(
            &mut model,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        model.apply(Update::Services(0, names(count), Ok(Vec::new())));
        model
    }

    fn services_of(count: usize) -> Model {
        let mut model = opened("packt");
        model.apply(Update::Services(0, names(count), Ok(Vec::new())));
        focus_tabs(&mut model);
        model
    }

    fn devices_of(count: usize) -> Model {
        let mut model = devices_model(workshop_located("infrahub"));
        let first = model.devices()[0].clone();
        let list = (1..=count)
            .map(|index| {
                let mut device = first.clone();
                device.name = format!("d{index:02}");
                device
            })
            .collect();
        model.apply(Update::Devices(Ok(list)));
        model
    }

    fn alerts_of(count: usize) -> Model {
        let mut model = devices_model(workshop_located("infrahub"));
        let alerts = (0..count)
            .map(|index| {
                let starts_at = format!("2026-11-09T13:{:02}:00Z", 59 - index);
                alert("srl1", &format!("a{index:02}"), &starts_at)
            })
            .collect();
        model.apply(Update::Alerts(Ok(alerts)));
        moved(model, 'j', 2)
    }

    fn timeline_of(count: usize) -> Model {
        let mut model = devices_model(workshop_located("infrahub"));
        let lines = (0..count)
            .map(|index| nobs_core::loki::Line {
                nanoseconds: 0,
                text: format!("line {index:02}"),
            })
            .collect();
        model.apply(Update::DeviceLogs("srl1".to_string(), Ok(lines)));
        open_tab(&mut model, Tab::Timeline);
        focus_tabs(&mut model);
        model
    }

    fn scenarios(count: usize) -> String {
        let rows: Vec<String> = (0..count)
            .map(|index| {
                format!(
                    r#"{{"id":"id{index:02}","name":"n{index:02}","state":"running","elapsed_secs":1,"degraded":false}}"#
                )
            })
            .collect();
        format!(r#"{{"scenarios":[{}]}}"#, rows.join(","))
    }

    fn sonda_of(count: usize) -> Model {
        let mut model = opened("a");
        model.apply(every_service_up(0));
        model.apply(Update::Sonda(data::Update {
            at: Instant::now(),
            list: Ok(scenarios(count)),
            selected: None,
        }));
        open_tab(&mut model, Tab::Sonda);
        focus_tabs(&mut model);
        model
    }

    fn nodes_of(count: usize) -> Model {
        let mut model = lab_bench(1);
        let nodes = (1..=count)
            .map(|index| NodeRow {
                name: format!("n{index:02}"),
                kind: "nokia_srlinux".to_string(),
                state: Some("running".to_string()),
            })
            .collect();
        model.apply(Update::Nodes(Ok(nodes)));
        focus_tabs(&mut model);
        model
    }

    fn interfaces_of(count: usize) -> Model {
        let mut model = devices_model(workshop_located("infrahub"));
        let mut list = model.devices().to_vec();
        let first = list[0].interfaces[0].clone();
        list[0].interfaces = (1..=count)
            .map(|index| {
                let mut interface = first.clone();
                interface.name = format!("e{index:02}");
                interface
            })
            .collect();
        model.apply(Update::Devices(Ok(list)));
        open_tab(&mut model, Tab::Interfaces);
        focus_tabs(&mut model);
        model
    }

    fn sessions_of(count: usize) -> Model {
        let mut model = devices_model(workshop_located("infrahub"));
        let mut list = model.devices().to_vec();
        let first = list[0].sessions[0].clone();
        list[0].sessions = (1..=count)
            .map(|index| {
                let mut session = first.clone();
                session.peer = format!("p{index:02}");
                session
            })
            .collect();
        model.apply(Update::Devices(Ok(list)));
        focus_tabs(&mut model);
        model
    }

    #[test]
    fn the_device_overview_moves_by_one_line_and_says_how_many_lines_are_hidden() {
        let mut model = sessions_of(12);
        let turn = |model: &mut Model, key: char, times: usize| {
            for _ in 0..times {
                press(model, key);
            }
            let rows = drawn(model, 60, 15);
            model.set_overview_overflow(overview_overflow(model, Rect::new(0, 0, 60, 15)));
            let right: Vec<String> = rows
                .iter()
                .map(|row| row.chars().skip(usize::from(LEFT_NARROW)).collect())
                .collect();
            assert!(
                right.iter().all(|row| !row.contains('>')),
                "{}",
                rows.join("\n")
            );
            rows.join("\n")
        };
        let steps = [
            ('j', 0, "↓8", "kind ", true),
            ('j', 1, "↑1 ↓7", "kind ", false),
            ('j', 20, "↑8", "alerts   none", true),
            ('k', 1, "↑7 ↓1", "alerts   none", false),
        ];
        for (key, times, mark, line, shown) in steps {
            let screen = turn(&mut model, key, times);

            let edge = format!("═ {mark} ═╝");
            assert!(screen.contains(&edge), "{key} {times}: {edge}\n{screen}");
            assert_eq!(
                screen.contains(line),
                shown,
                "{key} {times}: {line}\n{screen}"
            );
        }
    }

    fn tabbed(mut model: Model, times: usize) -> Model {
        for _ in 0..times {
            focus_tabs(&mut model);
        }
        model
    }

    fn on_view(tab: Tab) -> Model {
        let mut model = moved(devices_bench(), '3', 1);
        while model.tab() != tab {
            press(&mut model, ']');
        }
        model
    }

    #[test]
    fn the_rows_of_each_box_at_60x15_and_100x30() {
        let boxes: [(&str, Holder, Build, usize, usize); 24] = [
            (
                "chooser",
                Holder::Chooser,
                || Model::new(start(many_stacks(4))),
                10,
                25,
            ),
            (
                "keys",
                Holder::Keys,
                || moved(devices_bench(), '?', 1),
                6,
                8,
            ),
            ("entry", Holder::Entry, boxed_entry, 4, 19),
            ("stacks", Holder::Stacks, services_bench, 9, 24),
            ("chapters", Holder::Chapters, || lab_bench(2), 2, 2),
            ("nodes", Holder::Nodes, || tabbed(lab_bench(2), 1), 4, 19),
            (
                "lab overview",
                Holder::Overview,
                || tabbed(lab_bench(2), 2),
                8,
                23,
            ),
            (
                "services overview",
                Holder::Pane,
                || tabbed(services_bench(), 1),
                7,
                22,
            ),
            (
                "service log",
                Holder::Pane,
                || services_log_of(forty_lines()),
                8,
                23,
            ),
            (
                "device overview",
                Holder::Pane,
                || tabbed(devices_bench(), 1),
                8,
                23,
            ),
            ("interfaces", Holder::Pane, || interfaces_of(4), 6, 21),
            ("timeline", Holder::Pane, || timeline_of(2), 7, 22),
            ("config", Holder::Pane, config_tab, 3, 14),
            ("device log", Holder::Pane, || device_log_of(2), 8, 23),
            (
                "terminal",
                Holder::Pane,
                || {
                    let mut model = lab_devices_model();
                    open_tab(&mut model, Tab::Terminal);
                    model
                },
                8,
                23,
            ),
            ("device list", Holder::Devices, devices_bench, 5, 16),
            ("alerts box", Holder::Devices, || alerts_of(2), 2, 6),
            (
                "scenarios",
                Holder::Sonda(view::Part::Scenarios),
                || sonda_of(1),
                3,
                18,
            ),
            (
                "details",
                Holder::Sonda(view::Part::Details),
                || tabbed(sonda_of(1), 1),
                4,
                19,
            ),
            (
                "run pane",
                Holder::Run,
                || {
                    let mut model = answered(&numbered(40));
                    tab_to_run(&mut model, 60, 15);
                    model
                },
                10,
                25,
            ),
            (
                "alerts view",
                Holder::Alerts,
                || on_view(Tab::Alerts),
                5,
                16,
            ),
            (
                "alerts view with a selected alert",
                Holder::Alerts,
                || moved(alerts_of(20), '3', 1),
                5,
                16,
            ),
            ("logs view", Holder::Alerts, || on_view(Tab::Logs), 8, 23),
            (
                "metrics view",
                Holder::Alerts,
                || on_view(Tab::Metrics),
                8,
                23,
            ),
        ];
        for (label, box_holder, build, small, large) in boxes {
            for (width, height, rows) in [(60, 15, small), (100, 30, large)] {
                let model = build();

                assert_eq!(holder(&model), box_holder, "{label}");
                assert_eq!(
                    shown_rows(&model, Rect::new(0, 0, width, height)),
                    rows,
                    "{label} {width}x{height}"
                );
            }
        }
    }

    fn rich(pane: Option<bool>) -> Model {
        let dir = fixture("book-lab");
        let located = locate(&dir, Some(&dir)).unwrap();
        let mut first = stacks(Some(&located), Vec::new()).unwrap().remove(0);
        first.env.push((
            "SONDA_SERVER_URL".to_string(),
            "http://localhost:8085".to_string(),
        ));
        let chapters = (1..=12)
            .map(|index| Stack {
                name: format!("ch{index}"),
                ..first.clone()
            })
            .collect();
        let mut model = Model::new(Start {
            nodes: located.nodes(),
            topology: Some(located.topology()),
            lab: Some(located),
            home: Workbench::Devices,
            ..start(chapters)
        });
        let template = devices_model(workshop_located("infrahub")).devices()[0].clone();
        let devices = (1..=40)
            .map(|index| {
                let mut device = template.clone();
                device.name = format!("d{index:02}");
                device.interfaces = vec![template.interfaces[0].clone(); 30];
                device.sessions = vec![template.sessions[0].clone(); 12];
                device
            })
            .collect();
        let alerts = (0..20)
            .map(|index| alert("d01", &format!("a{index:02}"), "2026-11-09T13:53:32Z"))
            .collect();
        let text: Vec<String> = (1..=12).map(|index| format!("text {index:02}")).collect();
        let lines = std::iter::once(log_line(41, &text.join("\n")))
            .chain((1..=40).map(|index| log_line(index, &format!("line {index:02}"))))
            .collect();
        let nodes = (1..=30)
            .map(|index| NodeRow {
                name: format!("n{index:02}"),
                kind: "nokia_srlinux".to_string(),
                state: Some("running".to_string()),
            })
            .collect();
        model.apply(every_service_up(0));
        model.apply(Update::Devices(Ok(devices)));
        model.apply(Update::Alerts(Ok(alerts)));
        model.apply(Update::DeviceLogs("d01".to_string(), Ok(lines)));
        model.apply(Update::Nodes(Ok(nodes)));
        model.apply(Update::Logs(
            0,
            "loki".to_string(),
            Err(LONG_ERROR.to_string()),
        ));
        let Some(pane) = pane else {
            return model;
        };
        handle(
            &mut model,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        model.apply(Update::Sonda(data::Update {
            at: Instant::now(),
            list: Ok(scenarios(20)),
            selected: None,
        }));
        "sx".chars().for_each(|typed| press(&mut model, typed));
        handle(
            &mut model,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        for line in numbered(40) {
            model.apply(Update::ActionLine(line));
        }
        model.apply(Update::ActionEnded(End::Exited(0)));
        model.apply(lab_lines("d01", 40));
        if !pane {
            press(&mut model, 'o');
        }
        model
    }

    fn lab_lines(device: &str, count: i64) -> Update {
        Update::LabLogs(Ok((1..=count)
            .map(|index| {
                (
                    [("device".to_string(), device.to_string())].into(),
                    log_line(index, &format!("lab {index:02}")),
                )
            })
            .collect()))
    }

    fn lab_logs_of(count: i64) -> Model {
        let mut model = devices_model(workshop_located("infrahub"));
        model.apply(lab_lines("srl1", count));
        press(&mut model, '3');
        press(&mut model, ']');
        model
    }

    fn looped(model: &mut Model, area: Rect) {
        crate::settle(model, area);
        model.set_terminal_size(terminal_size(model, area));
        crate::measure(model, area);
    }

    fn long_details() -> Model {
        let fields: Vec<String> = (1..=30)
            .map(|index| format!(r#""f{index:02}":1"#))
            .collect();
        let show = format!("{{{}}}", fields.join(","));
        tabbed(sonda_details(r#"{"state":"running"}"#, &show), 1)
    }

    fn services_box() -> Model {
        let mut model = opened("a");
        let rows = vec![row("grafana", "running"), row("loki", "running")];
        model.apply(listing(0, Ok(rows)));
        tabbed(model, 1)
    }

    fn config_tab() -> Model {
        let mut model = lab_devices_model();
        press(&mut model, 'g');
        for line in numbered(60) {
            model.apply(Update::ActionLine(line));
        }
        model.apply(Update::ActionEnded(End::Exited(0)));
        open_tab(&mut model, Tab::Config);
        focus_tabs(&mut model);
        model
    }

    #[test]
    fn after_a_g_with_no_difference_the_second_header_line_says_none_missing() {
        let mut model = lab_devices_model();
        model.apply(Update::Intended(vec![(
            "nobs-t-srl1".to_string(),
            Ok("set / a b on\n".to_string()),
        )]));
        press(&mut model, 'g');
        model.apply(Update::ActionLine("set / a b on".to_string()));
        model.apply(Update::ActionEnded(End::Exited(0)));
        open_tab(&mut model, Tab::Config);
        focus_tabs(&mut model);

        let screen = drawn(&mut model, 100, 30);

        assert!(row_with(&screen, "║fetched ").contains("  none missing "));
    }

    #[test]
    fn the_intended_view_gives_the_page_keys_the_rows_under_its_two_header_lines() {
        let area = Rect::new(0, 0, 60, 15);
        let mut model = lab_devices_model();
        model.apply(Update::Intended(vec![(
            "nobs-t-srl1".to_string(),
            Ok(numbered(40).join("\n")),
        )]));
        open_tab(&mut model, Tab::Config);
        focus_tabs(&mut model);
        looped(&mut model, area);

        let screen = drawn(&mut model, 60, 15);
        let listed = screen.iter().filter(|row| row.contains("║ line ")).count();
        assert!(screen
            .iter()
            .any(|row| row.contains("║press g to compare with the device")));
        assert_eq!(shown_rows(&model, area), listed);

        handle(
            &mut model,
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
        );
        assert_eq!(model.intended_view().unwrap().scroll, listed - 1);
    }

    #[test]
    fn wherever_j_moves_a_page_key_moves_as_j_pressed_page_times() {
        let area = Rect::new(0, 0, 60, 15);
        let pressed = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let row = |typed| pressed(KeyCode::Char(typed));
        let reach = |(build, path): &(Build, Vec<KeyEvent>), moves: &[KeyEvent]| {
            let mut model = build();
            for key in path {
                looped(&mut model, area);
                handle(&mut model, *key);
            }
            looped(&mut model, area);
            moves.iter().for_each(|key| drop(handle(&mut model, *key)));
            looped(&mut model, area);
            model
        };
        let bare: Build = || rich(None);
        let mut states = vec![
            (bare, vec![]),
            (bare, vec![row('?')]),
            (long_details, vec![]),
            (services_box, vec![]),
            (|| services_log_of(forty_lines()), vec![]),
            (config_tab, vec![]),
        ];
        let panes: [Build; 2] = [|| rich(Some(false)), || rich(Some(true))];
        for pane in panes {
            for digit in ['1', '2', '3', '4'] {
                for tabs in 0..4 {
                    for turns in 0..7 {
                        for enter in [false, true] {
                            for help in [false, true] {
                                let mut path = vec![row(digit)];
                                path.extend(std::iter::repeat_n(pressed(KeyCode::Tab), tabs));
                                path.extend(std::iter::repeat_n(row(']'), turns));
                                path.extend(enter.then(|| pressed(KeyCode::Enter)));
                                path.extend(help.then(|| row('?')));
                                states.push((pane, path));
                            }
                        }
                    }
                }
            }
        }
        let place_of = |model: &Model| {
            let (bench, tab) = (model.workbench(), model.tab());
            let following = match (bench, tab) {
                (Workbench::Services, Tab::Logs) => model.logs().map(|logs| logs.2),
                (Workbench::Devices, Tab::Logs) => model.device_logs().map(|logs| logs.2),
                (Workbench::Observability, Tab::Logs) => Some(model.lab_logs().1),
                _ => None,
            };
            let state = match following {
                Some(true) => " following",
                Some(false) => " paused",
                None => "",
            };
            match holder(model) {
                Holder::Pane | Holder::Alerts => format!("{bench:?} {tab:?}{state}"),
                other => format!("{other:?}"),
            }
        };
        let bottom = vec![row('j'); 80];
        let top: Vec<KeyEvent> = bottom.iter().copied().chain(vec![row('k'); 80]).collect();
        let mut frames = Vec::new();
        let mut compared = Vec::new();
        for state in states {
            let base = drawn(&mut reach(&state, &[]), 60, 15);
            let moves = |typed| drawn(&mut reach(&state, &[row(typed)]), 60, 15) != base;
            if frames.contains(&base) || !(moves('j') || moves('k')) {
                continue;
            }
            frames.push(base);
            for edge in [&[][..], &bottom, &top] {
                for (page, typed) in [(KeyCode::PageDown, 'j'), (KeyCode::PageUp, 'k')] {
                    let mut paged = reach(&state, edge);
                    let place = place_of(&paged);
                    handle(&mut paged, pressed(page));
                    let mut rowed = reach(&state, edge);
                    let presses = vec![row(typed); rowed.page()];
                    presses
                        .iter()
                        .for_each(|key| drop(handle(&mut rowed, *key)));
                    assert_eq!(
                        drawn(&mut paged, 60, 15),
                        drawn(&mut rowed, 60, 15),
                        "{place} {:?} {} {page:?}",
                        state.1,
                        edge.len()
                    );
                    compared.push(place);
                }
            }
        }
        compared.sort();
        compared.dedup();
        assert_eq!(
            compared,
            [
                "Chapters",
                "Chooser",
                "Devices",
                "Devices Config",
                "Devices Interfaces",
                "Devices Logs following",
                "Devices Logs paused",
                "Devices Overview",
                "Devices Timeline",
                "Entry",
                "Keys",
                "Nodes",
                "Observability Alerts",
                "Observability Logs following",
                "Observability Logs paused",
                "Observability Metrics",
                "Overview",
                "Run",
                "Services Logs following",
                "Services Logs paused",
                "Services Overview",
                "Sonda(Details)",
                "Sonda(Scenarios)",
                "Stacks"
            ]
        );
    }

    #[test]
    fn a_move_away_from_the_newest_line_pauses_a_following_log_and_moves() {
        type Seen = fn(&Model) -> (bool, usize);
        type Follow = fn(bool) -> Vec<Command>;
        type OfLines = fn(i64) -> Model;
        let area = Rect::new(0, 0, 100, 30);
        let pressed = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let frame = |model: &mut Model| drawn(model, 100, 30).join("\n");
        let logs: [(OfLines, char, Seen, Follow, &str); 4] = [
            (
                |count| services_log_of((0..count).map(|index| format!("l{index}")).collect()),
                'k',
                |model| model.logs().map(|logs| (logs.2, logs.3)).unwrap(),
                |on| vec![Command::Follow(on)],
                "paused",
            ),
            (
                device_log_of,
                'j',
                |model| model.device_logs().map(|logs| (logs.2, logs.3)).unwrap(),
                |on| vec![Command::FollowDevice(on)],
                "paused",
            ),
            (
                lab_logs_of,
                'j',
                |model| (model.lab_logs().1, model.lab_logs().2.unwrap_or(0)),
                |_| Vec::new(),
                "paused",
            ),
            (
                |count| {
                    let mut model = answered(&numbered(count as usize));
                    tab_to_run(&mut model, 100, 30);
                    model
                },
                'k',
                |model| {
                    let back = model.run_overflow() - model.run_scroll();
                    (back == 0, back)
                },
                |_| Vec::new(),
                "f follow",
            ),
        ];
        for (index, (build, away, seen, follow, word)) in logs.into_iter().enumerate() {
            let (toward, away_page, toward_page) = match away {
                'k' => ('j', KeyCode::PageUp, KeyCode::PageDown),
                _ => ('k', KeyCode::PageDown, KeyCode::PageUp),
            };
            for (key, paged) in [(KeyCode::Char(away), false), (away_page, true)] {
                let mut model = build(100);
                looped(&mut model, area);
                assert_eq!(seen(&model), (true, 0), "{index} {key:?}");
                let rows = if paged { model.page() } else { 1 };
                assert!(!paged || rows > 1, "{index}");

                let commands = handle(&mut model, pressed(key));
                assert_eq!(commands, follow(false), "{index} {key:?}");
                looped(&mut model, area);
                assert_eq!(seen(&model), (false, rows), "{index} {key:?}");
                assert!(frame(&mut model).contains(word), "{index} {key:?}");

                let commands = handle(&mut model, pressed(KeyCode::Char('f')));
                assert_eq!(commands, follow(true), "{index} {key:?} f");
                looped(&mut model, area);
                assert_eq!(seen(&model), (true, 0), "{index} {key:?} f");
                assert!(!frame(&mut model).contains(word), "{index} {key:?} f");
            }
            for key in [KeyCode::Char(toward), toward_page] {
                let mut model = build(100);
                looped(&mut model, area);
                let before = frame(&mut model);

                assert_eq!(handle(&mut model, pressed(key)), [], "{index} {key:?}");
                looped(&mut model, area);
                assert_eq!(seen(&model), (true, 0), "{index} {key:?}");
                assert_eq!(frame(&mut model), before, "{index} {key:?}");
            }
            if word == "paused" {
                for key in [KeyCode::Char(away), away_page] {
                    let mut model = build(1);
                    looped(&mut model, area);

                    let commands = handle(&mut model, pressed(key));
                    assert_eq!(commands, [], "{index} one line {key:?}");
                    assert_eq!(seen(&model), (true, 0), "{index} one line {key:?}");
                }
            }
        }
    }

    fn log_line(seconds: i64, text: &str) -> nobs_core::loki::Line {
        nobs_core::loki::Line {
            nanoseconds: seconds * 1_000_000_000,
            text: text.to_string(),
        }
    }

    fn boxed_entry() -> Model {
        let mut model = devices_model(workshop_located("infrahub"));
        let text: Vec<String> = (1..=12).map(|index| format!("text {index:02}")).collect();
        model.apply(Update::DeviceLogs(
            "srl1".to_string(),
            Ok(vec![
                log_line(2, &text.join("\n")),
                log_line(1, "older entry"),
            ]),
        ));
        open_tab(&mut model, Tab::Timeline);
        focus_tabs(&mut model);
        handle(
            &mut model,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        model
    }

    fn box_title(rows: &[String]) -> String {
        let row = rows.iter().find(|row| row.contains("╔ log")).unwrap();
        row.chars().skip(usize::from(LEFT_NARROW)).collect()
    }

    #[test]
    fn the_box_of_an_entry_moves_its_text_by_one_line_and_says_how_many_lines_are_hidden() {
        let mut model = boxed_entry();
        let title = box_title(&drawn(&mut model, 60, 15));
        let turn = |model: &mut Model, key: char, times: usize| {
            for _ in 0..times {
                press(model, key);
            }
            let rows = drawn(model, 60, 15);
            model.set_popup_overflow(popup_overflow(model, Rect::new(0, 0, 60, 15)));
            let screen = rows.join("\n");
            assert_eq!(box_title(&rows), title, "{screen}");
            assert_eq!(model.timeline_row(), 0, "{screen}");
            let right: Vec<String> = rows
                .iter()
                .map(|row| row.chars().skip(usize::from(LEFT_NARROW)).collect())
                .collect();
            assert!(right.iter().all(|row| !row.contains('>')), "{screen}");
            screen
        };
        let steps = [
            ('j', 0, "↓8", "text 01", "text 12"),
            ('j', 1, "↑1 ↓7", "text 02", "text 01"),
            ('j', 20, "↑8", "text 12", "text 08"),
            ('k', 1, "↑7 ↓1", "text 08", "text 12"),
        ];
        for (key, times, mark, shown, hidden) in steps {
            let screen = turn(&mut model, key, times);

            let edge = format!("═ {mark} ═╝  ");
            assert!(screen.contains(&edge), "{key} {times}: {edge}\n{screen}");
            assert!(screen.contains(shown), "{key} {times}: {shown}\n{screen}");
            assert!(
                !screen.contains(hidden),
                "{key} {times}: {hidden}\n{screen}"
            );
        }
    }

    fn services_log_of(texts: Vec<String>) -> Model {
        let mut model = opened("a");
        model.apply(listing(0, Ok(vec![row("grafana", "running")])));
        press(&mut model, 'l');
        let lines = texts
            .iter()
            .map(|text| format!("2026-11-09T13:53:32Z {text}"))
            .collect();
        model.apply(Update::Logs(0, "grafana".to_string(), Ok(lines)));
        model
    }

    fn device_log_of(count: i64) -> Model {
        let mut model = devices_model(workshop_located("infrahub"));
        let lines = (0..count)
            .map(|index| log_line(index, &format!("line {index:02}")))
            .collect();
        model.apply(Update::DeviceLogs("srl1".to_string(), Ok(lines)));
        open_tab(&mut model, Tab::Logs);
        focus_tabs(&mut model);
        model
    }

    fn forty_lines() -> Vec<String> {
        (0..40).map(|index| format!("line {index:02}")).collect()
    }

    fn log_marked(model: &mut Model, mark: &str) -> String {
        let rows = drawn(model, 60, 15);
        let screen = rows.join("\n");
        let edge = format!("═ {mark} ═╝");
        assert!(
            rows.iter().any(|row| row.ends_with(&edge)),
            "{edge}\n{screen}"
        );
        screen
    }

    #[test]
    fn the_service_log_pane_counts_the_lines_above_and_below() {
        let mut model = services_log_of(forty_lines());
        log_marked(&mut model, "↑32");
        press(&mut model, 'f');

        for (times, mark) in [(3, "↑29 ↓3"), (36, "↓39")] {
            model = moved(model, 'k', times);
            log_marked(&mut model, mark);
        }
    }

    #[test]
    fn the_device_log_pane_counts_the_lines_above_and_below() {
        let mut model = device_log_of(40);
        log_marked(&mut model, "↓32");
        press(&mut model, 'f');

        for (times, mark) in [(3, "↑3 ↓29"), (36, "↑39")] {
            model = moved(model, 'j', times);
            log_marked(&mut model, mark);
        }
    }

    #[test]
    fn in_the_paused_device_log_j_shows_the_older_lines_below_and_k_the_newer_above() {
        for (down, up) in [
            (KeyCode::Char('j'), KeyCode::Char('k')),
            (KeyCode::Down, KeyCode::Up),
        ] {
            let mut model = device_log_of(40);
            press(&mut model, 'f');

            handle(&mut model, KeyEvent::new(down, KeyModifiers::NONE));
            let screen = log_marked(&mut model, "↑1 ↓31");
            assert!(!screen.contains("line 39"), "{down:?}\n{screen}");
            assert!(screen.contains("line 38"), "{down:?}\n{screen}");

            handle(&mut model, KeyEvent::new(up, KeyModifiers::NONE));
            let screen = log_marked(&mut model, "↓32");
            assert!(screen.contains("line 39"), "{up:?}\n{screen}");
        }
    }

    #[test]
    fn a_service_log_line_wider_than_the_pane_ends_with_an_ellipsis() {
        let mut model = services_log_of(vec!["word ".repeat(30)]);

        let rows = drawn(&mut model, 60, 15);

        let row = row_with(&rows, "word word");
        assert!(row.ends_with("…║"), "{}", rows.join("\n"));
    }

    #[test]
    fn the_open_box_keeps_the_entry_that_was_opened_when_newer_entries_arrive() {
        let mut model = boxed_entry();
        let before = drawn(&mut model, 60, 15);

        model.apply(Update::DeviceLogs(
            "srl1".to_string(),
            Ok(vec![log_line(3, "newest entry")]),
        ));
        let after = drawn(&mut model, 60, 15);

        let screen = after.join("\n");
        assert_eq!(model.timeline()[1].text.lines().next(), Some("text 01"));
        assert_eq!(box_title(&after), box_title(&before), "{screen}");
        assert!(screen.contains("text 01"), "{screen}");
        assert!(!screen.contains("newest entry"), "{screen}");
    }

    #[test]
    fn the_key_line_and_the_help_name_tab_and_j_k_for_each_lab_focus_and_the_device_tabs() {
        let lab_line = " Tab focus  j/k move  D deploy  X destroy  P purge  R prepare  U update  B rebuild  S show";
        let mut lab = lab_bench(1);
        assert_eq!(key_line(&lab, 100), [lab_line], "chapters");
        focus_tabs(&mut lab);
        assert_eq!(lab.focus(), Focus::Tabs);
        assert_eq!(key_line(&lab, 100), [lab_line], "nodes");

        let tab_line = " Tab focus  j/k move  [ ] tab  Esc back  ";
        for (tab, start) in [
            (Tab::Overview, tab_line),
            (Tab::Interfaces, tab_line),
            (
                Tab::Timeline,
                " j/k move  Enter full text  - + window  [ ] tab  Esc back  ",
            ),
            (
                Tab::Logs,
                " f pause  / filter  j/k scroll  [ ] tab  Esc back to list  ",
            ),
        ] {
            let mut model = devices_bench();
            open_tab(&mut model, tab);
            focus_tabs(&mut model);
            let line = key_line(&model, 100).concat();
            assert!(line.starts_with(start), "{tab:?}: {line}");
        }
        let boxed = key_line(&boxed_entry(), 100).concat();
        assert!(boxed.starts_with(" j/k move  Esc close  "), "{boxed}");

        let entries = help_entries(&lab_bench(1));
        assert_eq!(entries[1], ("Tab".to_string(), "focus next pane"));
        assert!(entries.contains(&("Esc".to_string(), "close/fold/back")));
        assert!(HELP_LAB
            .iter()
            .all(|(_, meaning)| meaning.chars().count() <= 17));
    }

    #[test]
    fn a_list_longer_than_its_pane_says_how_many_rows_are_above_and_below() {
        type Steps = &'static [(usize, &'static str, &'static str)];
        let cases: [(&str, Build, char, &str, Steps); 10] = [
            (
                "stacks",
                || stacks_of(20),
                'j',
                "╝",
                &[
                    (0, "↓11", ">s01"),
                    (10, "↑2 ↓9", ">s11"),
                    (19, "↑11", ">s20"),
                ],
            ),
            (
                "chapters",
                || lab_bench(10),
                'k',
                "╝",
                &[(0, "↑5", ">ch10"), (3, "↑2 ↓3", ">ch7"), (9, "↓5", ">ch1 ")],
            ),
            (
                "services",
                || services_of(22),
                'j',
                "╝",
                &[(0, "↓15", "s01"), (10, "↑4 ↓11", "s11"), (21, "↑15", "s22")],
            ),
            (
                "devices",
                || devices_of(30),
                'j',
                "╣",
                &[(0, "↓25", "d01"), (10, "↑6 ↓19", "d11"), (29, "↑25", "d30")],
            ),
            (
                "alerts",
                || alerts_of(10),
                'j',
                "╝",
                &[(0, "↓8", "a00"), (5, "↑4 ↓4", "a05"), (9, "↑8", "a09")],
            ),
            (
                "timeline",
                || timeline_of(20),
                'j',
                "╝",
                &[(0, "↓13", ">"), (10, "↑4 ↓9", ">"), (19, "↑13", ">")],
            ),
            (
                "sonda",
                || sonda_of(10),
                'j',
                "╝",
                &[(0, "↓7", "n00"), (5, "↑3 ↓4", "n05"), (9, "↑7", "n09")],
            ),
            (
                "nodes",
                || nodes_of(10),
                'j',
                "╝",
                &[(0, "↓5", ">n01"), (5, "↑1 ↓4", ">n06"), (9, "↑5", ">n10")],
            ),
            (
                "interfaces",
                || interfaces_of(20),
                'j',
                "╝",
                &[
                    (0, "↓14", ">e01"),
                    (10, "↑5 ↓9", ">e11"),
                    (19, "↑14", ">e20"),
                ],
            ),
            (
                "chooser",
                || Model::new(start(many_stacks(20))),
                'j',
                "╝",
                &[
                    (0, "↓10", ">s01"),
                    (10, "↑1 ↓9", ">s11"),
                    (19, "↑10", ">s20"),
                ],
            ),
        ];
        for (label, build, key, corner, steps) in cases {
            for &(times, mark, shown) in steps {
                let mut model = moved(build(), key, times);
                let screen = drawn(&mut model, 60, 15).join("\n");
                let edge = if corner == "╣" {
                    format!(" {mark} ═╣")
                } else {
                    format!("═ {mark} ═╝")
                };

                assert!(screen.contains(&edge), "{label} {times}: {edge}\n{screen}");
                assert!(screen.contains(shown), "{label} {times}: {shown}\n{screen}");
            }
        }
    }

    #[test]
    fn j_on_the_lab_overview_draws_the_hidden_line_and_marks_the_lines_above() {
        let mut model = lab_bench(1);
        press(&mut model, 'D');
        press(&mut model, 'y');
        model.apply(Update::ActionLine(REFUSAL.to_string()));
        model.apply(Update::ActionEnded(End::Exited(1)));
        model.apply(Update::Services(
            0,
            defined(&["grafana"]),
            Err("exit 1".to_string()),
        ));
        focus_tabs(&mut model);
        focus_tabs(&mut model);

        let before = drawn(&mut model, 60, 15);
        model.set_lab_overflow(lab_overflow(&model, Rect::new(0, 0, 60, 15)));
        press(&mut model, 'j');
        let after = drawn(&mut model, 60, 15);

        let screens = [before.join("\n"), after.join("\n")];
        assert!(screens[0].contains(" ps ch1: exit 1"), "{}", screens[0]);
        assert!(!screens[0].contains("║ last"), "{}", screens[0]);
        assert!(bottom(&before).ends_with("═ ↓1 ═╝"), "{}", screens[0]);
        assert!(screens[1].contains("║ last"), "{}", screens[1]);
        assert!(bottom(&after).ends_with("═ ↑1 ═╝"), "{}", screens[1]);
    }

    #[test]
    fn the_answer_of_s_stands_in_the_run_pane_with_its_exit_status() {
        let answers: [(&str, &[&str], &[&str], i32); 2] = [
            (
                "show version",
                &[
                    "Hostname             : srl1",
                    "OS                   : SR Linux",
                ],
                &[],
                0,
            ),
            (
                "show nonsense words",
                &[],
                &["Parsing error: Unknown token 'nonsense'. Options are [...]"],
                1,
            ),
        ];
        for (command, out, error, code) in answers {
            let mut model = lab_devices_model();
            press(&mut model, 's');
            command.chars().for_each(|typed| press(&mut model, typed));
            handle(
                &mut model,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            );
            for line in out {
                model.apply(Update::ActionLine(line.to_string()));
            }
            for line in error {
                model.apply(Update::ActionError(line.to_string()));
            }
            model.apply(Update::ActionEnded(End::Exited(code)));

            let screen = drawn(&mut model, 100, 30).join("\n").replace(NO_BREAK, " ");

            let title = format!(" run  device run nobs-t-srl1 {command}  exit {code}");
            assert!(screen.contains(&title), "{title}\n{screen}");
            for line in out.iter().chain(error) {
                assert!(screen.contains(line), "{line}\n{screen}");
            }
        }
    }

    fn bottom(rows: &[String]) -> String {
        rows.iter()
            .find(|row| row.contains('╚'))
            .cloned()
            .unwrap_or_default()
    }

    fn sonda_details(stats: &str, show: &str) -> Model {
        let mut model = sonda_of(1);
        model.apply(Update::Sonda(data::Update {
            at: Instant::now(),
            list: Ok(scenarios(1)),
            selected: Some((
                "id00".to_string(),
                Ok(show.to_string()),
                Ok(stats.to_string()),
            )),
        }));
        model
    }

    fn details_rows(rows: &[String]) -> Vec<String> {
        rows.iter()
            .filter(|row| !row.contains('╔') && !row.contains('╚'))
            .filter_map(|row| row.split('║').nth(1))
            .map(|row| row.trim_end().to_string())
            .collect()
    }

    #[test]
    fn j_on_the_sonda_details_draws_the_hidden_line_and_marks_the_lines_above() {
        let mut model = sonda_details(r#"{"state":"running"}"#, r#"{"name":"cpu"}"#);
        focus_tabs(&mut model);

        let before = drawn(&mut model, 60, 15);
        model.set_details_overflow(sonda_overflow(&model, Rect::new(0, 0, 60, 15)));
        press(&mut model, 'j');
        let after = drawn(&mut model, 60, 15);

        let screens = [before.join("\n"), after.join("\n")];
        assert_eq!(
            details_rows(&before),
            ["state", "\"running\"", "{", "  \"name\":"],
            "{}",
            screens[0]
        );
        assert!(bottom(&before).contains("═ ↓2 ═╝"), "{}", screens[0]);
        assert_eq!(
            details_rows(&after),
            ["\"running\"", "{", "  \"name\":", "\"cpu\""],
            "{}",
            screens[1]
        );
        assert!(bottom(&after).contains("═ ↑1 ↓1 ═╝"), "{}", screens[1]);
    }

    #[test]
    fn a_wide_details_line_goes_on_in_the_next_line_and_loses_no_letter() {
        let path = "/srv/sonda/scenarios/interface-flap.yaml";
        let mut model = sonda_details(
            r#"{"current_rate":1.0,"total_events":3359}"#,
            &format!(r#"{{"file":"{path}"}}"#),
        );
        focus_tabs(&mut model);
        let mut rows = details_rows(&drawn(&mut model, 60, 15));
        let overflow = sonda_overflow(&model, Rect::new(0, 0, 60, 15));
        model.set_details_overflow(overflow);
        for _ in 0..overflow {
            press(&mut model, 'j');
            rows.extend(details_rows(&drawn(&mut model, 60, 15)).pop());
        }

        let joined = rows.concat().replace(' ', "");
        for text in ["current_rate1.0", "total_events3359", path] {
            assert!(joined.contains(text), "{text}\n{}", rows.join("\n"));
        }
    }

    fn doubles(rows: &[String]) -> Vec<String> {
        rows.iter()
            .flat_map(|row| row.split('╔').skip(1))
            .map(|after| {
                after
                    .split('═')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            })
            .collect()
    }

    fn one_double(rows: &[String], title: &str) -> bool {
        matches!(&doubles(rows)[..], [shown] if shown.starts_with(title))
    }

    #[test]
    fn a_box_on_top_draws_the_double_border_and_the_pane_under_it_the_single_one() {
        let escape = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let builds: [(Build, &str); 3] = [
            (services_bench, "stacks"),
            (devices_bench, "devices"),
            (|| lab_bench(2), "chapters"),
        ];
        for (build, under) in builds {
            let mut model = build();
            press(&mut model, '?');
            for (width, height) in [(60, 15), (100, 30)] {
                let rows = drawn(&mut model, width, height);
                assert!(one_double(&rows, "keys"), "{under}\n{}", rows.join("\n"));
            }
            handle(&mut model, escape);
            for (width, height) in [(60, 15), (100, 30)] {
                let rows = drawn(&mut model, width, height);
                assert!(one_double(&rows, under), "{}", rows.join("\n"));
            }
        }

        let mut model = devices_bench();
        open_tab(&mut model, Tab::Timeline);
        focus_tabs(&mut model);
        handle(
            &mut model,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        for (width, height) in [(60, 15), (100, 30)] {
            let rows = drawn(&mut model, width, height);
            let screen = rows.join("\n");
            assert!(one_double(&rows, "log"), "{screen}");
            assert!(screen.contains("┌ srl1 "), "{screen}");
        }
        handle(&mut model, escape);
        for (width, height) in [(60, 15), (100, 30)] {
            let rows = drawn(&mut model, width, height);
            assert!(one_double(&rows, "srl1"), "{}", rows.join("\n"));
        }
    }

    #[test]
    fn in_the_sonda_tab_the_inner_box_with_the_focus_draws_the_double_border_and_the_pane_the_single_one(
    ) {
        let mut sonda = sonda_details(r#"{"state":"running"}"#, r#"{"name":"cpu"}"#);
        let mut seen = Vec::new();
        let mut screens = Vec::new();
        for _ in 0..3 {
            for width in [60, 100] {
                let height = if width == 60 { 15 } else { 30 };
                let rows = drawn(&mut sonda, width, height);
                screens.push(rows.join("\n"));
                let tab_row = rows
                    .iter()
                    .find_map(|row| row.split_once("│q quit"))
                    .map(|(_, row)| format!("q quit{}", row.trim_end_matches('│').trim_end()));
                let scenarios = rows.iter().any(|row| row.contains("│╔"));
                let details = rows.iter().any(|row| row.ends_with("═╗│"));
                seen.push((sonda.focus(), doubles(&rows), scenarios, details, tab_row));
            }
            if sonda.focus() == Focus::Lines {
                assert_eq!(
                    key_line(&sonda, 60),
                    [" Tab focus  j/k move  [ ] tab  Esc back"]
                );
            }
            focus_tabs(&mut sonda);
        }

        let row = |text: &str| Some(text.to_string());
        let untitled = || vec![String::new()];
        let stacks = || vec!["stacks 1".to_string()];
        assert_eq!(
            seen,
            [
                (
                    Focus::Tabs,
                    untitled(),
                    true,
                    false,
                    row("q quit  j/k select  r refresh")
                ),
                (
                    Focus::Tabs,
                    untitled(),
                    true,
                    false,
                    row("q quit  j/k select  r refresh  l load  d delete")
                ),
                (
                    Focus::Lines,
                    untitled(),
                    false,
                    true,
                    row("q quit  r refresh  l load")
                ),
                (
                    Focus::Lines,
                    untitled(),
                    false,
                    true,
                    row("q quit  r refresh  l load  d delete")
                ),
                (Focus::List, stacks(), false, false, None),
                (Focus::List, stacks(), false, false, None),
            ],
            "{}",
            screens.join("\n\n")
        );

        press(&mut sonda, '?');
        let help = drawn(&mut sonda, 60, 15).join("\n");
        assert!(
            help.contains("║sonda tab: Tab details  l load  d delete  r refresh"),
            "{help}"
        );
    }

    #[test]
    fn a_question_or_the_filter_leaves_the_double_border_on_the_box_that_had_it() {
        let cases: [(Build, char, &str, &str); 2] = [
            (services_bench, 'u', "stack", "stacks"),
            (devices_bench, 'm', "srl", "devices"),
        ];
        for (build, question, filter, title) in cases {
            let mut asked = build();
            press(&mut asked, question);
            assert!(asked.question().is_some(), "{title}");
            let mut filtered = build();
            press(&mut filtered, '/');
            filter.chars().for_each(|typed| press(&mut filtered, typed));
            assert!(filtered.typed_filter().is_some(), "{title}");
            for mut model in [build(), asked, filtered] {
                let rows = drawn(&mut model, 60, 15);

                assert!(one_double(&rows, title), "{}", rows.join("\n"));
            }
        }
    }

    #[test]
    fn the_chooser_draws_the_double_border() {
        let mut model = Model::new(start(many_stacks(3)));
        for (width, height) in [(60, 15), (100, 30)] {
            let rows = drawn(&mut model, width, height);

            assert_eq!(doubles(&rows), ["labs found"], "{}", rows.join("\n"));
        }
    }

    #[test]
    fn the_nodes_and_interfaces_mark_shows_only_with_the_focus_on_their_pane() {
        let mut model = lab_bench(1);
        let screen = drawn(&mut model, 60, 15).join("\n");
        assert!(!screen.contains(">nobs-t-srl1"), "{screen}");
        assert!(screen.contains(">ch1"), "{screen}");

        focus_tabs(&mut model);
        let screen = drawn(&mut model, 60, 15).join("\n");
        assert!(screen.contains(">nobs-t-srl1"), "{screen}");
        assert!(screen.contains(">ch1"), "{screen}");

        let mut model = devices_model(workshop_located("infrahub"));
        open_tab(&mut model, Tab::Interfaces);
        let screen = drawn(&mut model, 60, 15).join("\n");
        assert!(!screen.contains(">ethernet"), "{screen}");

        focus_tabs(&mut model);
        let screen = drawn(&mut model, 60, 15).join("\n");
        assert!(screen.contains(">ethernet-1/1 "), "{screen}");
    }

    #[test]
    fn a_list_whose_rows_all_fit_has_no_mark() {
        let builds: [(&str, Build); 7] = [
            ("stacks and services", || stacks_of(3)),
            ("chooser", || Model::new(start(many_stacks(3)))),
            ("devices and alerts", || {
                devices_model(workshop_located("infrahub"))
            }),
            ("timeline", || {
                let mut model = devices_bench();
                open_tab(&mut model, Tab::Timeline);
                model
            }),
            ("interfaces", || {
                let mut model = devices_model(workshop_located("infrahub"));
                open_tab(&mut model, Tab::Interfaces);
                model
            }),
            ("sonda", || {
                let mut model = opened("a");
                model.apply(every_service_up(0));
                sonda_rows(&mut model);
                open_tab(&mut model, Tab::Sonda);
                model
            }),
            ("nodes and chapters", || lab_bench(2)),
        ];
        for (label, build) in builds {
            for (width, height) in [(60, 15), (100, 30)] {
                let screen = drawn(&mut build(), width, height).join("\n");

                assert!(
                    !screen.contains(['↑', '↓']),
                    "{label} {width}x{height}\n{screen}"
                );
            }
        }
    }

    #[test]
    fn the_mark_fits_beside_alerts_n_at_60_columns_for_every_count_under_1000() {
        let windows = [
            ((99, 1, 199), "↑99 ↓99"),
            ((0, 1, 1000), "↓999"),
            ((999, 1, 1000), "↑999"),
        ];
        let lines = [
            (border(false), '├', '─', '┤', "├ alerts 999 ─────────┤"),
            (border(true), '╠', '═', '╣', "╠ alerts 999 ═════════╣"),
        ];
        for (border, left, fill, right, whole) in lines {
            for count in 0..1000 {
                for (window, mark) in windows {
                    let rows = painted(LEFT_NARROW, 1, |frame| {
                        alerts_separator(
                            frame,
                            frame.area(),
                            &count.to_string(),
                            Some(window),
                            border,
                        );
                    });

                    assert!(
                        rows[0].starts_with(&format!("{left} alerts {count} ")),
                        "{}",
                        rows[0]
                    );
                    assert!(
                        rows[0].ends_with(&format!(" {mark} {fill}{right}")),
                        "{}",
                        rows[0]
                    );
                }
            }
            let rows = painted(LEFT_NARROW, 1, |frame| {
                alerts_separator(frame, frame.area(), "999", Some((100, 1, 200)), border);
            });
            assert_eq!(rows[0], whole);
        }
    }

    fn settled(model: &mut Model, width: u16, height: u16) -> Vec<String> {
        model.tick(model.now(), UNIX_EPOCH, (width, height));
        crate::settle(model, Rect::new(0, 0, width, height));
        painted(width, height, |frame| draw(frame, model))
    }

    fn tab_to_run(model: &mut Model, width: u16, height: u16) {
        settled(model, width, height);
        for _ in 0..4 {
            if model.focus() == Focus::Run {
                break;
            }
            handle(model, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        }
        assert_eq!(model.focus(), Focus::Run, "{width}x{height}");
    }

    fn answered(lines: &[String]) -> Model {
        let mut model = running_with(lines);
        model.apply(Update::ActionEnded(End::Exited(0)));
        model
    }

    fn running_with(lines: &[String]) -> Model {
        let mut model = lab_devices_model();
        press(&mut model, 's');
        "show version"
            .chars()
            .for_each(|typed| press(&mut model, typed));
        handle(
            &mut model,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        for line in lines {
            model.apply(Update::ActionLine(line.clone()));
        }
        model
    }

    fn numbered(count: usize) -> Vec<String> {
        (1..=count)
            .map(|index| format!("line {index:02}"))
            .collect()
    }

    fn heights(model: &Model, width: u16, height: u16) -> (u16, u16) {
        let (_, [_, _, _, body, bottom, _]) = regions(model, Rect::new(0, 0, width, height));
        (bottom.height, body.height)
    }

    fn pane_text(rows: &[String]) -> Vec<String> {
        details_rows(rows)
            .into_iter()
            .map(|row| row.replace(NO_BREAK, " "))
            .collect()
    }

    #[test]
    fn the_run_pane_with_the_focus_draws_the_only_double_border() {
        for bench in benches() {
            for (width, height) in [(60, 15), (100, 30)] {
                let mut model = staged(&bench, bench.tabs[0].0, false, Keys::Hints, true);
                let before = settled(&mut model, width, height);
                tab_to_run(&mut model, width, height);
                let rows = settled(&mut model, width, height);

                assert!(
                    !doubles(&before)
                        .iter()
                        .any(|shown| shown.starts_with("run")),
                    "{}",
                    before.join("\n")
                );
                assert!(one_double(&rows, "run"), "{}", rows.join("\n"));
            }
        }
    }

    #[test]
    fn a_workbench_without_body_rows_leaves_every_cell_of_the_run_pane_to_the_pane() {
        let sizes = [(60, 15), (100, 30)];
        let mut cases = 0;
        let mut check = |label: String, tab: Tab, width: u16, height: u16, mut model: Model| {
            for line in numbered(60) {
                model.apply(Update::ActionLine(line));
            }
            tab_to_run(&mut model, width, height);
            settled(&mut model, width, height);
            let (_, [_, _, _, body, pane, _]) = regions(&model, Rect::new(0, 0, width, height));
            let cells = |paint: &dyn Fn(&mut Frame)| {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| paint(frame)).unwrap();
                let buffer = terminal.backend().buffer();
                pane.positions()
                    .map(|at| (buffer[at].symbol().to_string(), buffer[at].style()))
                    .collect::<Vec<_>>()
            };
            let screen = cells(&|frame| draw(frame, &model));
            let alone = cells(&|frame| run_pane(frame, pane, &model));

            assert_eq!(model.tab(), tab, "{label}");
            assert_eq!(model.focus(), Focus::Run, "{label}");
            assert_eq!(body.height, 0, "{label}");
            assert!(pane.height > 2, "{label}");
            assert_eq!(screen, alone, "{label}");
            cases += 1;
        };
        for workbench in WORKBENCHES {
            let mut set_up = lab_devices_model().workbench() == workbench;
            for bench in benches() {
                if (bench.build)().workbench() != workbench {
                    continue;
                }
                set_up = true;
                let tabs: &[Tab] = match workbench {
                    Workbench::Devices => (bench.build)().device_tabs(),
                    Workbench::Services => &SERVICES_TABS,
                    Workbench::Lab => &[Tab::Overview],
                    Workbench::Observability => &OBSERVABILITY_TABS,
                };
                for &tab in tabs {
                    for (width, height) in sizes {
                        let label = format!("{} {tab:?} {width}x{height}", bench.selected);
                        let model = staged(&bench, tab, false, Keys::Hints, true);
                        check(label, tab, width, height, model);
                    }
                }
            }
            if workbench == Workbench::Observability {
                set_up = true;
                for (width, height) in sizes {
                    let label = format!("observability {width}x{height}");
                    let mut model = staged(&benches()[0], Tab::Overview, false, Keys::Hints, true);
                    press(&mut model, '3');
                    check(label, Tab::Alerts, width, height, model);
                }
            }
            assert!(set_up, "no setup for {workbench:?}");
        }
        for &tab in lab_devices_model().device_tabs() {
            for (width, height) in sizes {
                let label = format!("lab devices {tab:?} {width}x{height}");
                let mut model = lab_devices_model();
                press(&mut model, 'g');
                for line in numbered(60) {
                    model.apply(Update::ActionLine(line));
                }
                model.apply(Update::ActionEnded(End::Exited(0)));
                open_tab(&mut model, tab);
                assert!(
                    matches!(model.config(), Some((_, Some(kept), _, _)) if kept.lines.len() == 60),
                    "{label}"
                );
                check(label, tab, width, height, model);
            }
        }
        assert_eq!(cases, 32);
    }

    #[test]
    fn the_marks_of_the_run_pane_count_the_rows_hidden_above_and_below() {
        let mut model = answered(&numbered(40));
        tab_to_run(&mut model, 60, 15);
        let following = settled(&mut model, 60, 15);
        let run = model.run().unwrap();
        let total = command_rows(run, 58).len() + 40;
        let room = usize::from(heights(&model, 60, 15).0 - 2);
        let hidden = total - room;
        for _ in 0..3 {
            press(&mut model, 'k');
        }
        let moved = settled(&mut model, 60, 15);
        for _ in 0..hidden {
            press(&mut model, 'k');
        }
        let top = settled(&mut model, 60, 15);

        assert!(
            bottom(&following).ends_with(&format!("═ ↑{hidden} ═╝")),
            "{}",
            following.join("\n")
        );
        assert!(
            bottom(&moved).ends_with(&format!("═ ↑{} ↓3 ═╝", hidden - 3)),
            "{}",
            moved.join("\n")
        );
        assert!(
            bottom(&top).ends_with(&format!("═ ↓{hidden} ═╝")),
            "{}",
            top.join("\n")
        );
        assert!(!bottom(&top).contains('↑'), "{}", top.join("\n"));
    }

    #[test]
    fn with_the_focus_the_run_pane_grows_to_its_rows_up_to_the_whole_middle() {
        for (width, height) in [(100, 30), (60, 15)] {
            for count in [1, 10, 40] {
                let label = format!("{width}x{height} {count} lines");
                let mut model = answered(&numbered(count));
                settled(&mut model, width, height);
                let (main, main_body) = heights(&model, width, height);
                let least = workbench_least(&model);
                let middle = main + main_body;
                let want =
                    command_rows(model.run().unwrap(), usize::from(width - 2)).len() + count + 2;
                tab_to_run(&mut model, width, height);
                let rows = settled(&mut model, width, height);
                let (pane, body) = heights(&model, width, height);
                handle(&mut model, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
                settled(&mut model, width, height);
                let tabbed = heights(&model, width, height);
                tab_to_run(&mut model, width, height);
                handle(&mut model, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                settled(&mut model, width, height);
                let folded = heights(&model, width, height).0;
                press(&mut model, 'o');
                settled(&mut model, width, height);
                let opened = heights(&model, width, height);

                assert!(main_body >= least, "{label}");
                assert_eq!(pane + body, middle, "{label}");
                assert!(body == 0 || body >= least, "{label}");
                match (count, width) {
                    (1, _) => {
                        assert!(want <= usize::from(main), "{label}");
                        assert_eq!(pane, main, "{label}");
                    }
                    (10, 100) => assert_eq!(usize::from(pane), want, "{label}"),
                    _ => assert_eq!((pane, body), (middle, 0), "{label}"),
                }
                if body == 0 {
                    assert!(rows[2].starts_with('╔'), "{label}\n{}", rows.join("\n"));
                }
                assert_eq!(tabbed, (main, main_body), "{label}");
                assert_eq!((folded, model.focus()), (1, Focus::List), "{label}");
                assert_eq!(opened, (main, main_body), "{label}");
            }
        }
    }

    #[test]
    fn a_run_pane_that_is_not_drawn_gives_the_focus_to_the_list_through_settle() {
        let mut model = answered(&numbered(2));
        tab_to_run(&mut model, 100, 30);
        settled(&mut model, 59, 15);
        let small = model.focus();
        settled(&mut model, 100, 30);
        assert_eq!((small, model.focus()), (Focus::List, Focus::List));

        let chooser = Model::new(start(vec![stack("a", &[]), stack("b", &[])]));
        assert!(chooser.chooser());
        assert!(run_view(&chooser, Rect::new(0, 0, 100, 30)).is_none());

        let services = benches()
            .into_iter()
            .find(|bench| bench.tabs.iter().any(|(tab, _)| *tab == Tab::Sonda))
            .unwrap();
        let mut line = staged(&services, Tab::Sonda, true, Keys::Hints, true);
        settled(&mut line, 60, 15);
        assert_eq!(heights(&line, 60, 15).0, 1);
        assert!(run_view(&line, Rect::new(0, 0, 60, 15)).is_none());
        let mut stops = Vec::new();
        for _ in 0..4 {
            handle(&mut line, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
            settled(&mut line, 60, 15);
            stops.push(line.focus());
        }
        assert!(!stops.contains(&Focus::Run), "{stops:?}");
    }

    #[test]
    fn with_the_focus_every_row_of_the_run_pane_and_of_a_wrapped_line_comes_into_view() {
        let long = (1..=30)
            .map(|index| format!("word{index:02}"))
            .collect::<Vec<_>>()
            .join(" ");
        let mut lines = numbered(20);
        lines.insert(10, long.clone());
        let mut model = answered(&lines);
        tab_to_run(&mut model, 60, 15);
        settled(&mut model, 60, 15);
        let run = model.run().unwrap();
        let mut expected: Vec<String> = command_rows(run, 58);
        for line in &lines {
            expected.extend(wrapped(line, 58));
        }
        let expected: Vec<String> = expected
            .iter()
            .map(|row| row.replace(NO_BREAK, " ").trim_end().to_string())
            .collect();
        for _ in 0..expected.len() {
            press(&mut model, 'k');
        }

        let mut seen = Vec::new();
        for _ in 0..expected.len() {
            let rows = pane_text(&settled(&mut model, 60, 15));
            let scroll = model.run_scroll();
            press(&mut model, 'j');
            if model.run_scroll() == scroll {
                seen.extend(rows);
                break;
            }
            seen.push(rows[0].clone());
        }

        assert!(wrapped(&long, 58).len() > 1);
        assert_eq!(seen, expected);
    }

    #[test]
    fn a_moved_up_run_pane_keeps_its_rows_when_wrapped_lines_leave_at_the_limit() {
        let long = "gone ".repeat(40);
        let mut lines = vec![long.clone(); 3];
        lines.extend(numbered(1997));
        let mut model = running_with(&lines);
        tab_to_run(&mut model, 60, 15);
        settled(&mut model, 60, 15);
        for _ in 0..30 {
            press(&mut model, 'k');
        }
        let before = pane_text(&settled(&mut model, 60, 15));
        for index in 0..3 {
            model.apply(Update::ActionLine(format!("new {index}")));
        }
        let after = pane_text(&settled(&mut model, 60, 15));

        assert!(wrapped(&long, 58).len() > 1);
        assert_eq!(model.run().unwrap().lines[0], "line 01");
        assert_eq!(after, before);
    }

    #[test]
    fn with_the_focus_in_the_run_pane_the_key_line_holds_its_keys_and_f_only_while_moved_up() {
        let at_end = " j/k scroll  Tab focus  Esc fold";
        let moved_up = " j/k scroll  f follow  Tab focus  Esc fold";
        for bench in benches() {
            for (width, height) in [(60, 15), (100, 30)] {
                let mut model = staged(&bench, bench.tabs[0].0, false, Keys::Hints, true);
                tab_to_run(&mut model, width, height);
                let rows = settled(&mut model, width, height);
                assert_eq!(rows[rows.len() - 1], at_end, "{}", rows.join("\n"));
            }
        }
        let mut model = answered(&numbered(40));
        tab_to_run(&mut model, 60, 15);
        let following = settled(&mut model, 60, 15);
        press(&mut model, 'k');
        let moved = settled(&mut model, 60, 15);
        press(&mut model, 'j');
        let back = settled(&mut model, 60, 15);

        assert_eq!(following[14], at_end);
        assert_eq!(moved[14], moved_up);
        assert_eq!(back[14], at_end);
    }

    #[test]
    fn the_keys_overlay_names_the_run_panes_keys_only_while_the_pane_is_drawn() {
        let named = |rows: &[String]| rows.iter().any(|row| row.contains(HELP_RUN));
        for bench in benches() {
            for (width, height) in [(60, 15), (100, 30)] {
                let mut model = staged(&bench, bench.tabs[0].0, false, Keys::Hints, true);
                settled(&mut model, width, height);
                press(&mut model, '?');
                let open = settled(&mut model, width, height);
                press(&mut model, '?');
                press(&mut model, 'o');
                settled(&mut model, width, height);
                press(&mut model, '?');
                let folded = settled(&mut model, width, height);

                assert!(named(&open), "{}", open.join("\n"));
                assert!(!named(&folded), "{}", folded.join("\n"));
            }
        }
    }
}
