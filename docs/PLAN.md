# lazydisk — a DaisyDisk-style disk analyzer in Rust (ratatui + gpui)

> Status (2026-11-02): milestones 1–7 are implemented; milestone 8 (polish) is open.
> Measured on this Mac: scanning `~/Library` (1.87M entries) takes 10–16 s against 34 s for `du -sk`,
> and the allocated total matches `du` within 0.4 % before clone deduplication.

## Context

The user wants a disk-space analyzer like DaisyDisk (sunburst map of what uses space, drill in, collect, delete) but keyboard-driven in the lazygit style: numbered panels, single-key hotkeys, `?` help, everything reachable without a mouse. Two frontends: a terminal UI with ratatui and a desktop app with gpui. Both share one core.

Decisions already made with the user:
- Build order: shared core + ratatui TUI first, gpui desktop second.
- Deletion: Collector moves items to the macOS Trash by default; permanent delete is a separate, confirmed action.
- Platform: macOS first with a portable core. Linux must compile, but macOS extras (hidden space, snapshots, Quick Look, Finder) are `cfg(target_os = "macos")`.

`/Users/henrikeidlaug/code/lazydisk` is empty and not a git repo. The plan starts from `cargo new`. No Lapp notes are possible until the repo has a git remote (the `lapp-notes` skill requires one); plan lives in this file until then.

## Research findings that shape the design

- **gpui**: the official `gpui` crate on crates.io is stuck at 0.2.2 (Oct 2025). Use **`gpui-ce` 0.3.x** (community fork for standalone apps, Rust ≥1.95, we have 1.95), renamed in Cargo.toml so code says `use gpui::…`. Custom drawing: `canvas()` element + `PathBuilder::fill()` with `move_to / arc_to / line_to / close` → `Window::paint_path`. Build paths once per layout change and cache them (tessellation is the expensive step). Metal toolchain is present (Xcode at `/Applications/Xcode.app`), which gpui needs on macOS.
- **ratatui 0.30**: built-in `Canvas` gives one colour per cell, which is wrong for a colour-coded sunburst. Write a custom `Widget` that rasterizes directly into the `Buffer` using half-block glyphs (`▀ ▄ █`) with fg/bg so every cell carries two independently coloured vertical pixels. Compensate the ~2:1 cell aspect ratio in the polar math.
- **Scanning**: `jwalk` is deprecated in favour of **`dua-core` 6.0** (parallel work-stealing traversal, `walk_roots`, metadata collected during enumeration on macOS, symlinks never followed, `descend` predicate for boundaries). `dust`/`dua` conventions to copy: size on disk (`st_blocks * 512`) by default, apparent size as a toggle, hard links counted once by `(dev, ino)` when `nlink > 1`.
- **APFS clones** cannot be detected reliably from userland (`EF_MAY_SHARE_BLOCKS` only means "was ever cloned"). DaisyDisk itself double-counts them. Accept this, document it, show the "may share blocks" flag in details.
- **DaisyDisk model worth copying**: center = current folder, click center = go up; petal width ∝ size; folders coloured, files grey; too-thin items merged into a translucent "smaller objects" group; sidebar lists the hovered/selected folder's children; purple = unreadable (needs admin); "Hidden space" virtual item = volume used − scanned total; Collector at the bottom, refuses `/System`, `/Library`, `/`, the home folder; history back/forward (⌘[ ⌘]); Space = Quick Look; ⌘-click = reveal in Finder; ⌘R rescan folder, ⇧⌘R rescan all.

## Architecture

Cargo workspace, three crates, two binaries:

```
lazydisk/
  Cargo.toml                 # [workspace], shared [workspace.dependencies], lints
  crates/
    lazydisk-core/           # no UI deps. Everything both frontends share.
    lazydisk-tui/            # bin `lazydisk` (ratatui + crossterm)
    lazydisk-gpui/           # bin `lazydisk-desktop` (gpui-ce)   — phase 2
  docs/
    KEYMAP.md                # single source of truth for hotkeys, mirrored in `?` help
```

### lazydisk-core

- `scan/` — `Scanner::start(ScanRequest) -> (ScanHandle, Receiver<ScanEvent>)`. Runs `dua_core::walk_roots` on a background thread pool, builds the tree incrementally, emits `ScanEvent::{Progress{entries, bytes}, Snapshot(Arc<Tree>)/Delta, Finished, Error}` at a throttled rate (~10 Hz) so the UI animates while scanning. Options: `SizeMode::{Allocated, Apparent}`, `count_hardlinks_once`, `stay_on_device`, `follow_firmlinks` (macOS: let `/` descend into `/System/Volumes/Data`). Permission errors are recorded on the node (`NodeFlags::UNREADABLE`) not fatal.
- `tree/` — arena `Tree { nodes: Vec<Node>, root: NodeId }`, `Node { name: Box<str>, parent, children: Range or Vec<NodeId>, size, apparent, kind: Dir|File|Symlink|Other|Synthetic, flags, mtime, ino/dev }`. Children kept sorted by size descending after each aggregate pass. Synthetic nodes: `SmallerObjects`, `HiddenSpace`, `FreeSpace`.
- `layout/` — pure geometry shared by both renderers. `sunburst_layout(tree, focus, depth_limit, min_angle) -> Vec<Segment { node, depth, a0, a1, kind }>` in normalized units (angles in radians, depth as ring index). Hit test `segment_at(angle, depth)`. Colour assignment: hue by top-level child angle, lightness by depth, grey for files, translucent for "smaller objects", purple for unreadable. Both frontends only map radians/rings to pixels or cells.
- `state/` — `AppState` + `Action` enum + `fn apply(&mut self, Action) -> Vec<Effect>`. Holds focus node, selected child, history stack (back/forward), collector, filter string, size mode, scan status, panel focus. Frontends translate keys → `Action` and run `Effect`s (StartScan, Delete, OpenInFinder, Preview…). This keeps both UIs identical in behaviour and makes the whole app testable without a terminal.
- `ops/` — `trash` crate for move-to-Trash; `std::fs::remove_*` for permanent delete behind a typed confirmation; `reveal(path)` (`open -R`), `preview(path)` (`qlmanage -p`); safeguards list (`/`, `/System`, `/Library`, `/Users`, `$HOME`, `/Volumes/*` mount points, `/private/var/db`). Delete runs on a background thread and reports per-item results; tree is patched (node struck through) rather than rescanned.
- `volumes/` — list mounted volumes with capacity/free (`statfs` via `nix`/`libc`; `sysinfo` as the portable fallback). macOS: compute hidden space = used − scanned; `tmutil listlocalsnapshots /` for snapshots (read-only listing in v1).
- `config/` — `~/.config/lazydisk/config.toml`: keybinding overrides (lazygit-style), colour theme, defaults (size mode, depth rings). `serde` + `toml`.
- `format/` — human sizes (binary by default, SI toggle), percentages.

### lazydisk-tui (ratatui 0.30 + crossterm)

Layout, lazygit-flavoured:

```
┌1 Volumes ───┐┌ /Users/henrik/Library  42.1 GB  ──────────────────┐
│ Macintosh HD││                                                      │
│ External    ││               ◉  sunburst (half-block raster)        │
├2 Files ─────┤│                                                      │
│ ▶ Caches  12G│                                                      │
│   Appl…   9G ││                                                      │
│   Cont…   7G │├ 4 Details ──────────────────────────────────────────┤
│   …          ││ Caches  12.3 GB (29%)  dirs 1,204  files 88,102      │
├3 Collector ──┤│ modified 2026-10-30  allocated / apparent            │
│ 2 items 3.1G ││                                                      │
└──────────────┘└──────────────────────────────────────────────────────┘
 ? help  j/k move  l/enter open  h/esc up  c collect  d delete  / filter   scanning… 1.2M files
```

- Panels `1`–`4` switch focus; `Tab`/`Shift-Tab` cycle. Panel 2 (Files) is the primary, the sunburst highlights the selected row and the row list follows the sunburst focus.
- Custom `Sunburst` widget: for each cell compute two sample points (top/bottom half), convert to polar with aspect correction, call `layout::segment_at`, pick colours, write `▀/▄/█` + fg/bg. Center shows name + size of focus node. Selected segment drawn brighter with a thin outline ring. Cache the `Vec<Segment>` and recompute only when focus/tree-version changes.
- Scan progress drawn live (DaisyDisk-style growing petals) by re-laying out on each `Snapshot`.
- Help overlay (`?`) generated from the keymap table; filter prompt (`/`); confirm dialogs for delete; typed-word confirm (`DELETE`) for permanent removal.
- `crossterm` event loop with `tokio` or plain threads + `mpsc` (`select` on key events and scan events). Prefer threads + `crossbeam-channel`; no async needed.

### lazydisk-gpui (phase 2, gpui-ce 0.3)

- Same `AppState`/`Action`; key bindings via gpui `actions!` + `KeyBinding`, generated from the same keymap table.
- Left sidebar list (`uniform_list`), center `canvas()` sunburst using `PathBuilder::fill()` annular sectors (`move_to` outer start, `arc_to` outer end, `line_to` inner end, `arc_to` inner start, `close`). Cache built `Path`s per layout version. Hover → highlight + sidebar follows pointer (DaisyDisk behaviour); click → drill; click center → up; scroll/pinch → up/down.
- Collector as a bottom bar with drag-and-drop from list and sunburst.
- Window chrome, native menu with the same shortcuts (⌘[ ⌘] ⌘R ⇧⌘R Space ⌘⌫).

## Keymap (v1, lazygit conventions)

| Key | Action |
|---|---|
| `j`/`k`, `↓`/`↑` | move selection |
| `l`, `Enter`, `→` | drill into selected folder / expand "smaller objects" |
| `h`, `Esc`, `-`, `←` | go to parent |
| `[` / `]` | history back / forward |
| `g` / `G` | first / last |
| `1`–`4`, `Tab` | focus panel |
| `/` | filter current folder |
| `s` | toggle allocated / apparent size |
| `o` | reveal in Finder (`open -R`) |
| `Space` | Quick Look preview |
| `y` | copy path |
| `c` | toggle item in Collector |
| `d` | delete Collector contents to Trash (confirm) |
| `D` | permanent delete (typed confirm) |
| `r` / `R` | rescan folder / rescan volume |
| `+` / `=` `-` | more / fewer sunburst rings (1–6, default 4) |
| `x` | context menu for the selected item |
| `?` | help overlay |
| `q` | quit |

## Dependencies (workspace)

`dua-core 6`, `ratatui 0.30`, `crossterm`, `crossbeam-channel`, `trash`, `nix` (statfs/stat), `libc`, `serde`, `toml`, `dirs`, `thiserror`, `anyhow`, `clap` (paths, `--apparent`, `--no-cross-device`), `tracing` + `tracing-appender` (log file, never stdout in TUI). Phase 2: `gpui-ce 0.3` (as `gpui`). Dev: `tempfile`, `insta` (snapshot tests of rendered buffers), `proptest` for layout invariants.

## Milestones

1. **Scaffold** — `git init`, workspace, three crates (gpui stubbed), CI-ish `cargo fmt/clippy/test` script, `README.md`, `docs/KEYMAP.md`.
2. **Core: tree + scan** — arena tree, `dua-core` scanner with progress events, size aggregation, hard-link dedup, device boundary, firmlink handling, unreadable flags. Tests on a `tempfile` fixture tree. Bench on `~/Library`.
3. **Core: layout + state** — `sunburst_layout`, `segment_at`, colour scheme; `AppState::apply` with history, selection, filter, collector. Proptest: segments of one ring are contiguous, non-overlapping, sum to the parent's angle; `segment_at` round-trips.
4. **TUI MVP** — event loop, panels 1/2/4, live scan, sunburst widget, drill in/out, help overlay. `cargo run -- ~` works end to end.
5. **TUI ops** — Collector panel, Trash delete with confirm, permanent delete, reveal, Quick Look, copy path, rescan, filter, size-mode toggle, config file with key overrides.
6. **macOS extras** — volumes panel with capacity/free, hidden-space synthetic node, snapshots listing, `sudo` hint for unreadable areas (admin scan via re-exec with `sudo` as a documented option, not automatic).
7. **GPUI desktop** — window, sidebar list, canvas sunburst with cached paths, hover/click/scroll, same actions and shortcuts, Collector bar, drag-and-drop.
8. **Polish** — themes, SI units, snapshot tests of TUI frames with `insta`, Homebrew formula/tap draft, release build sizes.

## Verification

- `cargo test --workspace`: fixture-tree scan sizes equal `du -sk` (allocated) and `du -Ask` (apparent) on the fixture; hard-link fixture counted once; layout proptests; `AppState` action sequences (drill, back, forward, collect, delete-marks).
- Manual TUI: `cargo run -p lazydisk-tui -- ~/Library`; compare the total with `du -sh ~/Library`; drill 3 levels, go back with `[`, collect a temp dir created for the test, `d` → confirm it appears in the Trash; `D` on a second temp dir → gone; `o` opens Finder; `?` lists every binding in `docs/KEYMAP.md`.
- Scan of `/` on this Mac: total scanned + hidden space ≈ `df -h /` used, within a few GB; unreadable folders shown purple, not fatal; firmlinked `/System/Volumes/Data` not double counted.
- GPUI: `cargo run -p lazydisk-gpui`; hover follows into sidebar, click drills, click center goes up, shortcuts match the TUI, 60 fps while idle (paths cached, no per-frame tessellation).
- `cargo clippy --workspace --all-targets -D warnings`, `cargo build --target x86_64-unknown-linux-gnu -p lazydisk-core -p lazydisk-tui` (cross-check that cfg gates compile; via `cross` or a Linux CI job later).

## Known limits to document in the README

APFS clones are double counted (no userland API). Purgeable space is not computed in v1 (only snapshots are listed). Admin scans require launching with `sudo`. Cloud-storage remote scanning is out of scope.
