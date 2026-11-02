//! Keyboard-driven application state shared by all frontends.
//!
//! Frontends feed [`Action`]s (usually [`Action::Key`]) into [`AppState::apply`],
//! call [`AppState::poll`] on every tick to pick up scanner and deletion
//! results, and render from the accessors. Nothing here knows about terminals
//! or windows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use parking_lot::RwLock;

use crate::config::Config;
use crate::format::Units;
use crate::keymap::{Chord, Command, Key, Keymap};
use crate::layout::{DEFAULT_MIN_SPAN, SunburstLayout};
use crate::ops::{self, DeleteMode, DeleteResult};
use crate::scan::{self, ScanEvent, ScanHandle, ScanOptions, SharedTree};
use crate::tree::{NodeFlags, NodeId, NodeKind, SizeMode, Tree};
use crate::volumes::{self, Volume};

/// Focusable panels, in `Tab` order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Volumes,
    Files,
    Collector,
    Details,
}

impl Panel {
    pub const ALL: [Panel; 4] = [
        Panel::Volumes,
        Panel::Files,
        Panel::Collector,
        Panel::Details,
    ];
    pub fn number(self) -> u8 {
        match self {
            Panel::Volumes => 1,
            Panel::Files => 2,
            Panel::Collector => 3,
            Panel::Details => 4,
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Panel::Volumes => "Volumes",
            Panel::Files => "Files",
            Panel::Collector => "Collector",
            Panel::Details => "Details",
        }
    }
    fn next(self) -> Panel {
        let i = Panel::ALL.iter().position(|p| *p == self).unwrap();
        Panel::ALL[(i + 1) % Panel::ALL.len()]
    }
    fn prev(self) -> Panel {
        let i = Panel::ALL.iter().position(|p| *p == self).unwrap();
        Panel::ALL[(i + Panel::ALL.len() - 1) % Panel::ALL.len()]
    }
}

/// Input to the state machine.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// A key chord; resolved through overlays first, then the keymap.
    Key(Chord),
    /// A command, bypassing the keymap (menus, GUI buttons).
    Command(Command),
    /// Make `node` the centre of the sunburst (mouse click on a directory).
    Focus(NodeId),
    /// Select `node` in the Files list without changing focus (hover / click).
    Select(NodeId),
    /// Start scanning the volume at `index` in [`AppState::volumes`].
    ScanVolume(usize),
    /// Scan an arbitrary path (CLI argument, drag-and-drop).
    ScanPath(PathBuf),
    /// Choose entry `index` of the open menu.
    MenuPick(usize),
}

/// Things the frontend must do; everything else is handled inside the core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Quit,
}

/// Modal layer above the panels.
#[derive(Clone, Debug, PartialEq)]
pub enum Overlay {
    Help,
    /// Filter prompt; the live text is [`AppState::filter`].
    Filter,
    ConfirmDelete {
        mode: DeleteMode,
        items: Vec<NodeId>,
        /// For permanent deletes the user must type [`CONFIRM_WORD`].
        typed: String,
    },
    Menu {
        title: String,
        items: Vec<(String, Command)>,
        selected: usize,
    },
    Message {
        title: String,
        lines: Vec<String>,
    },
}

/// Word that must be typed to confirm a permanent delete.
pub const CONFIRM_WORD: &str = "DELETE";

/// Severity of a status line message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug)]
pub struct Status {
    pub text: String,
    pub level: Level,
    pub at: Instant,
}

/// Where the current scan stands.
#[derive(Clone, Debug, PartialEq)]
pub enum ScanStatus {
    Idle,
    Running {
        entries: u64,
        bytes: u64,
        since: Instant,
    },
    Done {
        entries: u64,
        bytes: u64,
        errors: u64,
        elapsed: Duration,
    },
    Failed(String),
}

impl ScanStatus {
    pub fn is_running(&self) -> bool {
        matches!(self, ScanStatus::Running { .. })
    }
}

/// A deletion in progress: results channel, path → node, mode, successes, failures.
type Deletion = (
    Receiver<DeleteResult>,
    HashMap<PathBuf, NodeId>,
    DeleteMode,
    usize,
    usize,
);

/// How long a status message stays visible.
pub const STATUS_TTL: Duration = Duration::from_secs(4);

pub struct AppState {
    tree: SharedTree,
    root_path: PathBuf,
    keymap: Keymap,
    pub units: Units,
    size_mode: SizeMode,
    pub rings: u8,
    scan_options: ScanOptions,

    focus: NodeId,
    selected: usize,
    back: Vec<NodeId>,
    forward: Vec<NodeId>,
    pub panel: Panel,
    filter: String,
    filter_cache: Option<(u64, NodeId, String, Vec<NodeId>)>,
    layout_cache: Option<SunburstLayout>,

    collector: Vec<NodeId>,
    collector_selected: usize,
    pub volumes: Vec<Volume>,
    pub volume_selected: usize,
    /// Volume containing the scan root, refreshed after each scan.
    pub root_volume: Option<Volume>,

    overlay: Option<Overlay>,
    status: Option<Status>,
    scan: Option<(ScanHandle, Receiver<ScanEvent>, NodeId)>,
    scan_status: ScanStatus,
    pub scan_errors: Vec<String>,
    deleting: Option<Deletion>,
    quit: bool,
}

impl AppState {
    /// Create state for scanning `root`. Call [`AppState::start_scan`] to begin.
    pub fn new(root: &Path, config: &Config) -> Self {
        let mut keymap = Keymap::defaults();
        if let Err(e) = keymap.apply_overrides(&config.keys) {
            tracing::warn!("ignoring key overrides: {e}");
        }
        let mut tree = Tree::new(root);
        tree.set_size_mode(config.size_mode);
        let focus = tree.root();
        let volumes = volumes::list();
        let root_volume = volumes::containing(root);
        let volume_selected = root_volume
            .as_ref()
            .and_then(|rv| volumes.iter().position(|v| v.mount_point == rv.mount_point))
            .unwrap_or(0);
        Self {
            tree: Arc::new(RwLock::new(tree)),
            root_path: root.to_path_buf(),
            keymap,
            units: config.units,
            size_mode: config.size_mode,
            rings: config.rings.clamp(1, 8),
            scan_options: ScanOptions {
                threads: config.threads,
                stay_on_device: config.stay_on_device,
                dedupe_hardlinks: config.dedupe_hardlinks,
                dedupe_clones: config.dedupe_clones,
                prune: Vec::new(),
            },
            focus,
            selected: 0,
            back: Vec::new(),
            forward: Vec::new(),
            panel: Panel::Files,
            filter: String::new(),
            filter_cache: None,
            layout_cache: None,
            collector: Vec::new(),
            collector_selected: 0,
            volumes,
            volume_selected,
            root_volume,
            overlay: None,
            status: None,
            scan: None,
            scan_status: ScanStatus::Idle,
            scan_errors: Vec::new(),
            deleting: None,
            quit: false,
        }
    }

    // ----- accessors -------------------------------------------------------

    pub fn tree(&self) -> &SharedTree {
        &self.tree
    }
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }
    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }
    pub fn focus(&self) -> NodeId {
        self.focus
    }
    pub fn size_mode(&self) -> SizeMode {
        self.size_mode
    }
    pub fn overlay(&self) -> Option<&Overlay> {
        self.overlay.as_ref()
    }
    pub fn scan_status(&self) -> &ScanStatus {
        &self.scan_status
    }
    pub fn filter(&self) -> &str {
        &self.filter
    }
    pub fn collector(&self) -> &[NodeId] {
        &self.collector
    }
    pub fn collector_selected(&self) -> usize {
        self.collector_selected
    }
    pub fn should_quit(&self) -> bool {
        self.quit
    }
    pub fn is_deleting(&self) -> bool {
        self.deleting.is_some()
    }

    /// Current status message, if it has not expired.
    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref().filter(|s| s.at.elapsed() < STATUS_TTL)
    }

    pub fn set_status(&mut self, level: Level, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            level,
            at: Instant::now(),
        });
    }

    /// Children of the focused directory that match the filter, largest first.
    pub fn visible_children(&mut self) -> Vec<NodeId> {
        let tree = self.tree.read();
        let version = tree.version();
        if let Some((v, f, filt, cached)) = &self.filter_cache
            && *v == version
            && *f == self.focus
            && *filt == self.filter
        {
            return cached.clone();
        }
        let needle = self.filter.to_lowercase();
        let list: Vec<NodeId> = tree
            .children(self.focus)
            .iter()
            .copied()
            .filter(|c| {
                let n = tree.node(*c);
                !n.is_deleted() && (needle.is_empty() || n.name.to_lowercase().contains(&needle))
            })
            .collect();
        drop(tree);
        self.filter_cache = Some((version, self.focus, self.filter.clone(), list.clone()));
        list
    }

    /// Index of the selected row in [`AppState::visible_children`].
    pub fn selected_index(&mut self) -> usize {
        let len = self.visible_children().len();
        if len == 0 {
            0
        } else {
            self.selected.min(len - 1)
        }
    }

    /// The selected child of the focus, if any.
    pub fn selected_node(&mut self) -> Option<NodeId> {
        let list = self.visible_children();
        list.get(self.selected_index()).copied()
    }

    /// The node the next item-command acts on: depends on the focused panel.
    pub fn target_node(&mut self) -> Option<NodeId> {
        match self.panel {
            Panel::Collector => self.collector.get(self.collector_selected).copied(),
            _ => self.selected_node(),
        }
    }

    /// Sunburst layout for the current focus, recomputed only when inputs change.
    pub fn layout(&mut self) -> &SunburstLayout {
        let tree = self.tree.read();
        let stale = self.layout_cache.as_ref().is_none_or(|l| {
            l.tree_version != tree.version() || l.focus != self.focus || l.rings != self.rings
        });
        if stale {
            self.layout_cache = Some(SunburstLayout::compute(
                &tree,
                self.focus,
                self.rings,
                DEFAULT_MIN_SPAN,
            ));
        }
        drop(tree);
        self.layout_cache.as_ref().unwrap()
    }

    /// Total display size of the collected items.
    pub fn collector_total(&self) -> u64 {
        let tree = self.tree.read();
        self.collector.iter().map(|n| tree.display_size(*n)).sum()
    }

    // ----- scanning --------------------------------------------------------

    /// Scan the root from scratch.
    pub fn start_scan(&mut self) {
        let root = self.root_path.clone();
        self.replace_tree(&root);
    }

    fn replace_tree(&mut self, root: &Path) {
        self.scan = None; // cancels any running scan
        self.deleting = None;
        self.root_path = root.to_path_buf();
        let mut tree = Tree::new(root);
        tree.set_size_mode(self.size_mode);
        let focus = tree.root();
        *self.tree.write() = tree;
        self.focus = focus;
        self.selected = 0;
        self.back.clear();
        self.forward.clear();
        self.collector.clear();
        self.collector_selected = 0;
        self.filter.clear();
        self.filter_cache = None;
        self.layout_cache = None;
        self.root_volume = volumes::containing(root);
        self.scan_errors.clear();
        self.scan_node(focus);
    }

    fn scan_node(&mut self, node: NodeId) {
        let (handle, rx) = scan::scan(Arc::clone(&self.tree), node, self.scan_options.clone());
        self.scan = Some((handle, rx, node));
        self.scan_status = ScanStatus::Running {
            entries: 0,
            bytes: 0,
            since: Instant::now(),
        };
    }

    fn rescan(&mut self, node: NodeId) {
        if self.scan_status.is_running() {
            self.set_status(Level::Warn, "A scan is already running");
            return;
        }
        {
            let mut tree = self.tree.write();
            if !tree.node(node).kind.is_dir() {
                return;
            }
            tree.clear_children(node);
        }
        self.collector.retain(|c| {
            let t = self.tree.read();
            !t.node(*c).is_deleted()
        });
        self.filter_cache = None;
        self.scan_node(node);
    }

    /// Drain scanner and deletion results. Returns `true` when something changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        let scan_rx = self.scan.as_ref().map(|(_, rx, _)| rx.clone());
        if let Some(rx) = scan_rx {
            let mut finished = false;
            while let Ok(ev) = rx.try_recv() {
                changed = true;
                match ev {
                    ScanEvent::Progress { entries, bytes } => {
                        if let ScanStatus::Running { since, .. } = self.scan_status {
                            self.scan_status = ScanStatus::Running {
                                entries,
                                bytes,
                                since,
                            };
                        }
                    }
                    ScanEvent::Finished {
                        entries,
                        bytes,
                        errors,
                        elapsed,
                        error_samples,
                        cancelled,
                    } => {
                        self.scan_status = ScanStatus::Done {
                            entries,
                            bytes,
                            errors,
                            elapsed,
                        };
                        self.scan_errors = error_samples;
                        finished = true;
                        if cancelled {
                            self.set_status(Level::Warn, "Scan cancelled");
                        } else if errors > 0 {
                            self.set_status(
                                Level::Warn,
                                format!(
                                    "Scan finished; {errors} items could not be read (try sudo)"
                                ),
                            );
                        } else {
                            self.set_status(
                                Level::Info,
                                format!("Scan finished in {:.1}s", elapsed.as_secs_f64()),
                            );
                        }
                    }
                    ScanEvent::Failed(msg) => {
                        self.scan_status = ScanStatus::Failed(msg.clone());
                        self.set_status(Level::Error, msg);
                        finished = true;
                    }
                }
            }
            if finished {
                self.scan = None;
                self.add_volume_items();
            }
        }
        if let Some((rx, map, mode, ok, failed)) = &mut self.deleting {
            let mut done = false;
            loop {
                match rx.try_recv() {
                    Ok(res) => {
                        changed = true;
                        match res.result {
                            Ok(()) => {
                                *ok += 1;
                                if let Some(id) = map.get(&res.path) {
                                    self.tree.write().remove(*id);
                                    self.collector.retain(|c| c != id);
                                }
                            }
                            Err(e) => {
                                *failed += 1;
                                tracing::warn!("delete {}: {e}", res.path.display());
                                self.status = Some(Status {
                                    text: format!("{}: {e}", res.path.display()),
                                    level: Level::Error,
                                    at: Instant::now(),
                                });
                            }
                        }
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        done = true;
                        break;
                    }
                }
            }
            if done {
                let (ok, failed, mode) = (*ok, *failed, *mode);
                self.deleting = None;
                self.filter_cache = None;
                self.collector_selected = 0;
                if failed == 0 {
                    let what = match mode {
                        DeleteMode::Trash => "moved to the Trash",
                        DeleteMode::Permanent => "deleted",
                    };
                    self.set_status(Level::Info, format!("{ok} items {what}"));
                } else {
                    self.set_status(Level::Error, format!("{ok} items deleted, {failed} failed"));
                }
                changed = true;
            }
        }
        if changed {
            let mut tree = self.tree.write();
            tree.sort_children();
        }
        changed
    }

    /// After a scan of a mount point, add hidden and free space as virtual children.
    fn add_volume_items(&mut self) {
        let Some(vol) = volumes::containing(&self.root_path) else {
            return;
        };
        let canon =
            std::fs::canonicalize(&self.root_path).unwrap_or_else(|_| self.root_path.clone());
        self.root_volume = Some(vol.clone());
        if vol.mount_point != canon {
            return;
        }
        let mut tree = self.tree.write();
        let root = tree.root();
        let scanned = tree.node(root).size;
        let hidden = vol.used().saturating_sub(scanned);
        if hidden > 0 {
            tree.set_virtual_child(root, NodeKind::HiddenSpace, hidden);
        }
        tree.set_virtual_child(root, NodeKind::FreeSpace, vol.available);
        tree.sort_children();
    }

    // ----- actions ---------------------------------------------------------

    pub fn apply(&mut self, action: Action) -> Vec<Effect> {
        let mut effects = Vec::new();
        match action {
            Action::Key(chord) => {
                if self.overlay.is_some() {
                    self.overlay_key(chord, &mut effects);
                } else if let Some(cmd) = self.keymap.lookup(chord) {
                    self.command(cmd, &mut effects);
                }
            }
            Action::Command(cmd) => self.command(cmd, &mut effects),
            Action::Focus(node) => self.set_focus(node, true),
            Action::Select(node) => self.select_node(node),
            Action::ScanVolume(i) => {
                if let Some(v) = self.volumes.get(i).cloned() {
                    self.volume_selected = i;
                    self.panel = Panel::Files;
                    self.replace_tree(&v.mount_point);
                }
            }
            Action::ScanPath(p) => self.replace_tree(&p),
            Action::MenuPick(i) => {
                if let Some(Overlay::Menu { items, .. }) = &self.overlay
                    && let Some((_, cmd)) = items.get(i).cloned()
                {
                    self.overlay = None;
                    self.command(cmd, &mut effects);
                }
            }
        }
        if self.quit && !effects.contains(&Effect::Quit) {
            effects.push(Effect::Quit);
        }
        effects
    }

    fn overlay_key(&mut self, chord: Chord, effects: &mut Vec<Effect>) {
        let Some(overlay) = self.overlay.clone() else {
            return;
        };
        match overlay {
            Overlay::Help | Overlay::Message { .. } => {
                self.overlay = None;
            }
            Overlay::Filter => match chord.key {
                Key::Esc => {
                    self.filter.clear();
                    self.overlay = None;
                }
                Key::Enter => self.overlay = None,
                Key::Backspace => {
                    self.filter.pop();
                    self.selected = 0;
                }
                Key::Char(c) if !chord.ctrl && !chord.alt => {
                    self.filter.push(c);
                    self.selected = 0;
                }
                Key::Space => self.filter.push(' '),
                Key::Down | Key::Up => {
                    self.overlay = None;
                    self.move_selection(if chord.key == Key::Down { 1 } else { -1 });
                }
                _ => {
                    if chord == Chord::ctrl('c') {
                        self.filter.clear();
                        self.overlay = None;
                    }
                }
            },
            Overlay::ConfirmDelete {
                mode,
                items,
                mut typed,
            } => match (mode, chord.key) {
                (_, Key::Esc) => {
                    self.overlay = None;
                    self.set_status(Level::Info, "Cancelled");
                }
                (DeleteMode::Trash, Key::Char('y' | 'Y')) | (DeleteMode::Trash, Key::Enter) => {
                    self.overlay = None;
                    self.begin_delete(items, mode);
                }
                (DeleteMode::Trash, Key::Char('n' | 'N')) => {
                    self.overlay = None;
                    self.set_status(Level::Info, "Cancelled");
                }
                (DeleteMode::Permanent, Key::Enter) => {
                    if typed == CONFIRM_WORD {
                        self.overlay = None;
                        self.begin_delete(items, mode);
                    }
                }
                (DeleteMode::Permanent, Key::Backspace) => {
                    typed.pop();
                    self.overlay = Some(Overlay::ConfirmDelete { mode, items, typed });
                }
                (DeleteMode::Permanent, Key::Char(c))
                    if !chord.ctrl && typed.len() < CONFIRM_WORD.len() =>
                {
                    typed.push(c);
                    self.overlay = Some(Overlay::ConfirmDelete { mode, items, typed });
                }
                _ => {
                    if chord == Chord::ctrl('c') {
                        self.overlay = None;
                    }
                }
            },
            Overlay::Menu {
                title,
                items,
                mut selected,
            } => {
                let cmd = self.keymap.lookup(chord);
                match (chord.key, cmd) {
                    (Key::Esc, _) | (_, Some(Command::Menu)) | (_, Some(Command::Quit)) => {
                        self.overlay = None
                    }
                    (Key::Enter, _) => {
                        self.overlay = None;
                        if let Some((_, c)) = items.get(selected) {
                            self.command(*c, effects);
                        }
                    }
                    (_, Some(Command::Down)) => {
                        selected = (selected + 1) % items.len().max(1);
                        self.overlay = Some(Overlay::Menu {
                            title,
                            items,
                            selected,
                        });
                    }
                    (_, Some(Command::Up)) => {
                        selected = (selected + items.len().max(1) - 1) % items.len().max(1);
                        self.overlay = Some(Overlay::Menu {
                            title,
                            items,
                            selected,
                        });
                    }
                    (Key::Char(c), _) if c.is_ascii_digit() => {
                        let i = c.to_digit(10).unwrap() as usize;
                        if i >= 1 && i <= items.len() {
                            self.overlay = None;
                            self.command(items[i - 1].1, effects);
                        }
                    }
                    (_, Some(c)) if items.iter().any(|(_, ic)| *ic == c) => {
                        // Pressing a command's own key inside the menu runs it.
                        self.overlay = None;
                        self.command(c, effects);
                    }
                    _ => {}
                }
            }
        }
    }

    fn command(&mut self, cmd: Command, effects: &mut Vec<Effect>) {
        match cmd {
            Command::Quit => {
                self.quit = true;
                effects.push(Effect::Quit);
            }
            Command::Help => self.overlay = Some(Overlay::Help),
            Command::Up => self.move_selection(-1),
            Command::Down => self.move_selection(1),
            Command::PageUp => self.move_selection(-10),
            Command::PageDown => self.move_selection(10),
            Command::First => self.move_selection(isize::MIN / 2),
            Command::Last => self.move_selection(isize::MAX / 2),
            Command::Enter => self.enter(),
            Command::Parent => self.go_parent(),
            Command::Back => self.history(true),
            Command::Forward => self.history(false),
            Command::FocusVolumes => self.panel = Panel::Volumes,
            Command::FocusFiles => self.panel = Panel::Files,
            Command::FocusCollector => self.panel = Panel::Collector,
            Command::FocusDetails => self.panel = Panel::Details,
            Command::NextPanel => self.panel = self.panel.next(),
            Command::PrevPanel => self.panel = self.panel.prev(),
            Command::Filter => {
                self.panel = Panel::Files;
                self.overlay = Some(Overlay::Filter);
            }
            Command::ToggleSizeMode => {
                self.size_mode = self.size_mode.toggle();
                self.tree.write().set_size_mode(self.size_mode);
                self.filter_cache = None;
                self.set_status(
                    Level::Info,
                    format!("Showing {} size", self.size_mode.label()),
                );
            }
            Command::ToggleUnits => {
                self.units = match self.units {
                    Units::Binary => Units::Si,
                    Units::Si => Units::Binary,
                };
            }
            Command::MoreRings => self.rings = (self.rings + 1).min(8),
            Command::FewerRings => self.rings = (self.rings - 1).max(1),
            Command::Reveal => self.with_target_path(ops::reveal, "Revealed"),
            Command::Preview => self.with_target_path(ops::preview, "Preview opened"),
            Command::CopyPath => self.with_target_path(
                |p| ops::copy_to_clipboard(&p.to_string_lossy()),
                "Path copied",
            ),
            Command::ToggleCollect => self.toggle_collect(),
            Command::ClearCollector => {
                self.collector.clear();
                self.collector_selected = 0;
                self.set_status(Level::Info, "Collector emptied");
            }
            Command::DeleteTrash => self.request_delete(DeleteMode::Trash),
            Command::DeletePermanent => self.request_delete(DeleteMode::Permanent),
            Command::Rescan => {
                let f = self.focus;
                self.rescan(f);
            }
            Command::RescanAll => {
                let root = self.root_path.clone();
                self.replace_tree(&root);
            }
            Command::Menu => self.open_menu(),
        }
    }

    fn move_selection(&mut self, delta: isize) {
        match self.panel {
            Panel::Volumes => {
                let len = self.volumes.len();
                self.volume_selected = step(self.volume_selected, delta, len);
            }
            Panel::Collector => {
                let len = self.collector.len();
                self.collector_selected = step(self.collector_selected, delta, len);
            }
            Panel::Files | Panel::Details => {
                let len = self.visible_children().len();
                self.selected = step(self.selected_index(), delta, len);
            }
        }
    }

    fn enter(&mut self) {
        match self.panel {
            Panel::Volumes => {
                let i = self.volume_selected;
                if let Some(v) = self.volumes.get(i).cloned() {
                    self.panel = Panel::Files;
                    self.replace_tree(&v.mount_point);
                }
            }
            Panel::Collector => {
                // Jump to the collected item.
                if let Some(node) = self.collector.get(self.collector_selected).copied() {
                    let parent = self.tree.read().node(node).parent;
                    if let Some(p) = parent {
                        self.set_focus(p, true);
                    }
                    self.select_node(node);
                    self.panel = Panel::Files;
                }
            }
            Panel::Files | Panel::Details => {
                if let Some(node) = self.selected_node() {
                    let kind = self.tree.read().node(node).kind;
                    if kind.is_dir() {
                        self.set_focus(node, true);
                    } else if kind.is_virtual() {
                        self.explain_virtual(kind);
                    } else {
                        self.set_status(
                            Level::Info,
                            "Not a folder; press space to preview, o to reveal",
                        );
                    }
                }
            }
        }
    }

    fn explain_virtual(&mut self, kind: NodeKind) {
        let lines = match kind {
            NodeKind::HiddenSpace => vec![
                "Space the volume reports as used that no scanned file accounts for.".into(),
                "Typically: purgeable space (local Time Machine snapshots, swap),".into(),
                "other APFS volumes sharing this container, folders this user may not read,".into(),
                "and filesystem overhead. Scanning with sudo usually shrinks it.".into(),
            ],
            NodeKind::FreeSpace => vec!["Space available on the volume.".into()],
            _ => return,
        };
        self.overlay = Some(Overlay::Message {
            title: if kind == NodeKind::HiddenSpace {
                "Hidden space"
            } else {
                "Free space"
            }
            .into(),
            lines,
        });
    }

    fn set_focus(&mut self, node: NodeId, record_history: bool) {
        if node == self.focus {
            return;
        }
        {
            let tree = self.tree.read();
            if !tree.node(node).kind.is_dir() || tree.node(node).is_deleted() {
                return;
            }
        }
        if record_history {
            self.back.push(self.focus);
            self.forward.clear();
        }
        let old = self.focus;
        self.focus = node;
        self.filter.clear();
        self.selected = 0;
        // When going up, keep the folder we came from selected.
        let is_parent = self.tree.read().node(old).parent == Some(node);
        if is_parent {
            self.select_node(old);
        }
    }

    fn select_node(&mut self, node: NodeId) {
        let list = self.visible_children();
        if let Some(i) = list.iter().position(|n| *n == node) {
            self.selected = i;
            if self.panel != Panel::Files {
                self.panel = Panel::Files;
            }
        }
    }

    fn go_parent(&mut self) {
        if self.panel != Panel::Files {
            self.panel = Panel::Files;
            return;
        }
        if !self.filter.is_empty() {
            self.filter.clear();
            self.selected = 0;
            return;
        }
        let parent = self.tree.read().node(self.focus).parent;
        if let Some(p) = parent {
            self.set_focus(p, true);
        }
    }

    fn history(&mut self, back: bool) {
        let (from, to) = if back {
            (&mut self.back, &mut self.forward)
        } else {
            (&mut self.forward, &mut self.back)
        };
        if let Some(node) = from.pop() {
            to.push(self.focus);
            self.set_focus(node, false);
        }
    }

    fn with_target_path(&mut self, f: impl Fn(&Path) -> Result<(), String>, ok: &str) {
        let Some(node) = self.target_node() else {
            self.set_status(Level::Warn, "Nothing selected");
            return;
        };
        let (path, is_virtual) = {
            let t = self.tree.read();
            (t.path(node), t.node(node).kind.is_virtual())
        };
        if is_virtual {
            self.set_status(Level::Warn, "That is a virtual item");
            return;
        }
        match f(&path) {
            Ok(()) => self.set_status(Level::Info, ok),
            Err(e) => self.set_status(Level::Error, e),
        }
    }

    fn toggle_collect(&mut self) {
        let Some(node) = self.target_node() else {
            self.set_status(Level::Warn, "Nothing selected");
            return;
        };
        if let Some(i) = self.collector.iter().position(|n| *n == node) {
            self.collector.remove(i);
            self.collector_selected = self
                .collector_selected
                .min(self.collector.len().saturating_sub(1));
            self.set_status(Level::Info, "Removed from Collector");
            return;
        }
        let (path, kind) = {
            let t = self.tree.read();
            (t.path(node), t.node(node).kind)
        };
        if kind.is_virtual() {
            self.set_status(Level::Warn, "Virtual items cannot be collected");
            return;
        }
        if let Err(r) = ops::guard_delete(&path) {
            self.set_status(Level::Error, r.to_string());
            return;
        }
        // Drop descendants/ancestors already collected to avoid double deletion.
        let tree = self.tree.read();
        if self
            .collector
            .iter()
            .any(|c| tree.is_ancestor_or_self(*c, node))
        {
            drop(tree);
            self.set_status(Level::Warn, "An enclosing folder is already collected");
            return;
        }
        let nested: Vec<NodeId> = self
            .collector
            .iter()
            .copied()
            .filter(|c| tree.is_ancestor_or_self(node, *c))
            .collect();
        drop(tree);
        self.collector.retain(|c| !nested.contains(c));
        self.collector.push(node);
        let msg = if nested.is_empty() {
            "Added to Collector".to_string()
        } else {
            format!(
                "Added to Collector (replaced {} nested items)",
                nested.len()
            )
        };
        self.set_status(Level::Info, msg);
        if self.panel == Panel::Files {
            self.move_selection(1);
        }
    }

    fn request_delete(&mut self, mode: DeleteMode) {
        if self.deleting.is_some() {
            self.set_status(Level::Warn, "A deletion is already running");
            return;
        }
        if self.collector.is_empty() {
            self.set_status(
                Level::Warn,
                "Collector is empty; press c to collect items first",
            );
            return;
        }
        self.overlay = Some(Overlay::ConfirmDelete {
            mode,
            items: self.collector.clone(),
            typed: String::new(),
        });
    }

    fn begin_delete(&mut self, items: Vec<NodeId>, mode: DeleteMode) {
        let tree = self.tree.read();
        let mut map = HashMap::new();
        let mut paths = Vec::new();
        for id in items {
            let p = tree.path(id);
            map.insert(p.clone(), id);
            paths.push(p);
        }
        drop(tree);
        let rx = ops::delete_all(paths, mode);
        self.deleting = Some((rx, map, mode, 0, 0));
        self.set_status(Level::Info, format!("{}…", mode.verb()));
    }

    fn open_menu(&mut self) {
        let Some(node) = self.target_node() else {
            self.set_status(Level::Warn, "Nothing selected");
            return;
        };
        let (name, is_dir, collected) = {
            let t = self.tree.read();
            (
                t.node(node).name.to_string(),
                t.node(node).kind.is_dir(),
                self.collector.contains(&node),
            )
        };
        let mut items = Vec::new();
        if is_dir {
            items.push(("Open".to_string(), Command::Enter));
        }
        items.push(("Reveal in Finder".to_string(), Command::Reveal));
        items.push(("Preview".to_string(), Command::Preview));
        items.push(("Copy path".to_string(), Command::CopyPath));
        items.push((
            if collected {
                "Remove from Collector"
            } else {
                "Add to Collector"
            }
            .to_string(),
            Command::ToggleCollect,
        ));
        if is_dir {
            items.push(("Rescan this folder".to_string(), Command::Rescan));
        }
        if !self.collector.is_empty() {
            items.push(("Move collected to Trash".to_string(), Command::DeleteTrash));
            items.push((
                "Delete collected permanently".to_string(),
                Command::DeletePermanent,
            ));
        }
        self.overlay = Some(Overlay::Menu {
            title: name,
            items,
            selected: 0,
        });
    }

    /// Breadcrumb from the root to the focus.
    pub fn breadcrumb(&self) -> Vec<(NodeId, String)> {
        let tree = self.tree.read();
        tree.ancestry(self.focus)
            .into_iter()
            .map(|id| (id, tree.node(id).name.to_string()))
            .collect()
    }

    /// Flag text for a node (`hard link`, `clone`, `unreadable`, ...).
    pub fn flag_labels(flags: NodeFlags) -> Vec<&'static str> {
        let mut v = Vec::new();
        if flags.contains(NodeFlags::UNREADABLE) {
            v.push("unreadable");
        }
        if flags.contains(NodeFlags::HARDLINK_DUP) {
            v.push("hard link (counted elsewhere)");
        }
        if flags.contains(NodeFlags::CLONE_DUP) {
            v.push("APFS clone (counted elsewhere)");
        }
        if flags.contains(NodeFlags::PRUNED) {
            v.push("not entered (other volume)");
        }
        if flags.contains(NodeFlags::SCANNING) {
            v.push("scanning");
        }
        v
    }
}

fn step(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let cur = current.min(len - 1) as isize;
    (cur.saturating_add(delta)).clamp(0, len as isize - 1) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn fixture() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("big/inner")).unwrap();
        fs::create_dir_all(root.join("small")).unwrap();
        let mut f = fs::File::create(root.join("big/inner/data.bin")).unwrap();
        f.write_all(&vec![0u8; 50_000]).unwrap();
        fs::write(root.join("small/a.txt"), b"hello").unwrap();
        fs::write(root.join("top.txt"), b"hi").unwrap();
        let mut st = AppState::new(root, &Config::default());
        st.start_scan();
        let deadline = Instant::now() + Duration::from_secs(10);
        while st.scan_status().is_running() && Instant::now() < deadline {
            st.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            matches!(st.scan_status(), ScanStatus::Done { .. }),
            "{:?}",
            st.scan_status()
        );
        (dir, st)
    }

    fn key(st: &mut AppState, spec: &str) -> Vec<Effect> {
        st.apply(Action::Key(Chord::parse(spec).unwrap()))
    }

    #[test]
    fn navigation_and_history() {
        let (_d, mut st) = fixture();
        let root = st.focus();
        let list = st.visible_children();
        assert_eq!(list.len(), 3);
        let big = list[0];
        assert_eq!(st.tree().read().node(big).name.as_ref(), "big");
        key(&mut st, "enter");
        assert_eq!(st.focus(), big);
        key(&mut st, "l");
        let inner = st.focus();
        assert_eq!(st.tree().read().node(inner).name.as_ref(), "inner");
        key(&mut st, "h");
        assert_eq!(st.focus(), big);
        assert_eq!(
            st.selected_node(),
            Some(inner),
            "coming up keeps the child selected"
        );
        key(&mut st, "[");
        assert_eq!(st.focus(), inner);
        key(&mut st, "]");
        assert_eq!(st.focus(), big);
        key(&mut st, "esc");
        key(&mut st, "esc");
        assert_eq!(st.focus(), root);
        assert_eq!(st.breadcrumb().len(), 1);
        assert_eq!(key(&mut st, "q"), vec![Effect::Quit]);
    }

    #[test]
    fn selection_moves_and_clamps() {
        let (_d, mut st) = fixture();
        key(&mut st, "j");
        assert_eq!(st.selected_index(), 1);
        key(&mut st, "G");
        assert_eq!(st.selected_index(), 2);
        key(&mut st, "j");
        assert_eq!(st.selected_index(), 2);
        key(&mut st, "g");
        assert_eq!(st.selected_index(), 0);
        key(&mut st, "ctrl+d");
        assert_eq!(st.selected_index(), 2);
    }

    #[test]
    fn filter_narrows_list() {
        let (_d, mut st) = fixture();
        key(&mut st, "/");
        assert_eq!(st.overlay(), Some(&Overlay::Filter));
        key(&mut st, "s");
        key(&mut st, "m");
        assert_eq!(st.filter(), "sm");
        assert_eq!(st.visible_children().len(), 1);
        key(&mut st, "enter");
        assert_eq!(st.overlay(), None);
        key(&mut st, "esc"); // esc with an active filter clears it instead of going up
        assert_eq!(st.filter(), "");
        assert_eq!(st.visible_children().len(), 3);
    }

    #[test]
    fn panels_cycle() {
        let (_d, mut st) = fixture();
        assert_eq!(st.panel, Panel::Files);
        key(&mut st, "tab");
        assert_eq!(st.panel, Panel::Collector);
        key(&mut st, "1");
        assert_eq!(st.panel, Panel::Volumes);
        key(&mut st, "shift+tab");
        assert_eq!(st.panel, Panel::Details);
    }

    #[test]
    fn collect_and_trash_confirm_flow() {
        let (_d, mut st) = fixture();
        key(&mut st, "d");
        assert!(
            st.overlay().is_none(),
            "empty collector shows a status, not a dialog"
        );
        key(&mut st, "c");
        assert_eq!(st.collector().len(), 1);
        assert_eq!(st.selected_index(), 1, "collecting advances the selection");
        key(&mut st, "c");
        assert_eq!(st.collector().len(), 2);
        key(&mut st, "d");
        assert!(matches!(
            st.overlay(),
            Some(Overlay::ConfirmDelete {
                mode: DeleteMode::Trash,
                ..
            })
        ));
        key(&mut st, "n");
        assert!(st.overlay().is_none());
        key(&mut st, "C");
        assert!(st.collector().is_empty());
    }

    #[test]
    fn nested_collect_replaces_children() {
        let (_d, mut st) = fixture();
        let big = st.visible_children()[0];
        key(&mut st, "enter");
        key(&mut st, "c"); // collects big/inner
        key(&mut st, "h");
        st.apply(Action::Select(big));
        key(&mut st, "c"); // collects big, replacing inner
        assert_eq!(st.collector(), &[big]);
        st.apply(Action::Select(big)); // collecting advanced the selection; go back to big
        key(&mut st, "enter");
        key(&mut st, "c"); // inner again: refused, enclosing folder collected
        assert_eq!(st.collector(), &[big]);
    }

    #[test]
    fn permanent_delete_requires_typed_word_and_removes_files() {
        let (dir, mut st) = fixture();
        let list = st.visible_children();
        let small = list[1];
        assert_eq!(st.tree().read().node(small).name.as_ref(), "small");
        st.apply(Action::Select(small));
        key(&mut st, "c");
        key(&mut st, "D");
        key(&mut st, "enter");
        assert!(
            st.overlay().is_some(),
            "enter without the word does nothing"
        );
        for ch in "DELETE".chars() {
            key(&mut st, &ch.to_string());
        }
        key(&mut st, "enter");
        assert!(st.overlay().is_none());
        let deadline = Instant::now() + Duration::from_secs(5);
        while st.is_deleting() && Instant::now() < deadline {
            st.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!dir.path().join("small").exists());
        assert!(st.collector().is_empty());
        assert_eq!(st.visible_children().len(), 2);
        assert!(st.tree().read().node(small).is_deleted());
    }

    #[test]
    fn menu_runs_commands() {
        let (_d, mut st) = fixture();
        key(&mut st, "x");
        assert!(matches!(st.overlay(), Some(Overlay::Menu { .. })));
        key(&mut st, "esc");
        assert!(st.overlay().is_none());
        key(&mut st, "x");
        key(&mut st, "enter"); // first item for a dir is Open
        let big = st.focus();
        assert_eq!(st.tree().read().node(big).name.as_ref(), "big");
    }

    #[test]
    fn rescan_keeps_totals() {
        let (_d, mut st) = fixture();
        let before = st.tree().read().node(st.focus()).size;
        key(&mut st, "r");
        let deadline = Instant::now() + Duration::from_secs(10);
        while st.scan_status().is_running() && Instant::now() < deadline {
            st.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(st.tree().read().node(st.focus()).size, before);
        assert_eq!(st.visible_children().len(), 3);
        let l = st.layout();
        assert!(l.ring(1).len() >= 2);
    }

    #[test]
    fn help_and_toggles() {
        let (_d, mut st) = fixture();
        key(&mut st, "?");
        assert_eq!(st.overlay(), Some(&Overlay::Help));
        key(&mut st, "j");
        assert!(st.overlay().is_none());
        key(&mut st, "s");
        assert_eq!(st.size_mode(), SizeMode::Apparent);
        key(&mut st, "+");
        assert_eq!(st.rings, 5);
        key(&mut st, "-");
        key(&mut st, "-");
        assert_eq!(st.rings, 3);
        key(&mut st, "u");
        assert_eq!(st.units, Units::Si);
    }
}
