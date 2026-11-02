//! Key chords, bindable commands and the default lazygit-style keymap.
//!
//! The keymap is context free: a chord maps to one [`Command`], and the
//! application state interprets the command according to the focused panel.

use std::collections::HashMap;
use std::fmt;

/// A key independent of any terminal or GUI toolkit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Tab,
    BackTab,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Delete,
    Space,
    F(u8),
}

/// A key with modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: Key,
    pub ctrl: bool,
    pub alt: bool,
    /// Only meaningful for non-character keys; `Char` carries its own case.
    pub shift: bool,
}

impl Chord {
    pub const fn key(key: Key) -> Self {
        Self {
            key,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }
    pub const fn char(c: char) -> Self {
        Self::key(Key::Char(c))
    }
    pub const fn ctrl(c: char) -> Self {
        Self {
            key: Key::Char(c),
            ctrl: true,
            alt: false,
            shift: false,
        }
    }

    /// Parse `"ctrl+c"`, `"shift+tab"`, `"enter"`, `"pgdn"`, `"?"`, `"f5"`.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("empty key".into());
        }
        let mut chord = Chord::key(Key::Esc);
        // A trailing `+` is the plus key itself (`+`, `ctrl++`).
        let (mods_str, name) = match spec.rsplit_once('+') {
            Some((m, "")) => (m.trim_end_matches('+'), "+"),
            Some((m, n)) => (m, n),
            None => ("", spec),
        };
        for m in mods_str.split('+').filter(|m| !m.is_empty()) {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "c" => chord.ctrl = true,
                "alt" | "opt" | "a" | "meta" => chord.alt = true,
                "shift" | "s" => chord.shift = true,
                other => return Err(format!("unknown modifier `{other}` in `{spec}`")),
            }
        }
        let lower = name.to_ascii_lowercase();
        chord.key = match lower.as_str() {
            "enter" | "return" => Key::Enter,
            "esc" | "escape" => Key::Esc,
            "backspace" | "bs" => Key::Backspace,
            "tab" => Key::Tab,
            "backtab" => Key::BackTab,
            "up" => Key::Up,
            "down" => Key::Down,
            "left" => Key::Left,
            "right" => Key::Right,
            "pgup" | "pageup" => Key::PageUp,
            "pgdn" | "pagedown" => Key::PageDown,
            "home" => Key::Home,
            "end" => Key::End,
            "del" | "delete" => Key::Delete,
            "space" => Key::Space,
            "plus" => Key::Char('+'),
            "minus" => Key::Char('-'),
            _ if lower.len() > 1
                && lower.starts_with('f')
                && lower[1..].chars().all(|c| c.is_ascii_digit()) =>
            {
                Key::F(
                    lower[1..]
                        .parse()
                        .map_err(|_| format!("bad function key `{name}`"))?,
                )
            }
            _ => {
                let mut chars = name.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Key::Char(c),
                    _ => return Err(format!("unknown key `{name}` in `{spec}`")),
                }
            }
        };
        if chord.key == Key::Tab && chord.shift {
            chord.key = Key::BackTab;
            chord.shift = false;
        }
        Ok(chord)
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("ctrl+")?;
        }
        if self.alt {
            f.write_str("alt+")?;
        }
        if self.shift {
            f.write_str("shift+")?;
        }
        match self.key {
            Key::Char(' ') | Key::Space => f.write_str("space"),
            Key::Char(c) => write!(f, "{c}"),
            Key::Enter => f.write_str("enter"),
            Key::Esc => f.write_str("esc"),
            Key::Backspace => f.write_str("backspace"),
            Key::Tab => f.write_str("tab"),
            Key::BackTab => f.write_str("shift+tab"),
            Key::Up => f.write_str("↑"),
            Key::Down => f.write_str("↓"),
            Key::Left => f.write_str("←"),
            Key::Right => f.write_str("→"),
            Key::PageUp => f.write_str("pgup"),
            Key::PageDown => f.write_str("pgdn"),
            Key::Home => f.write_str("home"),
            Key::End => f.write_str("end"),
            Key::Delete => f.write_str("del"),
            Key::F(n) => write!(f, "f{n}"),
        }
    }
}

macro_rules! commands {
    ($( $variant:ident => $id:literal, $help:literal, [$($key:literal),*] ;)*) => {
        /// Everything a key can be bound to.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Command { $($variant,)* }

        impl Command {
            pub const ALL: &'static [Command] = &[$(Command::$variant,)*];

            /// Stable identifier used in the config file.
            pub fn id(self) -> &'static str {
                match self { $(Command::$variant => $id,)* }
            }
            /// One-line description for help screens.
            pub fn help(self) -> &'static str {
                match self { $(Command::$variant => $help,)* }
            }
            pub fn from_id(id: &str) -> Option<Command> {
                match id { $($id => Some(Command::$variant),)* _ => None }
            }
            fn default_keys(self) -> &'static [&'static str] {
                match self { $(Command::$variant => &[$($key),*],)* }
            }
        }
    };
}

commands! {
    Up => "up", "Select previous item", ["k", "up"];
    Down => "down", "Select next item", ["j", "down"];
    PageUp => "page_up", "Select one page up", ["ctrl+u", "pgup"];
    PageDown => "page_down", "Select one page down", ["ctrl+d", "pgdn"];
    First => "first", "Select first item", ["g", "home"];
    Last => "last", "Select last item", ["G", "end"];
    Enter => "enter", "Open selected folder / scan selected volume", ["l", "enter", "right"];
    Parent => "parent", "Go to the enclosing folder", ["h", "esc", "left", "backspace"];
    Back => "back", "Go back in history", ["["];
    Forward => "forward", "Go forward in history", ["]"];
    FocusVolumes => "focus_volumes", "Focus the Volumes panel", ["1"];
    FocusFiles => "focus_files", "Focus the Files panel", ["2"];
    FocusCollector => "focus_collector", "Focus the Collector panel", ["3"];
    FocusDetails => "focus_details", "Focus the Details panel", ["4"];
    NextPanel => "next_panel", "Focus next panel", ["tab"];
    PrevPanel => "prev_panel", "Focus previous panel", ["shift+tab"];
    Filter => "filter", "Filter the current folder by name", ["/"];
    ToggleSizeMode => "toggle_size_mode", "Toggle allocated / apparent size", ["s"];
    ToggleUnits => "toggle_units", "Toggle binary / SI units", ["u"];
    MoreRings => "more_rings", "Show one more sunburst ring", ["+", "="];
    FewerRings => "fewer_rings", "Show one fewer sunburst ring", ["-"];
    Reveal => "reveal", "Reveal in Finder / file manager", ["o"];
    Preview => "preview", "Quick Look preview", ["space"];
    CopyPath => "copy_path", "Copy path to clipboard", ["y"];
    ToggleCollect => "toggle_collect", "Add to / remove from the Collector", ["c"];
    ClearCollector => "clear_collector", "Empty the Collector", ["C"];
    DeleteTrash => "delete_trash", "Move collected items to the Trash", ["d"];
    DeletePermanent => "delete_permanent", "Delete collected items permanently", ["D"];
    Rescan => "rescan", "Rescan the current folder", ["r"];
    RescanAll => "rescan_all", "Rescan everything", ["R"];
    Menu => "menu", "Open the context menu", ["x"];
    Help => "help", "Show this help", ["?"];
    Quit => "quit", "Quit", ["q", "ctrl+c"];
}

/// Chord → command table.
#[derive(Clone, Debug)]
pub struct Keymap {
    bindings: Vec<(Chord, Command)>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::defaults()
    }
}

impl Keymap {
    pub fn defaults() -> Self {
        let mut bindings = Vec::new();
        for &cmd in Command::ALL {
            for spec in cmd.default_keys() {
                let chord = Chord::parse(spec).expect("default key spec parses");
                bindings.push((chord, cmd));
            }
        }
        Self { bindings }
    }

    pub fn lookup(&self, chord: Chord) -> Option<Command> {
        self.bindings
            .iter()
            .find(|(c, _)| *c == chord)
            .map(|(_, cmd)| *cmd)
    }

    pub fn keys_for(&self, cmd: Command) -> Vec<Chord> {
        self.bindings
            .iter()
            .filter(|(_, c)| *c == cmd)
            .map(|(k, _)| *k)
            .collect()
    }

    /// Short label like `j/↓` for status bars.
    pub fn label(&self, cmd: Command) -> String {
        self.keys_for(cmd)
            .iter()
            .take(2)
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Replace the keys of every command named in `overrides` (`command id` → key specs).
    pub fn apply_overrides(
        &mut self,
        overrides: &HashMap<String, Vec<String>>,
    ) -> Result<(), String> {
        for (id, specs) in overrides {
            let cmd =
                Command::from_id(id).ok_or_else(|| format!("unknown command `{id}` in [keys]"))?;
            let chords = specs
                .iter()
                .map(|s| Chord::parse(s))
                .collect::<Result<Vec<_>, _>>()?;
            self.bindings.retain(|(_, c)| *c != cmd);
            for ch in chords {
                self.bindings.retain(|(k, _)| *k != ch);
                self.bindings.push((ch, cmd));
            }
        }
        Ok(())
    }

    /// `(keys, description)` rows for the help overlay, in command order.
    pub fn help_rows(&self) -> Vec<(String, &'static str)> {
        Command::ALL
            .iter()
            .map(|cmd| {
                let keys = self
                    .keys_for(*cmd)
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("  ");
                (keys, cmd.help())
            })
            .collect()
    }

    /// Markdown table of the keymap (used to keep `docs/KEYMAP.md` in sync).
    pub fn markdown(&self) -> String {
        let mut out = String::from("| Keys | Command | Description |\n|---|---|---|\n");
        for cmd in Command::ALL {
            let keys = self
                .keys_for(*cmd)
                .iter()
                .map(|c| format!("`{c}`"))
                .collect::<Vec<_>>()
                .join(" ");
            out.push_str(&format!("| {keys} | `{}` | {} |\n", cmd.id(), cmd.help()));
        }
        out
    }
}

/// The full `docs/KEYMAP.md` document.
pub fn docs_markdown() -> String {
    let mut out = String::from(
        "# Keymap\n\n\
         Default bindings for lazydisk (terminal and desktop). Every command can be rebound\n\
         in `~/.config/lazydisk/config.toml` under `[keys]` using the command id.\n\n\
         Generated from `lazydisk_core::keymap`; regenerate with\n\
         `cargo run -p lazydisk-core --example keymap > docs/KEYMAP.md`.\n\n",
    );
    out.push_str(&Keymap::defaults().markdown());
    out.push_str(
        "\n## Inside overlays\n\n\
         | Keys | Action |\n|---|---|\n\
         | any key | close the help or a message |\n\
         | typing, `backspace`, `enter`, `esc` | edit, accept or cancel the filter |\n\
         | `y` / `n` / `esc` | confirm or cancel moving to the Trash |\n\
         | type `DELETE`, `enter` / `esc` | confirm or cancel a permanent delete |\n\
         | `j`/`k`, `1`-`9`, `enter`, `esc` | move, pick or close a menu |\n\n\
         ## Mouse (terminal)\n\n\
         Click a segment to select it, click it again to open it, click the centre to go up,\n\
         scroll over the sunburst to go up and down, click rows to select them.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docs_keymap_is_up_to_date() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/KEYMAP.md");
        let on_disk = std::fs::read_to_string(path).unwrap_or_default();
        assert_eq!(
            on_disk,
            docs_markdown(),
            "docs/KEYMAP.md is stale; run `cargo run -p lazydisk-core --example keymap > docs/KEYMAP.md`"
        );
    }

    #[test]
    fn parse_specs() {
        assert_eq!(Chord::parse("ctrl+c").unwrap(), Chord::ctrl('c'));
        assert_eq!(Chord::parse("?").unwrap(), Chord::char('?'));
        assert_eq!(Chord::parse("shift+tab").unwrap(), Chord::key(Key::BackTab));
        assert_eq!(Chord::parse("PgDn").unwrap(), Chord::key(Key::PageDown));
        assert_eq!(Chord::parse("f5").unwrap(), Chord::key(Key::F(5)));
        assert!(Chord::parse("hyper+x").is_err());
        assert_eq!(Chord::parse("+").unwrap(), Chord::char('+'));
        assert_eq!(Chord::parse("ctrl++").unwrap(), Chord::ctrl('+'));
        assert!(Chord::parse("").is_err());
        assert_eq!(Chord::parse("space").unwrap().to_string(), "space");
    }

    #[test]
    fn defaults_have_no_conflicts() {
        let km = Keymap::defaults();
        let mut seen = HashMap::new();
        for (chord, cmd) in &km.bindings {
            if let Some(prev) = seen.insert(*chord, *cmd) {
                panic!("{chord} bound to both {prev:?} and {cmd:?}");
            }
        }
        assert_eq!(km.lookup(Chord::char('q')), Some(Command::Quit));
        assert_eq!(km.lookup(Chord::char('G')), Some(Command::Last));
    }

    #[test]
    fn overrides_replace_keys() {
        let mut km = Keymap::defaults();
        let mut o = HashMap::new();
        o.insert("quit".to_string(), vec!["ctrl+q".to_string()]);
        km.apply_overrides(&o).unwrap();
        assert_eq!(km.lookup(Chord::char('q')), None);
        assert_eq!(km.lookup(Chord::ctrl('q')), Some(Command::Quit));
        let mut bad = HashMap::new();
        bad.insert("fly".to_string(), vec!["f".to_string()]);
        assert!(km.apply_overrides(&bad).is_err());
    }
}
