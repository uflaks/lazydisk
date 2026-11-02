//! lazydisk: keyboard-driven disk usage analyzer for the terminal.

mod app;
mod keys;
mod sunburst;
mod ui;

use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use lazydisk_core::config::{self, Config};
use lazydisk_core::tree::SizeMode;

/// Keyboard-driven disk space analyzer with a sunburst map.
#[derive(Parser, Debug)]
#[command(name = "lazydisk", version, about)]
struct Args {
    /// Folder or volume to scan (default: current directory).
    path: Option<PathBuf>,
    /// Show apparent (logical) sizes instead of allocated size on disk.
    #[arg(short = 'A', long)]
    apparent: bool,
    /// Number of sunburst rings (1-8).
    #[arg(long)]
    rings: Option<u8>,
    /// Scanner threads (0 = automatic).
    #[arg(short = 'j', long)]
    threads: Option<usize>,
    /// Follow into other filesystems mounted below the root.
    #[arg(long)]
    cross_device: bool,
    /// Count every hard link and APFS clone separately.
    #[arg(long)]
    no_dedupe: bool,
    /// Print an example configuration file and exit.
    #[arg(long)]
    print_config: bool,
    /// Write a debug log to this file.
    #[arg(long, value_name = "FILE")]
    log: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if args.print_config {
        print!("{}", config::example());
        return Ok(());
    }
    if let Some(log) = &args.log {
        let file =
            std::fs::File::create(log).with_context(|| format!("creating {}", log.display()))?;
        tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?),
            )
            .init();
    }

    let (mut cfg, cfg_path): (Config, _) = match config::load() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("warning: {e}; using defaults");
            (Config::default(), None)
        }
    };
    if let Some(p) = cfg_path {
        tracing::info!("config loaded from {}", p.display());
    }
    if args.apparent {
        cfg.size_mode = SizeMode::Apparent;
    }
    if let Some(r) = args.rings {
        cfg.rings = r.clamp(1, 8);
    }
    if let Some(t) = args.threads {
        cfg.threads = t;
    }
    if args.cross_device {
        cfg.stay_on_device = false;
    }
    if args.no_dedupe {
        cfg.dedupe_hardlinks = false;
        cfg.dedupe_clones = false;
    }

    let path = args.path.unwrap_or_else(|| PathBuf::from("."));
    let path = std::fs::canonicalize(&path)
        .with_context(|| format!("{} does not exist", path.display()))?;
    if !path.is_dir() {
        anyhow::bail!("{} is not a directory", path.display());
    }

    let terminal = ratatui::init();
    let result = app::run(terminal, &path, &cfg);
    ratatui::restore();
    result
}
