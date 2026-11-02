//! Mounted volumes and their capacity.

use std::path::{Path, PathBuf};

/// A mounted filesystem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// Display name (last path component, or the mount point for `/`).
    pub name: String,
    pub mount_point: PathBuf,
    pub device: String,
    pub fs_type: String,
    pub total: u64,
    /// Bytes available to unprivileged users.
    pub available: u64,
    pub free: u64,
}

impl Volume {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }
}

fn display_name(mount: &Path) -> String {
    match mount.file_name() {
        Some(n) if !n.is_empty() => n.to_string_lossy().into_owned(),
        _ => mount.to_string_lossy().into_owned(),
    }
}

/// Hide pseudo filesystems and macOS system-volume plumbing.
fn is_interesting(v: &Volume) -> bool {
    const SKIP_FS: &[&str] = &[
        "devfs",
        "autofs",
        "proc",
        "sysfs",
        "tmpfs",
        "devtmpfs",
        "cgroup",
        "cgroup2",
        "pstore",
        "securityfs",
        "debugfs",
        "tracefs",
        "mqueue",
        "hugetlbfs",
        "fusectl",
        "configfs",
        "bpf",
        "binfmt_misc",
        "nsfs",
        "squashfs",
        "overlay",
        "efivarfs",
        "rpc_pipefs",
        "fuse.portal",
    ];
    if SKIP_FS.contains(&v.fs_type.as_str()) {
        return false;
    }
    let m = v.mount_point.to_string_lossy();
    const SKIP_PREFIX: &[&str] = &[
        "/System/Volumes/",
        "/private/",
        "/dev",
        "/Library/Developer/CoreSimulator/",
        "/snap/",
        "/boot/efi",
        "/run/",
        "/proc",
        "/sys",
        "/var/lib/docker",
    ];
    if SKIP_PREFIX.iter().any(|p| m.starts_with(p)) {
        return false;
    }
    v.total > 0
}

/// Mounted volumes worth showing, root first then by mount point.
pub fn list() -> Vec<Volume> {
    let mut v: Vec<Volume> = platform::all_mounts()
        .into_iter()
        .filter(is_interesting)
        .collect();
    v.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    v.dedup_by(|a, b| a.mount_point == b.mount_point);
    v
}

/// The volume that contains `path`, with current capacity figures.
pub fn containing(path: &Path) -> Option<Volume> {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut best: Option<Volume> = None;
    for v in platform::all_mounts() {
        if path.starts_with(&v.mount_point)
            && best
                .as_ref()
                .is_none_or(|b| v.mount_point.as_os_str().len() > b.mount_point.as_os_str().len())
        {
            best = Some(v);
        }
    }
    best
}

/// `true` when `path` is exactly a mount point.
pub fn is_mount_point(path: &Path) -> bool {
    platform::all_mounts().iter().any(|v| v.mount_point == path)
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod platform {
    use super::{Volume, display_name};
    use std::ffi::CStr;
    use std::path::PathBuf;

    fn cstr(buf: &[libc::c_char]) -> String {
        // SAFETY: buffers from statfs are NUL-terminated within their fixed size.
        unsafe { CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }

    /// macOS links `/Volumes/<Name>` to `/`; that symlink's name is the startup volume's name.
    fn root_volume_name() -> Option<String> {
        for entry in std::fs::read_dir("/Volumes").ok()?.flatten() {
            let path = entry.path();
            if path.is_symlink() && std::fs::read_link(&path).ok()? == std::path::Path::new("/") {
                return Some(entry.file_name().to_string_lossy().into_owned());
            }
        }
        None
    }

    pub fn all_mounts() -> Vec<Volume> {
        let mut out = Vec::new();
        let mut mounts: *mut libc::statfs = std::ptr::null_mut();
        // SAFETY: getmntinfo allocates a per-thread buffer owned by libc and returns its length.
        let count = unsafe { libc::getmntinfo(&mut mounts, libc::MNT_NOWAIT) };
        if count <= 0 || mounts.is_null() {
            return out;
        }
        // SAFETY: libc guarantees `count` valid entries behind `mounts`.
        let slice = unsafe { std::slice::from_raw_parts(mounts, count as usize) };
        let root_name = root_volume_name();
        for s in slice {
            let mount_point = PathBuf::from(cstr(&s.f_mntonname));
            let bsize = s.f_bsize as u64;
            let name = match (&root_name, mount_point == std::path::Path::new("/")) {
                (Some(n), true) => n.clone(),
                _ => display_name(&mount_point),
            };
            out.push(Volume {
                name,
                mount_point,
                device: cstr(&s.f_mntfromname),
                fs_type: cstr(&s.f_fstypename),
                total: s.f_blocks * bsize,
                available: s.f_bavail * bsize,
                free: s.f_bfree * bsize,
            });
        }
        out
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::{Volume, display_name};
    use std::path::PathBuf;

    pub fn all_mounts() -> Vec<Volume> {
        let Ok(text) = std::fs::read_to_string("/proc/mounts") else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for line in text.lines() {
            let mut it = line.split_whitespace();
            let (Some(dev), Some(mnt), Some(fs)) = (it.next(), it.next(), it.next()) else {
                continue;
            };
            let mount_point = PathBuf::from(unescape(mnt));
            let (total, free, available) = match nix::sys::statvfs::statvfs(&mount_point) {
                Ok(st) => {
                    let fr = st.fragment_size() as u64;
                    (
                        st.blocks() as u64 * fr,
                        st.blocks_free() as u64 * fr,
                        st.blocks_available() as u64 * fr,
                    )
                }
                Err(_) => (0, 0, 0),
            };
            out.push(Volume {
                name: display_name(&mount_point),
                mount_point,
                device: dev.to_string(),
                fs_type: fs.to_string(),
                total,
                available,
                free,
            });
        }
        out
    }

    fn unescape(s: &str) -> String {
        s.replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_listed_and_contains_home() {
        let vols = list();
        assert!(
            vols.iter().any(|v| v.mount_point == Path::new("/")),
            "{vols:?}"
        );
        assert!(is_mount_point(Path::new("/")));
        let home = dirs::home_dir().unwrap();
        let v = containing(&home).unwrap();
        assert!(home.starts_with(&v.mount_point));
        assert!(v.total >= v.free);
    }
}
