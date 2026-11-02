//! Human-readable sizes and percentages.

use serde::{Deserialize, Serialize};

/// Which unit system to use when printing byte counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Units {
    /// 1 KiB = 1024 bytes, printed as `KiB`, `MiB`, ... (default).
    #[default]
    Binary,
    /// 1 kB = 1000 bytes, printed as `kB`, `MB`, ...
    Si,
}

const BINARY: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
const SI: [&str; 7] = ["B", "kB", "MB", "GB", "TB", "PB", "EB"];

/// Format `bytes` with at most one decimal, e.g. `12.3 GiB` or `512 B`.
pub fn size(bytes: u64, units: Units) -> String {
    let (base, names) = match units {
        Units::Binary => (1024f64, &BINARY),
        Units::Si => (1000f64, &SI),
    };
    if bytes < 1000 && units == Units::Si || bytes < 1024 && units == Units::Binary {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut idx = 0;
    while value >= base && idx + 1 < names.len() {
        value /= base;
        idx += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", names[idx])
    } else {
        format!("{value:.1} {}", names[idx])
    }
}

/// Compact variant for narrow columns, e.g. `12.3G`, `512B`.
pub fn size_short(bytes: u64, units: Units) -> String {
    let s = size(bytes, units);
    let (num, unit) = s.split_once(' ').unwrap_or((&s, ""));
    format!("{num}{}", unit.chars().next().unwrap_or('B'))
}

/// `part` as a percentage of `whole`, e.g. `29%`. Returns `0%` for an empty whole.
pub fn percent(part: u64, whole: u64) -> String {
    if whole == 0 {
        return "0%".into();
    }
    let p = part as f64 / whole as f64 * 100.0;
    if p < 1.0 && part > 0 {
        "<1%".into()
    } else {
        format!("{p:.0}%")
    }
}

/// Thousands-separated integer, e.g. `1,204`.
pub fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(0, Units::Binary), "0 B");
        assert_eq!(size(1023, Units::Binary), "1023 B");
        assert_eq!(size(1024, Units::Binary), "1.0 KiB");
        assert_eq!(size(12_345_678_901, Units::Binary), "11.5 GiB");
        assert_eq!(size(12_345_678_901, Units::Si), "12.3 GB");
        assert_eq!(size(150 * 1024 * 1024, Units::Binary), "150 MiB");
        assert_eq!(size_short(12_345_678_901, Units::Si), "12.3G");
    }

    #[test]
    fn percents_and_counts() {
        assert_eq!(percent(29, 100), "29%");
        assert_eq!(percent(1, 1000), "<1%");
        assert_eq!(percent(0, 0), "0%");
        assert_eq!(count(1_204), "1,204");
        assert_eq!(count(88_102_000), "88,102,000");
        assert_eq!(count(7), "7");
    }
}
