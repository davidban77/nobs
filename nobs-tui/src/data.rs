use std::fs;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use nobs_core::sonda::{check_runnable, scenario_rows, Reply, ScenarioRow, Sonda};

const NOTE_TIME: Duration = Duration::from_secs(3);

pub struct Model {
    server: String,
    rows: Vec<ScenarioRow>,
    selected: Option<usize>,
    stats: Option<String>,
    show: Option<String>,
    refreshed_at: Option<Instant>,
    error: Option<String>,
    prompt: Prompt,
    note: Option<Note>,
    quit: bool,
}

#[derive(Debug)]
pub struct Update {
    pub at: Instant,
    pub list: Result<String, String>,
    #[allow(clippy::type_complexity)]
    pub selected: Option<(String, Result<String, String>, Result<String, String>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Refresh,
    Select(String),
    Load(String),
    Delete(String),
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    None,
    Path(String),
    Delete(String),
}

pub struct Poller {
    commands: Sender<Command>,
    thread: JoinHandle<()>,
}

#[derive(Debug)]
pub struct Note {
    pub at: Instant,
    pub text: String,
}

impl Model {
    pub fn new(server: String) -> Self {
        Self {
            server,
            rows: Vec::new(),
            selected: None,
            stats: None,
            show: None,
            refreshed_at: None,
            error: None,
            prompt: Prompt::None,
            note: None,
            quit: false,
        }
    }

    pub fn apply(&mut self, update: Update) {
        let rows = update.list.and_then(|body| {
            scenario_rows(&body)
                .map_err(|reason| format!("sonda answered a list nobs cannot read: {reason}"))
        });
        match rows {
            Ok(rows) => {
                let before = self.selected().map(|row| row.id.clone());
                self.rows = rows;
                self.selected = pick(&self.rows, before.as_deref());
                if self.selected().map(|row| &row.id) != before.as_ref() {
                    self.stats = None;
                    self.show = None;
                }
                self.refreshed_at = Some(update.at);
                self.error = None;
            }
            Err(text) => self.error = Some(text),
        }
        let Some((id, show, stats)) = update.selected else {
            return;
        };
        if self.selected().is_none_or(|row| row.id != id) {
            return;
        }
        match show {
            Ok(body) => self.show = Some(body),
            Err(text) => self.error = Some(text),
        }
        match stats {
            Ok(body) => self.stats = Some(body),
            Err(text) => self.error = Some(text),
        }
    }

    pub fn handle(&mut self, key: KeyEvent) -> Vec<Command> {
        if key.kind != KeyEventKind::Press {
            return Vec::new();
        }
        match std::mem::replace(&mut self.prompt, Prompt::None) {
            Prompt::Path(mut path) => match key.code {
                KeyCode::Enter => return vec![Command::Load(path)],
                KeyCode::Esc => {}
                KeyCode::Backspace => {
                    path.pop();
                    self.prompt = Prompt::Path(path);
                }
                KeyCode::Char(typed) => {
                    path.push(typed);
                    self.prompt = Prompt::Path(path);
                }
                _ => self.prompt = Prompt::Path(path),
            },
            Prompt::Delete(id) => {
                if key.code == KeyCode::Char('y') {
                    return vec![Command::Delete(id)];
                }
            }
            Prompt::None => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
                KeyCode::Char('j') | KeyCode::Down => return self.step(1),
                KeyCode::Char('k') | KeyCode::Up => return self.step(-1),
                KeyCode::Char('r') => return vec![Command::Refresh],
                KeyCode::Char('l') => self.prompt = Prompt::Path(String::new()),
                KeyCode::Char('d') => {
                    if let Some(row) = self.selected() {
                        self.prompt = Prompt::Delete(row.id.clone());
                    }
                }
                _ => {}
            },
        }
        Vec::new()
    }

    pub fn note(&mut self, text: String, at: Instant) {
        self.note = Some(Note { at, text });
    }

    pub fn rows(&self) -> &[ScenarioRow] {
        &self.rows
    }

    pub fn selected(&self) -> Option<&ScenarioRow> {
        self.selected.and_then(|index| self.rows.get(index))
    }

    pub fn stats_body(&self) -> Option<&str> {
        self.stats.as_deref()
    }

    pub fn show_body(&self) -> Option<&str> {
        self.show.as_deref()
    }

    pub fn refreshed_at(&self) -> Option<Instant> {
        self.refreshed_at
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn prompt(&self) -> &Prompt {
        &self.prompt
    }

    pub fn status(&self, now: Instant) -> Option<&str> {
        self.note
            .as_ref()
            .filter(|note| now.saturating_duration_since(note.at) < NOTE_TIME)
            .map(|note| note.text.as_str())
    }

    pub fn quit(&self) -> bool {
        self.quit
    }

    pub(crate) fn server(&self) -> &str {
        &self.server
    }

    pub(crate) fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    fn step(&mut self, by: isize) -> Vec<Command> {
        let Some(next) = self
            .selected
            .and_then(|index| index.checked_add_signed(by))
            .filter(|next| *next < self.rows.len())
        else {
            return Vec::new();
        };
        self.selected = Some(next);
        self.stats = None;
        self.show = None;
        vec![Command::Select(self.rows[next].id.clone())]
    }
}

impl Poller {
    pub fn start(sonda: Sonda, period: Duration) -> (Poller, Receiver<Update>, Receiver<Note>) {
        let (commands, inbox) = mpsc::channel();
        let (updates, update_out) = mpsc::channel();
        let (notes, note_out) = mpsc::channel();
        let thread = thread::spawn(move || poll(&sonda, period, &inbox, &updates, &notes));
        (Poller { commands, thread }, update_out, note_out)
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub fn stop(self) {
        self.send(Command::Stop);
        let _ = self.thread.join();
    }
}

fn pick(rows: &[ScenarioRow], id: Option<&str>) -> Option<usize> {
    if rows.is_empty() {
        return None;
    }
    Some(
        id.and_then(|id| rows.iter().position(|row| row.id == id))
            .unwrap_or(0),
    )
}

fn poll(
    sonda: &Sonda,
    period: Duration,
    inbox: &Receiver<Command>,
    updates: &Sender<Update>,
    notes: &Sender<Note>,
) {
    let mut selected: Option<String> = None;
    loop {
        let list = body(sonda.list());
        let details = match list.as_deref().map(scenario_rows) {
            Ok(Ok(rows)) => {
                selected = pick(&rows, selected.as_deref()).map(|index| rows[index].id.clone());
                selected.clone().map(|id| {
                    let show = body(sonda.show(&id));
                    let stats = body(sonda.stats(&id));
                    (id, show, stats)
                })
            }
            _ => None,
        };
        let update = Update {
            at: Instant::now(),
            list,
            selected: details,
        };
        if updates.send(update).is_err() {
            return;
        }
        let next = Instant::now() + period;
        loop {
            match inbox.recv_timeout(next.saturating_duration_since(Instant::now())) {
                Ok(Command::Select(id)) => selected = Some(id),
                Ok(Command::Load(path)) => send_note(notes, load(sonda, &path)),
                Ok(Command::Delete(id)) => send_note(notes, note_text(sonda.delete(&id))),
                Ok(Command::Refresh) | Err(RecvTimeoutError::Timeout) => break,
                Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

fn body(result: Result<Reply, String>) -> Result<String, String> {
    match result {
        Ok(reply) if reply.status >= 400 => Err(format!(
            "sonda answered {}: {}",
            reply.status,
            reply.body.trim()
        )),
        Ok(reply) => Ok(reply.body),
        Err(text) => Err(text),
    }
}

fn load(sonda: &Sonda, path: &str) -> String {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => return format!("nobs: {path}: {error}"),
    };
    if let Err(reason) = check_runnable(&text) {
        return format!("nobs: {path}: {reason}");
    }
    note_text(sonda.load(text.as_bytes()))
}

fn note_text(result: Result<Reply, String>) -> String {
    let (Ok(text) | Err(text)) = body(result);
    text.trim().lines().collect::<Vec<_>>().join(" ")
}

fn send_note(notes: &Sender<Note>, text: String) {
    let _ = notes.send(Note {
        at: Instant::now(),
        text,
    });
}

#[cfg(test)]
#[path = "../../tests/support/stub.rs"]
mod stub;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::mpsc::TryRecvError;

    use crossterm::event::KeyModifiers;

    use super::stub::Stub;
    use super::*;

    const PERIOD: Duration = Duration::from_millis(300);
    const WAIT: Duration = Duration::from_secs(5);
    const TWO: &str = r#"{"scenarios":[{"id":"a1","name":"cpu","state":"running"},{"id":"b2","name":"mem","state":"paused"}]}"#;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn list(ids: &[&str]) -> String {
        let rows: Vec<String> = ids
            .iter()
            .map(|id| format!(r#"{{"id":"{id}","name":"n-{id}","state":"running"}}"#))
            .collect();
        format!(r#"{{"scenarios":[{}]}}"#, rows.join(","))
    }

    fn good(ids: &[&str]) -> Update {
        Update {
            at: Instant::now(),
            list: Ok(list(ids)),
            selected: None,
        }
    }

    fn model(ids: &[&str]) -> Model {
        let mut model = Model::new("http://sonda".to_string());
        model.apply(good(ids));
        model
    }

    fn ids(model: &Model) -> Vec<&str> {
        model.rows().iter().map(|row| row.id.as_str()).collect()
    }

    fn selected(model: &Model) -> Option<&str> {
        model.selected().map(|row| row.id.as_str())
    }

    fn fixture(name: &str) -> String {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/sonda")
            .join(name)
            .to_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn apply_with_a_good_list_fills_the_rows_and_the_time_and_clears_the_error() {
        let mut model = Model::new("http://sonda".to_string());
        model.apply(Update {
            at: Instant::now(),
            list: Err("cannot reach http://sonda: refused".to_string()),
            selected: None,
        });
        let update = good(&["a1", "b2"]);
        let at = update.at;

        model.apply(update);

        assert_eq!(ids(&model), ["a1", "b2"]);
        assert_eq!(model.refreshed_at(), Some(at));
        assert_eq!(model.error(), None);
    }

    #[test]
    fn apply_with_an_error_list_keeps_the_rows_and_records_the_error() {
        let mut model = model(&["a1", "b2"]);
        let at = model.refreshed_at();

        model.apply(Update {
            at: Instant::now(),
            list: Err("cannot reach http://sonda: refused".to_string()),
            selected: None,
        });

        assert_eq!(ids(&model), ["a1", "b2"]);
        assert_eq!(model.refreshed_at(), at);
        assert_eq!(model.error(), Some("cannot reach http://sonda: refused"));
    }

    #[test]
    fn the_selection_is_kept_by_id_and_falls_to_the_first_row_when_the_id_is_gone() {
        let mut model = model(&["a1", "b2"]);
        model.handle(key(KeyCode::Char('j')));

        model.apply(good(&["c3", "b2"]));
        assert_eq!(selected(&model), Some("b2"));

        model.apply(good(&["c3", "d4"]));
        assert_eq!(selected(&model), Some("c3"));

        model.apply(good(&[]));
        assert_eq!(selected(&model), None);
    }

    #[test]
    fn q_and_esc_set_quit() {
        for code in [KeyCode::Char('q'), KeyCode::Esc] {
            let mut model = model(&["a1"]);

            assert_eq!(model.handle(key(code)), []);

            assert!(model.quit(), "{code:?}");
        }
    }

    #[test]
    fn j_and_k_move_the_selection_within_bounds() {
        let mut model = model(&["a1", "b2", "c3"]);

        assert_eq!(model.handle(key(KeyCode::Char('k'))), []);
        assert_eq!(
            model.handle(key(KeyCode::Char('j'))),
            [Command::Select("b2".to_string())]
        );
        assert_eq!(
            model.handle(key(KeyCode::Down)),
            [Command::Select("c3".to_string())]
        );
        assert_eq!(model.handle(key(KeyCode::Char('j'))), []);
        assert_eq!(selected(&model), Some("c3"));
        assert_eq!(
            model.handle(key(KeyCode::Char('k'))),
            [Command::Select("b2".to_string())]
        );
        assert_eq!(
            model.handle(key(KeyCode::Up)),
            [Command::Select("a1".to_string())]
        );
    }

    #[test]
    fn r_asks_for_a_refresh() {
        assert_eq!(
            model(&["a1"]).handle(key(KeyCode::Char('r'))),
            [Command::Refresh]
        );
    }

    #[test]
    fn l_then_typed_text_then_enter_gives_load_and_closes_the_prompt() {
        let mut model = model(&["a1"]);
        model.handle(key(KeyCode::Char('l')));
        for typed in "q.yamlx".chars() {
            model.handle(key(KeyCode::Char(typed)));
        }
        model.handle(key(KeyCode::Backspace));
        assert_eq!(model.prompt(), &Prompt::Path("q.yaml".to_string()));

        let commands = model.handle(key(KeyCode::Enter));

        assert_eq!(commands, [Command::Load("q.yaml".to_string())]);
        assert_eq!(model.prompt(), &Prompt::None);
        assert!(!model.quit());
    }

    #[test]
    fn l_then_esc_gives_nothing_and_closes_the_prompt() {
        let mut model = model(&["a1"]);
        model.handle(key(KeyCode::Char('l')));
        model.handle(key(KeyCode::Char('x')));

        assert_eq!(model.handle(key(KeyCode::Esc)), []);

        assert_eq!(model.prompt(), &Prompt::None);
        assert!(!model.quit());
    }

    #[test]
    fn d_then_y_gives_delete_of_the_selected_row() {
        let mut model = model(&["a1", "b2"]);
        model.handle(key(KeyCode::Char('j')));
        model.handle(key(KeyCode::Char('d')));
        assert_eq!(model.prompt(), &Prompt::Delete("b2".to_string()));

        let commands = model.handle(key(KeyCode::Char('y')));

        assert_eq!(commands, [Command::Delete("b2".to_string())]);
        assert_eq!(model.prompt(), &Prompt::None);
    }

    #[test]
    fn d_then_n_gives_nothing_and_closes_the_question() {
        let mut model = model(&["a1"]);
        model.handle(key(KeyCode::Char('d')));

        assert_eq!(model.handle(key(KeyCode::Char('n'))), []);

        assert_eq!(model.prompt(), &Prompt::None);
    }

    #[test]
    fn every_state_text_is_kept_as_sent() {
        let states = [
            "pending",
            "running",
            "paused",
            "held",
            "unresolved",
            "finished",
            "stopped",
        ];
        let rows: Vec<String> = states
            .iter()
            .map(|state| format!(r#"{{"id":"{state}","name":"n","state":"{state}"}}"#))
            .collect();
        let mut model = Model::new("http://sonda".to_string());

        model.apply(Update {
            at: Instant::now(),
            list: Ok(format!(r#"{{"scenarios":[{}]}}"#, rows.join(","))),
            selected: None,
        });

        let shown: Vec<&str> = model.rows().iter().map(|row| row.state.as_str()).collect();
        assert_eq!(shown, states);
    }

    #[test]
    fn the_poller_delivers_the_list_show_and_stats_of_the_first_row_and_stop_joins() {
        let stub = Stub::start(&[
            ("GET", "/scenarios", 200, TWO),
            ("GET", "/scenarios/a1", 200, r#"{"id":"a1"}"#),
            ("GET", "/scenarios/a1/stats", 200, r#"{"total_events":7}"#),
        ]);

        let (poller, updates, _notes) = Poller::start(Sonda::new(stub.url()), PERIOD);
        let update = updates.recv_timeout(PERIOD * 2).unwrap();
        poller.stop();

        assert_eq!(update.list, Ok(TWO.to_string()));
        assert_eq!(
            update.selected,
            Some((
                "a1".to_string(),
                Ok(r#"{"id":"a1"}"#.to_string()),
                Ok(r#"{"total_events":7}"#.to_string())
            ))
        );
        let _ = updates.try_iter().count();
        assert_eq!(updates.try_recv().unwrap_err(), TryRecvError::Disconnected);
    }

    #[test]
    fn select_changes_which_show_and_stats_the_next_round_asks() {
        let stub = Stub::start(&[("GET", "/scenarios", 200, TWO)]);
        let (poller, updates, _notes) = Poller::start(Sonda::new(stub.url()), PERIOD);
        updates.recv_timeout(WAIT).unwrap();

        poller.send(Command::Select("b2".to_string()));
        poller.send(Command::Refresh);
        let update = updates.recv_timeout(WAIT).unwrap();
        poller.stop();

        assert_eq!(update.selected.unwrap().0, "b2");
        let paths: Vec<String> = stub.requests().into_iter().map(|seen| seen.path).collect();
        assert!(paths.contains(&"/scenarios/b2".to_string()), "{paths:?}");
        assert!(
            paths.contains(&"/scenarios/b2/stats".to_string()),
            "{paths:?}"
        );
    }

    #[test]
    fn load_posts_a_runnable_file_and_delivers_the_reply_as_a_note() {
        let stub = Stub::start(&[
            ("GET", "/scenarios", 200, r#"{"scenarios":[]}"#),
            ("POST", "/scenarios", 201, "{\"id\":\"x1\"}\n"),
        ]);
        let file = fixture("hello.yaml");
        let (poller, _updates, notes) = Poller::start(Sonda::new(stub.url()), PERIOD);

        poller.send(Command::Load(file.clone()));
        let note = notes.recv_timeout(WAIT).unwrap();
        poller.stop();

        assert_eq!(note.text, r#"{"id":"x1"}"#);
        let posts: Vec<_> = stub
            .requests()
            .into_iter()
            .filter(|seen| seen.method == "POST")
            .collect();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].body, fs::read(&file).unwrap());
    }

    #[test]
    fn load_of_a_pack_delivers_the_refusal_and_posts_nothing() {
        let stub = Stub::start(&[
            ("GET", "/scenarios", 200, r#"{"scenarios":[]}"#),
            ("POST", "/scenarios", 201, r#"{"id":"x1"}"#),
        ]);
        let file = fixture("pack.yaml");
        let (poller, _updates, notes) = Poller::start(Sonda::new(stub.url()), PERIOD);

        poller.send(Command::Load(file.clone()));
        let note = notes.recv_timeout(WAIT).unwrap();
        poller.stop();

        assert!(
            note.text.starts_with(&format!("nobs: {file}: ")),
            "{}",
            note.text
        );
        assert!(note.text.contains("pack"), "{}", note.text);
        assert!(stub.requests().iter().all(|seen| seen.method != "POST"));
    }

    #[test]
    fn delete_sends_delete_of_the_id_and_delivers_the_reply_as_a_note() {
        let reply = r#"{"id":"a1","status":"stopped","total_events":5}"#;
        let stub = Stub::start(&[
            ("GET", "/scenarios", 200, TWO),
            ("DELETE", "/scenarios/a1", 200, reply),
        ]);
        let (poller, _updates, notes) = Poller::start(Sonda::new(stub.url()), PERIOD);

        poller.send(Command::Delete("a1".to_string()));
        let note = notes.recv_timeout(WAIT).unwrap();
        poller.stop();

        assert_eq!(note.text, reply);
        assert!(stub
            .requests()
            .iter()
            .any(|seen| seen.method == "DELETE" && seen.path == "/scenarios/a1"));
    }

    #[test]
    fn a_held_connection_never_keeps_quit_waiting_and_the_rows_stay() {
        let stub = Stub::start_silent(Duration::from_secs(2));
        let mut model = model(&["a1"]);
        let started = Instant::now();

        let (poller, updates, _notes) = Poller::start(
            Sonda::new(stub.url()).timeout(Duration::from_millis(300)),
            PERIOD,
        );
        model.handle(key(KeyCode::Char('q')));

        assert!(model.quit());
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "{:?}",
            started.elapsed()
        );
        let update = updates.recv_timeout(WAIT).unwrap();
        let error = update.list.clone().unwrap_err();
        assert!(
            error.contains("cannot reach") || error.contains("imeout"),
            "{error}"
        );
        model.apply(update);
        poller.stop();
        assert_eq!(ids(&model), ["a1"]);
        assert_eq!(model.error(), Some(error.as_str()));
    }
}
