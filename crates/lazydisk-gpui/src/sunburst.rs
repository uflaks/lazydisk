//! Sunburst painting for gpui: annular sectors tessellated once per layout and
//! reused across frames, plus pointer hit testing.

use std::f64::consts::{PI, TAU};

use gpui::{Bounds, Path, PathBuilder, Pixels, Point, point, px};
use lazydisk_core::layout::{Segment, SegmentKind, SunburstLayout, polar};
use lazydisk_core::tree::{NodeId, Tree};

/// Ring geometry in logical pixels relative to the canvas origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub cx: f64,
    pub cy: f64,
    pub hole: f64,
    pub radius: f64,
    pub ring_w: f64,
    pub rings: u8,
}

impl Geometry {
    pub fn new(width: f64, height: f64, rings: u8) -> Self {
        let radius = (width.min(height) / 2.0 - 8.0).max(10.0);
        let hole = (radius * 0.3).max(24.0);
        Self {
            cx: width / 2.0,
            cy: height / 2.0,
            hole,
            radius,
            ring_w: ((radius - hole) / f64::from(rings.max(1))).max(1.0),
            rings: rings.max(1),
        }
    }

    pub fn inner_radius(&self, depth: u8) -> f64 {
        self.hole + self.ring_w * f64::from(depth - 1)
    }

    pub fn locate(&self, x: f64, y: f64) -> Located {
        let (angle, r) = polar(x - self.cx, y - self.cy);
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
}

pub enum Located {
    Outside,
    Center,
    Ring(u8, f64),
}

pub enum HitResult {
    None,
    Center,
    Segment(NodeId, bool),
}

/// Tessellated sunburst for one layout at one size.
pub struct SunburstPaths {
    pub key: (u64, NodeId, u8, i32, i32),
    pub geometry: Geometry,
    pub origin: Point<Pixels>,
    pub items: Vec<(Segment, Path<Pixels>)>,
}

/// Build (or reuse) the paths for `layout` inside `bounds`.
pub fn build(
    layout: &SunburstLayout,
    bounds: Bounds<Pixels>,
    previous: Option<SunburstPaths>,
) -> SunburstPaths {
    let w = f64::from(bounds.size.width);
    let h = f64::from(bounds.size.height);
    let key = (
        layout.tree_version,
        layout.focus,
        layout.rings,
        w as i32,
        h as i32,
    );
    if let Some(prev) = previous
        && prev.key == key
    {
        return SunburstPaths {
            origin: bounds.origin,
            ..prev
        };
    }
    let geometry = Geometry::new(w, h, layout.rings);
    let ox = f64::from(bounds.origin.x);
    let oy = f64::from(bounds.origin.y);
    let mut items = Vec::new();
    for seg in layout.segments() {
        if let Some(path) = sector(&geometry, seg, ox, oy) {
            items.push((*seg, path));
        }
    }
    SunburstPaths {
        key,
        geometry,
        origin: bounds.origin,
        items,
    }
}

/// Angular gap between neighbouring petals, in pixels of arc length.
const GAP_PX: f64 = 1.5;
/// Radial gap between rings in pixels.
const RING_GAP_PX: f64 = 1.5;

fn sector(geo: &Geometry, seg: &Segment, ox: f64, oy: f64) -> Option<Path<Pixels>> {
    let r0 = geo.inner_radius(seg.depth) + RING_GAP_PX / 2.0;
    let r1 = (geo.inner_radius(seg.depth) + geo.ring_w - RING_GAP_PX / 2.0).max(r0 + 0.5);
    // Shrink the angular span by half a gap on each side, measured on the outer radius.
    let gap = (GAP_PX / r1).min(seg.span() / 4.0);
    let (a0, a1) = if seg.span() >= TAU - 1e-9 {
        (0.0, TAU)
    } else {
        (seg.a0 + gap / 2.0, seg.a1 - gap / 2.0)
    };
    if a1 <= a0 {
        return None;
    }
    let to_point = |angle: f64, r: f64| {
        point(
            px((ox + geo.cx + r * angle.sin()) as f32),
            px((oy + geo.cy - r * angle.cos()) as f32),
        )
    };
    // Polyline steps of roughly one pixel along the outer arc, at least 4 per petal.
    let steps = (((a1 - a0) * r1).ceil() as usize).clamp(4, 2048);
    let mut b = PathBuilder::fill();
    b.move_to(to_point(a0, r1));
    for i in 1..=steps {
        let a = a0 + (a1 - a0) * i as f64 / steps as f64;
        b.line_to(to_point(a, r1));
    }
    let inner_steps = (((a1 - a0) * r0).ceil() as usize).clamp(2, 2048);
    for i in 0..=inner_steps {
        let a = a1 - (a1 - a0) * i as f64 / inner_steps as f64;
        b.line_to(to_point(a, r0));
    }
    b.close();
    b.build().ok()
}

/// Resolve a pointer position (window coordinates) against the current paths.
pub fn hit(
    paths: &SunburstPaths,
    layout: &SunburstLayout,
    tree: &Tree,
    pos: Point<Pixels>,
) -> HitResult {
    let x = f64::from(pos.x) - f64::from(paths.origin.x);
    let y = f64::from(pos.y) - f64::from(paths.origin.y);
    match paths.geometry.locate(x, y) {
        Located::Outside => HitResult::None,
        Located::Center => HitResult::Center,
        Located::Ring(ring, angle) => match layout.segment_at(ring, angle) {
            Some(seg) if seg.kind == SegmentKind::Smaller => {
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

/// Position for a label at the middle of a segment, if the petal is wide enough.
pub fn label_anchor(geo: &Geometry, seg: &Segment, min_arc_px: f64) -> Option<(f64, f64)> {
    let r = geo.inner_radius(seg.depth) + geo.ring_w / 2.0;
    if seg.span() * r < min_arc_px || geo.ring_w < 14.0 {
        return None;
    }
    let a = seg.mid();
    Some((geo.cx + r * a.sin(), geo.cy - r * a.cos()))
}

#[allow(dead_code)]
pub const HALF_TURN: f64 = PI;
