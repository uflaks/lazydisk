//! Sunburst geometry shared by every renderer.
//!
//! Angles are radians in `[0, TAU)`, measured clockwise from 12 o'clock. Ring
//! `1` holds the children of the focused directory, ring `2` their children,
//! and so on; the focused directory itself is the centre. Renderers map a
//! point to `(ring, angle)` and call [`SunburstLayout::segment_at`].

use std::f64::consts::TAU;

use crate::tree::{NodeFlags, NodeId, NodeKind, Tree};

/// What a sunburst segment stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentKind {
    Dir,
    File,
    /// Several siblings too thin to draw individually, merged into one petal.
    /// `Segment::node` is their parent; `Segment::group_from` indexes the first grouped child.
    Smaller,
    HiddenSpace,
    FreeSpace,
}

/// One annular sector of the sunburst.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub node: NodeId,
    /// Ring index, `1` is the innermost ring around the centre.
    pub depth: u8,
    /// Start angle (radians, clockwise from top).
    pub a0: f64,
    /// End angle, always greater than `a0`.
    pub a1: f64,
    pub kind: SegmentKind,
    /// Base hue in degrees, inherited from the ring-1 ancestor.
    pub hue: f32,
    /// Directory could not be read.
    pub unreadable: bool,
    /// For [`SegmentKind::Smaller`]: index into the parent's children where the group starts.
    pub group_from: u32,
    /// Display size of the segment in bytes.
    pub size: u64,
}

impl Segment {
    pub fn span(&self) -> f64 {
        self.a1 - self.a0
    }
    pub fn mid(&self) -> f64 {
        (self.a0 + self.a1) / 2.0
    }
    pub fn contains_angle(&self, angle: f64) -> bool {
        angle >= self.a0 && angle < self.a1
    }
}

/// Computed sunburst for a focus node.
#[derive(Clone, Debug)]
pub struct SunburstLayout {
    pub focus: NodeId,
    pub rings: u8,
    /// Display size of the focus node (or of its children when larger).
    pub total: u64,
    pub tree_version: u64,
    /// `ring_segments[0]` is ring 1. Each ring is sorted by `a0`.
    ring_segments: Vec<Vec<Segment>>,
}

/// Minimum angular span (radians) below which siblings are merged, as a fraction of a full turn.
pub const DEFAULT_MIN_SPAN: f64 = TAU / 180.0;

impl SunburstLayout {
    /// Lay out `rings` rings below `focus`. Siblings thinner than `min_span` radians are grouped.
    pub fn compute(tree: &Tree, focus: NodeId, rings: u8, min_span: f64) -> Self {
        let rings = rings.clamp(1, 8);
        let mut layout = Self {
            focus,
            rings,
            total: 0,
            tree_version: tree.version(),
            ring_segments: vec![Vec::new(); rings as usize],
        };
        let total = total_of(tree, focus);
        layout.total = total;
        if total > 0 {
            layout.fill(tree, focus, 1, 0.0, TAU, None, min_span);
        }
        for ring in &mut layout.ring_segments {
            ring.sort_by(|a, b| a.a0.total_cmp(&b.a0));
        }
        layout
    }

    #[allow(clippy::too_many_arguments)]
    fn fill(
        &mut self,
        tree: &Tree,
        node: NodeId,
        depth: u8,
        a0: f64,
        a1: f64,
        hue: Option<f32>,
        min_span: f64,
    ) {
        if depth > self.rings {
            return;
        }
        let total = total_of(tree, node);
        if total == 0 {
            return;
        }
        let span = a1 - a0;
        let mut cursor = a0;
        let children = tree.children(node);
        let mut grouped_from: Option<usize> = None;
        let mut grouped_size = 0u64;
        for (i, &child) in children.iter().enumerate() {
            let n = tree.node(child);
            if n.is_deleted() {
                continue;
            }
            let size = tree.display_size(child);
            if size == 0 {
                continue;
            }
            let child_span = span * (size as f64 / total as f64);
            if child_span < min_span {
                if grouped_from.is_none() {
                    grouped_from = Some(i);
                }
                grouped_size += size;
                continue;
            }
            let c0 = cursor;
            let c1 = (cursor + child_span).min(a1);
            cursor = c1;
            let kind = match n.kind {
                NodeKind::Dir => SegmentKind::Dir,
                NodeKind::HiddenSpace => SegmentKind::HiddenSpace,
                NodeKind::FreeSpace => SegmentKind::FreeSpace,
                _ => SegmentKind::File,
            };
            let seg_hue = hue.unwrap_or_else(|| ((c0 + c1) / 2.0 / TAU * 360.0) as f32);
            self.ring_segments[depth as usize - 1].push(Segment {
                node: child,
                depth,
                a0: c0,
                a1: c1,
                kind,
                hue: seg_hue,
                unreadable: n.flags.contains(NodeFlags::UNREADABLE),
                group_from: 0,
                size,
            });
            if kind == SegmentKind::Dir {
                self.fill(tree, child, depth + 1, c0, c1, Some(seg_hue), min_span);
            }
        }
        if let Some(from) = grouped_from
            && grouped_size > 0
        {
            let g_span = span * (grouped_size as f64 / total as f64);
            let c0 = cursor;
            let c1 = (cursor + g_span).min(a1);
            if c1 > c0 {
                self.ring_segments[depth as usize - 1].push(Segment {
                    node,
                    depth,
                    a0: c0,
                    a1: c1,
                    kind: SegmentKind::Smaller,
                    hue: hue.unwrap_or(0.0),
                    unreadable: false,
                    group_from: from as u32,
                    size: grouped_size,
                });
            }
        }
    }

    /// Segments of ring `depth` (1-based), sorted by start angle.
    pub fn ring(&self, depth: u8) -> &[Segment] {
        if depth == 0 || depth as usize > self.ring_segments.len() {
            return &[];
        }
        &self.ring_segments[depth as usize - 1]
    }

    pub fn segments(&self) -> impl Iterator<Item = &Segment> {
        self.ring_segments.iter().flatten()
    }

    /// The segment covering `angle` on ring `depth`, if any.
    pub fn segment_at(&self, depth: u8, angle: f64) -> Option<&Segment> {
        let ring = self.ring(depth);
        if ring.is_empty() {
            return None;
        }
        let angle = angle.rem_euclid(TAU);
        let idx = ring.partition_point(|s| s.a0 <= angle);
        if idx == 0 {
            return None;
        }
        let seg = &ring[idx - 1];
        seg.contains_angle(angle).then_some(seg)
    }

    /// The segment that represents `node` directly (not as part of a group).
    pub fn segment_for(&self, node: NodeId) -> Option<&Segment> {
        self.segments()
            .find(|s| s.node == node && s.kind != SegmentKind::Smaller)
    }

    /// The group segment on ring 1 that swallows `node` (a child of the focus), if any.
    pub fn group_containing(&self, tree: &Tree, node: NodeId) -> Option<&Segment> {
        let parent = tree.node(node).parent?;
        let idx = tree.children(parent).iter().position(|c| *c == node)? as u32;
        self.segments()
            .find(|s| s.kind == SegmentKind::Smaller && s.node == parent && idx >= s.group_from)
    }
}

/// Size used as the denominator for a node's children: the node's own display size,
/// or the sum of its children when virtual children (hidden/free space) push it higher.
pub fn total_of(tree: &Tree, node: NodeId) -> u64 {
    let own = tree.display_size(node);
    let has_virtual = tree
        .children(node)
        .iter()
        .any(|c| tree.node(*c).kind.is_virtual());
    if has_virtual {
        let sum: u64 = tree
            .children(node)
            .iter()
            .filter(|c| !tree.node(**c).is_deleted())
            .map(|c| tree.display_size(*c))
            .sum();
        own.max(sum)
    } else {
        own
    }
}

/// Convert a point relative to the centre into `(angle, radius)`. `dy` grows downwards.
pub fn polar(dx: f64, dy: f64) -> (f64, f64) {
    let angle = dx.atan2(-dy).rem_euclid(TAU);
    (angle, (dx * dx + dy * dy).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::path::Path;

    fn tree_from(sizes: &[(&str, &[u64])]) -> Tree {
        // Each top-level dir gets a list of file sizes.
        let mut t = Tree::new(Path::new("/r"));
        let r = t.root();
        for (name, files) in sizes {
            let d = t.add_child(r, (*name).into(), NodeKind::Dir);
            for (i, s) in files.iter().enumerate() {
                let f = t.add_child(d, format!("f{i}").into(), NodeKind::File);
                t.add_sizes(f, *s, *s);
            }
        }
        t.sort_children();
        t
    }

    #[test]
    fn ring_one_covers_full_turn() {
        let t = tree_from(&[("a", &[600]), ("b", &[300]), ("c", &[100])]);
        let l = SunburstLayout::compute(&t, t.root(), 3, 0.0);
        let ring = l.ring(1);
        assert_eq!(ring.len(), 3);
        assert!((ring[0].a0 - 0.0).abs() < 1e-9);
        assert!((ring[2].a1 - TAU).abs() < 1e-9);
        assert!((ring[0].span() - TAU * 0.6).abs() < 1e-9);
        // files on ring 2 sit inside their parent's span
        for s in l.ring(2) {
            let p = t.node(s.node).parent.unwrap();
            let ps = l.segment_for(p).unwrap();
            assert!(s.a0 >= ps.a0 - 1e-9 && s.a1 <= ps.a1 + 1e-9);
            assert_eq!(s.hue, ps.hue);
        }
    }

    #[test]
    fn tiny_siblings_are_grouped() {
        let files: Vec<u64> = std::iter::once(10_000)
            .chain(std::iter::repeat_n(1, 50))
            .collect();
        let t = tree_from(&[("a", &files)]);
        let a = t.children(t.root())[0];
        let l = SunburstLayout::compute(&t, a, 1, DEFAULT_MIN_SPAN);
        let ring = l.ring(1);
        assert_eq!(ring.len(), 2);
        assert_eq!(ring[1].kind, SegmentKind::Smaller);
        assert_eq!(ring[1].group_from, 1);
        assert_eq!(ring[1].size, 50);
        let small = t.children(a)[5];
        assert!(l.group_containing(&t, small).is_some());
        assert!(l.segment_for(small).is_none());
    }

    #[test]
    fn hit_testing() {
        let t = tree_from(&[("a", &[500]), ("b", &[500])]);
        let l = SunburstLayout::compute(&t, t.root(), 2, 0.0);
        let a = t.child_named(t.root(), "a").unwrap();
        let b = t.child_named(t.root(), "b").unwrap();
        assert_eq!(l.segment_at(1, 0.1).unwrap().node, a);
        assert_eq!(l.segment_at(1, TAU / 2.0 + 0.1).unwrap().node, b);
        assert_eq!(l.segment_at(1, TAU + 0.1).unwrap().node, a, "angles wrap");
        assert!(l.segment_at(3, 0.1).is_none());
        assert!(l.segment_at(0, 0.1).is_none());
        // 3 o'clock is a quarter turn clockwise from the top.
        let (angle, r) = polar(10.0, 0.0);
        assert!((angle - TAU / 4.0).abs() < 1e-9);
        assert!((r - 10.0).abs() < 1e-9);
        let (angle, _) = polar(0.0, -10.0);
        assert!(angle.abs() < 1e-9, "straight up is angle 0");
    }

    #[test]
    fn empty_focus_has_no_segments() {
        let t = tree_from(&[("a", &[])]);
        let l = SunburstLayout::compute(&t, t.root(), 4, 0.0);
        assert_eq!(l.segments().count(), 0);
        assert_eq!(l.total, 0);
    }

    proptest! {
        #[test]
        fn rings_are_contiguous_and_within_parent(sizes in prop::collection::vec(prop::collection::vec(0u64..10_000, 0..8), 1..8)) {
            let named: Vec<(String, Vec<u64>)> = sizes.into_iter().enumerate().map(|(i, v)| (format!("d{i}"), v)).collect();
            let refs: Vec<(&str, &[u64])> = named.iter().map(|(n, v)| (n.as_str(), v.as_slice())).collect();
            let t = tree_from(&refs);
            let l = SunburstLayout::compute(&t, t.root(), 2, 0.0);
            for depth in 1..=2u8 {
                let ring = l.ring(depth);
                for w in ring.windows(2) {
                    prop_assert!(w[0].a1 <= w[1].a0 + 1e-9, "segments overlap");
                }
                for s in ring {
                    prop_assert!(s.a1 > s.a0);
                    prop_assert!(s.a0 >= -1e-9 && s.a1 <= TAU + 1e-9);
                    if depth == 2 {
                        let p = t.node(s.node).parent.unwrap();
                        let ps = l.segment_for(p).unwrap();
                        prop_assert!(s.a0 >= ps.a0 - 1e-9 && s.a1 <= ps.a1 + 1e-9);
                    }
                }
            }
            let sum: f64 = l.ring(1).iter().map(Segment::span).sum();
            if l.total > 0 {
                prop_assert!((sum - TAU).abs() < 1e-6, "ring 1 sums to a full turn, got {sum}");
            }
        }
    }
}
