//! Half-block raster of the sunburst. Every terminal cell carries two vertically
//! stacked pixels (`▀`/`▄`/`█` with independent fg/bg), which makes pixels
//! roughly square in common terminal fonts.

use std::f64::consts::TAU;

use lazydisk_core::layout::{SegmentKind, SunburstLayout, polar};
use lazydisk_core::palette::{Rgb, segment_color};
use lazydisk_core::tree::{NodeId, Tree};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::Widget;

/// Where the rings sit inside a cell rectangle, in pixel units (1 column = 1 px, 1 row = 2 px).
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub cx: f64,
    pub cy: f64,
    pub hole: f64,
    pub radius: f64,
    pub ring_w: f64,
    pub rings: u8,
}

pub enum Located {
    Outside,
    Center,
    Ring(u8, f64),
}

impl Geometry {
    pub fn new(area: Rect, rings: u8) -> Self {
        let w = f64::from(area.width);
        let h = f64::from(area.height) * 2.0;
        let radius = (w / 2.0).min(h / 2.0) - 0.5;
        let hole = (radius * 0.28).max(3.0);
        let rings = rings.max(1);
        Self {
            cx: w / 2.0,
            cy: h / 2.0,
            hole,
            radius,
            ring_w: ((radius - hole) / f64::from(rings)).max(0.5),
            rings,
        }
    }

    pub fn locate(&self, px: f64, py: f64) -> Located {
        let (angle, r) = polar(px - self.cx, py - self.cy);
        if r < self.hole {
            return Located::Center;
        }
        if r >= self.radius {
            return Located::Outside;
        }
        let ring = ((r - self.hole) / self.ring_w) as u8 + 1;
        if ring > self.rings {
            return Located::Outside;
        }
        Located::Ring(ring, angle)
    }

    /// Distance from the inner edge of the ring containing `r`, in pixels.
    fn ring_offset(&self, r: f64) -> f64 {
        (r - self.hole) % self.ring_w
    }
}

pub enum HitResult {
    None,
    Center,
    /// Node under the pointer and whether it is a directory.
    Segment(NodeId, bool),
}

/// Resolve a mouse click at terminal cell (`col`, `row`) inside `area`.
pub fn hit(area: Rect, layout: &SunburstLayout, tree: &Tree, col: u16, row: u16) -> HitResult {
    if col < area.x || row < area.y {
        return HitResult::None;
    }
    let geo = Geometry::new(area, layout.rings);
    let px = f64::from(col - area.x) + 0.5;
    let py = f64::from(row - area.y) * 2.0 + 1.0;
    match geo.locate(px, py) {
        Located::Outside => HitResult::None,
        Located::Center => HitResult::Center,
        Located::Ring(ring, angle) => match layout.segment_at(ring, angle) {
            Some(seg) if seg.kind == SegmentKind::Smaller => {
                // Clicking a group selects its first member.
                match tree.children(seg.node).get(seg.group_from as usize) {
                    Some(n) => HitResult::Segment(*n, tree.node(*n).kind.is_dir()),
                    None => HitResult::None,
                }
            }
            Some(seg) => HitResult::Segment(seg.node, seg.kind == SegmentKind::Dir),
            None => HitResult::None,
        },
    }
}

pub struct Sunburst<'a> {
    pub layout: &'a SunburstLayout,
    /// Node whose own segment is highlighted.
    pub highlight: Option<NodeId>,
    /// `(parent, group_from)` of a "smaller objects" group to highlight.
    pub highlight_group: Option<(NodeId, u32)>,
    /// Text drawn in the centre hole.
    pub center: Vec<Line<'a>>,
    /// Draws thin dark separators between segments and rings.
    pub separators: bool,
}

fn darken(c: Rgb, f: f32) -> Rgb {
    (
        (c.0 as f32 * f) as u8,
        (c.1 as f32 * f) as u8,
        (c.2 as f32 * f) as u8,
    )
}

impl Sunburst<'_> {
    fn pixel(&self, geo: &Geometry, px: f64, py: f64) -> Option<Rgb> {
        let Located::Ring(ring, angle) = geo.locate(px, py) else {
            return None;
        };
        let seg = self.layout.segment_at(ring, angle)?;
        let highlighted = match seg.kind {
            SegmentKind::Smaller => self.highlight_group == Some((seg.node, seg.group_from)),
            _ => self.highlight == Some(seg.node),
        };
        let mut color = segment_color(seg, highlighted);
        if self.separators {
            let (_, r) = polar(px - geo.cx, py - geo.cy);
            let ring_edge = geo.ring_offset(r) < 0.6 && geo.ring_w >= 3.0;
            // Angular separator: within ~0.6px of arc length from the segment start.
            let arc_from_start = (angle - seg.a0).rem_euclid(TAU) * r;
            let seg_edge = arc_from_start < 0.6 && seg.span() * r > 2.5;
            if ring_edge || seg_edge {
                color = darken(color, 0.55);
            }
        }
        Some(color)
    }
}

impl Widget for Sunburst<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 4 || area.height < 2 {
            return;
        }
        let geo = Geometry::new(area, self.layout.rings);
        for row in 0..area.height {
            for col in 0..area.width {
                let px = f64::from(col) + 0.5;
                let top = self.pixel(&geo, px, f64::from(row) * 2.0 + 0.5);
                let bottom = self.pixel(&geo, px, f64::from(row) * 2.0 + 1.5);
                let Some(cell) = buf.cell_mut((area.x + col, area.y + row)) else {
                    continue;
                };
                match (top, bottom) {
                    (None, None) => {}
                    (Some(t), Some(b)) if t == b => {
                        cell.set_char('█').set_style(Style::new().fg(rgb(t)));
                    }
                    (Some(t), Some(b)) => {
                        cell.set_char('▀')
                            .set_style(Style::new().fg(rgb(t)).bg(rgb(b)));
                    }
                    (Some(t), None) => {
                        cell.set_char('▀').set_style(Style::new().fg(rgb(t)));
                    }
                    (None, Some(b)) => {
                        cell.set_char('▄').set_style(Style::new().fg(rgb(b)));
                    }
                }
            }
        }
        // Centre label: as many lines as fit in the hole, centred.
        let hole_cols = (geo.hole * 2.0 * 0.9) as u16;
        let hole_rows = (geo.hole * 0.9) as u16; // hole is in pixels, 2 px per row
        if hole_cols < 3 || hole_rows == 0 {
            return;
        }
        let lines: Vec<Line> = self.center.into_iter().take(hole_rows as usize).collect();
        let n = lines.len() as u16;
        let cy_row = (geo.cy / 2.0) as u16;
        let start_row = cy_row.saturating_sub(n / 2);
        for (i, line) in lines.into_iter().enumerate() {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let text = truncate(&text, hole_cols as usize);
            let width = text.chars().count() as u16;
            let x = area.x + (geo.cx as u16).saturating_sub(width / 2);
            let y = area.y + start_row + i as u16;
            if y >= area.y + area.height {
                break;
            }
            let style = line.style;
            for (j, ch) in text.chars().enumerate() {
                if let Some(cell) = buf.cell_mut((x + j as u16, y)) {
                    cell.set_char(ch).set_style(style);
                }
            }
        }
    }
}

fn rgb(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

/// Shorten `s` to `max` characters, keeping the start and adding `…`.
pub fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use lazydisk_core::tree::NodeKind;
    use std::path::Path;

    fn tree() -> Tree {
        let mut t = Tree::new(Path::new("/r"));
        let r = t.root();
        let a = t.add_child(r, "a".into(), NodeKind::Dir);
        let f = t.add_child(a, "f".into(), NodeKind::File);
        t.add_sizes(f, 300, 300);
        let g = t.add_child(r, "g".into(), NodeKind::File);
        t.add_sizes(g, 100, 100);
        t.sort_children();
        t
    }

    #[test]
    fn renders_without_panicking_and_fills_ring() {
        let t = tree();
        let layout = SunburstLayout::compute(&t, t.root(), 3, 0.0);
        let area = Rect::new(0, 0, 40, 20);
        let mut buf = Buffer::empty(area);
        Sunburst {
            layout: &layout,
            highlight: None,
            highlight_group: None,
            center: vec![Line::from("/r"), Line::from("400 B")],
            separators: true,
        }
        .render(area, &mut buf);
        let painted = buf.content().iter().filter(|c| c.symbol() != " ").count();
        assert!(painted > 200, "painted {painted}");
        // Centre text present
        let row: String = (0..40).map(|x| buf[(x, 10)].symbol().to_string()).collect();
        assert!(row.contains("400 B") || row.contains("/r"), "{row:?}");
    }

    #[test]
    fn hit_testing_maps_cells() {
        let t = tree();
        let layout = SunburstLayout::compute(&t, t.root(), 2, 0.0);
        let area = Rect::new(5, 3, 40, 20);
        assert!(matches!(hit(area, &layout, &t, 25, 13), HitResult::Center));
        // Right edge middle row is inside ring 1 at 3 o'clock → 'a' covers 0..75% clockwise from top.
        let right = hit(area, &layout, &t, 5 + 20 + 9, 13);
        let a = t.child_named(t.root(), "a").unwrap();
        assert!(matches!(right, HitResult::Segment(n, true) if n == a));
        assert!(matches!(hit(area, &layout, &t, 0, 0), HitResult::None));
    }

    #[test]
    fn truncation() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello world", 6), "hello…");
        assert_eq!(truncate("héllo", 3), "hé…");
    }
}
