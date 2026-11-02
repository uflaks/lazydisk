//! gpui keystrokes → toolkit-neutral chords.

use gpui::Keystroke;
use lazydisk_core::keymap::{Chord, Key};

pub fn chord_from(ks: &Keystroke) -> Option<Chord> {
    let m = &ks.modifiers;
    // Command-shortcuts belong to the native menu, not to the in-app keymap.
    if m.platform {
        return None;
    }
    let ctrl = m.control;
    let alt = m.alt;
    let key = match ks.key.as_str() {
        "enter" => Key::Enter,
        "escape" => Key::Esc,
        "backspace" => Key::Backspace,
        "tab" => Key::Tab,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "home" => Key::Home,
        "end" => Key::End,
        "delete" => Key::Delete,
        "space" => Key::Space,
        k if k.len() > 1 && k.starts_with('f') && k[1..].chars().all(|c| c.is_ascii_digit()) => {
            Key::F(k[1..].parse().ok()?)
        }
        _ => {
            // Prefer the typed character so `?`, `+`, `G` arrive as themselves.
            let typed = ks
                .key_char
                .as_deref()
                .filter(|_| !ctrl && !alt)
                .and_then(|s| {
                    let mut it = s.chars();
                    match (it.next(), it.next()) {
                        (Some(c), None) if !c.is_control() => Some(c),
                        _ => None,
                    }
                });
            match typed {
                Some(c) => Key::Char(c),
                None => {
                    let mut it = ks.key.chars();
                    match (it.next(), it.next()) {
                        (Some(c), None) => {
                            Key::Char(if m.shift { c.to_ascii_uppercase() } else { c })
                        }
                        _ => return None,
                    }
                }
            }
        }
    };
    let key = if key == Key::Tab && m.shift {
        Key::BackTab
    } else {
        key
    };
    let shift = m.shift && !matches!(key, Key::Char(_) | Key::BackTab);
    Some(Chord {
        key,
        ctrl,
        alt,
        shift,
    })
}
