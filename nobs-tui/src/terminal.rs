use crossterm::event::KeyEvent;

use crate::actions::{Action, Ask, HandsBack, Row};
use crate::keys;
use crate::shell::{Command, Workbench, LOG_LINES};

const LIMIT: usize = 4;
const FIRST_SIZE: (u16, u16) = (24, 80);

pub(crate) const SHELL: Action = Action {
    key: 't',
    workbench: Workbench::Devices,
    row: Row::Node,
    words: &["device", "shell"],
    hint: "type in the terminal",
    ask: Ask::No,
    sudo: false,
    hands_back: HandsBack::Nothing,
    login: true,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Closed {
    ShellEnded,
    Exit(i32),
    ByYou,
    Limit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    Connecting { typing: bool },
    Typing,
    Connected { up: usize },
    Closed { why: Closed, up: usize },
}

impl State {
    pub(crate) fn word(&self) -> String {
        match *self {
            State::Connecting { .. } => "connecting…".to_string(),
            State::Typing => "typing".to_string(),
            State::Connected { up: 0 } => "connected".to_string(),
            State::Closed { why, up: 0 } => match why {
                Closed::ShellEnded => "closed: the shell ended".to_string(),
                Closed::Exit(code) => format!("closed: exit {code}"),
                Closed::ByYou => "closed: by you".to_string(),
                Closed::Limit => "closed: the limit of four".to_string(),
            },
            State::Connected { up } | State::Closed { up, .. } => format!("{up} lines up"),
        }
    }

    pub(crate) fn keys(&self) -> &'static [&'static str] {
        match self {
            _ if self.typed() => &["Ctrl-] leave (session stays open)"],
            State::Closed { .. } => &["t connect again", "j/k scroll"],
            _ => &["t type in the terminal", "j/k scroll", "c close session"],
        }
    }

    pub(crate) fn marked(&self) -> bool {
        !matches!(self, State::Closed { .. })
    }

    pub(crate) fn typed(&self) -> bool {
        matches!(self, State::Connecting { typing: true } | State::Typing)
    }
}

pub(crate) struct Session {
    pub(crate) id: u64,
    pub(crate) device: String,
    pub(crate) state: State,
    pub(crate) screen: vt100::Parser,
    left: u64,
    ended: u64,
}

impl Session {
    pub(crate) fn write(&self, key: KeyEvent) -> Option<Command> {
        let bytes = keys::bytes(key, self.screen.screen().application_cursor())?;
        Some(Command::TermWrite { id: self.id, bytes })
    }

    pub(crate) fn can_scroll(&self) -> bool {
        let (State::Connected { up } | State::Closed { up, .. }) = self.state else {
            return false;
        };
        if up > 0 {
            return true;
        }
        let mut probe = self.screen.screen().clone();
        probe.set_scrollback(1);
        probe.scrollback() > 0
    }
}

pub(crate) struct Sessions {
    list: Vec<Session>,
    next: u64,
    size: (u16, u16),
}

impl Sessions {
    pub(crate) fn new() -> Sessions {
        Sessions {
            list: Vec::new(),
            next: 0,
            size: FIRST_SIZE,
        }
    }

    pub(crate) fn size(&self) -> (u16, u16) {
        self.size
    }

    pub(crate) fn of(&self, device: &str) -> Option<&Session> {
        self.list.iter().find(|session| session.device == device)
    }

    pub(crate) fn typed(&self) -> Option<&Session> {
        self.list.iter().find(|session| session.state.typed())
    }

    pub(crate) fn live(&self) -> impl Iterator<Item = &Session> {
        self.list.iter().filter(|session| session.state.marked())
    }

    fn stamp(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn by_id(&mut self, id: u64) -> Option<&mut Session> {
        self.list.iter_mut().find(|session| session.id == id)
    }

    fn by_device(&mut self, device: &str) -> Option<&mut Session> {
        self.list
            .iter_mut()
            .find(|session| session.device == device)
    }

    pub(crate) fn resume(&mut self, device: &str) -> bool {
        let Some(session) = self.by_device(device) else {
            return false;
        };
        session.state = match session.state {
            State::Connecting { .. } => State::Connecting { typing: true },
            State::Connected { .. } => State::Typing,
            State::Typing | State::Closed { .. } => return false,
        };
        session.screen.screen_mut().set_scrollback(0);
        true
    }

    pub(crate) fn open(&mut self, device: &str) -> (u64, Option<(Command, String)>) {
        self.list.retain(|session| session.device != device);
        let limited = self.make_room();
        let id = self.stamp();
        let (rows, cols) = self.size;
        self.list.push(Session {
            id,
            device: device.to_string(),
            state: State::Connecting { typing: true },
            screen: vt100::Parser::new(rows, cols, LOG_LINES),
            left: id,
            ended: 0,
        });
        (id, limited)
    }

    fn make_room(&mut self) -> Option<(Command, String)> {
        if self.list.len() >= LIMIT {
            let closed_first = self
                .list
                .iter()
                .enumerate()
                .filter(|(_, session)| !session.state.marked())
                .min_by_key(|(_, session)| session.ended)
                .map(|(at, _)| at);
            if let Some(at) = closed_first {
                self.list.remove(at);
            }
        }
        if self.live().count() < LIMIT {
            return None;
        }
        let (id, name) = self
            .live()
            .filter(|session| !session.state.typed())
            .min_by_key(|session| session.left)
            .map(|session| (session.id, session.device.clone()))?;
        self.close(id, Closed::Limit).map(|command| (command, name))
    }

    pub(crate) fn leave(&mut self) {
        let left = self.stamp();
        let Some(session) = self.list.iter_mut().find(|session| session.state.typed()) else {
            return;
        };
        session.state = match session.state {
            State::Connecting { .. } => State::Connecting { typing: false },
            _ => State::Connected { up: 0 },
        };
        session.left = left;
    }

    pub(crate) fn scroll(&mut self, device: &str, up: bool, rows: usize) {
        let Some(session) = self.by_device(device) else {
            return;
        };
        let (State::Connected { up: shown } | State::Closed { up: shown, .. }) = &mut session.state
        else {
            return;
        };
        let wanted = if up {
            *shown + rows
        } else {
            shown.saturating_sub(rows)
        };
        session.screen.screen_mut().set_scrollback(wanted);
        *shown = session.screen.screen().scrollback();
    }

    fn end(&mut self, id: u64, why: Closed) -> bool {
        let ended = self.stamp();
        let Some(session) = self.by_id(id).filter(|session| session.state.marked()) else {
            return false;
        };
        let up = match session.state {
            State::Connected { up } => up,
            _ => 0,
        };
        session.state = State::Closed { why, up };
        session.ended = ended;
        true
    }

    pub(crate) fn close(&mut self, id: u64, why: Closed) -> Option<Command> {
        self.end(id, why).then_some(Command::TermClose(id))
    }

    pub(crate) fn close_live(&mut self) -> Vec<Command> {
        let ids: Vec<u64> = self.live().map(|session| session.id).collect();
        ids.into_iter()
            .filter_map(|id| self.close(id, Closed::ByYou))
            .collect()
    }

    pub(crate) fn output(&mut self, id: u64, bytes: &[u8]) {
        let Some(session) = self.by_id(id) else {
            return;
        };
        session.screen.process(bytes);
        session.state = match session.state {
            State::Connecting { typing: true } => State::Typing,
            State::Connecting { typing: false } => State::Connected { up: 0 },
            state => state,
        };
    }

    pub(crate) fn ended(&mut self, id: u64, code: i32) {
        let why = if code == 0 {
            Closed::ShellEnded
        } else {
            Closed::Exit(code)
        };
        self.end(id, why);
    }

    pub(crate) fn resize(&mut self, size: (u16, u16)) -> Vec<Command> {
        if size == self.size {
            return Vec::new();
        }
        self.size = size;
        for session in &mut self.list {
            session.screen.screen_mut().set_size(size.0, size.1);
        }
        self.live()
            .map(|session| Command::TermResize {
                id: session.id,
                size,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_reader_follows_the_one_state() {
        let typing: &[&str] = &["Ctrl-] leave (session stays open)"];
        let open: &[&str] = &["t type in the terminal", "j/k scroll", "c close session"];
        let closed: &[&str] = &["t connect again", "j/k scroll"];
        let states = [
            State::Connecting { typing: true },
            State::Connecting { typing: false },
            State::Typing,
            State::Connected { up: 0 },
            State::Connected { up: 3 },
            ended(Closed::ShellEnded),
            ended(Closed::Exit(1)),
            ended(Closed::ByYou),
            ended(Closed::Limit),
            State::Closed {
                why: Closed::Limit,
                up: 2,
            },
        ];

        let words: Vec<String> = states.iter().map(State::word).collect();
        let keys: Vec<&[&str]> = states.iter().map(State::keys).collect();
        let marked: Vec<bool> = states.iter().map(State::marked).collect();
        let typed: Vec<bool> = states.iter().map(State::typed).collect();

        assert_eq!(
            words,
            [
                "connecting…",
                "connecting…",
                "typing",
                "connected",
                "3 lines up",
                "closed: the shell ended",
                "closed: exit 1",
                "closed: by you",
                "closed: the limit of four",
                "2 lines up",
            ]
        );
        assert_eq!(
            keys,
            [typing, open, typing, open, open, closed, closed, closed, closed, closed]
        );
        assert_eq!(
            marked,
            [true, true, true, true, true, false, false, false, false, false]
        );
        assert_eq!(
            typed,
            [true, false, true, false, false, false, false, false, false, false]
        );
    }

    fn ended(why: Closed) -> State {
        State::Closed { why, up: 0 }
    }

    fn opened(sessions: &mut Sessions, device: &str) -> Option<String> {
        let (_, limited) = sessions.open(device);
        let closed = limited.map(|(command, name)| {
            let id = id_of(sessions, &name);
            assert_eq!(command, Command::TermClose(id));
            name
        });
        assert!(sessions.live().count() <= LIMIT, "{device}");
        assert_eq!(
            state_of(sessions, device),
            Some(State::Connecting { typing: true })
        );
        closed
    }

    fn id_of(sessions: &Sessions, device: &str) -> u64 {
        sessions.of(device).unwrap().id
    }

    fn state_of(sessions: &Sessions, device: &str) -> Option<State> {
        sessions.of(device).map(|session| session.state)
    }

    #[test]
    fn the_fifth_closes_the_oldest_closed_then_the_open_one_left_longest_ago_never_the_typed_one() {
        let mut sessions = Sessions::new();
        for device in ["nobs-t-a", "nobs-t-b", "nobs-t-c", "nobs-t-d"] {
            assert_eq!(opened(&mut sessions, device), None);
            sessions.leave();
        }
        sessions.ended(id_of(&sessions, "nobs-t-b"), 0);

        assert_eq!(opened(&mut sessions, "nobs-t-e"), None);
        assert_eq!(state_of(&sessions, "nobs-t-b"), None);
        sessions.leave();
        assert!(sessions.resume("nobs-t-a"));
        sessions.leave();

        let closed = opened(&mut sessions, "nobs-t-f");
        assert_eq!(closed.as_deref(), Some("nobs-t-c"));
        assert_eq!(state_of(&sessions, "nobs-t-c"), Some(ended(Closed::Limit)));
        sessions.leave();
        assert!(sessions.resume("nobs-t-d"));

        let closed = opened(&mut sessions, "nobs-t-g");
        assert_eq!(closed.as_deref(), Some("nobs-t-e"));
        assert_eq!(state_of(&sessions, "nobs-t-c"), None);
        assert_eq!(
            state_of(&sessions, "nobs-t-d"),
            Some(State::Connecting { typing: true })
        );
        let live: Vec<&str> = sessions
            .live()
            .map(|session| session.device.as_str())
            .collect();
        assert_eq!(live, ["nobs-t-a", "nobs-t-d", "nobs-t-f", "nobs-t-g"]);
    }

    #[test]
    fn a_place_is_made_by_dropping_the_session_closed_longest_ago_not_the_one_opened_first() {
        let mut sessions = Sessions::new();
        for device in ["nobs-t-a", "nobs-t-b", "nobs-t-c", "nobs-t-d"] {
            opened(&mut sessions, device);
            sessions.leave();
        }
        sessions.ended(id_of(&sessions, "nobs-t-b"), 0);
        sessions.ended(id_of(&sessions, "nobs-t-a"), 0);

        assert_eq!(opened(&mut sessions, "nobs-t-e"), None);

        assert_eq!(state_of(&sessions, "nobs-t-b"), None);
        assert_eq!(
            state_of(&sessions, "nobs-t-a"),
            Some(ended(Closed::ShellEnded))
        );
    }

    #[test]
    fn never_more_than_four_connect_or_stay_open() {
        let mut sessions = Sessions::new();
        let devices = ["nobs-t-a", "nobs-t-b", "nobs-t-c", "nobs-t-d", "nobs-t-e"];
        for round in 0..30 {
            let device = devices[(round * 7) % devices.len()];
            if !sessions.resume(device) {
                opened(&mut sessions, device);
            }
            if round % 5 == 0 {
                sessions.ended(id_of(&sessions, device), 1);
            } else {
                sessions.leave();
            }
            assert!(sessions.live().count() <= LIMIT, "{round}");
        }
    }
}
