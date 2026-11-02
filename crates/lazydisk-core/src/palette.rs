//! Colours for sunburst segments, shared by the terminal and desktop renderers.

use crate::layout::{Segment, SegmentKind};

/// 8-bit RGB triple.
pub type Rgb = (u8, u8, u8);

/// HSL (hue in degrees, saturation and lightness in `0..=1`) to RGB.
pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Rgb {
    let h = h.rem_euclid(360.0) / 360.0;
    let s = s.clamp(0.0, 1.0);
    let l = l.clamp(0.0, 1.0);
    if s == 0.0 {
        let v = (l * 255.0).round() as u8;
        return (v, v, v);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let f = |t: f32| {
        let t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round() as u8
    };
    (f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
}

/// Colour of a segment. `highlight` brightens the selected segment.
pub fn segment_color(seg: &Segment, highlight: bool) -> Rgb {
    let depth = seg.depth.saturating_sub(1) as f32;
    let (h, s, mut l) = match seg.kind {
        SegmentKind::Dir if seg.unreadable => (280.0, 0.55, 0.42 + depth * 0.04),
        SegmentKind::Dir => (seg.hue, 0.62, 0.48 + depth * 0.055),
        SegmentKind::File => (seg.hue, 0.08, 0.50 + depth * 0.05),
        SegmentKind::Smaller => (seg.hue, 0.18, 0.70 + depth * 0.03),
        SegmentKind::HiddenSpace => (270.0, 0.25, 0.38),
        SegmentKind::FreeSpace => (0.0, 0.0, 0.24),
    };
    if highlight {
        l = (l + 0.18).min(0.92);
    }
    hsl_to_rgb(h, s, l)
}

/// Colour for a hue swatch in list rows (matches ring-1 directory colour).
pub fn hue_swatch(hue: f32) -> Rgb {
    hsl_to_rgb(hue, 0.62, 0.48)
}

/// Grey used for files in lists.
pub const FILE_GREY: Rgb = (140, 140, 140);
/// Purple used for unreadable items (needs admin).
pub const UNREADABLE_PURPLE: Rgb = hsl_const(280);

const fn hsl_const(_h: u16) -> Rgb {
    (160, 96, 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert_eq!(hsl_to_rgb(0.0, 1.0, 0.5), (255, 0, 0));
        assert_eq!(hsl_to_rgb(120.0, 1.0, 0.5), (0, 255, 0));
        assert_eq!(hsl_to_rgb(240.0, 1.0, 0.5), (0, 0, 255));
        assert_eq!(hsl_to_rgb(0.0, 0.0, 0.5), (128, 128, 128));
        assert_eq!(
            hsl_to_rgb(360.0 + 120.0, 1.0, 0.5),
            (0, 255, 0),
            "hue wraps"
        );
    }
}
