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
/// directory for a complete one. A directory without its own `.git`
/// is a plain entry, never a pending notebook; only a `.git`
/// holder with an unresolvable `HEAD` waits as pending.
enum Readiness {
    Ready,
    Absent,
    Pending,
    NotADirectory,
    NotANotebook,
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
    if head_resolvable(path)? {
        return Ok(Readiness::Ready);
    }
    match std::fs::symlink_metadata(path.join(".git")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(Readiness::NotANotebook)
        },
        Err(e) => Err(NbError::Io {
            path: path.join(".git"),
            source: e.into(),
        }),
        Ok(_) => Ok(Readiness::Pending),
    }
}

/// Whether a symbolic-ref target is a confined notebook ref:
/// under `refs/`, without escapes or git-special sequences, so
/// unvalidated paths are never joined outside `.git`.
fn is_valid_ref_name(r: &str) -> bool {
    if !r.starts_with("refs/") || r.len() <= "refs/".len() {
        return false;
    }
    if r.contains("..")
        || r.contains("//")
        || r.starts_with('/')
        || r.ends_with('/')
        || r.ends_with(".lock")
        || r.contains("@{")
    {
        return false;
    }
    r.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-')
    })
}

/// Whether a string is a full object hash (SHA-1 or SHA-256).
fn is_valid_hex_hash(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64)
        && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Whether the notebook's `HEAD` resolves: a validated loose
/// ref target, a validated packed-refs entry, or a full
/// detached hash. Anything else means initialization has not
/// finished or the repository is malformed. Read failures
/// propagate as `Io` instead of collapsing into absence.
fn head_resolvable(root: &Path) -> Result<bool, NbError> {
    let head = match std::fs::read_to_string(root.join(".git/HEAD")) {
        Ok(head) => head,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(false);
        },
        Err(e) => {
            return Err(NbError::Io {
                path: root.join(".git/HEAD"),
                source: e.into(),
            });
        },
    };
    let head = head.trim();
    if let Some(r) = head.strip_prefix("ref: ") {
        let r = r.trim();
        if !is_valid_ref_name(r) {
            return Ok(false);
        }
        match std::fs::read_to_string(root.join(".git").join(r)) {
            Ok(content) => {
                return Ok(is_valid_hex_hash(content.trim()));
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => {
                return Err(NbError::Io {
                    path: root.join(".git").join(r),
                    source: e.into(),
                });
            },
        }
        let packed =
            match std::fs::read_to_string(root.join(".git/packed-refs")) {
                Ok(packed) => packed,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(false);
                },
                Err(e) => {
                    return Err(NbError::Io {
                        path: root.join(".git/packed-refs"),
                        source: e.into(),
                    });
                },
            };
        for line in packed.lines() {
            let line = line.trim();
            if line.is_empty()
                || line.starts_with('#')
                || line.starts_with('^')
            {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (hash, name) = match (parts.next(), parts.next()) {
                (Some(hash), Some(name)) => (hash, name),
                _ => continue,
            };
            if name == r && is_valid_hex_hash(hash) {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    Ok(is_valid_hex_hash(head))
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
    NotANotebook,
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
        Readiness::NotANotebook => Ok(Probe::NotANotebook),
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
            // A plain directory waits here: it may be a concurrent
            // winner between `mkdir` and `git init`, or a
            // pre-existing plain entry that settles as a refusal.
            Probe::NotADirectory => {
                return Err(not_a_notebook(&root.join(notebook)));
            },
            Probe::Pending | Probe::NotANotebook => {},
        }
    }
    settle_pending(root, notebook).await
}

/// Settles an entry that stayed not-ready past the convergence
/// window: vanished is absent, plain entries are refused, and
/// `.git`-without-resolvable-`HEAD` surfaces the genuine git
/// failure. The fallback never lets git search a parent: the
/// directory must carry its own `.git` marker first.
async fn settle_pending(
    root: &Path,
    notebook: &str,
) -> Result<Option<PathBuf>, NbError> {
    let path = root.join(notebook);
    match readiness(&path)? {
        Readiness::Ready => Ok(Some(confined_canonical(root, &path)?)),
        Readiness::Absent => Ok(None),
        Readiness::NotADirectory | Readiness::NotANotebook => {
            Err(not_a_notebook(&path))
        },
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
/// notebook, `None` for true absence or plain entries. Never
/// creates, ensures, or waits: a present-but-not-ready entry
/// surfaces the genuine git failure instead of reading as absent,
/// so malformed notebooks are never mistaken for missing ones.
/// The fallback requires the directory's own `.git` marker and
/// never lets git resolve through an ancestor repository.
pub(super) async fn notebook_dir_from_env(
    notebook: &str,
) -> Result<Option<PathBuf>, NbError> {
    let root = notebook_root_dir()?;
    match probe_notebook_dir(&root, notebook)? {
        Probe::Ready(path) => Ok(Some(path)),
        Probe::Absent | Probe::NotADirectory | Probe::NotANotebook => Ok(None),
        Probe::Pending => {
            let path = root.join(notebook);
            match std::fs::symlink_metadata(path.join(".git")) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(None);
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
            Ok(Some(confined_canonical(&root, &path)?))
        },
    }
}

/// Ensures the notebook exists, creating it when allowed.
/// Converges idempotently on concurrent winners; disabled creation
/// refuses only truly missing notebooks. Present entries
/// (plain or malformed) surface their own refusal or git failure
/// instead of reading as creation-disabled. Plain entries are
/// refused, never initialized.
pub(super) async fn ensure_notebook_dir(
    notebook: &str,
    allow_create: bool,
) -> Result<PathBuf, NbError> {
    let root = notebook_root_dir()?;
    match probe_notebook_dir(&root, notebook)? {
        Probe::Ready(path) => Ok(path),
        Probe::NotADirectory | Probe::NotANotebook => {
            Err(not_a_notebook(&root.join(notebook)))
        },
        Probe::Absent => {
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
        Probe::Pending => match await_ready(&root, notebook).await? {
            Some(path) => Ok(path),
            None => {
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
    match probe_notebook_dir(&root, notebook)? {
        Probe::Ready(existing) => {
            return match mode {
                CreateMode::Ensure => Ok(existing),
                CreateMode::Exact => Err(NbError::ValidationError {
                    reason: format!("notebook `{notebook}` already exists"),
                    location: None,
                }),
            };
        },
        Probe::NotADirectory => {
            return Err(not_a_notebook(&root.join(notebook)));
        },
        // Absent, Pending, and plain entries all reach the
        // `mkdir` race: a plain entry here may be a concurrent
        // winner between `mkdir` and `git init` rather than a
        // pre-existing plain directory. Contenders that lose the
        // race converge in `await_ready`; a truly plain entry
        // settles as a refusal there.
        Probe::Absent | Probe::Pending | Probe::NotANotebook => {},
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
/// writer's contents; preservation is reported in the error when
/// the directory is retained for inspection.
async fn finish_new_notebook_dir(
    root: &Path,
    notebook: &str,
    path: &Path,
) -> Result<PathBuf, NbError> {
    let ours = match crate::git::notebook_init_empty(path) {
        Ok(hash) => hash,
        Err(e) => {
            if !remove_owned_notebook_dir(path, None) {
                tracing::warn!(
                    "notebook init failed; partial directory preserved at `{}` for inspection",
                    path.display()
                );
            }
            return Err(e);
        },
    };
    if head_resolvable(path).unwrap_or(false)
        && let Ok(canonical) = confined_canonical(root, path)
        && canonical == root.join(notebook)
    {
        return Ok(canonical);
    }
    let removed = remove_owned_notebook_dir(path, Some(&ours));
    if removed {
        return Err(NbError::ValidationError {
            reason: format!(
                "created notebook `{notebook}` failed its validity probe; concurrent modification suspected; owned artifacts cleaned up"
            ),
            location: None,
        });
    }
    Err(NbError::ValidationError {
        reason: format!(
            "created notebook `{notebook}` failed its validity probe; concurrent modification suspected; partial directory preserved at `{}` for inspection",
            path.display()
        ),
        location: None,
    })
}

/// Removes a just-created notebook directory only when ownership is
/// established: no symlinks, exactly the artifacts creation
/// produced, ref state consistent with `ours` (`None` means no
/// commit of ours exists), and a clean worktree. Anything else
/// present means another writer arrived; it is left untouched and
/// reported. Returns whether the directory was removed.
fn remove_owned_notebook_dir(path: &Path, ours: Option<&str>) -> bool {
    if !owned_for_cleanup(path, ours) {
        tracing::warn!(
            "refusing to clean notebook directory `{}`: ownership not established",
            path.display()
        );
        return false;
    }
    if let Err(e) = std::fs::remove_dir_all(path) {
        tracing::warn!(
            "failed to clean owned notebook directory `{}`: {e}",
            path.display()
        );
        return false;
    }
    true
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
        None => owned_fresh_for_cleanup(path),
        Some(hash) => {
            let Ok(head) = std::fs::read_to_string(path.join(".git/HEAD"))
            else {
                return false;
            };
            let Some(r) = head.trim().strip_prefix("ref: ") else {
                return false;
            };
            let r = r.trim();
            if !is_valid_ref_name(r) {
                return false;
            }
            let Ok(content) =
                std::fs::read_to_string(path.join(".git").join(r))
            else {
                return false;
            };
            if content.trim() != hash {
                return false;
            }
            if !is_valid_hex_hash(hash) {
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

/// Ownership for a directory whose init never produced our commit:
/// only the exact creation artifacts, no refs (unknown reads fail
/// closed), no packed-refs, an unborn symbolic `HEAD`, an empty
/// `.index`, and a status of exactly the staged `.index` addition.
/// Any foreign edit, detached/packed state, or read failure
/// preserves the directory.
fn owned_fresh_for_cleanup(path: &Path) -> bool {
    if has_refs(path) {
        return false;
    }
    if path.join(".git/packed-refs").exists() {
        return false;
    }
    let head = match std::fs::read_to_string(path.join(".git/HEAD")) {
        Ok(head) => head,
        Err(_) => return false,
    };
    let Some(r) = head.trim().strip_prefix("ref: ") else {
        return false;
    };
    let r = r.trim();
    if !is_valid_ref_name(r) {
        return false;
    }
    if path.join(".git").join(r).exists() {
        return false;
    }
    let index_len = match std::fs::metadata(path.join(".index")) {
        Ok(meta) => {
            if !meta.is_file() {
                return false;
            }
            meta.len()
        },
        Err(_) => return false,
    };
    if index_len != 0 {
        return false;
    }
    match crate::git::git_capture(
        path,
        &["status".into(), "--porcelain".into()],
    ) {
        Ok(status) => status.trim() == "A  .index",
        Err(_) => false,
    }
}

/// Whether any refs exist under `.git/refs` at all. Unknown read
/// failures count as refs (unsafe), so cleanup preserves.
fn has_refs(path: &Path) -> bool {
    match refs_files(&path.join(".git/refs")) {
        Some(files) => !files.is_empty(),
        None => true,
    }
}

/// Whether `.git/refs` holds any file other than the given ref.
/// Unknown read failures count as foreign (unsafe), so cleanup
/// preserves. `allowed` is a full `refs/...` target; entries are
/// relative to `.git/refs`, so the prefix is stripped before
/// comparison.
fn has_other_refs(path: &Path, allowed: &str) -> bool {
    let allowed_rel = allowed.strip_prefix("refs/").unwrap_or(allowed);
    match refs_files(&path.join(".git/refs")) {
        Some(files) => {
            files.iter().any(|f| f.to_string_lossy() != allowed_rel)
        },
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
