//! Event loop: keys and mouse → actions, periodic polling of the scanner, drawing.

use std::path::Path;
use std::time::{Duration, Instant};

use lazydisk_core::config::Config;
use lazydisk_core::keymap::Command;
use lazydisk_core::state::{Action, AppState, Effect, Panel};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, MouseButton, MouseEventKind};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;

use crate::keys::chord_from;
use crate::ui::Ui;

/// Per-frame geometry remembered for mouse hit testing.
#[derive(Default, Clone, Copy)]
pub struct Hit {
    pub sunburst: Rect,
    pub files: Rect,
    pub volumes: Rect,
    pub collector: Rect,
    pub files_offset: usize,
    pub collector_offset: usize,
    pub volumes_offset: usize,
}

pub fn run(mut terminal: DefaultTerminal, root: &Path, cfg: &Config) -> anyhow::Result<()> {
    let _ = execute!(std::io::stdout(), event::EnableMouseCapture);
    let mut state = AppState::new(root, cfg);
    state.start_scan();
    let mut ui = Ui::default();
    let mut last_draw = Instant::now() - Duration::from_secs(1);
    let mut dirty = true;
    let result = loop {
        if state.poll() {
            dirty = true;
        }
        // Redraw at ~30 fps while scanning (totals change), otherwise only on input.
        let animating = state.scan_status().is_running() || state.is_deleting();
        if dirty || (animating && last_draw.elapsed() >= Duration::from_millis(33)) {
            if let Err(e) = terminal.draw(|f| ui.draw(f, &mut state)) {
                break Err(e.into());
            }
            last_draw = Instant::now();
            dirty = false;
        }
        let timeout = if animating {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(250)
        };
        match event::poll(timeout) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => break Err(e.into()),
        }
        let ev = match event::read() {
            Ok(ev) => ev,
            Err(e) => break Err(e.into()),
        };
        let effects = match ev {
            Event::Key(k) => match chord_from(&k) {
                Some(chord) => state.apply(Action::Key(chord)),
                None => Vec::new(),
            },
            Event::Mouse(m) => mouse(&mut state, &ui, m.kind, m.column, m.row),
            Event::Resize(_, _) => Vec::new(),
            _ => Vec::new(),
        };
        dirty = true;
        if effects.contains(&Effect::Quit) || state.should_quit() {
            break Ok(());
        }
    };
    let _ = execute!(std::io::stdout(), event::DisableMouseCapture);
    result
}

fn mouse(state: &mut AppState, ui: &Ui, kind: MouseEventKind, col: u16, row: u16) -> Vec<Effect> {
    let hit = ui.hit;
    let inside = |r: Rect| col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height;
    match kind {
        MouseEventKind::ScrollDown => {
            if inside(hit.sunburst) {
                return state.apply(Action::Command(Command::Enter));
            }
            state.apply(Action::Command(Command::Down))
        }
        MouseEventKind::ScrollUp => {
            if inside(hit.sunburst) {
                return state.apply(Action::Command(Command::Parent));
            }
            state.apply(Action::Command(Command::Up))
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if state.overlay().is_some() {
                return Vec::new();
            }
            if inside(hit.sunburst) {
                return match ui.sunburst_hit(state, col, row) {
                    crate::sunburst::HitResult::Segment(node, is_dir) => {
                        if is_dir && state.selected_node() == Some(node) {
                            state.apply(Action::Focus(node))
                        } else {
                            state.apply(Action::Select(node))
                        }
                    }
                    crate::sunburst::HitResult::Center => {
                        state.apply(Action::Command(Command::Parent))
                    }
                    crate::sunburst::HitResult::None => Vec::new(),
                };
            }
            if inside(hit.files) {
                state.panel = Panel::Files;
                let idx = (row - hit.files.y) as usize + hit.files_offset;
                let list = state.visible_children();
                if let Some(node) = list.get(idx).copied() {
                    if state.selected_node() == Some(node) {
                        return state.apply(Action::Command(Command::Enter));
                    }
                    return state.apply(Action::Select(node));
                }
            } else if inside(hit.volumes) {
                state.panel = Panel::Volumes;
                let idx = (row - hit.volumes.y) as usize + hit.volumes_offset;
                if idx < state.volumes.len() {
                    if state.volume_selected == idx {
                        return state.apply(Action::ScanVolume(idx));
                    }
                    state.volume_selected = idx;
                }
            } else if inside(hit.collector) {
                state.panel = Panel::Collector;
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}
