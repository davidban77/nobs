use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(crate) fn bytes(key: KeyEvent, application_cursor: bool) -> Option<Vec<u8>> {
    if key.modifiers.contains(KeyModifiers::ALT) {
        let mut plain = key;
        plain.modifiers.remove(KeyModifiers::ALT);
        let mut out = vec![0x1B];
        out.extend(bytes(plain, application_cursor)?);
        return Some(out);
    }
    match key.code {
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            control(c).map(|byte| vec![byte])
        }
        KeyCode::Char(c) => Some(c.to_string().into_bytes()),
        KeyCode::Enter => Some(vec![0x0D]),
        KeyCode::Tab => Some(vec![0x09]),
        KeyCode::BackTab => Some(b"\x1b[Z".to_vec()),
        KeyCode::Backspace => Some(vec![0x7F]),
        KeyCode::Esc => Some(vec![0x1B]),
        KeyCode::Up => Some(cursor(application_cursor, b'A')),
        KeyCode::Down => Some(cursor(application_cursor, b'B')),
        KeyCode::Right => Some(cursor(application_cursor, b'C')),
        KeyCode::Left => Some(cursor(application_cursor, b'D')),
        KeyCode::Home => Some(cursor(application_cursor, b'H')),
        KeyCode::End => Some(cursor(application_cursor, b'F')),
        KeyCode::Insert => Some(tilde("2")),
        KeyCode::Delete => Some(tilde("3")),
        KeyCode::PageUp => Some(tilde("5")),
        KeyCode::PageDown => Some(tilde("6")),
        KeyCode::F(n @ 1..=4) => Some(vec![0x1B, b'O', b"PQRS"[usize::from(n - 1)]]),
        KeyCode::F(n @ 5..=12) => Some(tilde(
            ["15", "17", "18", "19", "20", "21", "23", "24"][usize::from(n - 5)],
        )),
        _ => None,
    }
}

fn control(c: char) -> Option<u8> {
    match c {
        'a'..='z' | 'A'..='Z' => Some(c as u8 & 0x1F),
        ' ' | '@' | '2' => Some(0x00),
        '[' | '3' => Some(0x1B),
        '\\' | '4' => Some(0x1C),
        '^' | '6' | '~' => Some(0x1E),
        '_' | '7' | '/' => Some(0x1F),
        '8' | '?' => Some(0x7F),
        _ => None,
    }
}

fn cursor(application: bool, last: u8) -> Vec<u8> {
    vec![0x1B, if application { b'O' } else { b'[' }, last]
}

fn tilde(code: &str) -> Vec<u8> {
    format!("\x1b[{code}~").into_bytes()
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MediaKeyCode};

    use super::bytes;

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;
    const SHIFT: KeyModifiers = KeyModifiers::SHIFT;

    fn table() -> Vec<(KeyCode, KeyModifiers, &'static [u8], &'static [u8])> {
        vec![
            (KeyCode::Char('a'), NONE, b"a", b"a"),
            (KeyCode::Char('q'), NONE, b"q", b"q"),
            (KeyCode::Char('A'), SHIFT, b"A", b"A"),
            (KeyCode::Char('1'), NONE, b"1", b"1"),
            (KeyCode::Char(' '), NONE, b" ", b" "),
            (KeyCode::Char('é'), NONE, "é".as_bytes(), "é".as_bytes()),
            (KeyCode::Char('a'), CTRL, b"\x01", b"\x01"),
            (KeyCode::Char('A'), CTRL, b"\x01", b"\x01"),
            (KeyCode::Char('c'), CTRL, b"\x03", b"\x03"),
            (KeyCode::Char('h'), CTRL, b"\x08", b"\x08"),
            (KeyCode::Char('j'), CTRL, b"\x0a", b"\x0a"),
            (KeyCode::Char('m'), CTRL, b"\x0d", b"\x0d"),
            (KeyCode::Char('z'), CTRL, b"\x1a", b"\x1a"),
            (KeyCode::Char('Z'), CTRL, b"\x1a", b"\x1a"),
            (KeyCode::Char(' '), CTRL, b"\x00", b"\x00"),
            (KeyCode::Char('@'), CTRL, b"\x00", b"\x00"),
            (KeyCode::Char('2'), CTRL, b"\x00", b"\x00"),
            (KeyCode::Char('['), CTRL, b"\x1b", b"\x1b"),
            (KeyCode::Char('3'), CTRL, b"\x1b", b"\x1b"),
            (KeyCode::Char('\\'), CTRL, b"\x1c", b"\x1c"),
            (KeyCode::Char('4'), CTRL, b"\x1c", b"\x1c"),
            (KeyCode::Char('^'), CTRL, b"\x1e", b"\x1e"),
            (KeyCode::Char('6'), CTRL, b"\x1e", b"\x1e"),
            (KeyCode::Char('~'), CTRL, b"\x1e", b"\x1e"),
            (KeyCode::Char('_'), CTRL, b"\x1f", b"\x1f"),
            (KeyCode::Char('7'), CTRL, b"\x1f", b"\x1f"),
            (KeyCode::Char('/'), CTRL, b"\x1f", b"\x1f"),
            (KeyCode::Char('8'), CTRL, b"\x7f", b"\x7f"),
            (KeyCode::Char('?'), CTRL, b"\x7f", b"\x7f"),
            (KeyCode::Enter, NONE, b"\x0d", b"\x0d"),
            (KeyCode::Tab, NONE, b"\x09", b"\x09"),
            (KeyCode::BackTab, SHIFT, b"\x1b[Z", b"\x1b[Z"),
            (KeyCode::Backspace, NONE, b"\x7f", b"\x7f"),
            (KeyCode::Esc, NONE, b"\x1b", b"\x1b"),
            (KeyCode::Up, NONE, b"\x1b[A", b"\x1bOA"),
            (KeyCode::Down, NONE, b"\x1b[B", b"\x1bOB"),
            (KeyCode::Right, NONE, b"\x1b[C", b"\x1bOC"),
            (KeyCode::Left, NONE, b"\x1b[D", b"\x1bOD"),
            (KeyCode::Home, NONE, b"\x1b[H", b"\x1bOH"),
            (KeyCode::End, NONE, b"\x1b[F", b"\x1bOF"),
            (KeyCode::Insert, NONE, b"\x1b[2~", b"\x1b[2~"),
            (KeyCode::Delete, NONE, b"\x1b[3~", b"\x1b[3~"),
            (KeyCode::PageUp, NONE, b"\x1b[5~", b"\x1b[5~"),
            (KeyCode::PageDown, NONE, b"\x1b[6~", b"\x1b[6~"),
            (KeyCode::F(1), NONE, b"\x1bOP", b"\x1bOP"),
            (KeyCode::F(2), NONE, b"\x1bOQ", b"\x1bOQ"),
            (KeyCode::F(3), NONE, b"\x1bOR", b"\x1bOR"),
            (KeyCode::F(4), NONE, b"\x1bOS", b"\x1bOS"),
            (KeyCode::F(5), NONE, b"\x1b[15~", b"\x1b[15~"),
            (KeyCode::F(6), NONE, b"\x1b[17~", b"\x1b[17~"),
            (KeyCode::F(7), NONE, b"\x1b[18~", b"\x1b[18~"),
            (KeyCode::F(8), NONE, b"\x1b[19~", b"\x1b[19~"),
            (KeyCode::F(9), NONE, b"\x1b[20~", b"\x1b[20~"),
            (KeyCode::F(10), NONE, b"\x1b[21~", b"\x1b[21~"),
            (KeyCode::F(11), NONE, b"\x1b[23~", b"\x1b[23~"),
            (KeyCode::F(12), NONE, b"\x1b[24~", b"\x1b[24~"),
        ]
    }

    #[test]
    fn the_table_key_by_key_in_both_cursor_modes() {
        for (code, modifiers, normal, application) in table() {
            let key = KeyEvent::new(code, modifiers);
            assert_eq!(bytes(key, false).as_deref(), Some(normal), "{key:?}");
            assert_eq!(bytes(key, true).as_deref(), Some(application), "{key:?}");
        }
    }

    #[test]
    fn alt_puts_esc_before_the_bytes_of_every_key_of_the_table() {
        for (code, modifiers, normal, application) in table() {
            let key = KeyEvent::new(code, modifiers | KeyModifiers::ALT);
            assert_eq!(
                bytes(key, false),
                Some([b"\x1b", normal].concat()),
                "{key:?}"
            );
            assert_eq!(
                bytes(key, true),
                Some([b"\x1b", application].concat()),
                "{key:?}"
            );
        }
    }

    #[test]
    fn ctrl_close_bracket_and_ctrl_5_give_no_bytes() {
        for c in [']', '5'] {
            for modifiers in [CTRL, CTRL | KeyModifiers::ALT] {
                let key = KeyEvent::new(KeyCode::Char(c), modifiers);
                assert_eq!(bytes(key, false), None, "{key:?}");
                assert_eq!(bytes(key, true), None, "{key:?}");
            }
        }
    }

    #[test]
    fn keys_outside_the_table_give_no_bytes() {
        let keys = [
            KeyEvent::new(KeyCode::Null, NONE),
            KeyEvent::new(KeyCode::F(13), NONE),
            KeyEvent::new(KeyCode::F(24), NONE),
            KeyEvent::new(KeyCode::CapsLock, NONE),
            KeyEvent::new(KeyCode::Media(MediaKeyCode::Play), NONE),
            KeyEvent::new(KeyCode::Char('1'), CTRL),
            KeyEvent::new(KeyCode::Char('.'), CTRL),
            KeyEvent::new(KeyCode::Null, KeyModifiers::ALT),
        ];
        for key in keys {
            assert_eq!(bytes(key, false), None, "{key:?}");
            assert_eq!(bytes(key, true), None, "{key:?}");
        }
    }
}
