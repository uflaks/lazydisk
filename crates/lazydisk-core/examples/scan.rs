//! Scan a path and print totals: `cargo run --release -p lazydisk-core --example scan -- ~/Library`

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use lazydisk_core::format::{Units, size};
use lazydisk_core::scan::{ScanEvent, ScanOptions, scan};
use lazydisk_core::tree::Tree;
use parking_lot::RwLock;

fn main() {
    let path = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let tree = Arc::new(RwLock::new(Tree::new(&path)));
    let started = Instant::now();
    // Take the root id in its own statement: a read guard alive inside the `scan` call
    // would deadlock against the write lock the scanner takes on start.
    let root = tree.read().root();
    let mut opts = ScanOptions::default();
    if std::env::var_os("LAZYDISK_NO_DEDUPE").is_some() {
        opts.dedupe_hardlinks = false;
        opts.dedupe_clones = false;
    }
    let (_handle, rx) = scan(Arc::clone(&tree), root, opts);
    for ev in rx {
        if let ScanEvent::Finished {
            entries,
            errors,
            elapsed,
            error_samples,
            ..
        } = ev
        {
            let t = tree.read();
            let root = t.node(t.root());
            println!(
                "{}: {} allocated, {} apparent, {} files, {} dirs, {} entries, {} errors in {:.2}s (wall {:.2}s)",
                path.display(),
                size(root.size, Units::Binary),
                size(root.apparent, Units::Binary),
                root.files,
                root.dirs,
                entries,
                errors,
                elapsed.as_secs_f64(),
                started.elapsed().as_secs_f64()
            );
            println!("allocated_kib={}", root.size / 1024);
            for e in error_samples.iter().take(3) {
                println!("  err: {e}");
            }
            for c in t.children(t.root()).iter().take(5) {
                let n = t.node(*c);
                println!("  {:>10}  {}", size(n.size, Units::Binary), n.name);
            }
        }
    }
}
