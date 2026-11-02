//! User configuration (`~/.config/lazydisk/config.toml`).

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::format::Units;
use crate::tree::SizeMode;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// `allocated` (size on disk) or `apparent` (file lengths).
    pub size_mode: SizeMode,
    /// `binary` (KiB) or `si` (kB).
    pub units: Units,
    /// Sunburst rings shown by default (1–8).
    pub rings: u8,
    /// Scanner threads, `0` = automatic.
    pub threads: usize,
    /// Stop at filesystem boundaries.
    pub stay_on_device: bool,
    /// Count hard-linked content once.
    pub dedupe_hardlinks: bool,
    /// Count APFS clone families once (macOS).
    pub dedupe_clones: bool,
    /// Key overrides: command id → key specs, e.g. `quit = ["q", "ctrl+c"]`.
    pub keys: HashMap<String, Vec<String>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            size_mode: SizeMode::Allocated,
            units: Units::Binary,
            rings: 4,
            threads: 0,
            stay_on_device: true,
            dedupe_hardlinks: true,
            dedupe_clones: true,
            keys: HashMap::new(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid config {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
}

/// Candidate config paths, most preferred first.
pub fn candidate_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(x) = std::env::var("LAZYDISK_CONFIG") {
        v.push(PathBuf::from(x));
    }
    if let Some(home) = dirs::home_dir() {
        v.push(home.join(".config/lazydisk/config.toml"));
    }
    if let Some(c) = dirs::config_dir() {
        v.push(c.join("lazydisk/config.toml"));
    }
    v.dedup();
    v
}

/// Load the first existing config file, or defaults when there is none.
/// Returns the path that was used, if any.
pub fn load() -> Result<(Config, Option<PathBuf>), ConfigError> {
    for path in candidate_paths() {
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Read {
            path: path.clone(),
            source,
        })?;
        let cfg: Config = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.clone(),
            source,
        })?;
        return Ok((cfg, Some(path)));
    }
    Ok((Config::default(), None))
}

/// A commented example config for `--print-config`.
pub fn example() -> String {
    let mut s = String::from(
        "# lazydisk configuration. Place at ~/.config/lazydisk/config.toml\n\
         # Any key you leave out keeps its default.\n\n",
    );
    s.push_str(&toml::to_string_pretty(&Config::default()).unwrap_or_default());
    s.push_str(
        "\n# Rebind commands by id. Specs: a single character, `enter`, `esc`, `space`,\n\
         # `tab`, `up`/`down`/`left`/`right`, `pgup`/`pgdn`, `home`/`end`, `f1`..`f12`,\n\
         # optionally prefixed with `ctrl+`, `alt+` or `shift+`.\n\
         # [keys]\n\
         # quit = [\"q\", \"ctrl+c\"]\n\
         # delete_trash = [\"d\", \"del\"]\n",
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_partial_config() {
        let cfg: Config =
            toml::from_str("rings = 6\nunits = \"si\"\n[keys]\nquit = [\"ctrl+q\"]\n").unwrap();
        assert_eq!(cfg.rings, 6);
        assert_eq!(cfg.units, Units::Si);
        assert_eq!(cfg.size_mode, SizeMode::Allocated);
        assert_eq!(cfg.keys["quit"], vec!["ctrl+q"]);
    }

    #[test]
    fn example_round_trips() {
        let cfg: Config = toml::from_str(&example()).unwrap();
        assert_eq!(cfg.rings, 4);
    }
}
