//! Parallel filesystem scanning into a shared [`Tree`].
//!
//! The scanner runs on its own thread, drives a `dua-core` work-stealing walk and
//! batches entries into the tree under a short write lock so frontends can render
//! live totals. Hard links and APFS clones are counted once (configurable).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::RwLock;

use crate::tree::{NodeFlags, NodeId, NodeKind, Tree};

/// A tree shared between the scanner thread and the UI thread.
pub type SharedTree = Arc<RwLock<Tree>>;

/// Tunables for a scan.
#[derive(Clone, Debug)]
pub struct ScanOptions {
    /// Worker threads; `0` uses the available parallelism.
    pub threads: usize,
    /// Do not enter directories on a different filesystem than the root.
    pub stay_on_device: bool,
    /// Count hard-linked content once (first occurrence wins).
    pub dedupe_hardlinks: bool,
    /// Count APFS clone families once (macOS only, first occurrence wins).
    pub dedupe_clones: bool,
    /// Directories that are shown but never entered.
    pub prune: Vec<PathBuf>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            threads: 0,
            stay_on_device: true,
            dedupe_hardlinks: true,
            dedupe_clones: true,
            prune: Vec::new(),
        }
    }
}

/// Progress notifications emitted by a running scan.
#[derive(Clone, Debug)]
pub enum ScanEvent {
    Progress {
        entries: u64,
        bytes: u64,
    },
    Finished {
        entries: u64,
        bytes: u64,
        errors: u64,
        elapsed: Duration,
        /// Up to [`MAX_REPORTED_ERRORS`] error messages, oldest first.
        error_samples: Vec<String>,
        cancelled: bool,
    },
    /// The root itself could not be read; nothing was scanned.
    Failed(String),
}

/// How many error messages a [`ScanEvent::Finished`] carries.
pub const MAX_REPORTED_ERRORS: usize = 50;

/// Handle to a running scan. Dropping it cancels the scan and joins the thread.
pub struct ScanHandle {
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl ScanHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn is_running(&self) -> bool {
        !self.done.load(Ordering::Relaxed)
    }
}

impl Drop for ScanHandle {
    fn drop(&mut self) {
        self.cancel();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Flush entries to the tree at least this often so the UI stays live.
const FLUSH_INTERVAL: Duration = Duration::from_millis(40);
/// Re-sort dirty directories at most this often while scanning.
const SORT_INTERVAL: Duration = Duration::from_millis(250);
const FLUSH_BATCH: usize = 4096;

/// Scan the directory that `attach` points at and add its contents below `attach`.
///
/// For a fresh scan, create the tree with [`Tree::new`] and pass its root. For a
/// rescan call [`Tree::clear_children`] first. The returned receiver yields
/// [`ScanEvent`]s; the final one is always `Finished` or `Failed`.
pub fn scan(
    tree: SharedTree,
    attach: NodeId,
    opts: ScanOptions,
) -> (ScanHandle, Receiver<ScanEvent>) {
    let (tx, rx) = unbounded();
    let cancel = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let root_path = {
        let mut t = tree.write();
        t.node_mut(attach).flags.insert(NodeFlags::SCANNING);
        t.path(attach)
    };
    let c = Arc::clone(&cancel);
    let d = Arc::clone(&done);
    let join = std::thread::Builder::new()
        .name("lazydisk-scan".into())
        .spawn(move || {
            let worker = Worker {
                tree,
                attach,
                root_path,
                opts,
                cancel: c,
                tx,
            };
            worker.run();
            d.store(true, Ordering::Relaxed);
        })
        .expect("spawn scan thread");
    (
        ScanHandle {
            cancel,
            done,
            join: Some(join),
        },
        rx,
    )
}

/// Platform-neutral view of the metadata `dua-core` hands us.
struct Meta {
    len: u64,
    allocated: u64,
    dev: u64,
    ino: u64,
    nlink: u64,
    clone_id: Option<u64>,
    mtime: Option<std::time::SystemTime>,
}

#[cfg(target_os = "macos")]
fn meta(m: &dua_core::Metadata) -> Meta {
    Meta {
        len: m.len(),
        allocated: m.allocated_size(),
        dev: m.dev(),
        ino: m.ino(),
        nlink: m.nlink(),
        clone_id: m.clone_id().map(|c| c.get()),
        mtime: m.modified().ok(),
    }
}

#[cfg(not(target_os = "macos"))]
fn meta(m: &dua_core::Metadata) -> Meta {
    use std::os::unix::fs::MetadataExt;
    Meta {
        len: m.len(),
        allocated: m.blocks().saturating_mul(512),
        dev: m.dev(),
        ino: m.ino(),
        nlink: m.nlink(),
        clone_id: None,
        mtime: m.modified().ok(),
    }
}

fn root_dev(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.dev())
}

/// Devices a scan rooted at `root` may enter when staying on device.
///
/// On macOS the system volume group splits `/` across the read-only system
/// volume and the Data volume (reached through firmlinks such as `/Users`), so
/// both are allowed and `/System/Volumes/Data` itself is pruned to avoid
/// counting the firmlinked folders twice.
pub fn allowed_devices(root: &Path, prune: &mut Vec<PathBuf>) -> HashSet<u64> {
    let mut set = HashSet::new();
    if let Some(d) = root_dev(root) {
        set.insert(d);
    }
    #[cfg(target_os = "macos")]
    if root == Path::new("/") {
        let data = Path::new("/System/Volumes/Data");
        if let Some(d) = root_dev(data) {
            set.insert(d);
            prune.push(data.to_path_buf());
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = prune;
    set
}

struct Worker {
    tree: SharedTree,
    attach: NodeId,
    root_path: PathBuf,
    opts: ScanOptions,
    cancel: Arc<AtomicBool>,
    tx: Sender<ScanEvent>,
}

struct Stats {
    entries: u64,
    bytes: u64,
    errors: u64,
    error_samples: Vec<String>,
}

impl Worker {
    fn run(self) {
        let started = Instant::now();
        let mut prune = self.opts.prune.clone();
        let devices = if self.opts.stay_on_device {
            Some(Arc::new(allowed_devices(&self.root_path, &mut prune)))
        } else {
            None
        };
        let prune: Arc<HashSet<PathBuf>> = Arc::new(prune.into_iter().collect());
        let threads = if self.opts.threads == 0 {
            std::thread::available_parallelism().map_or(4, |n| n.get())
        } else {
            self.opts.threads
        };

        let mut options = dua_core::Options::default();
        #[cfg(target_os = "macos")]
        {
            options.apfs_clone_metadata = self.opts.dedupe_clones;
        }

        let descend = {
            let devices = devices.clone();
            let prune = prune.clone();
            move |entry: &dua_core::Entry| should_descend(entry, devices.as_deref(), &prune)
        };
        let mut walk = dua_core::walk(
            &self.root_path,
            threads,
            dua_core::Order::ParentFirst,
            options,
            descend,
        );

        let mut stats = Stats {
            entries: 0,
            bytes: 0,
            errors: 0,
            error_samples: Vec::new(),
        };
        let mut dir_map: Vec<Option<NodeId>> = Vec::new();
        let mut hardlinks: HashSet<(u64, u64)> = HashSet::new();
        let mut clones: HashSet<(u64, u64)> = HashSet::new();
        let mut batch: Vec<dua_core::Entry> = Vec::with_capacity(FLUSH_BATCH);
        let mut errors: Vec<dua_core::Error> = Vec::new();
        let mut last_flush = Instant::now();
        let mut last_sort = Instant::now();
        let mut root_failed = false;

        while let Some(item) = walk.next_cancellable(&self.cancel) {
            match item {
                Ok(entry) => batch.push(entry),
                Err(err) => {
                    if stats.entries == 0
                        && batch.is_empty()
                        && err.path() == Some(self.root_path.as_path())
                    {
                        root_failed = true;
                        let _ = self.tx.send(ScanEvent::Failed(err.to_string()));
                        break;
                    }
                    errors.push(err);
                }
            }
            if batch.len() >= FLUSH_BATCH || last_flush.elapsed() >= FLUSH_INTERVAL {
                let sort = last_sort.elapsed() >= SORT_INTERVAL;
                self.flush(
                    &mut batch,
                    &mut errors,
                    &mut dir_map,
                    &mut hardlinks,
                    &mut clones,
                    &mut stats,
                    devices.as_deref(),
                    &prune,
                    sort,
                );
                if sort {
                    last_sort = Instant::now();
                }
                last_flush = Instant::now();
                let _ = self.tx.send(ScanEvent::Progress {
                    entries: stats.entries,
                    bytes: stats.bytes,
                });
            }
        }
        drop(walk);
        if root_failed {
            let mut t = self.tree.write();
            let n = t.node_mut(self.attach);
            n.flags.remove(NodeFlags::SCANNING);
            n.flags.insert(NodeFlags::UNREADABLE);
            return;
        }
        self.flush(
            &mut batch,
            &mut errors,
            &mut dir_map,
            &mut hardlinks,
            &mut clones,
            &mut stats,
            devices.as_deref(),
            &prune,
            true,
        );
        {
            let mut t = self.tree.write();
            t.node_mut(self.attach).flags.remove(NodeFlags::SCANNING);
        }
        let _ = self.tx.send(ScanEvent::Finished {
            entries: stats.entries,
            bytes: stats.bytes,
            errors: stats.errors,
            elapsed: started.elapsed(),
            error_samples: stats.error_samples,
            cancelled: self.cancel.load(Ordering::Relaxed),
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn flush(
        &self,
        batch: &mut Vec<dua_core::Entry>,
        errors: &mut Vec<dua_core::Error>,
        dir_map: &mut Vec<Option<NodeId>>,
        hardlinks: &mut HashSet<(u64, u64)>,
        clones: &mut HashSet<(u64, u64)>,
        stats: &mut Stats,
        devices: Option<&HashSet<u64>>,
        prune: &HashSet<PathBuf>,
        sort: bool,
    ) {
        if batch.is_empty() && errors.is_empty() && !sort {
            return;
        }
        let mut tree = self.tree.write();
        for entry in batch.drain(..) {
            self.insert(
                &mut tree, entry, dir_map, hardlinks, clones, stats, devices, prune,
            );
        }
        for err in errors.drain(..) {
            stats.errors += 1;
            if stats.error_samples.len() < MAX_REPORTED_ERRORS {
                stats.error_samples.push(err.to_string());
            }
            if let Some(p) = err.path()
                && let Some(id) = resolve_path(&tree, self.attach, &self.root_path, p)
            {
                tree.node_mut(id).flags.insert(NodeFlags::UNREADABLE);
            }
        }
        if sort {
            tree.sort_children();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn insert(
        &self,
        tree: &mut Tree,
        entry: dua_core::Entry,
        dir_map: &mut Vec<Option<NodeId>>,
        hardlinks: &mut HashSet<(u64, u64)>,
        clones: &mut HashSet<(u64, u64)>,
        stats: &mut Stats,
        devices: Option<&HashSet<u64>>,
        prune: &HashSet<PathBuf>,
    ) {
        let kind = if entry.file_type.is_dir() {
            NodeKind::Dir
        } else if entry.file_type.is_file() {
            NodeKind::File
        } else if entry.file_type.is_symlink() {
            NodeKind::Symlink
        } else {
            NodeKind::Other
        };

        let id = if entry.depth == 0 {
            self.attach
        } else {
            let parent = entry
                .parent_directory_id
                .and_then(|d| dir_map.get(d.index()).copied().flatten())
                .unwrap_or(self.attach);
            let name = entry
                .file_name
                .to_string_lossy()
                .into_owned()
                .into_boxed_str();
            tree.add_child(parent, name, kind)
        };
        if kind == NodeKind::Dir {
            if let Some(d) = entry.directory_id {
                let idx = d.index();
                if dir_map.len() <= idx {
                    dir_map.resize(idx + 1, None);
                }
                dir_map[idx] = Some(id);
            }
            if entry.depth > 0 && !should_descend(&entry, devices, prune) {
                tree.node_mut(id).flags.insert(NodeFlags::PRUNED);
            }
        }
        stats.entries += 1;

        match &entry.metadata {
            Some(Ok(m)) => {
                let m = meta(m);
                // Allocated size includes directory blocks (like `du`); apparent size only
                // counts file contents so it reads as "bytes of data".
                let mut allocated = m.allocated;
                let mut apparent = if kind == NodeKind::Dir { 0 } else { m.len };
                if kind == NodeKind::File {
                    if self.opts.dedupe_hardlinks
                        && m.nlink > 1
                        && !hardlinks.insert((m.dev, m.ino))
                    {
                        tree.node_mut(id).flags.insert(NodeFlags::HARDLINK_DUP);
                        allocated = 0;
                        apparent = 0;
                    } else if self.opts.dedupe_clones
                        && let Some(c) = m.clone_id
                        && !clones.insert((m.dev, c))
                    {
                        tree.node_mut(id).flags.insert(NodeFlags::CLONE_DUP);
                        allocated = 0;
                    }
                }
                {
                    let n = tree.node_mut(id);
                    n.mtime = m.mtime;
                }
                tree.add_sizes(id, allocated, apparent);
                stats.bytes += allocated;
            }
            Some(Err(_)) => {
                tree.node_mut(id).flags.insert(NodeFlags::UNREADABLE);
            }
            None => {}
        }
    }
}

fn should_descend(
    entry: &dua_core::Entry,
    devices: Option<&HashSet<u64>>,
    prune: &HashSet<PathBuf>,
) -> bool {
    if entry.depth == 0 {
        return true;
    }
    if let Some(devs) = devices
        && let Some(Ok(m)) = &entry.metadata
        && !devs.contains(&meta(m).dev)
    {
        return false;
    }
    if !prune.is_empty() && prune.contains(&entry.path()) {
        return false;
    }
    true
}

/// Find the node for `path` by walking name components below `attach`.
fn resolve_path(tree: &Tree, attach: NodeId, root: &Path, path: &Path) -> Option<NodeId> {
    let rel = path.strip_prefix(root).ok()?;
    let mut cur = attach;
    for comp in rel.components() {
        let name = comp.as_os_str().to_string_lossy();
        cur = tree.child_named(cur, &name)?;
    }
    Some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn write(path: &Path, bytes: usize) {
        let mut f = fs::File::create(path).unwrap();
        f.write_all(&vec![b'x'; bytes]).unwrap();
    }

    fn run(root: &Path, opts: ScanOptions) -> (Tree, ScanEvent) {
        let tree = Arc::new(RwLock::new(Tree::new(root)));
        let (handle, rx) = scan(Arc::clone(&tree), NodeId(0), opts);
        let mut last = None;
        for ev in rx {
            last = Some(ev);
        }
        drop(handle);
        let t = tree.read().clone();
        (t, last.unwrap())
    }

    #[test]
    fn scans_fixture_tree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("a/deep")).unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        write(&root.join("a/one.bin"), 10_000);
        write(&root.join("a/deep/two.bin"), 20_000);
        write(&root.join("b/three.bin"), 5_000);
        write(&root.join("top.bin"), 1_000);

        let (t, last) = run(root, ScanOptions::default());
        let r = t.root();
        assert!(matches!(
            last,
            ScanEvent::Finished {
                cancelled: false,
                ..
            }
        ));
        assert_eq!(t.node(r).files, 4);
        assert_eq!(t.node(r).dirs, 3);
        // Apparent size is exact; allocated size is at least the apparent size rounded to blocks.
        assert_eq!(t.node(r).apparent, 36_000);
        assert!(t.node(r).size >= 36_000, "allocated {}", t.node(r).size);
        let a = t.child_named(r, "a").unwrap();
        assert_eq!(t.node(a).apparent, 30_000);
        assert_eq!(t.children(r)[0], a, "largest child sorted first");
        let deep = t.child_named(a, "deep").unwrap();
        assert_eq!(t.path(deep), root.join("a/deep"));
        assert_eq!(t.node(deep).kind, NodeKind::Dir);
    }

    #[test]
    fn hard_links_counted_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("orig.bin"), 8_192);
        fs::hard_link(root.join("orig.bin"), root.join("link.bin")).unwrap();

        let (t, _) = run(root, ScanOptions::default());
        assert_eq!(t.node(t.root()).apparent, 8_192);
        let dup = t
            .children(t.root())
            .iter()
            .find(|c| t.node(**c).flags.contains(NodeFlags::HARDLINK_DUP));
        assert!(dup.is_some(), "one of the links is flagged as duplicate");

        let (t2, _) = run(
            root,
            ScanOptions {
                dedupe_hardlinks: false,
                ..ScanOptions::default()
            },
        );
        assert_eq!(t2.node(t2.root()).apparent, 16_384);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apfs_clones_counted_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("orig.bin"), 65_536);
        let status = std::process::Command::new("cp")
            .arg("-c")
            .arg(root.join("orig.bin"))
            .arg(root.join("clone.bin"))
            .status()
            .unwrap();
        if !status.success() {
            eprintln!("cp -c unsupported here, skipping");
            return;
        }
        let (t, _) = run(root, ScanOptions::default());
        let r = t.root();
        // Apparent size counts both files, allocated counts the shared blocks once.
        assert_eq!(t.node(r).apparent, 131_072);
        let single = t.node(t.child_named(r, "orig.bin").unwrap()).size
            + t.node(t.child_named(r, "clone.bin").unwrap()).size;
        assert_eq!(t.node(r).size, single);
        let flagged = t
            .children(r)
            .iter()
            .filter(|c| t.node(**c).flags.contains(NodeFlags::CLONE_DUP))
            .count();
        assert_eq!(flagged, 1);
    }

    #[test]
    fn missing_root_fails() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("nope");
        let (t, last) = run(&root, ScanOptions::default());
        assert!(matches!(last, ScanEvent::Failed(_)));
        assert!(t.node(t.root()).flags.contains(NodeFlags::UNREADABLE));
    }

    #[test]
    fn cancel_stops_scan() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for i in 0..200 {
            write(&root.join(format!("f{i}")), 10);
        }
        let tree = Arc::new(RwLock::new(Tree::new(root)));
        let (handle, rx) = scan(Arc::clone(&tree), NodeId(0), ScanOptions::default());
        handle.cancel();
        let last = rx.iter().last().unwrap();
        assert!(matches!(
            last,
            ScanEvent::Finished { .. } | ScanEvent::Failed(_)
        ));
        assert!(
            !tree
                .read()
                .node(NodeId(0))
                .flags
                .contains(NodeFlags::SCANNING)
        );
    }
}
