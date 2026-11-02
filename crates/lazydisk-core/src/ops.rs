//! File operations: deletion (Trash or permanent), reveal, preview, clipboard.
//! Deletion runs on a background thread and reports one result per item.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crossbeam_channel::{Receiver, unbounded};

/// Paths that are never deleted, no matter what the user collected.
#[cfg(target_os = "macos")]
const PROTECTED: &[&str] = &[
    "/",
    "/System",
    "/Library",
    "/Users",
    "/Applications",
    "/private",
    "/usr",
    "/bin",
    "/sbin",
    "/etc",
    "/var",
    "/tmp",
    "/opt",
    "/Volumes",
    "/cores",
    "/home",
    "/dev",
];
#[cfg(not(target_os = "macos"))]
const PROTECTED: &[&str] = &[
    "/", "/bin", "/boot", "/dev", "/etc", "/home", "/lib", "/lib64", "/opt", "/proc", "/root",
    "/run", "/sbin", "/srv", "/sys", "/tmp", "/usr", "/var", "/mnt", "/media",
];

/// Why a path may not be deleted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("{0} is a system folder")]
    SystemFolder(PathBuf),
    #[error("your home folder cannot be deleted")]
    Home,
    #[error("{0} is a volume mount point")]
    MountPoint(PathBuf),
    #[error("{0} is a top-level folder; delete its contents instead")]
    TopLevel(PathBuf),
}

/// Check the safeguards for deleting `path`.
pub fn guard_delete(path: &Path) -> Result<(), Refusal> {
    let p = path.components().collect::<PathBuf>();
    if PROTECTED.iter().any(|s| Path::new(s) == p) {
        return Err(Refusal::SystemFolder(p));
    }
    if let Some(home) = dirs::home_dir()
        && home == p
    {
        return Err(Refusal::Home);
    }
    if p.parent() == Some(Path::new("/")) {
        return Err(Refusal::TopLevel(p));
    }
    if crate::volumes::is_mount_point(&p) {
        return Err(Refusal::MountPoint(p));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteMode {
    /// Move to the system Trash (recoverable).
    Trash,
    /// Remove for good.
    Permanent,
}

impl DeleteMode {
    pub fn verb(self) -> &'static str {
        match self {
            DeleteMode::Trash => "Move to Trash",
            DeleteMode::Permanent => "Delete permanently",
        }
    }
}

/// Outcome for one collected item.
#[derive(Clone, Debug)]
pub struct DeleteResult {
    pub path: PathBuf,
    pub result: Result<(), String>,
}

/// Delete `paths` in the background. The receiver yields one result per path and
/// then closes. Guards are re-checked here so no caller can bypass them.
pub fn delete_all(paths: Vec<PathBuf>, mode: DeleteMode) -> Receiver<DeleteResult> {
    let (tx, rx) = unbounded();
    std::thread::Builder::new()
        .name("lazydisk-delete".into())
        .spawn(move || {
            for path in paths {
                let result = match guard_delete(&path) {
                    Err(r) => Err(r.to_string()),
                    Ok(()) => match mode {
                        DeleteMode::Trash => trash::delete(&path).map_err(|e| e.to_string()),
                        DeleteMode::Permanent => {
                            remove_permanently(&path).map_err(|e| e.to_string())
                        }
                    },
                };
                if tx.send(DeleteResult { path, result }).is_err() {
                    break;
                }
            }
        })
        .expect("spawn delete thread");
    rx
}

fn remove_permanently(path: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

fn spawn_detached(mut cmd: Command) -> Result<(), String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("{}: {e}", cmd.get_program().to_string_lossy()))
}

/// Show `path` in Finder (macOS) or open its folder in the default file manager.
pub fn reveal(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let mut c = Command::new("open");
        c.arg("-R").arg(path);
        spawn_detached(c)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let target = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        let mut c = Command::new("xdg-open");
        c.arg(target);
        spawn_detached(c)
    }
}

/// Quick Look (macOS) or open with the default application.
pub fn preview(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let mut c = Command::new("qlmanage");
        c.arg("-p").arg(path);
        spawn_detached(c)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut c = Command::new("xdg-open");
        c.arg(path);
        spawn_detached(c)
    }
}

/// Copy `text` to the system clipboard using the platform helper.
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    use std::io::Write;
    let candidates: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else {
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    };
    let mut last_err = String::from("no clipboard helper found");
    for (prog, args) in candidates {
        let child = Command::new(prog)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match child {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    stdin
                        .write_all(text.as_bytes())
                        .map_err(|e| e.to_string())?;
                }
                let status = child.wait().map_err(|e| e.to_string())?;
                if status.success() {
                    return Ok(());
                }
                last_err = format!("{prog} exited with {status}");
            }
            Err(e) => last_err = format!("{prog}: {e}"),
        }
    }
    Err(last_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guards() {
        assert!(matches!(
            guard_delete(Path::new("/")),
            Err(Refusal::SystemFolder(_))
        ));
        assert!(matches!(
            guard_delete(Path::new("/usr")),
            Err(Refusal::SystemFolder(_))
        ));
        assert!(guard_delete(Path::new("/Anything")).is_err());
        if let Some(home) = dirs::home_dir() {
            assert_eq!(guard_delete(&home), Err(Refusal::Home));
            assert!(guard_delete(&home.join("some/deep/file.txt")).is_ok());
        }
    }

    #[test]
    fn permanent_delete_removes_dir_and_reports() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        std::fs::create_dir_all(victim.join("inner")).unwrap();
        std::fs::write(victim.join("inner/f"), b"x").unwrap();
        let missing = dir.path().join("missing");
        let results: Vec<DeleteResult> =
            delete_all(vec![victim.clone(), missing.clone()], DeleteMode::Permanent)
                .iter()
                .collect();
        assert_eq!(results.len(), 2);
        assert!(results[0].result.is_ok());
        assert!(!victim.exists());
        assert!(results[1].result.is_err());
    }
}
