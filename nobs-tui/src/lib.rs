use std::fmt::Display;
use std::io::{self, IsTerminal};
use std::panic;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEvent};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, execute};
use nobs_core::sonda::Sonda;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::Terminal;

use data::{Command, Model, Note, Poller, Update};

pub mod data;
pub mod view;

const PERIOD: Duration = Duration::from_secs(1);
const KEY_WAIT: Duration = Duration::from_millis(100);

pub fn run(sonda: Sonda) -> Result<(), String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("the screen needs a terminal".to_string());
    }
    let mut model = Model::new(sonda.url().to_string());
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
    let screen = Screen::open()?;
    let (poller, updates, notes) = Poller::start(sonda, PERIOD);
    let result = Terminal::new(CrosstermBackend::new(io::stdout()))
        .map_err(failed)
        .and_then(|mut terminal| {
            turns(
                &mut terminal,
                &mut model,
                &poller,
                &updates,
                &notes,
                read_key,
            )
        });
    drop(screen);
    poller.send(Command::Stop);
    result
}

fn turns<B: Backend>(
    terminal: &mut Terminal<B>,
    model: &mut Model,
    poller: &Poller,
    updates: &Receiver<Update>,
    notes: &Receiver<Note>,
    mut next_key: impl FnMut(Duration) -> Result<Option<KeyEvent>, String>,
) -> Result<(), String> {
    loop {
        for update in updates.try_iter() {
            model.apply(update);
        }
        for note in notes.try_iter() {
            model.note(note.text, note.at);
        }
        let now = Instant::now();
        terminal
            .draw(|frame| view::draw(frame, model, now))
            .map_err(failed)?;
        if let Some(key) = next_key(KEY_WAIT)? {
            for command in model.handle(key) {
                poller.send(command);
            }
        }
        if model.quit() {
            return Ok(());
        }
    }
}

fn read_key(wait: Duration) -> Result<Option<KeyEvent>, String> {
    if !event::poll(wait).map_err(failed)? {
        return Ok(None);
    }
    match event::read().map_err(failed)? {
        Event::Key(key) => Ok(Some(key)),
        _ => Ok(None),
    }
}

fn failed(error: impl Display) -> String {
    format!("the screen failed: {error}")
}

fn restore() {
    let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
    let _ = disable_raw_mode();
}

struct Screen;

impl Screen {
    fn open() -> Result<Screen, String> {
        enable_raw_mode().map_err(failed)?;
        let screen = Screen;
        execute!(io::stdout(), EnterAlternateScreen).map_err(failed)?;
        Ok(screen)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        restore();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::data::stub::Stub;

    const LIST: &str = r#"{"scenarios":[{"id":"a1","name":"cpu","state":"running"}]}"#;

    fn q() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)
    }

    fn held() -> (Stub, Poller, Receiver<Update>, Receiver<Note>) {
        let stub = Stub::start_silent(Duration::from_secs(2));
        let (poller, updates, notes) = Poller::start(Sonda::new(stub.url()), PERIOD);
        (stub, poller, updates, notes)
    }

    fn terminal() -> Terminal<TestBackend> {
        Terminal::new(TestBackend::new(80, 24)).unwrap()
    }

    #[test]
    fn q_ends_the_loop_at_once_while_the_server_holds_the_connection() {
        let (stub, poller, updates, notes) = held();
        let mut model = Model::new(stub.url().to_string());
        let started = Instant::now();

        let result = turns(
            &mut terminal(),
            &mut model,
            &poller,
            &updates,
            &notes,
            |_| Ok(Some(q())),
        );

        assert_eq!(result, Ok(()));
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "{:?}",
            started.elapsed()
        );
        poller.send(Command::Stop);
    }

    #[test]
    fn each_turn_waits_100_ms_for_a_key() {
        let (stub, poller, updates, notes) = held();
        let mut model = Model::new(stub.url().to_string());
        let mut waits = Vec::new();

        turns(
            &mut terminal(),
            &mut model,
            &poller,
            &updates,
            &notes,
            |wait| {
                waits.push(wait);
                Ok((waits.len() == 2).then(q))
            },
        )
        .unwrap();

        assert_eq!(waits, [Duration::from_millis(100); 2]);
        poller.send(Command::Stop);
    }

    #[test]
    fn an_update_sent_during_a_turn_is_drawn_on_the_next_turn() {
        let (stub, poller, _updates, notes) = held();
        let (sender, updates) = mpsc::channel();
        let mut model = Model::new(stub.url().to_string());
        let mut terminal = terminal();
        let mut turn = 0;

        turns(&mut terminal, &mut model, &poller, &updates, &notes, |_| {
            turn += 1;
            if turn == 1 {
                sender
                    .send(Update {
                        at: Instant::now(),
                        list: Ok(LIST.to_string()),
                        selected: None,
                    })
                    .unwrap();
                return Ok(None);
            }
            Ok(Some(q()))
        })
        .unwrap();

        let buffer = terminal.backend().buffer();
        let screen: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(screen.contains("> a1       cpu"), "{screen}");
        poller.send(Command::Stop);
    }
}
