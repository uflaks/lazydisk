//! The desktop window: panels, sunburst canvas, overlays and input.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Bounds, ClickEvent, Context, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, Rgba, ScrollDelta, ScrollStrategy, ScrollWheelEvent, Task,
    UniformListScrollHandle, Window, canvas, colors::Colors, div, prelude::*, px, rgb, rgba,
    uniform_list,
};
use lazydisk_core::format::{count, percent, size, size_short};
use lazydisk_core::keymap::{Chord, Command, Key};
use lazydisk_core::layout::{SegmentKind, total_of};
use lazydisk_core::ops::DeleteMode;
use lazydisk_core::palette::{self, Rgb, hue_swatch, segment_color};
use lazydisk_core::state::{
    Action, AppState, CONFIRM_WORD, Effect, Level, Overlay, Panel, ScanStatus,
};
use lazydisk_core::tree::{NodeFlags, NodeId, NodeKind};

use crate::keys::chord_from;
use crate::sunburst::{self, HitResult, SunburstPaths};

const POLL: Duration = Duration::from_millis(50);
const ACCENT: u32 = 0x4ade80;
const COLLECTED: u32 = 0xfacc15;

fn to_rgba(c: Rgb) -> Rgba {
    rgb(((c.0 as u32) << 16) | ((c.1 as u32) << 8) | c.2 as u32)
}

pub struct LazyDiskView {
    pub state: AppState,
    focus: FocusHandle,
    files_scroll: UniformListScrollHandle,
    paths: Rc<RefCell<Option<SunburstPaths>>>,
    hover: Option<NodeId>,
    _poll: Task<()>,
}

impl LazyDiskView {
    pub fn new(state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let alive = this
                    .update(cx, |view, cx| {
                        let changed = view.state.poll();
                        if changed
                            || view.state.status().is_some()
                            || view.state.scan_status().is_running()
                        {
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        });
        Self {
            state,
            focus,
            files_scroll: UniformListScrollHandle::new(),
            paths: Rc::new(RefCell::new(None)),
            hover: None,
            _poll: poll,
        }
    }

    pub fn apply(&mut self, action: Action, cx: &mut Context<Self>) {
        let effects = self.state.apply(action);
        if effects.contains(&Effect::Quit) {
            cx.quit();
            return;
        }
        let idx = self.state.selected_index();
        self.files_scroll
            .scroll_to_item(idx, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn key_down(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        if let Some(chord) = chord_from(&ev.keystroke) {
            self.apply(Action::Key(chord), cx);
        }
    }

    fn sunburst_hit(&mut self, pos: gpui::Point<Pixels>) -> HitResult {
        let paths = self.paths.borrow();
        let Some(paths) = paths.as_ref() else {
            return HitResult::None;
        };
        let tree = self.state.tree().clone();
        let layout = self.state.layout().clone();
        let t = tree.read();
        sunburst::hit(paths, &layout, &t, pos)
    }

    fn sunburst_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.state.overlay().is_some() {
            return;
        }
        match self.sunburst_hit(ev.position) {
            HitResult::Segment(node, _) => {
                if self.hover != Some(node) {
                    self.hover = Some(node);
                    self.apply(Action::Select(node), cx);
                }
            }
            _ => {
                if self.hover.take().is_some() {
                    cx.notify();
                }
            }
        }
    }

    fn sunburst_down(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        if self.state.overlay().is_some() || ev.button != MouseButton::Left {
            return;
        }
        match self.sunburst_hit(ev.position) {
            HitResult::Segment(node, true) => self.apply(Action::Focus(node), cx),
            HitResult::Segment(node, false) => self.apply(Action::Select(node), cx),
            HitResult::Center => self.apply(Action::Command(Command::Parent), cx),
            HitResult::None => {}
        }
    }

    fn sunburst_scroll(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let dy = match ev.delta {
            ScrollDelta::Pixels(p) => f32::from(p.y),
            ScrollDelta::Lines(l) => l.y * 20.0,
        };
        if dy > 12.0 {
            self.apply(Action::Command(Command::Parent), cx);
        } else if dy < -12.0 {
            self.apply(Action::Command(Command::Enter), cx);
        }
    }

    // ----- rendering helpers -------------------------------------------------

    fn panel_header(
        &self,
        panel: Panel,
        extra: Option<String>,
        colors: &Colors,
    ) -> impl IntoElement {
        let focused = self.state.panel == panel;
        div()
            .flex()
            .items_center()
            .justify_between()
            .px_3()
            .py_1()
            .border_b_1()
            .border_color(colors.separator)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(if focused {
                                rgb(ACCENT)
                            } else {
                                colors.disabled
                            })
                            .child(format!("{}", panel.number())),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(panel.title()),
                    ),
            )
            .children(extra.map(|e| div().text_xs().text_color(colors.disabled).child(e)))
    }

    fn render_volumes(&mut self, colors: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let units = self.state.units;
        let current = self
            .state
            .root_volume
            .as_ref()
            .map(|v| v.mount_point.clone());
        let selected = self.state.volume_selected;
        let focused = self.state.panel == Panel::Volumes;
        let rows: Vec<_> = self
            .state
            .volumes
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let frac = if v.total > 0 {
                    v.used() as f32 / v.total as f32
                } else {
                    0.0
                };
                let is_current = current.as_deref() == Some(v.mount_point.as_path());
                let sel = i == selected;
                let row = div()
                    .id(("vol", i))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .cursor_pointer()
                    .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                        view.state.panel = Panel::Volumes;
                        if view.state.volume_selected == i {
                            view.apply(Action::ScanVolume(i), cx);
                        } else {
                            view.state.volume_selected = i;
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .w(px(8.))
                            .text_xs()
                            .text_color(rgb(ACCENT))
                            .child(if is_current { "●" } else { "" }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .truncate()
                            .child(v.name.clone()),
                    )
                    .child(
                        div()
                            .w(px(56.))
                            .h(px(6.))
                            .rounded_sm()
                            .bg(colors.separator)
                            .child(div().h_full().rounded_sm().w(px(56.0 * frac)).bg(
                                if frac > 0.9 {
                                    rgb(0xef4444)
                                } else {
                                    rgb(0x60a5fa)
                                },
                            )),
                    )
                    .child(div().text_xs().text_color(colors.disabled).child(format!(
                        "{}/{}",
                        size_short(v.used(), units),
                        size_short(v.total, units)
                    )));
                if sel && focused {
                    row.bg(colors.selected).text_color(colors.selected_text)
                } else if sel {
                    row.bg(colors.container)
                } else {
                    row.hover(|s| s.bg(colors.container))
                }
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(colors.separator)
            .child(self.panel_header(Panel::Volumes, None, colors))
            .children(rows)
    }

    fn render_files(&mut self, colors: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let list = self.state.visible_children();
        let selected = self.state.selected_index();
        let focused = self.state.panel == Panel::Files;
        let units = self.state.units;
        let focus = self.state.focus();
        let tree = self.state.tree().clone();
        let layout = Rc::new(self.state.layout().clone());
        let collector: Rc<Vec<NodeId>> = Rc::new(self.state.collector().to_vec());
        let items = Rc::new(list);
        let view = cx.weak_entity();
        let colors2 = colors.clone();
        let filter = self.state.filter().to_string();
        let filtering = matches!(self.state.overlay(), Some(Overlay::Filter));
        let extra = if !filter.is_empty() || filtering {
            Some(format!("/{filter}{}", if filtering { "▏" } else { "" }))
        } else {
            Some(format!("{} items", items.len()))
        };
        let scanning = self.state.scan_status().is_running();
        let count_items = items.len();

        let rows = uniform_list("files", count_items, move |range, _window, _cx| {
            let t = tree.read();
            let total = total_of(&t, focus).max(1);
            range
                .map(|ix| {
                    let id = items[ix];
                    let n = t.node(id);
                    let sz = t.display_size(id);
                    let is_sel = ix == selected;
                    let is_collected = collector.contains(&id);
                    let (glyph, swatch) = match n.kind {
                        NodeKind::Dir if n.flags.contains(NodeFlags::UNREADABLE) => {
                            ("▶", to_rgba(palette::UNREADABLE_PURPLE))
                        }
                        NodeKind::Dir => {
                            let hue = layout
                                .segment_for(id)
                                .or_else(|| layout.group_containing(&t, id))
                                .map(|s| s.hue);
                            (
                                "▶",
                                hue.map(|h| to_rgba(hue_swatch(h)))
                                    .unwrap_or(colors2.disabled),
                            )
                        }
                        NodeKind::HiddenSpace => {
                            ("◌", to_rgba(palette::hsl_to_rgb(270.0, 0.25, 0.5)))
                        }
                        NodeKind::FreeSpace => ("○", colors2.disabled),
                        NodeKind::Symlink => ("↪", to_rgba(palette::FILE_GREY)),
                        _ => ("·", to_rgba(palette::FILE_GREY)),
                    };
                    let view = view.clone();
                    let mut name = div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .truncate()
                        .child(n.name.to_string());
                    if is_collected {
                        name = name.text_color(rgb(COLLECTED));
                    } else if n.kind == NodeKind::Dir {
                        name = name.font_weight(gpui::FontWeight::SEMIBOLD);
                    } else if n.flags.contains(NodeFlags::HARDLINK_DUP)
                        || n.flags.contains(NodeFlags::CLONE_DUP)
                    {
                        name = name.text_color(colors2.disabled);
                    } else if n.kind.is_virtual() {
                        name = name.italic().text_color(colors2.disabled);
                    }
                    let row = div()
                        .id(ix)
                        .w_full()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_0p5()
                        .cursor_pointer()
                        .on_click(move |_ev, _window, cx| {
                            view.update(cx, |view, cx| {
                                view.state.panel = Panel::Files;
                                if view.state.selected_node() == Some(id) {
                                    view.apply(Action::Command(Command::Enter), cx);
                                } else {
                                    view.apply(Action::Select(id), cx);
                                }
                            })
                            .ok();
                        })
                        .child(
                            div()
                                .w(px(10.))
                                .text_xs()
                                .text_color(rgb(COLLECTED))
                                .child(if is_collected { "✓" } else { "" }),
                        )
                        .child(div().w(px(12.)).text_xs().text_color(swatch).child(glyph))
                        .child(name)
                        .child(
                            div()
                                .w(px(64.))
                                .text_xs()
                                .text_color(colors2.disabled)
                                .child(format!("{:>7}", size_short(sz, units))),
                        )
                        .child(
                            div()
                                .w(px(36.))
                                .text_xs()
                                .text_color(colors2.disabled)
                                .child(percent(sz, total)),
                        );
                    if is_sel && focused {
                        row.bg(colors2.selected).text_color(colors2.selected_text)
                    } else if is_sel {
                        row.bg(colors2.container)
                    } else {
                        row.hover(|s| s.bg(colors2.container))
                    }
                })
                .collect()
        })
        .track_scroll(&self.files_scroll)
        .w_full()
        .flex_1()
        .min_h_0();

        let empty = if count_items == 0 {
            Some(
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(colors.disabled)
                    .child(if scanning {
                        "scanning…"
                    } else if !filter.is_empty() {
                        "no matches"
                    } else {
                        "empty folder"
                    }),
            )
        } else {
            None
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(self.panel_header(Panel::Files, extra, colors))
            .children(empty)
            .child(rows)
    }

    fn render_collector(&mut self, colors: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let units = self.state.units;
        let items = self.state.collector().to_vec();
        let selected = self.state.collector_selected();
        let focused = self.state.panel == Panel::Collector;
        let tree = self.state.tree().read().clone();
        let extra = if items.is_empty() {
            None
        } else {
            Some(format!(
                "{} items, {}",
                items.len(),
                size(self.state.collector_total(), units)
            ))
        };
        let km = self.state.keymap();
        let hint = format!(
            "{} collect · {} trash · {} delete",
            km.label(Command::ToggleCollect),
            km.label(Command::DeleteTrash),
            km.label(Command::DeletePermanent)
        );
        let rows: Vec<_> = items
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let n = tree.node(*id);
                let row = div()
                    .id(("coll", i))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_0p5()
                    .text_sm()
                    .text_color(rgb(COLLECTED))
                    .cursor_pointer()
                    .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                        view.state.panel = Panel::Collector;
                        view.state.apply(Action::Command(Command::FocusCollector));
                        while view.state.collector_selected() < i {
                            view.state.apply(Action::Command(Command::Down));
                        }
                        while view.state.collector_selected() > i {
                            view.state.apply(Action::Command(Command::Up));
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(n.name.to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.disabled)
                            .child(size_short(tree.display_size(*id), units)),
                    );
                if i == selected && focused {
                    row.bg(colors.selected)
                } else {
                    row
                }
            })
            .collect();
        let body: Vec<gpui::AnyElement> = if rows.is_empty() {
            vec![
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(colors.disabled)
                    .child(hint)
                    .into_any_element(),
            ]
        } else {
            rows.into_iter()
                .map(IntoElement::into_any_element)
                .collect()
        };
        div()
            .flex()
            .flex_col()
            .max_h(px(180.))
            .border_t_1()
            .border_color(colors.separator)
            .child(self.panel_header(Panel::Collector, extra, colors))
            .child(div().flex().flex_col().overflow_hidden().children(body))
    }

    fn render_header(&mut self, colors: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let crumbs = self.state.breadcrumb();
        let units = self.state.units;
        let focus = self.state.focus();
        let (total, files, dirs) = {
            let t = self.state.tree().read();
            let n = t.node(focus);
            (t.display_size(focus), n.files, n.dirs)
        };
        let last = crumbs.len().saturating_sub(1);
        let mut row = div()
            .flex()
            .items_center()
            .gap_1()
            .px_4()
            .py_2()
            .flex_wrap()
            .text_sm();
        for (i, (id, name)) in crumbs.into_iter().enumerate() {
            if i > 0 {
                row = row.child(div().text_color(colors.disabled).child("›"));
            }
            let is_last = i == last;
            row = row.child(
                div()
                    .id(("crumb", i))
                    .cursor_pointer()
                    .when(is_last, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                    .when(!is_last, |d| {
                        d.text_color(colors.disabled)
                            .hover(|s| s.text_color(colors.text))
                    })
                    .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                        view.apply(Action::Focus(id), cx)
                    }))
                    .child(name),
            );
        }
        row.child(
            div()
                .ml_2()
                .text_color(rgb(0x22d3ee))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(size(total, units)),
        )
        .child(div().text_xs().text_color(colors.disabled).child(format!(
            "{} · {} files, {} folders",
            self.state.size_mode().label(),
            count(files),
            count(dirs)
        )))
    }

    fn render_sunburst(&mut self, colors: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = Rc::new(self.state.layout().clone());
        let selected = self.state.selected_node();
        let tree = self.state.tree().clone();
        let highlight_group = selected.and_then(|s| {
            let t = tree.read();
            layout
                .group_containing(&t, s)
                .filter(|g| g.kind == SegmentKind::Smaller)
                .map(|g| (g.node, g.group_from))
        });
        let focus = self.state.focus();
        let units = self.state.units;
        let (label, total_txt, sub) = {
            let t = tree.read();
            let n = t.node(focus);
            let label = if focus == t.root() {
                std::path::Path::new(&*n.name)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| n.name.to_string())
            } else {
                n.name.to_string()
            };
            let sub = match self.state.scan_status() {
                ScanStatus::Running { entries, .. } => {
                    format!("scanning {} items", count(*entries))
                }
                _ => selected
                    .map(|s| t.node(s).name.to_string())
                    .unwrap_or_default(),
            };
            (label, size(t.display_size(focus), units), sub)
        };

        // Real canvas: prepaint builds/caches the paths, paint draws them with current colours.
        let paths_pre = self.paths.clone();
        let paths_paint = self.paths.clone();
        let tree_labels = tree.clone();
        let layout_pre = layout.clone();
        let chart = canvas(
            move |bounds: Bounds<Pixels>, _window, _cx| {
                let mut slot = paths_pre.borrow_mut();
                let prev = slot.take();
                *slot = Some(sunburst::build(&layout_pre, bounds, prev));
            },
            move |_bounds, (), window, cx| {
                let slot = paths_paint.borrow();
                let Some(paths) = slot.as_ref() else { return };
                for (seg, path) in &paths.items {
                    let highlighted = match seg.kind {
                        SegmentKind::Smaller => highlight_group == Some((seg.node, seg.group_from)),
                        _ => selected == Some(seg.node),
                    };
                    window.paint_path(path.clone(), to_rgba(segment_color(seg, highlighted)));
                }
                // Names on petals wide enough to hold them.
                let t = tree_labels.read();
                let style = window.text_style();
                let font_size = px(11.);
                let line_height = px(14.);
                for (seg, _) in &paths.items {
                    if seg.kind == SegmentKind::Smaller || seg.kind == SegmentKind::FreeSpace {
                        continue;
                    }
                    let Some((ax, ay)) = sunburst::label_anchor(&paths.geometry, seg, 36.0) else {
                        continue;
                    };
                    let r = paths.geometry.inner_radius(seg.depth) + paths.geometry.ring_w / 2.0;
                    let max_chars = ((seg.span() * r) / 6.5) as usize;
                    if max_chars < 3 {
                        continue;
                    }
                    let name = &t.node(seg.node).name;
                    let text = if name.chars().count() > max_chars {
                        let mut s: String =
                            name.chars().take(max_chars.saturating_sub(1)).collect();
                        s.push('…');
                        s
                    } else {
                        name.to_string()
                    };
                    let mut run = style.to_run(text.len());
                    let bg = segment_color(seg, false);
                    let luma = 0.299 * bg.0 as f32 + 0.587 * bg.1 as f32 + 0.114 * bg.2 as f32;
                    run.color = if luma > 150.0 {
                        gpui::hsla(0.0, 0.0, 0.07, 1.0)
                    } else {
                        gpui::hsla(0.0, 0.0, 0.96, 1.0)
                    };
                    let line = window
                        .text_system()
                        .shape_line(text.into(), font_size, &[run]);
                    let w = f64::from(line.width());
                    let origin = gpui::point(
                        px((f64::from(paths.origin.x) + ax - w / 2.0) as f32),
                        px((f64::from(paths.origin.y) + ay - f64::from(line_height) / 2.0) as f32),
                    );
                    let _ =
                        line.paint(origin, line_height, gpui::TextAlign::Left, None, window, cx);
                }
            },
        )
        .size_full();

        div()
            .id("sunburst")
            .relative()
            .flex_1()
            .min_h_0()
            .on_mouse_move(
                cx.listener(|view, ev: &MouseMoveEvent, _, cx| view.sunburst_move(ev, cx)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, ev: &MouseDownEvent, _, cx| view.sunburst_down(ev, cx)),
            )
            .on_scroll_wheel(
                cx.listener(|view, ev: &ScrollWheelEvent, _, cx| view.sunburst_scroll(ev, cx)),
            )
            .child(chart)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .max_w(px(160.))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(label),
                            )
                            .child(div().text_sm().text_color(rgb(0x22d3ee)).child(total_txt))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.disabled)
                                    .truncate()
                                    .child(sub),
                            ),
                    ),
            )
    }

    fn render_details(&mut self, colors: &Colors) -> impl IntoElement {
        let units = self.state.units;
        let target = self.state.target_node().or(Some(self.state.focus()));
        let tree = self.state.tree().read();
        let mut lines: Vec<gpui::AnyElement> = Vec::new();
        if let Some(id) = target {
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
            lines.push(
                div()
                    .text_xs()
                    .text_color(colors.disabled)
                    .truncate()
                    .child(tree.path(id).to_string_lossy().into_owned())
                    .into_any_element(),
            );
            let mut l2 = div()
                .flex()
                .gap_3()
                .text_sm()
                .child(div().text_color(colors.disabled).child(kind))
                .child(div().child(format!("{} on disk", size(n.size, units))))
                .child(
                    div()
                        .text_color(colors.disabled)
                        .child(format!("{} apparent", size(n.apparent, units))),
                );
            if parent_total > 0 {
                l2 = l2.child(div().text_color(colors.disabled).child(format!(
                    "{} of parent",
                    percent(tree.display_size(id), parent_total)
                )));
            }
            lines.push(l2.into_any_element());
            if n.kind.is_dir() {
                lines.push(
                    div()
                        .text_xs()
                        .text_color(colors.disabled)
                        .child(format!(
                            "{} files, {} folders",
                            count(n.files),
                            count(n.dirs)
                        ))
                        .into_any_element(),
                );
            }
            let flags = AppState::flag_labels(n.flags);
            if !flags.is_empty() {
                lines.push(
                    div()
                        .text_xs()
                        .text_color(to_rgba(palette::UNREADABLE_PURPLE))
                        .child(flags.join(" · "))
                        .into_any_element(),
                );
            }
        }
        if let Some(v) = &self.state.root_volume {
            lines.push(
                div()
                    .text_xs()
                    .text_color(colors.disabled)
                    .child(format!(
                        "{}: {} used of {}, {} free",
                        v.name,
                        size(v.used(), units),
                        size(v.total, units),
                        size(v.available, units)
                    ))
                    .into_any_element(),
            );
        }
        if let ScanStatus::Done { errors, .. } = self.state.scan_status()
            && *errors > 0
        {
            lines.push(
                div()
                    .text_xs()
                    .text_color(rgb(0xfacc15))
                    .child(format!(
                        "{} items could not be read; run with sudo to include them",
                        count(*errors)
                    ))
                    .into_any_element(),
            );
        }
        drop(tree);
        div()
            .flex()
            .flex_col()
            .h(px(150.))
            .border_t_1()
            .border_color(colors.separator)
            .child(self.panel_header(Panel::Details, None, colors))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .px_3()
                    .py_1()
                    .children(lines),
            )
    }

    fn render_status(&self, colors: &Colors) -> impl IntoElement {
        let km = self.state.keymap();
        let hints: Vec<(String, &str)> = match self.state.panel {
            Panel::Volumes => vec![
                (km.label(Command::Down), "move"),
                (km.label(Command::Enter), "scan"),
            ],
            Panel::Collector => vec![
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
        let mut left = div().flex().gap_3().text_xs().child(
            div()
                .flex()
                .gap_1()
                .child(
                    div()
                        .text_color(rgb(0x22d3ee))
                        .child(km.label(Command::Help)),
                )
                .child(div().text_color(colors.disabled).child("help")),
        );
        for (k, h) in hints {
            left = left.child(
                div()
                    .flex()
                    .gap_1()
                    .child(div().text_color(rgb(0x22d3ee)).child(k))
                    .child(div().text_color(colors.disabled).child(h)),
            );
        }
        let right: (String, Rgba) = if let Some(st) = self.state.status() {
            let c = match st.level {
                Level::Info => rgb(ACCENT),
                Level::Warn => rgb(COLLECTED),
                Level::Error => rgb(0xef4444),
            };
            (st.text.clone(), c)
        } else {
            match self.state.scan_status() {
                ScanStatus::Running { entries, bytes, .. } => (
                    format!(
                        "scanning {} items, {}",
                        count(*entries),
                        size(*bytes, self.state.units)
                    ),
                    rgb(COLLECTED),
                ),
                ScanStatus::Done {
                    entries, elapsed, ..
                } => (
                    format!("{} items in {:.1}s", count(*entries), elapsed.as_secs_f64()),
                    colors.disabled,
                ),
                ScanStatus::Failed(_) => ("scan failed".into(), rgb(0xef4444)),
                ScanStatus::Idle => (String::new(), colors.disabled),
            }
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .px_3()
            .py_1()
            .border_t_1()
            .border_color(colors.separator)
            .bg(colors.container)
            .child(left)
            .child(
                div()
                    .text_xs()
                    .text_color(right.1)
                    .truncate()
                    .child(right.0),
            )
    }

    fn render_overlay(
        &mut self,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let overlay = self.state.overlay().cloned()?;
        let dialog = |title: &str, color: Rgba| {
            div()
                .flex()
                .flex_col()
                .min_w(px(360.))
                .max_w(px(720.))
                .max_h(px(560.))
                .rounded_lg()
                .border_1()
                .border_color(color)
                .bg(colors.background)
                .text_color(colors.text)
                .child(
                    div()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(colors.separator)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(color)
                        .child(title.to_string()),
                )
        };
        let content: gpui::AnyElement = match overlay {
            Overlay::Filter => return None,
            Overlay::Help => {
                let rows = self.state.keymap().help_rows();
                dialog("Keys (any key to close)", rgb(0x22d3ee))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .px_4()
                            .py_2()
                            .overflow_hidden()
                            .children(rows.into_iter().map(|(k, h)| {
                                div()
                                    .flex()
                                    .gap_4()
                                    .text_sm()
                                    .child(div().w(px(150.)).text_color(rgb(0x22d3ee)).child(k))
                                    .child(div().child(h))
                            })),
                    )
                    .into_any_element()
            }
            Overlay::Message { title, lines } => dialog(&title, rgb(0x22d3ee))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .px_4()
                        .py_3()
                        .text_sm()
                        .children(lines.into_iter().map(|l| div().child(l))),
                )
                .into_any_element(),
            Overlay::Menu {
                title,
                items,
                selected,
            } => {
                let km = self.state.keymap();
                let rows: Vec<_> = items
                    .iter()
                    .enumerate()
                    .map(|(i, (label, cmd))| {
                        let row = div()
                            .id(("menu", i))
                            .flex()
                            .gap_4()
                            .px_4()
                            .py_1()
                            .text_sm()
                            .cursor_pointer()
                            .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                                view.apply(Action::MenuPick(i), cx)
                            }))
                            .child(
                                div()
                                    .w(px(16.))
                                    .text_color(rgb(0x22d3ee))
                                    .child(format!("{}", i + 1)),
                            )
                            .child(div().flex_1().child(label.clone()))
                            .child(div().text_color(colors.disabled).child(km.label(*cmd)));
                        if i == selected {
                            row.bg(colors.selected)
                        } else {
                            row.hover(|s| s.bg(colors.container))
                        }
                    })
                    .collect();
                dialog(&title, rgb(0x22d3ee))
                    .child(div().flex().flex_col().py_1().children(rows))
                    .into_any_element()
            }
            Overlay::ConfirmDelete { mode, items, typed } => {
                let units = self.state.units;
                let tree = self.state.tree().read();
                let total: u64 = items.iter().map(|i| tree.display_size(*i)).sum();
                let list: Vec<_> = items
                    .iter()
                    .take(10)
                    .map(|i| {
                        div()
                            .flex()
                            .gap_3()
                            .text_sm()
                            .child(
                                div()
                                    .w(px(80.))
                                    .text_color(colors.disabled)
                                    .child(size(tree.display_size(*i), units)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(tree.path(*i).to_string_lossy().into_owned()),
                            )
                    })
                    .collect();
                let more = (items.len() > 10).then(|| format!("… and {} more", items.len() - 10));
                drop(tree);
                let (title, color) = match mode {
                    DeleteMode::Trash => ("Move to Trash", rgb(COLLECTED)),
                    DeleteMode::Permanent => ("Delete permanently", rgb(0xef4444)),
                };
                let question = match mode {
                    DeleteMode::Trash => format!(
                        "Move {} items ({}) to the Trash?",
                        items.len(),
                        size(total, units)
                    ),
                    DeleteMode::Permanent => format!(
                        "Permanently delete {} items ({})? This cannot be undone.",
                        items.len(),
                        size(total, units)
                    ),
                };
                let button = |label: &str, key: Key, bg: Rgba| {
                    div()
                        .id(label.to_string())
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .bg(bg)
                        .text_sm()
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.85))
                        .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                            view.apply(Action::Key(Chord::key(key)), cx)
                        }))
                        .child(label.to_string())
                };
                let mut body = div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .children(list)
                    .children(more.map(|m| div().text_xs().text_color(colors.disabled).child(m)))
                    .child(div().text_sm().mt_2().child(question));
                if mode == DeleteMode::Permanent {
                    body = body.child(
                        div()
                            .flex()
                            .gap_2()
                            .text_sm()
                            .child(div().child(format!("Type {CONFIRM_WORD} and press enter:")))
                            .child(
                                div()
                                    .text_color(rgb(0xef4444))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child(format!("{typed}▏")),
                            ),
                    );
                }
                let confirm_key = if mode == DeleteMode::Trash {
                    Key::Char('y')
                } else {
                    Key::Enter
                };
                body = body.child(
                    div()
                        .flex()
                        .gap_2()
                        .justify_end()
                        .mt_2()
                        .child(button("Cancel", Key::Esc, colors.container))
                        .child(button(mode.verb(), confirm_key, color)),
                );
                dialog(title, color).child(body).into_any_element()
            }
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000088))
                .child(content)
                .into_any_element(),
        )
    }
}

impl Render for LazyDiskView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = Colors::for_appearance(window);
        let overlay = self.render_overlay(&colors, cx);
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(colors.background)
            .text_color(colors.text)
            .track_focus(&self.focus)
            .key_context("LazyDisk")
            .on_key_down(cx.listener(|view, ev: &KeyDownEvent, _, cx| view.key_down(ev, cx)))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .w(px(340.))
                            .flex()
                            .flex_col()
                            .border_r_1()
                            .border_color(colors.separator)
                            .child(self.render_volumes(&colors, cx))
                            .child(self.render_files(&colors, cx))
                            .child(self.render_collector(&colors, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(self.render_header(&colors, cx))
                            .child(self.render_sunburst(&colors, cx))
                            .child(self.render_details(&colors)),
                    ),
            )
            .child(self.render_status(&colors))
            .children(overlay)
    }
}
