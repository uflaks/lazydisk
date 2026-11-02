//! crossterm key events → toolkit-neutral chords.

use lazydisk_core::keymap::{Chord, Key};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

pub fn chord_from(ev: &KeyEvent) -> Option<Chord> {
    if !matches!(ev.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    let alt = ev.modifiers.contains(KeyModifiers::ALT);
    let shift = ev.modifiers.contains(KeyModifiers::SHIFT);
    let key = match ev.code {
        KeyCode::Char(' ') => Key::Space,
        KeyCode::Char(c) => Key::Char(if ctrl { c.to_ascii_lowercase() } else { c }),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::Delete => Key::Delete,
        KeyCode::F(n) => Key::F(n),
        _ => return None,
    };
    // Shift is implied by the character itself; keep it only for special keys.
    let shift = shift && !matches!(key, Key::Char(_));
    Some(Chord {
        key,
        ctrl,
        alt,
        shift,
    })
}
