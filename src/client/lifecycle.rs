//! Notebook lifecycle: `NB_DIR` root validation, readiness-gated
//! existence probing, native creation with convergence, and root
//! confinement.

use std::path::{Path, PathBuf};

use crate::error::NbError;
use crate::nbrc::{ResolutionMode, resolve_nbrc_store};
use crate::validate::validate_notebook_name;

/// Creation semantics: idempotent ensure converges on the winner;
/// explicit creation refuses an existing notebook.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CreateMode {
    Ensure,
    Exact,
}

/// Central `NB_DIR` root: canonical directory or typed failure. The
/// store is resolved per the caller-selected [`ResolutionMode`]
/// (always parsing the rc when present, unless ignored or strict)
/// and validated the same afterwards: a missing, unset,
/// non-directory, or unresolvable root is a configuration failure;
/// genuine I/O failures propagate instead of collapsing into
/// absence.
pub(super) async fn notebook_root_dir(
    mode: ResolutionMode,
    strict: bool,
) -> Result<PathBuf, NbError> {
    let nb_dir = resolve_nbrc_store(mode, strict).await?;
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

/// Whether a symbolic-ref target is confined enough to join
/// under `.git` without escaping: under `refs/`, without parent
/// traversal or control characters. Git-grammar validity itself
/// (including `+` and non-ASCII names) is left to `git`, which
/// verifies the ref during readiness; this gate only ensures an
/// unvalidated path can never leave `.git`.
fn is_valid_ref_name(r: &str) -> bool {
    if !r.starts_with("refs/") || r.len() <= "refs/".len() {
        return false;
    }
    if r.contains("..")
        || r.contains("//")
        || r.starts_with('/')
        || r.ends_with('/')
        || r.contains('\0')
    {
        return false;
    }
    if r.chars().any(|c| c.is_control()) {
        return false;
    }
    // Path-risky and git-forbidden ASCII never joins: backslash
    // (Windows separator), tilde/caret/colon/question/star/bracket
    // and reflog syntax. `+`, `]` and non-ASCII pass through and
    // are adjudicated by git during verification.
    if r.contains(['~', '^', ':', '?', '*', '[', '\\']) || r.contains("@{") {
        return false;
    }
    true
}

/// Whether a string is a full object hash (SHA-1 or SHA-256).
fn is_valid_hex_hash(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64)
        && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Whether the notebook's `HEAD` resolves to a usable commit: a
/// confined loose ref target, a packed-refs entry, or a full
/// detached hash, each verified with `git rev-parse HEAD^{commit}`
/// so bogus, missing, or non-commit objects never read as ready.
/// Anything else means initialization has not finished or the
/// repository is malformed. Read failures propagate as `Io`
/// instead of collapsing into absence.
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
                if !is_valid_hex_hash(content.trim()) {
                    return Ok(false);
                }
                return head_commit_usable(root);
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
                return head_commit_usable(root);
            }
        }
        return Ok(false);
    }
    if !is_valid_hex_hash(head) {
        return Ok(false);
    }
    head_commit_usable(root)
}

/// Whether `HEAD` peels to an existing commit object. Runs bound
/// to the notebook's own Git directory, so a malformed local
/// repository can never resolve an ancestor's `HEAD`. A git
/// failure (bogus/missing/non-commit) reads as not-ready; spawn
/// and I/O failures propagate instead of collapsing.
fn head_commit_usable(root: &Path) -> Result<bool, NbError> {
    match crate::git::git_capture_local(
        root,
        &[
            "rev-parse".to_string(),
            "--verify".to_string(),
            "HEAD^{commit}".to_string(),
        ],
    ) {
        Ok(_) => Ok(true),
        Err(NbError::CommandFailed { .. }) => Ok(false),
        Err(e) => Err(e),
    }
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

/// Fast readiness probe with no waiting and no `nb` subprocesses
/// (git verification runs for usable-`HEAD` checks). Callers
/// decide convergence: waits belong to creation paths, never to
/// point-in-time reads.
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
            crate::git::git_capture_local(
                &path,
                &[
                    "rev-parse".into(),
                    "--verify".into(),
                    "HEAD^{commit}".into(),
                ],
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
    mode: ResolutionMode,
    strict: bool,
) -> Result<Option<PathBuf>, NbError> {
    let root = notebook_root_dir(mode, strict).await?;
    notebook_dir_from_env_at(&root, notebook).await
}

/// Probe mapping against an already-resolved store root: no
/// second resolution. The caller pins the store once.
pub(super) async fn notebook_dir_from_env_at(
    root: &Path,
    notebook: &str,
) -> Result<Option<PathBuf>, NbError> {
    match probe_notebook_dir(root, notebook)? {
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
            crate::git::git_capture_local(
                &path,
                &[
                    "rev-parse".into(),
                    "--verify".into(),
                    "HEAD^{commit}".into(),
                ],
            )?;
            Ok(Some(confined_canonical(root, &path)?))
        },
    }
}

/// Ensure against an already-resolved store root: no second
/// resolution. Creation converges under the same root that was
/// probed, so a concurrent rc or env change cannot split the
/// operation across stores.
pub(super) async fn ensure_notebook_dir_at(
    root: &Path,
    notebook: &str,
    allow_create: bool,
) -> Result<PathBuf, NbError> {
    match probe_notebook_dir(root, notebook)? {
        Probe::Ready(path) => Ok(path),
        Probe::NotADirectory => Err(not_a_notebook(&root.join(notebook))),
        Probe::Absent => {
            if !allow_create {
                return Err(NbError::ValidationError {
                    reason: format!(
                        "notebook `{notebook}` does not exist and automatic creation is disabled"
                    ),
                    location: None,
                });
            }
            create_notebook_dir_at(root, notebook, CreateMode::Ensure).await
        },
        Probe::Pending | Probe::NotANotebook => {
            match await_ready(root, notebook).await? {
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
                    create_notebook_dir_at(root, notebook, CreateMode::Ensure)
                        .await
                },
            }
        },
    }
}

/// Native notebook creation under `NB_DIR`, against an
/// already-resolved store root: no discovery, no second
/// resolution. Ensure mode converges idempotently on a concurrent
/// winner; exact mode refuses an existing notebook. Refusals never
/// initialize or remove anything.
pub(super) async fn create_notebook_dir_at(
    root: &std::path::Path,
    notebook: &str,
    mode: CreateMode,
) -> Result<PathBuf, NbError> {
    match probe_notebook_dir(root, notebook)? {
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
                #[cfg(feature = "testing")]
                if let Some(ms) =
                    std::env::var_os("NB_API_PAUSE_AFTER_MKDIR_MS")
                    && let Ok(ms) = ms.to_string_lossy().parse::<u64>()
                    && ms > 0
                {
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                }
                return finish_new_notebook_dir(root, notebook, &path).await;
            },
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                match await_ready(root, notebook).await? {
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
/// path). Failures retain the partial directory for inspection and
/// report its location; no recursive deletion runs, so foreign
/// state can never be destroyed by a failed create.
async fn finish_new_notebook_dir(
    root: &Path,
    notebook: &str,
    path: &Path,
) -> Result<PathBuf, NbError> {
    let _ours = match crate::git::notebook_init_empty(path) {
        Ok(hash) => hash,
        Err(e) => {
            tracing::warn!(
                "notebook init failed; partial directory preserved at `{}` for inspection",
                path.display()
            );
            return Err(e);
        },
    };
    let ready = match head_resolvable(path) {
        Ok(ready) => ready,
        Err(e) => {
            tracing::warn!(
                "notebook verification failed; partial directory preserved at `{}` for inspection",
                path.display()
            );
            return Err(e);
        },
    };
    if ready
        && let Ok(canonical) = confined_canonical(root, path)
        && canonical == root.join(notebook)
    {
        return Ok(canonical);
    }
    Err(NbError::ValidationError {
        reason: format!(
            "created notebook `{notebook}` failed its validity probe; concurrent modification suspected; partial directory preserved at `{}` for inspection",
            path.display()
        ),
        location: None,
    })
}
