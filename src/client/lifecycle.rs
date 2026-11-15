//! Notebook lifecycle: `NB_DIR` existence checks, native creation,
//! and root confinement.

use std::path::{Path, PathBuf};

use crate::error::NbError;
use crate::validate::validate_notebook_name;

/// Notebook dir from the environment: `$NB_DIR/<notebook>` when
/// present, without creating, ensuring, or spawning. Genuine
/// filesystem failures propagate; only true absence maps to `None`.
pub(super) fn notebook_dir_from_env(
    notebook: &str,
) -> Result<Option<PathBuf>, NbError> {
    let Some(nb_dir) = std::env::var_os("NB_DIR") else {
        return Ok(None);
    };
    let path = PathBuf::from(nb_dir).join(notebook);
    // Refuse symlink roots before anything follows them: resolution
    // must stay under `NB_DIR`. Only genuine misses map to `None`
    // while permission and I/O failures propagate instead of
    // collapsing into `NotFound`.
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        },
        Err(e) => {
            return Err(NbError::Io {
                path: path.clone(),
                source: e.into(),
            });
        },
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(NbError::UnsupportedStructure {
                reason: format!(
                    "refusing notebook root through symlink `{}`",
                    path.display()
                ),
            });
        },
        Ok(meta) if !meta.is_dir() => return Ok(None),
        Ok(_) => {},
    }
    match std::fs::metadata(path.join(".git")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(NbError::Io {
            path: path.join(".git"),
            source: e.into(),
        }),
        Ok(_) => Ok(Some(path)),
    }
}

/// Native notebook creation under `NB_DIR`: validates the name,
/// exclusively creates the directory, and initializes the empty
/// notebook layout. Concurrent creators converge: exactly one
/// `create_dir` wins while losers wait briefly for the winner's
/// init and then resolve idempotently. Refusals never initialize
/// or remove anything.
pub(super) async fn create_notebook_dir(
    notebook: &str,
) -> Result<PathBuf, NbError> {
    validate_notebook_name(notebook)?;
    let Some(nb_dir) = std::env::var_os("NB_DIR") else {
        return Err(NbError::ValidationError {
            reason: "notebook root is not configured (`NB_DIR` is unset)."
                .to_string(),
            location: None,
        });
    };
    let nb_dir = PathBuf::from(nb_dir);
    if !nb_dir.is_dir() {
        return Err(NbError::ValidationError {
            reason: "notebook root is not a directory (`NB_DIR`).".to_string(),
            location: None,
        });
    }
    let path = nb_dir.join(notebook);
    // Bounded convergence waits: a present-but-not-yet-valid target
    // is usually a concurrent creator mid-init, not a refusal.
    for _ in 0..20 {
        match std::fs::create_dir(&path) {
            Ok(()) => return finish_new_notebook_dir(notebook, &path).await,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                match resolve_existing_notebook_dir(notebook, &path)? {
                    Some(existing) => return Ok(existing),
                    None => {
                        tokio::time::sleep(std::time::Duration::from_millis(
                            50,
                        ))
                        .await;
                    },
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
            "`{}` exists but is not a notebook; refusing to initialize it.",
            path.display()
        ),
        location: None,
    })
}

/// Initializes a freshly created notebook directory and verifies the
/// result through the validity probe (causal barrier: creation is
/// done only when the notebook resolves). Cleans up owned artifacts
/// on failure, never another writer's contents.
async fn finish_new_notebook_dir(
    notebook: &str,
    path: &Path,
) -> Result<PathBuf, NbError> {
    if let Err(e) = crate::git::notebook_init_empty(path) {
        remove_owned_notebook_dir(path);
        return Err(e);
    }
    match notebook_dir_from_env(notebook)? {
        Some(verified) => {
            debug_assert_eq!(verified, path);
            Ok(path.to_path_buf())
        },
        None => {
            remove_owned_notebook_dir(path);
            Err(NbError::ValidationError {
                reason: format!(
                    "created notebook `{notebook}` failed its validity probe; concurrent modification suspected"
                ),
                location: None,
            })
        },
    }
}

/// Resolves an already-present creation target: valid notebooks pass
/// through (`Some`), symlinks and non-notebooks are refused, and a
/// present-but-not-yet-valid directory reports `None` so the caller
/// waits for a concurrent creator instead of refusing.
fn resolve_existing_notebook_dir(
    notebook: &str,
    path: &Path,
) -> Result<Option<PathBuf>, NbError> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        },
        Err(e) => {
            return Err(NbError::Io {
                path: path.to_path_buf(),
                source: e.into(),
            });
        },
    };
    if meta.file_type().is_symlink() {
        return Err(NbError::UnsupportedStructure {
            reason: format!(
                "refusing notebook root through symlink `{}`",
                path.display()
            ),
        });
    }
    if notebook_dir_from_env(notebook)?.is_some() {
        return Ok(Some(path.to_path_buf()));
    }
    Ok(None)
}

/// Removes a just-created notebook directory, but only when it still
/// holds exactly the artifacts creation produced. Anything else
/// present means another writer arrived; it is left untouched.
fn remove_owned_notebook_dir(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return;
        };
        let name = entry.file_name();
        if name != ".git" && name != ".index" {
            return;
        }
    }
    let _ = std::fs::remove_dir_all(path);
}
