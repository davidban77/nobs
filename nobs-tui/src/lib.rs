use std::io::{self, IsTerminal, Stdout};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, execute};
use nobs_core::sonda::Sonda;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use data::{Model, Note, Poller, Update};

pub mod data;
pub mod view;

const PERIOD: Duration = Duration::from_secs(1);
const KEY_WAIT: Duration = Duration::from_millis(100);

pub fn run(sonda: Sonda) -> Result<(), String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("the screen needs a terminal".to_string());
    }
    let mut model = Model::new(sonda.url().to_string());
    let screen = Screen::open()?;
    let (poller, updates, notes) = Poller::start(sonda, PERIOD);
    let result = Terminal::new(CrosstermBackend::new(io::stdout()))
        .map_err(failed)
        .and_then(|mut terminal| turns(&mut terminal, &mut model, &poller, &updates, &notes));
    drop(screen);
    poller.stop();
    result
}

fn turns(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    model: &mut Model,
    poller: &Poller,
    updates: &Receiver<Update>,
    notes: &Receiver<Note>,
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
        if event::poll(KEY_WAIT).map_err(failed)? {
            if let Event::Key(key) = event::read().map_err(failed)? {
                for command in model.handle(key) {
                    poller.send(command);
                }
            }
        }
        if model.quit() {
            return Ok(());
        }
    }
}

fn failed(error: io::Error) -> String {
    format!("the screen failed: {error}")
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
        let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
        let _ = disable_raw_mode();
    }
}
