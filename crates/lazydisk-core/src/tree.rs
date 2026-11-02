//! Arena-backed size tree. Nodes are addressed by [`NodeId`]; every directory
//! keeps aggregated allocated and apparent sizes so the UI can render totals
//! while a scan is still running.

use std::path::PathBuf;
use std::time::SystemTime;

/// Index of a node inside a [`Tree`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// What a node represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Dir,
    File,
    Symlink,
    /// Sockets, devices, fifos.
    Other,
    /// Virtual item: space used on the volume that no scanned file accounts for.
    HiddenSpace,
    /// Virtual item: free space on the volume.
    FreeSpace,
}

impl NodeKind {
    pub fn is_dir(self) -> bool {
        matches!(self, NodeKind::Dir)
    }
    pub fn is_virtual(self) -> bool {
        matches!(self, NodeKind::HiddenSpace | NodeKind::FreeSpace)
    }
}

/// Bit flags carried by a node.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeFlags(u8);

impl NodeFlags {
    /// Directory could not be read (permission denied etc.).
    pub const UNREADABLE: NodeFlags = NodeFlags(1 << 0);
    /// File is an additional hard link to content counted elsewhere; its sizes are zero here.
    pub const HARDLINK_DUP: NodeFlags = NodeFlags(1 << 1);
    /// File is an APFS clone of content counted elsewhere; its sizes are zero here.
    pub const CLONE_DUP: NodeFlags = NodeFlags(1 << 2);
    /// Node was deleted (or detached) after the scan.
    pub const DELETED: NodeFlags = NodeFlags(1 << 3);
    /// Children are being (re)scanned.
    pub const SCANNING: NodeFlags = NodeFlags(1 << 4);
    /// Directory was deliberately not entered (other device, pruned path).
    pub const PRUNED: NodeFlags = NodeFlags(1 << 5);

    pub fn contains(self, other: NodeFlags) -> bool {
        self.0 & other.0 == other.0
    }
    pub fn insert(&mut self, other: NodeFlags) {
        self.0 |= other.0;
    }
    pub fn remove(&mut self, other: NodeFlags) {
        self.0 &= !other.0;
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// One filesystem object (or virtual item) in the tree.
#[derive(Clone, Debug)]
pub struct Node {
    /// File name. The root node holds its full path.
    pub name: Box<str>,
    pub parent: Option<NodeId>,
    /// Children, kept sorted by descending display size (see [`Tree::sort_children`]).
    pub children: Vec<NodeId>,
    pub kind: NodeKind,
    pub flags: NodeFlags,
    /// Allocated bytes (size on disk), aggregated for directories.
    pub size: u64,
    /// Apparent bytes (file length), aggregated for directories.
    pub apparent: u64,
    /// Number of descendant files (0 for files).
    pub files: u64,
    /// Number of descendant directories (0 for files).
    pub dirs: u64,
    pub mtime: Option<SystemTime>,
    /// Set while a scan is adding children, cleared when children are sorted.
    dirty: bool,
}

impl Node {
    fn new(name: Box<str>, parent: Option<NodeId>, kind: NodeKind) -> Self {
        Self {
            name,
            parent,
            children: Vec::new(),
            kind,
            flags: NodeFlags::default(),
            size: 0,
            apparent: 0,
            files: 0,
            dirs: 0,
            mtime: None,
            dirty: false,
        }
    }

    pub fn is_deleted(&self) -> bool {
        self.flags.contains(NodeFlags::DELETED)
    }
}

/// Which size a frontend displays and lays out by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SizeMode {
    /// Bytes actually allocated on disk (default, matches `du`).
    #[default]
    Allocated,
    /// Logical file lengths (matches `du -A` / `ls -l`).
    Apparent,
}

impl SizeMode {
    pub fn toggle(self) -> Self {
        match self {
            SizeMode::Allocated => SizeMode::Apparent,
            SizeMode::Apparent => SizeMode::Allocated,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            SizeMode::Allocated => "allocated",
            SizeMode::Apparent => "apparent",
        }
    }
}

/// Arena of nodes rooted at [`Tree::root`].
#[derive(Clone, Debug)]
pub struct Tree {
    nodes: Vec<Node>,
    root: NodeId,
    /// Incremented on every structural or size change; frontends cache by it.
    version: u64,
    size_mode: SizeMode,
}

impl Tree {
    /// Create a tree whose root is the directory at `root_path`.
    pub fn new(root_path: &std::path::Path) -> Self {
        let name = root_path.to_string_lossy().into_owned().into_boxed_str();
        Self {
            nodes: vec![Node::new(name, None, NodeKind::Dir)],
            root: NodeId(0),
            version: 1,
            size_mode: SizeMode::Allocated,
        }
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn size_mode(&self) -> SizeMode {
        self.size_mode
    }

    /// Change the display size and re-sort every directory.
    pub fn set_size_mode(&mut self, mode: SizeMode) {
        if self.size_mode != mode {
            self.size_mode = mode;
            for n in &mut self.nodes {
                n.dirty = true;
            }
            self.sort_children();
        }
    }

    #[inline]
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    #[inline]
    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        self.version += 1;
        &mut self.nodes[id.index()]
    }

    /// The size used for display and layout under the current [`SizeMode`].
    #[inline]
    pub fn display_size(&self, id: NodeId) -> u64 {
        let n = &self.nodes[id.index()];
        match self.size_mode {
            SizeMode::Allocated => n.size,
            SizeMode::Apparent => n.apparent,
        }
    }

    /// Append a child to `parent`. Sizes are not added here; see [`Tree::add_sizes`].
    pub fn add_child(&mut self, parent: NodeId, name: Box<str>, kind: NodeKind) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(Node::new(name, Some(parent), kind));
        let p = &mut self.nodes[parent.index()];
        p.children.push(id);
        p.dirty = true;
        if kind == NodeKind::Dir {
            self.bump_counts(parent, 0, 1);
        } else if kind == NodeKind::File {
            self.bump_counts(parent, 1, 0);
        }
        self.version += 1;
        id
    }

    fn bump_counts(&mut self, mut id: NodeId, files: u64, dirs: u64) {
        loop {
            let n = &mut self.nodes[id.index()];
            n.files += files;
            n.dirs += dirs;
            match n.parent {
                Some(p) => id = p,
                None => break,
            }
        }
    }

    /// Add `size`/`apparent` bytes to `id` and all of its ancestors.
    pub fn add_sizes(&mut self, mut id: NodeId, size: u64, apparent: u64) {
        if size == 0 && apparent == 0 {
            return;
        }
        self.version += 1;
        loop {
            let n = &mut self.nodes[id.index()];
            n.size += size;
            n.apparent += apparent;
            n.dirty = true;
            match n.parent {
                Some(p) => id = p,
                None => break,
            }
        }
    }

    /// Subtract sizes from `id` and all of its ancestors (used after deletions).
    pub fn sub_sizes(&mut self, mut id: NodeId, size: u64, apparent: u64) {
        self.version += 1;
        loop {
            let n = &mut self.nodes[id.index()];
            n.size = n.size.saturating_sub(size);
            n.apparent = n.apparent.saturating_sub(apparent);
            n.dirty = true;
            match n.parent {
                Some(p) => id = p,
                None => break,
            }
        }
    }

    /// Sort the children of every directory touched since the last call, largest first.
    pub fn sort_children(&mut self) {
        let mode = self.size_mode;
        let mut changed = false;
        for i in 0..self.nodes.len() {
            if !self.nodes[i].dirty {
                continue;
            }
            self.nodes[i].dirty = false;
            if self.nodes[i].children.len() < 2 {
                continue;
            }
            let mut children = std::mem::take(&mut self.nodes[i].children);
            let key = |n: &Node| match mode {
                SizeMode::Allocated => n.size,
                SizeMode::Apparent => n.apparent,
            };
            children.sort_by(|a, b| {
                key(&self.nodes[b.index()])
                    .cmp(&key(&self.nodes[a.index()]))
                    .then_with(|| self.nodes[a.index()].name.cmp(&self.nodes[b.index()].name))
            });
            self.nodes[i].children = children;
            changed = true;
        }
        if changed {
            self.version += 1;
        }
    }

    /// Mark `id` deleted, detach it from its parent and subtract its sizes from ancestors.
    pub fn remove(&mut self, id: NodeId) {
        let (size, apparent, parent, kind, files, dirs) = {
            let n = &self.nodes[id.index()];
            (n.size, n.apparent, n.parent, n.kind, n.files, n.dirs)
        };
        if let Some(p) = parent {
            self.sub_sizes(p, size, apparent);
            let own_file = u64::from(kind == NodeKind::File);
            let own_dir = u64::from(kind == NodeKind::Dir);
            let mut cur = p;
            loop {
                let n = &mut self.nodes[cur.index()];
                n.files = n.files.saturating_sub(files + own_file);
                n.dirs = n.dirs.saturating_sub(dirs + own_dir);
                match n.parent {
                    Some(pp) => cur = pp,
                    None => break,
                }
            }
            self.nodes[p.index()].children.retain(|c| *c != id);
        }
        let n = &mut self.nodes[id.index()];
        n.flags.insert(NodeFlags::DELETED);
        n.size = 0;
        n.apparent = 0;
        self.version += 1;
    }

    /// Detach all children of `id` (before a rescan). The nodes stay allocated but unreachable.
    pub fn clear_children(&mut self, id: NodeId) {
        let children = std::mem::take(&mut self.nodes[id.index()].children);
        let (size, apparent, files, dirs) = {
            let n = &self.nodes[id.index()];
            (n.size, n.apparent, n.files, n.dirs)
        };
        // Pull the subtree totals out of the ancestors, then reset this node.
        self.sub_sizes(id, size, apparent);
        let mut cur = Some(id);
        while let Some(c) = cur {
            let n = &mut self.nodes[c.index()];
            n.files = n.files.saturating_sub(files);
            n.dirs = n.dirs.saturating_sub(dirs);
            cur = n.parent;
        }
        for c in children {
            self.nodes[c.index()].flags.insert(NodeFlags::DELETED);
            self.nodes[c.index()].parent = None;
        }
        self.version += 1;
    }

    /// Full filesystem path of `id` (virtual nodes map to their parent's path).
    pub fn path(&self, id: NodeId) -> PathBuf {
        let mut parts: Vec<&str> = Vec::new();
        let mut cur = Some(id);
        while let Some(c) = cur {
            let n = &self.nodes[c.index()];
            if !n.kind.is_virtual() {
                parts.push(&n.name);
            }
            cur = n.parent;
        }
        let mut path = PathBuf::new();
        for p in parts.iter().rev() {
            path.push(p);
        }
        path
    }

    /// Depth of `id` below the root (root = 0).
    pub fn depth(&self, id: NodeId) -> usize {
        let mut d = 0;
        let mut cur = self.nodes[id.index()].parent;
        while let Some(c) = cur {
            d += 1;
            cur = self.nodes[c.index()].parent;
        }
        d
    }

    /// Ancestors from the root down to (and including) `id`.
    pub fn ancestry(&self, id: NodeId) -> Vec<NodeId> {
        let mut v = vec![id];
        let mut cur = self.nodes[id.index()].parent;
        while let Some(c) = cur {
            v.push(c);
            cur = self.nodes[c.index()].parent;
        }
        v.reverse();
        v
    }

    /// `true` if `ancestor` is `id` or one of its ancestors.
    pub fn is_ancestor_or_self(&self, ancestor: NodeId, id: NodeId) -> bool {
        let mut cur = Some(id);
        while let Some(c) = cur {
            if c == ancestor {
                return true;
            }
            cur = self.nodes[c.index()].parent;
        }
        false
    }

    /// Children of `id` as a slice (sorted largest first once [`Tree::sort_children`] ran).
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.nodes[id.index()].children
    }

    /// Find a child of `parent` by name.
    pub fn child_named(&self, parent: NodeId, name: &str) -> Option<NodeId> {
        self.nodes[parent.index()]
            .children
            .iter()
            .copied()
            .find(|c| &*self.nodes[c.index()].name == name)
    }

    /// Add (or replace) a virtual child such as hidden/free space under `parent`.
    pub fn set_virtual_child(&mut self, parent: NodeId, kind: NodeKind, size: u64) -> NodeId {
        let name: &str = match kind {
            NodeKind::HiddenSpace => "Hidden space",
            NodeKind::FreeSpace => "Free space",
            _ => panic!("set_virtual_child called with a real kind"),
        };
        let existing = self.nodes[parent.index()]
            .children
            .iter()
            .copied()
            .find(|c| self.nodes[c.index()].kind == kind);
        let id = match existing {
            Some(id) => id,
            None => {
                let id = NodeId(self.nodes.len() as u32);
                self.nodes.push(Node::new(name.into(), Some(parent), kind));
                self.nodes[parent.index()].children.push(id);
                id
            }
        };
        // Virtual sizes are shown but never propagated to ancestors.
        let n = &mut self.nodes[id.index()];
        n.size = size;
        n.apparent = size;
        self.nodes[parent.index()].dirty = true;
        self.version += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn sample() -> (Tree, NodeId, NodeId, NodeId) {
        let mut t = Tree::new(Path::new("/root"));
        let r = t.root();
        let a = t.add_child(r, "a".into(), NodeKind::Dir);
        let b = t.add_child(r, "b".into(), NodeKind::Dir);
        let f1 = t.add_child(a, "f1".into(), NodeKind::File);
        t.add_sizes(f1, 100, 90);
        let f2 = t.add_child(b, "f2".into(), NodeKind::File);
        t.add_sizes(f2, 300, 250);
        t.sort_children();
        (t, a, b, f2)
    }

    #[test]
    fn aggregates_and_sorts() {
        let (t, a, b, _) = sample();
        let r = t.root();
        assert_eq!(t.node(r).size, 400);
        assert_eq!(t.node(r).apparent, 340);
        assert_eq!(t.node(r).files, 2);
        assert_eq!(t.node(r).dirs, 2);
        assert_eq!(t.children(r), &[b, a]);
        assert_eq!(t.path(t.children(a)[0]), Path::new("/root/a/f1"));
        assert_eq!(t.depth(t.children(a)[0]), 2);
    }

    #[test]
    fn apparent_mode_resorts() {
        let (mut t, a, b, f2) = sample();
        t.sub_sizes(f2, 0, 200); // f2 apparent becomes 50 < f1's 90
        t.set_size_mode(SizeMode::Apparent);
        assert_eq!(t.children(t.root()), &[a, b]);
    }

    #[test]
    fn remove_subtracts() {
        let (mut t, _, b, f2) = sample();
        t.remove(f2);
        assert_eq!(t.node(t.root()).size, 100);
        assert_eq!(t.node(b).size, 0);
        assert_eq!(t.node(t.root()).files, 1);
        assert!(t.node(f2).is_deleted());
        assert!(t.children(b).is_empty());
    }

    #[test]
    fn clear_children_resets_subtree() {
        let (mut t, a, _, _) = sample();
        t.clear_children(a);
        assert_eq!(t.node(a).size, 0);
        assert_eq!(t.node(a).files, 0);
        assert_eq!(t.node(t.root()).size, 300);
        assert_eq!(t.node(t.root()).files, 1);
        assert_eq!(t.node(t.root()).dirs, 2);
    }

    #[test]
    fn virtual_children_do_not_propagate() {
        let (mut t, _, _, _) = sample();
        let h = t.set_virtual_child(t.root(), NodeKind::HiddenSpace, 5000);
        assert_eq!(t.node(t.root()).size, 400);
        assert_eq!(t.node(h).size, 5000);
        assert_eq!(t.path(h), Path::new("/root"));
        let h2 = t.set_virtual_child(t.root(), NodeKind::HiddenSpace, 1);
        assert_eq!(h, h2);
    }
}
