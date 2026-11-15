//! Notebook lifecycle: `NB_DIR` root validation, readiness-gated
//! existence probing, native creation with convergence, and root
//! confinement.

use std::path::{Path, PathBuf};

use crate::error::NbError;
use crate::validate::validate_notebook_name;

/// Creation semantics: idempotent ensure converges on the winner;
/// explicit creation refuses an existing notebook.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CreateMode {
    Ensure,
    Exact,
}

/// Central `NB_DIR` root: canonical directory or typed failure. A
/// missing, unset, or non-directory root is a configuration
/// failure; genuine I/O failures propagate instead of collapsing
/// into absence.
pub(super) fn notebook_root_dir() -> Result<PathBuf, NbError> {
    let Some(nb_dir) = std::env::var_os("NB_DIR") else {
        return Err(NbError::ValidationError {
            reason: "notebook root is not configured (`NB_DIR` is unset)."
                .to_string(),
            location: None,
        });
    };
    let nb_dir = PathBuf::from(nb_dir);
    match std::fs::metadata(&nb_dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(NbError::ValidationError {
                reason: "notebook root does not exist (`NB_DIR`).".to_string(),
                location: None,
            })
        },
        Err(e) => Err(NbError::Io {
            path: nb_dir.clone(),
            source: e.into(),
        }),
        Ok(meta) if !meta.is_dir() => Err(NbError::ValidationError {
            reason: "notebook root is not a directory (`NB_DIR`).".to_string(),
            location: None,
        }),
        Ok(_) => std::fs::canonicalize(&nb_dir).map_err(|e| NbError::Io {
            path: nb_dir.clone(),
            source: e.into(),
        }),
    }
}

/// Readiness of a notebook directory: only a resolvable `HEAD`
/// counts as ready, so observers never mistake a mid-initialization
/// directory for a complete one.
enum Readiness {
    Ready,
    Absent,
    Pending,
    NotADirectory,
}

fn readiness(path: &Path) -> Result<Readiness, NbError> {
    let meta = match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Readiness::Absent);
        },
        Err(e) => {
            return Err(NbError::Io {
                path: path.to_path_buf(),
                source: e.into(),
            });
        },
        Ok(meta) => meta,
    };
    if meta.file_type().is_symlink() {
        return Err(NbError::UnsupportedStructure {
            reason: format!(
                "refusing notebook root through symlink `{}`",
                path.display()
            ),
        });
    }
    if !meta.is_dir() {
        return Ok(Readiness::NotADirectory);
    }
    if head_resolvable(path) {
        return Ok(Readiness::Ready);
    }
    Ok(Readiness::Pending)
}

/// Whether the notebook's `HEAD` resolves, from files alone: a
/// loose ref target, a packed-refs entry, or a detached hash.
/// Anything else means initialization has not finished — or the
/// repository is malformed.
fn head_resolvable(root: &Path) -> bool {
    let head = match std::fs::read_to_string(root.join(".git/HEAD")) {
        Ok(head) => head,
        Err(_) => return false,
    };
    let head = head.trim();
    if let Some(r) = head.strip_prefix("ref: ") {
        let r = r.trim();
        if root.join(".git").join(r).is_file() {
            return true;
        }
        if let Ok(packed) =
            std::fs::read_to_string(root.join(".git/packed-refs"))
        {
            for line in packed.lines() {
                let mut parts = line.split_whitespace();
                let (_hash, name) = match (parts.next(), parts.next()) {
                    (Some(hash), Some(name)) => (hash, name),
                    _ => continue,
                };
                if name == r && !_hash.starts_with('#') {
                    return true;
                }
            }
        }
        return false;
    }
    !head.is_empty()
        && head.len() >= 4
        && head.chars().all(|c| c.is_ascii_hexdigit())
}

/// Canonical path confined under `root`; anything escaping the
/// root is refused rather than followed.
fn confined_canonical(root: &Path, path: &Path) -> Result<PathBuf, NbError> {
    let canonical = std::fs::canonicalize(path).map_err(|e| NbError::Io {
        path: path.to_path_buf(),
        source: e.into(),
    })?;
    if !canonical.starts_with(root) {
        return Err(NbError::UnsupportedStructure {
            reason: format!(
                "refusing notebook path outside root `{}`",
                canonical.display()
            ),
        });
    }
    Ok(canonical)
}

/// Fast readiness probe with no waiting and no subprocesses.
/// Callers decide convergence: waits belong to creation paths,
/// never to point-in-time reads.
#[derive(PartialEq, Eq)]
pub(super) enum Probe {
    Ready(PathBuf),
    Absent,
    Pending,
    NotADirectory,
}

pub(super) fn probe_notebook_dir(
    root: &Path,
    notebook: &str,
) -> Result<Probe, NbError> {
    validate_notebook_name(notebook)?;
    let path = root.join(notebook);
    match readiness(&path)? {
        Readiness::Ready => Ok(Probe::Ready(confined_canonical(root, &path)?)),
        Readiness::Absent => Ok(Probe::Absent),
        Readiness::Pending => Ok(Probe::Pending),
        Readiness::NotADirectory => Ok(Probe::NotADirectory),
    }
}

/// Waits briefly for a present-but-not-ready notebook (usually a
/// concurrent creator mid-init), then settles: valid resolves,
/// vanished is absent, a plain entry is refused, and a `.git`
/// without resolvable `HEAD` surfaces the genuine git failure
/// instead of masquerading as absence. The bound keeps a stuck
/// initializer from hanging callers; a genuinely slow one that
/// finishes inside the window still converges instead of being
/// misclassified.
async fn await_ready(
    root: &Path,
    notebook: &str,
) -> Result<Option<PathBuf>, NbError> {
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        match probe_notebook_dir(root, notebook)? {
            Probe::Ready(path) => return Ok(Some(path)),
            Probe::Absent => return Ok(None),
            // A non-directory entry can never become a notebook
            // through the creation path; refuse without waiting.
            Probe::NotADirectory => {
                return Err(not_a_notebook(&root.join(notebook)));
            },
            Probe::Pending => {},
        }
    }
    settle_pending(root, notebook).await
}

/// Settles an entry that stayed not-ready past the convergence
/// window: vanished is absent, plain entries are refused, and
/// `.git`-without-`HEAD` surfaces the genuine git failure.
async fn settle_pending(
    root: &Path,
    notebook: &str,
) -> Result<Option<PathBuf>, NbError> {
    let path = root.join(notebook);
    match readiness(&path)? {
        Readiness::Ready => Ok(Some(confined_canonical(root, &path)?)),
        Readiness::Absent => Ok(None),
        Readiness::NotADirectory => Err(not_a_notebook(&path)),
        Readiness::Pending => {
            match std::fs::symlink_metadata(path.join(".git")) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(not_a_notebook(&path));
                },
                Err(e) => {
                    return Err(NbError::Io {
                        path: path.join(".git"),
                        source: e.into(),
                    });
                },
                Ok(_) => {},
            }
            crate::git::git_capture(
                &path,
                &["rev-parse".into(), "HEAD".into()],
            )?;
            Ok(Some(confined_canonical(root, &path)?))
        },
    }
}

fn not_a_notebook(path: &Path) -> NbError {
    NbError::ValidationError {
        reason: format!(
            "`{}` exists but is not a notebook; refusing to initialize it.",
            path.display()
        ),
        location: None,
    }
}

/// Notebook dir from the environment: canonical path for a ready
/// notebook, `None` for true absence or non-directory entries.
/// Never creates, ensures, or waits: a present-but-not-ready entry
/// surfaces the genuine git failure instead of reading as absent,
/// so malformed notebooks are never mistaken for missing ones.
pub(super) async fn notebook_dir_from_env(
    notebook: &str,
) -> Result<Option<PathBuf>, NbError> {
    let root = notebook_root_dir()?;
    match probe_notebook_dir(&root, notebook)? {
        Probe::Ready(path) => Ok(Some(path)),
        Probe::Absent | Probe::NotADirectory => Ok(None),
        Probe::Pending => {
            let path = root.join(notebook);
            crate::git::git_capture(
                &path,
                &["rev-parse".into(), "HEAD".into()],
            )?;
            Ok(Some(confined_canonical(&root, &path)?))
        },
    }
}

/// Ensures the notebook exists, creating it when allowed.
/// Converges idempotently on concurrent winners; disabled creation
/// refuses missing notebooks without touching anything. Plain
/// non-directory entries are refused, never initialized.
pub(super) async fn ensure_notebook_dir(
    notebook: &str,
    allow_create: bool,
) -> Result<PathBuf, NbError> {
    let root = notebook_root_dir()?;
    match probe_notebook_dir(&root, notebook)? {
        Probe::Ready(path) => Ok(path),
        Probe::NotADirectory => Err(not_a_notebook(&root.join(notebook))),
        Probe::Absent | Probe::Pending => {
            if !allow_create {
                return Err(NbError::ValidationError {
                    reason: format!(
                        "notebook `{notebook}` does not exist and automatic creation is disabled"
                    ),
                    location: None,
                });
            }
            create_notebook_dir(notebook, CreateMode::Ensure).await
        },
    }
}

/// Native notebook creation under `NB_DIR`. Ensure mode converges
/// idempotently on a concurrent winner; exact mode refuses an
/// existing notebook. Refusals never initialize or remove anything.
pub(super) async fn create_notebook_dir(
    notebook: &str,
    mode: CreateMode,
) -> Result<PathBuf, NbError> {
    validate_notebook_name(notebook)?;
    let root = notebook_root_dir()?;
    if let Probe::Ready(existing) = probe_notebook_dir(&root, notebook)? {
        return match mode {
            CreateMode::Ensure => Ok(existing),
            CreateMode::Exact => Err(NbError::ValidationError {
                reason: format!("notebook `{notebook}` already exists"),
                location: None,
            }),
        };
    }
    let path = root.join(notebook);
    for _ in 0..3 {
        match std::fs::create_dir(&path) {
            Ok(()) => {
                return finish_new_notebook_dir(&root, notebook, &path).await;
            },
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                match await_ready(&root, notebook).await? {
                    Some(existing) => {
                        return match mode {
                            CreateMode::Ensure => Ok(existing),
                            CreateMode::Exact => {
                                Err(NbError::ValidationError {
                                    reason: format!(
                                        "notebook `{notebook}` already exists"
                                    ),
                                    location: None,
                                })
                            },
                        };
                    },
                    None => continue,
                }
            },
            Err(e) => {
                return Err(NbError::Io {
                    path: path.clone(),
                    source: e.into(),
                });
            },
        }
    }
    Err(NbError::ValidationError {
        reason: format!(
            "notebook `{notebook}` creation did not settle; concurrent activity may still be in progress"
        ),
        location: None,
    })
}

/// Completes a freshly created notebook directory: initializes the
/// empty layout and verifies the result (causal barrier: creation
/// is done only when the notebook resolves to exactly the created
/// path). Cleans up owned artifacts on failure, never another
/// writer's contents.
async fn finish_new_notebook_dir(
    root: &Path,
    notebook: &str,
    path: &Path,
) -> Result<PathBuf, NbError> {
    let ours = match crate::git::notebook_init_empty(path) {
        Ok(hash) => hash,
        Err(e) => {
            remove_owned_notebook_dir(path, None);
            return Err(e);
        },
    };
    if head_resolvable(path)
        && let Ok(canonical) = confined_canonical(root, path)
        && canonical == root.join(notebook)
    {
        return Ok(canonical);
    }
    remove_owned_notebook_dir(path, Some(&ours));
    Err(NbError::ValidationError {
        reason: format!(
            "created notebook `{notebook}` failed its validity probe; concurrent modification suspected"
        ),
        location: None,
    })
}

/// Removes a just-created notebook directory only when ownership is
/// established: no symlinks, exactly the artifacts creation
/// produced, ref state consistent with `ours` (`None` means no
/// commit of ours exists), and a clean worktree. Anything else
/// present means another writer arrived; it is left untouched and
/// reported.
fn remove_owned_notebook_dir(path: &Path, ours: Option<&str>) {
    if !owned_for_cleanup(path, ours) {
        tracing::warn!(
            "refusing to clean notebook directory `{}`: ownership not established",
            path.display()
        );
        return;
    }
    if let Err(e) = std::fs::remove_dir_all(path) {
        tracing::warn!(
            "failed to clean owned notebook directory `{}`: {e}",
            path.display()
        );
    }
}

fn owned_for_cleanup(path: &Path, ours: Option<&str>) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return false;
    }
    let entries: Vec<std::ffi::OsString> = match std::fs::read_dir(path) {
        Ok(entries) => {
            let mut names = Vec::new();
            for entry in entries {
                let Ok(entry) = entry else {
                    return false;
                };
                names.push(entry.file_name());
            }
            names
        },
        Err(_) => return false,
    };
    if entries
        .iter()
        .any(|name| name != ".git" && name != ".index")
    {
        return false;
    }
    match ours {
        None => !has_refs(path),
        Some(hash) => {
            let Ok(head) = std::fs::read_to_string(path.join(".git/HEAD"))
            else {
                return false;
            };
            let Some(r) = head.trim().strip_prefix("ref: ") else {
                return false;
            };
            let r = r.trim();
            let Ok(content) =
                std::fs::read_to_string(path.join(".git").join(r))
            else {
                return false;
            };
            if content.trim() != hash {
                return false;
            }
            if has_other_refs(path, r) {
                return false;
            }
            if path.join(".git/packed-refs").exists() {
                return false;
            }
            match crate::git::git_capture(
                path,
                &["status".into(), "--porcelain".into()],
            ) {
                Ok(status) => status.trim().is_empty(),
                Err(_) => false,
            }
        },
    }
}

/// Whether any refs exist under `.git/refs` at all.
fn has_refs(path: &Path) -> bool {
    refs_files(&path.join(".git/refs")).is_some_and(|files| !files.is_empty())
}

/// Whether `.git/refs` holds any file other than the given ref.
fn has_other_refs(path: &Path, allowed: &str) -> bool {
    match refs_files(&path.join(".git/refs")) {
        Some(files) => files.iter().any(|f| f.to_string_lossy() != allowed),
        None => true,
    }
}

/// Relative file paths under a refs directory; `None` on any
/// read failure (treated as unknown, therefore unsafe).
fn refs_files(dir: &Path) -> Option<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = std::fs::read_dir(&current).ok()?;
        for entry in entries {
            let entry = entry.ok()?;
            let file_type = entry.file_type().ok()?;
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                let full = entry.path();
                let rel = full.strip_prefix(dir).ok()?;
                out.push(rel.to_path_buf());
            } else {
                return None;
            }
        }
    }
    Some(out)
}
