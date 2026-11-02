//! Drawing: panels, sunburst, details, status bar and overlays.

use std::time::SystemTime;

use lazydisk_core::format::{count, percent, size, size_short};
use lazydisk_core::keymap::Command;
use lazydisk_core::layout::SegmentKind;
use lazydisk_core::ops::DeleteMode;
use lazydisk_core::palette::{self, FILE_GREY, UNREADABLE_PURPLE, hue_swatch};
use lazydisk_core::state::{AppState, CONFIRM_WORD, Level, Overlay, Panel, ScanStatus};
use lazydisk_core::tree::{NodeFlags, NodeId, NodeKind};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
#[allow(unused_imports)]
use ratatui::style::Modifier;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Widget, Wrap};

use crate::app::Hit;
use crate::sunburst::{self, HitResult, Sunburst, truncate};

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const FOCUS_BORDER: Color = Color::Green;
const DIM_BORDER: Color = Color::DarkGray;
const SELECT_BG: Color = Color::Rgb(40, 60, 90);
const COLLECTED: Color = Color::Yellow;

#[derive(Default)]
pub struct Ui {
    pub hit: Hit,
    tick: usize,
}

fn rgb(c: palette::Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

fn panel_block(panel: Panel, focused: Panel, extra: Option<Line<'static>>) -> Block<'static> {
    let is_focused = panel == focused;
    let title = Line::from(vec![
        Span::styled(
            format!("{}", panel.number()),
            Style::new()
                .fg(if is_focused {
                    FOCUS_BORDER
                } else {
                    Color::Gray
                })
                .bold(),
        ),
        Span::raw(" "),
        Span::styled(panel.title(), Style::new().bold()),
    ]);
    let mut b = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(if is_focused { FOCUS_BORDER } else { DIM_BORDER }));
    if let Some(e) = extra {
        b = b.title_top(e.right_aligned());
    }
    b
}

/// Window of `len` rows that keeps `selected` visible within `height` rows.
fn window(selected: usize, len: usize, height: usize) -> usize {
    if height == 0 || len <= height {
        return 0;
    }
    let max_offset = len - height;
    selected.saturating_sub(height / 2).min(max_offset)
}

fn fmt_time(t: Option<SystemTime>) -> String {
    let Some(t) = t else { return "-".into() };
    let Ok(d) = t.duration_since(SystemTime::UNIX_EPOCH) else {
        return "-".into();
    };
    // Civil date from days since epoch (Howard Hinnant's algorithm).
    let secs = d.as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60
    )
}

impl Ui {
    pub fn draw(&mut self, frame: &mut Frame, state: &mut AppState) {
        self.tick = self.tick.wrapping_add(1);
        let area = frame.area();
        let [main, status] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(area);
        let left_w = (area.width / 3).clamp(26, 44);
        let [left, right] =
            Layout::horizontal([Constraint::Length(left_w), Constraint::Min(10)]).areas(main);

        let vol_h = (state.volumes.len() as u16 + 2).clamp(3, 7);
        let coll_h = (state.collector().len() as u16 + 2).clamp(3, 8);
        let [volumes, files, collector] = Layout::vertical([
            Constraint::Length(vol_h),
            Constraint::Min(4),
            Constraint::Length(coll_h),
        ])
        .areas(left);

        let details_h = 7;
        let [header, sunburst, details] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(details_h),
        ])
        .areas(right);

        self.draw_volumes(frame, state, volumes);
        self.draw_files(frame, state, files);
        self.draw_collector(frame, state, collector);
        self.draw_header(frame, state, header);
        self.draw_sunburst(frame, state, sunburst);
        self.draw_details(frame, state, details);
        self.draw_status(frame, state, status);
        self.draw_overlay(frame, state, area);
    }

    pub fn sunburst_hit(&self, state: &mut AppState, col: u16, row: u16) -> HitResult {
        let area = self.hit.sunburst;
        let tree = state.tree().clone();
        let layout = state.layout().clone();
        let t = tree.read();
        sunburst::hit(area, &layout, &t, col, row)
    }

    fn draw_volumes(&mut self, frame: &mut Frame, state: &AppState, area: Rect) {
        let block = panel_block(Panel::Volumes, state.panel, None);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let height = inner.height as usize;
        let offset = window(state.volume_selected, state.volumes.len(), height);
        self.hit.volumes = inner;
        self.hit.volumes_offset = offset;
        let scanned_mount = state.root_volume.as_ref().map(|v| v.mount_point.clone());
        for (i, v) in state.volumes.iter().enumerate().skip(offset).take(height) {
            let y = inner.y + (i - offset) as u16;
            let selected = i == state.volume_selected;
            let frac = if v.total > 0 {
                v.used() as f64 / v.total as f64
            } else {
                0.0
            };
            let bar_w = 6usize;
            let filled = (frac * bar_w as f64).round() as usize;
            let bar: String = "▰".repeat(filled) + &"▱".repeat(bar_w - filled);
            let is_current = scanned_mount.as_deref() == Some(v.mount_point.as_path());
            let right = format!(
                "{bar} {}/{}",
                size_short(v.used(), state.units),
                size_short(v.total, state.units)
            );
            let name_w = (inner.width as usize).saturating_sub(right.chars().count() + 2);
            let name = truncate(&v.name, name_w);
            let mut style = Style::new();
            if selected && state.panel == Panel::Volumes {
                style = style.bg(SELECT_BG).bold();
            } else if selected {
                style = style.bold();
            }
            let marker = if is_current { "●" } else { " " };
            let line = Line::from(vec![
                Span::styled(marker, Style::new().fg(FOCUS_BORDER)),
                Span::raw(format!("{name:<name_w$} ")),
                Span::styled(right, Style::new().fg(Color::Gray)),
            ])
            .style(style);
            frame.render_widget(Paragraph::new(line), Rect::new(inner.x, y, inner.width, 1));
        }
    }

    fn draw_files(&mut self, frame: &mut Frame, state: &mut AppState, area: Rect) {
        let filter = state.filter().to_string();
        let filtering = matches!(state.overlay(), Some(Overlay::Filter));
        let extra = if !filter.is_empty() || filtering {
            Some(Line::from(vec![
                Span::styled("/", Style::new().fg(Color::Yellow)),
                Span::styled(filter.clone(), Style::new().fg(Color::Yellow)),
                Span::styled(
                    if filtering { "▏" } else { "" },
                    Style::new().fg(Color::Yellow),
                ),
            ]))
        } else {
            None
        };
        let block = panel_block(Panel::Files, state.panel, extra);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let list = state.visible_children();
        let selected = state.selected_index();
        let height = inner.height as usize;
        let offset = window(selected, list.len(), height);
        self.hit.files = inner;
        self.hit.files_offset = offset;

        let layout = state.layout().clone();
        let tree = state.tree().read();
        let focus = state.focus();
        let total = lazydisk_core::layout::total_of(&tree, focus).max(1);
        let collector = state.collector();
        if list.is_empty() {
            let msg = if !filter.is_empty() {
                "no matches"
            } else if state.scan_status().is_running() {
                "scanning…"
            } else {
                "empty folder"
            };
            frame.render_widget(
                Paragraph::new(Line::from(msg).style(Style::new().fg(Color::DarkGray))),
                Rect::new(inner.x, inner.y, inner.width, 1),
            );
        }
        for (i, id) in list.iter().enumerate().skip(offset).take(height) {
            let y = inner.y + (i - offset) as u16;
            let n = tree.node(*id);
            let sz = tree.display_size(*id);
            let size_txt = size_short(sz, state.units);
            let pct = percent(sz, total);
            let right = format!("{size_txt:>7} {pct:>4}");
            let is_collected = collector.contains(id);
            let (swatch, swatch_color) = match n.kind {
                NodeKind::Dir if n.flags.contains(NodeFlags::UNREADABLE) => {
                    ("▶", rgb(UNREADABLE_PURPLE))
                }
                NodeKind::Dir => {
                    let hue = layout
                        .segment_for(*id)
                        .or_else(|| layout.group_containing(&tree, *id))
                        .map(|s| s.hue);
                    ("▶", hue.map(|h| rgb(hue_swatch(h))).unwrap_or(Color::Gray))
                }
                NodeKind::HiddenSpace => ("◌", rgb(palette::hsl_to_rgb(270.0, 0.25, 0.5))),
                NodeKind::FreeSpace => ("○", Color::DarkGray),
                NodeKind::Symlink => ("↪", rgb(FILE_GREY)),
                _ => ("·", rgb(FILE_GREY)),
            };
            let name_w = (inner.width as usize).saturating_sub(right.len() + 4);
            let name = truncate(&n.name, name_w);
            let mut name_style = Style::new();
            if is_collected {
                name_style = name_style.fg(COLLECTED);
            } else if n.kind == NodeKind::Dir {
                name_style = name_style.bold();
            } else if n.flags.contains(NodeFlags::HARDLINK_DUP)
                || n.flags.contains(NodeFlags::CLONE_DUP)
            {
                name_style = name_style.fg(Color::DarkGray);
            } else if n.kind.is_virtual() {
                name_style = name_style.fg(Color::Gray).italic();
            }
            let mut row_style = Style::new();
            if i == selected && state.panel == Panel::Files {
                row_style = row_style.bg(SELECT_BG);
            } else if i == selected {
                row_style = row_style.bg(Color::Rgb(35, 35, 35));
            }
            let mark = if is_collected { "✓" } else { " " };
            let line = Line::from(vec![
                Span::styled(mark, Style::new().fg(COLLECTED)),
                Span::styled(swatch, Style::new().fg(swatch_color)),
                Span::raw(" "),
                Span::styled(format!("{name:<name_w$}"), name_style),
                Span::styled(right, Style::new().fg(Color::Gray)),
            ])
            .style(row_style);
            frame.render_widget(Paragraph::new(line), Rect::new(inner.x, y, inner.width, 1));
        }
    }

    fn draw_collector(&mut self, frame: &mut Frame, state: &AppState, area: Rect) {
        let items = state.collector();
        let extra = if items.is_empty() {
            None
        } else {
            Some(Line::from(Span::styled(
                format!(
                    "{} items, {}",
                    items.len(),
                    size(state.collector_total(), state.units)
                ),
                Style::new().fg(COLLECTED),
            )))
        };
        let block = panel_block(Panel::Collector, state.panel, extra);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        self.hit.collector = inner;
        let height = inner.height as usize;
        let offset = window(state.collector_selected(), items.len(), height);
        self.hit.collector_offset = offset;
        let tree = state.tree().read();
        if items.is_empty() {
            let hint = format!(
                "{} collect · {} trash · {} delete",
                state.keymap().label(Command::ToggleCollect),
                state.keymap().label(Command::DeleteTrash),
                state.keymap().label(Command::DeletePermanent)
            );
            frame.render_widget(
                Paragraph::new(Line::from(hint).style(Style::new().fg(Color::DarkGray))),
                Rect::new(inner.x, inner.y, inner.width, 1),
            );
        }
        for (i, id) in items.iter().enumerate().skip(offset).take(height) {
            let y = inner.y + (i - offset) as u16;
            let n = tree.node(*id);
            let right = size_short(tree.display_size(*id), state.units);
            let name_w = (inner.width as usize).saturating_sub(right.len() + 2);
            let name = truncate(&n.name, name_w);
            let mut style = Style::new().fg(COLLECTED);
            if i == state.collector_selected() && state.panel == Panel::Collector {
                style = style.bg(SELECT_BG);
            }
            let line = Line::from(vec![
                Span::raw(format!(" {name:<name_w$} ")),
                Span::styled(right, Style::new().fg(Color::Gray)),
            ])
            .style(style);
            frame.render_widget(Paragraph::new(line), Rect::new(inner.x, y, inner.width, 1));
        }
    }

    fn draw_header(&mut self, frame: &mut Frame, state: &AppState, area: Rect) {
        let crumbs = state.breadcrumb();
        let tree = state.tree().read();
        let focus = state.focus();
        let total = tree.display_size(focus);
        let mut spans = Vec::new();
        for (i, (_, name)) in crumbs.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" › ", Style::new().fg(Color::DarkGray)));
            }
            let style = if i + 1 == crumbs.len() {
                Style::new().bold()
            } else {
                Style::new().fg(Color::Gray)
            };
            spans.push(Span::styled(name.clone(), style));
        }
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            size(total, state.units),
            Style::new().fg(Color::Cyan).bold(),
        ));
        spans.push(Span::styled(
            format!("  {}", state.size_mode().label()),
            Style::new().fg(Color::DarkGray),
        ));
        let n = tree.node(focus);
        spans.push(Span::styled(
            format!("  {} files, {} folders", count(n.files), count(n.dirs)),
            Style::new().fg(Color::DarkGray),
        ));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_sunburst(&mut self, frame: &mut Frame, state: &mut AppState, area: Rect) {
        self.hit.sunburst = area;
        let selected = state.selected_node();
        let scanning = state.scan_status().is_running();
        let units = state.units;
        let focus = state.focus();
        let tree = state.tree().clone();
        let layout = state.layout();
        let t = tree.read();
        let n = t.node(focus);
        // The root node's name is its full path; show only the last component in the centre.
        let label = if focus == t.root() {
            std::path::Path::new(&*n.name)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| n.name.to_string())
        } else {
            n.name.to_string()
        };
        let mut center = vec![
            Line::from(label).style(Style::new().bold().fg(Color::White)),
            Line::from(size(t.display_size(focus), units)).style(Style::new().fg(Color::Cyan)),
        ];
        if scanning {
            center.push(
                Line::from(format!("{} scanning", SPINNER[self.tick % SPINNER.len()]))
                    .style(Style::new().fg(Color::Yellow)),
            );
        } else if let Some(sel) = selected {
            let s = t.node(sel);
            center.push(Line::from(truncate(&s.name, 24)).style(Style::new().fg(Color::Gray)));
        }
        let highlight_group = selected.and_then(|s| {
            layout
                .group_containing(&t, s)
                .filter(|g| g.kind == SegmentKind::Smaller)
                .map(|g| (g.node, g.group_from))
        });
        let widget = Sunburst {
            layout,
            highlight: selected,
            highlight_group,
            center,
            separators: true,
        };
        widget.render(area, frame.buffer_mut());
    }

    fn draw_details(&mut self, frame: &mut Frame, state: &mut AppState, area: Rect) {
        let block = panel_block(Panel::Details, state.panel, None);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let target: Option<NodeId> = state.target_node().or(Some(state.focus()));
        let units = state.units;
        let tree = state.tree().read();
        let Some(id) = target else { return };
        let n = tree.node(id);
        let parent_total = n.parent.map(|p| tree.display_size(p)).unwrap_or(0);
        let kind = match n.kind {
            NodeKind::Dir => "folder",
            NodeKind::File => "file",
            NodeKind::Symlink => "symlink",
            NodeKind::Other => "special file",
            NodeKind::HiddenSpace => "hidden space",
            NodeKind::FreeSpace => "free space",
        };
        let mut lines = vec![
            Line::from(Span::styled(
                truncate(&tree.path(id).to_string_lossy(), inner.width as usize),
                Style::new().fg(Color::Gray),
            )),
            Line::from(vec![
                Span::styled(kind, Style::new().fg(Color::DarkGray)),
                Span::raw("  "),
                Span::styled(size(n.size, units), Style::new().fg(Color::Cyan).bold()),
                Span::styled(" on disk", Style::new().fg(Color::DarkGray)),
                Span::raw("  "),
                Span::styled(size(n.apparent, units), Style::new().fg(Color::Cyan)),
                Span::styled(" apparent", Style::new().fg(Color::DarkGray)),
                Span::styled(
                    if parent_total > 0 {
                        format!(
                            "  {} of parent",
                            percent(tree.display_size(id), parent_total)
                        )
                    } else {
                        String::new()
                    },
                    Style::new().fg(Color::Gray),
                ),
            ]),
        ];
        if n.kind.is_dir() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{} files, {} folders", count(n.files), count(n.dirs)),
                    Style::new().fg(Color::Gray),
                ),
                Span::styled(
                    format!("  modified {}", fmt_time(n.mtime)),
                    Style::new().fg(Color::DarkGray),
                ),
            ]));
        } else if !n.kind.is_virtual() {
            lines.push(Line::from(Span::styled(
                format!("modified {}", fmt_time(n.mtime)),
                Style::new().fg(Color::DarkGray),
            )));
        }
        let flags = AppState::flag_labels(n.flags);
        if !flags.is_empty() {
            lines.push(Line::from(Span::styled(
                flags.join(" · "),
                Style::new().fg(rgb(UNREADABLE_PURPLE)),
            )));
        }
        if let Some(v) = &state.root_volume {
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", v.name), Style::new().fg(Color::Gray)),
                Span::styled(
                    format!(
                        "{} used of {}, {} free",
                        size(v.used(), units),
                        size(v.total, units),
                        size(v.available, units)
                    ),
                    Style::new().fg(Color::DarkGray),
                ),
            ]));
        }
        match state.scan_status() {
            ScanStatus::Done { errors, .. } if *errors > 0 => {
                lines.push(Line::from(Span::styled(
                    format!(
                        "{} items could not be read; run with sudo to include them",
                        count(*errors)
                    ),
                    Style::new().fg(Color::Yellow),
                )));
            }
            ScanStatus::Failed(msg) => {
                lines.push(Line::from(Span::styled(
                    msg.clone(),
                    Style::new().fg(Color::Red),
                )));
            }
            _ => {}
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    fn draw_status(&mut self, frame: &mut Frame, state: &AppState, area: Rect) {
        let km = state.keymap();
        let hints: Vec<(String, &str)> = match state.panel {
            Panel::Volumes => vec![
                (km.label(Command::Down), "move"),
                (km.label(Command::Enter), "scan"),
                (km.label(Command::FocusFiles), "files"),
            ],
            Panel::Collector => vec![
                (km.label(Command::Down), "move"),
                (km.label(Command::Enter), "jump"),
                (km.label(Command::ToggleCollect), "remove"),
                (km.label(Command::DeleteTrash), "trash"),
                (km.label(Command::DeletePermanent), "delete"),
            ],
            _ => vec![
                (km.label(Command::Down), "move"),
                (km.label(Command::Enter), "open"),
                (km.label(Command::Parent), "up"),
                (km.label(Command::ToggleCollect), "collect"),
                (km.label(Command::DeleteTrash), "trash"),
                (km.label(Command::Filter), "filter"),
                (km.label(Command::Menu), "menu"),
            ],
        };
        let right = self.status_right(state);
        let right_w = (right.width() as u16).min(area.width / 2);
        let left_w = area.width.saturating_sub(right_w + 2) as usize;
        let mut spans = vec![
            Span::styled(km.label(Command::Help), Style::new().fg(Color::Cyan)),
            Span::styled(" help  ", Style::new().fg(Color::Gray)),
        ];
        let mut used = km.label(Command::Help).chars().count() + 7;
        for (k, h) in &hints {
            let w = k.chars().count() + h.chars().count() + 3;
            if used + w > left_w {
                break;
            }
            used += w;
            spans.push(Span::styled(k.clone(), Style::new().fg(Color::Cyan)));
            spans.push(Span::styled(
                format!(" {h}  "),
                Style::new().fg(Color::Gray),
            ));
        }
        let left = Line::from(spans);
        let [l, r] =
            Layout::horizontal([Constraint::Min(10), Constraint::Length(right_w)]).areas(area);
        frame.render_widget(Paragraph::new(left), l);
        frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), r);
    }

    fn status_right(&self, state: &AppState) -> Line<'static> {
        if let Some(st) = state.status() {
            let color = match st.level {
                Level::Info => Color::Green,
                Level::Warn => Color::Yellow,
                Level::Error => Color::Red,
            };
            Line::from(Span::styled(st.text.clone(), Style::new().fg(color)))
        } else {
            match state.scan_status() {
                ScanStatus::Running { entries, bytes, .. } => Line::from(vec![
                    Span::styled(
                        format!("{} ", SPINNER[self.tick % SPINNER.len()]),
                        Style::new().fg(Color::Yellow),
                    ),
                    Span::styled(
                        format!(
                            "scanning {} items, {}",
                            count(*entries),
                            size(*bytes, state.units)
                        ),
                        Style::new().fg(Color::Yellow),
                    ),
                ]),
                ScanStatus::Done {
                    entries, elapsed, ..
                } => Line::from(Span::styled(
                    format!("{} items in {:.1}s", count(*entries), elapsed.as_secs_f64()),
                    Style::new().fg(Color::DarkGray),
                )),
                ScanStatus::Failed(_) => {
                    Line::from(Span::styled("scan failed", Style::new().fg(Color::Red)))
                }
                ScanStatus::Idle => Line::default(),
            }
        }
    }

    fn draw_overlay(&mut self, frame: &mut Frame, state: &mut AppState, area: Rect) {
        let Some(overlay) = state.overlay().cloned() else {
            return;
        };
        match overlay {
            Overlay::Filter => {}
            Overlay::Help => {
                let rows = state.keymap().help_rows();
                let key_w = rows
                    .iter()
                    .map(|(k, _)| k.chars().count())
                    .max()
                    .unwrap_or(8);
                let lines: Vec<Line> = rows
                    .iter()
                    .map(|(k, h)| {
                        Line::from(vec![
                            Span::styled(format!("{k:<key_w$}  "), Style::new().fg(Color::Cyan)),
                            Span::raw(*h),
                        ])
                    })
                    .collect();
                let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
                let w = (key_w as u16 + 50).min(area.width.saturating_sub(4));
                let rect = centered(area, w, h);
                frame.render_widget(Clear, rect);
                let block = Block::bordered()
                    .title(" Keys (any key to close) ")
                    .border_style(Style::new().fg(Color::Cyan));
                frame.render_widget(Paragraph::new(lines).block(block), rect);
            }
            Overlay::Message { title, lines } => {
                let w = lines
                    .iter()
                    .map(|l| l.chars().count())
                    .max()
                    .unwrap_or(20)
                    .min(area.width as usize - 4) as u16
                    + 4;
                let rect = centered(area, w, lines.len() as u16 + 2);
                frame.render_widget(Clear, rect);
                let block = Block::bordered()
                    .title(format!(" {title} "))
                    .border_style(Style::new().fg(Color::Cyan));
                let text: Vec<Line> = lines.into_iter().map(Line::from).collect();
                frame.render_widget(
                    Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
                    rect,
                );
            }
            Overlay::Menu {
                title,
                items,
                selected,
            } => {
                let w = items
                    .iter()
                    .map(|(s, _)| s.chars().count())
                    .max()
                    .unwrap_or(10) as u16
                    + 10;
                let rect = centered(area, w.min(area.width - 2), items.len() as u16 + 2);
                frame.render_widget(Clear, rect);
                let lines: Vec<Line> = items
                    .iter()
                    .enumerate()
                    .map(|(i, (label, cmd))| {
                        let key = state.keymap().label(*cmd);
                        let mut style = Style::new();
                        if i == selected {
                            style = style.bg(SELECT_BG).bold();
                        }
                        Line::from(vec![
                            Span::styled(format!(" {} ", i + 1), Style::new().fg(Color::Cyan)),
                            Span::raw(format!("{label:<w$}", w = (w as usize).saturating_sub(10))),
                            Span::styled(key, Style::new().fg(Color::DarkGray)),
                        ])
                        .style(style)
                    })
                    .collect();
                let block = Block::bordered()
                    .title(format!(
                        " {} ",
                        truncate(&title, (w as usize).saturating_sub(4))
                    ))
                    .border_style(Style::new().fg(Color::Cyan));
                frame.render_widget(Paragraph::new(lines).block(block), rect);
            }
            Overlay::ConfirmDelete { mode, items, typed } => {
                let tree = state.tree().read();
                let total: u64 = items.iter().map(|i| tree.display_size(*i)).sum();
                let mut lines: Vec<Line> = items
                    .iter()
                    .take(8)
                    .map(|i| {
                        Line::from(vec![
                            Span::styled(
                                format!("{:>9}  ", size(tree.display_size(*i), state.units)),
                                Style::new().fg(Color::Gray),
                            ),
                            Span::raw(truncate(&tree.path(*i).to_string_lossy(), 60)),
                        ])
                    })
                    .collect();
                if items.len() > 8 {
                    lines.push(
                        Line::from(format!("… and {} more", items.len() - 8))
                            .style(Style::new().fg(Color::Gray)),
                    );
                }
                lines.push(Line::default());
                let (title, color) = match mode {
                    DeleteMode::Trash => {
                        lines.push(Line::from(vec![
                            Span::raw(format!(
                                "Move {} items ({}) to the Trash?  ",
                                items.len(),
                                size(total, state.units)
                            )),
                            Span::styled("y", Style::new().fg(Color::Green).bold()),
                            Span::raw("es / "),
                            Span::styled("n", Style::new().fg(Color::Red).bold()),
                            Span::raw("o"),
                        ]));
                        (" Move to Trash ", Color::Yellow)
                    }
                    DeleteMode::Permanent => {
                        lines.push(Line::from(vec![Span::styled(
                            format!(
                                "Permanently delete {} items ({})? This cannot be undone.",
                                items.len(),
                                size(total, state.units)
                            ),
                            Style::new().fg(Color::Red).bold(),
                        )]));
                        lines.push(Line::from(vec![
                            Span::raw(format!("Type {CONFIRM_WORD} and press enter: ")),
                            Span::styled(typed.clone(), Style::new().fg(Color::Red).bold()),
                            Span::styled("▏", Style::new().fg(Color::Red)),
                        ]));
                        (" Delete permanently ", Color::Red)
                    }
                };
                drop(tree);
                let w = 76.min(area.width.saturating_sub(2));
                let rect = centered(area, w, lines.len() as u16 + 2);
                frame.render_widget(Clear, rect);
                let block = Block::bordered()
                    .title(title)
                    .border_style(Style::new().fg(color));
                frame.render_widget(Paragraph::new(lines).block(block), rect);
            }
        }
    }
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_keeps_selection_visible() {
        assert_eq!(window(0, 100, 10), 0);
        assert_eq!(window(50, 100, 10), 45);
        assert_eq!(window(99, 100, 10), 90);
        assert_eq!(window(3, 5, 10), 0);
    }

    #[test]
    fn dates() {
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        assert_eq!(fmt_time(Some(t)), "2023-11-14 22:13");
        assert_eq!(fmt_time(None), "-");
    }
}
