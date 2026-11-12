//! Native filesystem scope enumeration for typed listings.
//!
//! Confined, symlink-free directory walks that power the typed
//! list/search/task/todo surfaces: direct-child and recursive
//! file listings plus todo-item extraction. All reads are
//! side-effect-free; missing scopes are terminal `NotFound` and
//! genuine I/O failures propagate.

use std::path::{Path, PathBuf};

use crate::error::NbError;
use crate::native::{confine, ensure_no_symlink_prefix};

/// Absolute path of a scope folder: confined, symlink-free
/// ancestors, existing directory. Missing or non-directory scopes
/// are terminal `NotFound`.
fn scope_dir_abs(
    notebook_root: &Path,
    folder: &str,
) -> Result<PathBuf, NbError> {
    let scope_rel = if folder.is_empty() {
        String::new()
    } else {
        match confine(folder).filter(|r| !r.is_empty()) {
            Some(rel) => rel,
            // Unconfineable non-empty scopes (absolute forms,
            // drive letters, `..` escapes) never resolve, not even
            // to the root: fail closed.
            None => {
                return Err(NbError::NotFound {
                    selector: folder.to_string(),
                });
            },
        }
    };
    if !scope_rel.is_empty() {
        ensure_no_symlink_prefix(notebook_root, &scope_rel).map_err(|e| {
            match e {
                NbError::NotFound { .. } => NbError::NotFound {
                    selector: folder.to_string(),
                },
                other => other,
            }
        })?;
    }
    let abs = notebook_root.join(&scope_rel);
    match std::fs::symlink_metadata(&abs) {
        Ok(meta) if meta.file_type().is_symlink() => Err(NbError::NotFound {
            selector: folder.to_string(),
        }),
        Ok(meta) if meta.is_dir() => Ok(abs),
        Ok(_) => Err(NbError::NotFound {
            selector: folder.to_string(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(NbError::NotFound {
                selector: folder.to_string(),
            })
        },
        Err(e) => Err(NbError::Io {
            path: abs,
            source: e.into(),
        }),
    }
}

/// Direct-child entries of a scope folder, lexical by relative path:
/// dotfiles and symlinks are skipped (never followed). `files`
/// selects files (`false` selects directories). Scope validation
/// errors and genuine I/O failures propagate.
fn scope_entries(
    notebook_root: &Path,
    folder: &str,
    files: bool,
) -> Result<Vec<String>, NbError> {
    let abs = scope_dir_abs(notebook_root, folder)?;
    let scope_rel = if folder.is_empty() {
        String::new()
    } else {
        confine(folder)
            .filter(|r| !r.is_empty())
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&abs).map_err(|e| NbError::Io {
        path: abs.clone(),
        source: e.into(),
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| NbError::Io {
            path: abs.clone(),
            source: e.into(),
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| NbError::Io {
            path: path.clone(),
            source: e.into(),
        })?;
        let wanted = if files {
            file_type.is_file()
        } else {
            file_type.is_dir()
        };
        if file_type.is_symlink() || !wanted {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if scope_rel.is_empty() {
            out.push(name.to_string());
        } else {
            out.push(format!("{scope_rel}/{name}"));
        }
    }
    out.sort();
    Ok(out)
}

/// Recursive file walk under a scope: lexical by relative path at
/// every level, skipping dotfiles, dot-directories, and symlinks.
/// Used by typed search and task/todo enumeration.
pub(crate) fn scope_files_recursive(
    notebook_root: &Path,
    folder: &str,
) -> Result<Vec<String>, NbError> {
    fn walk(
        abs: &Path,
        rel: &str,
        out: &mut Vec<String>,
    ) -> Result<(), NbError> {
        let entries = std::fs::read_dir(abs).map_err(|e| NbError::Io {
            path: abs.to_path_buf(),
            source: e.into(),
        })?;
        let mut names: Vec<(String, PathBuf, bool)> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| NbError::Io {
                path: abs.to_path_buf(),
                source: e.into(),
            })?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|e| NbError::Io {
                path: path.clone(),
                source: e.into(),
            })?;
            // Symlinks are never followed; special files are never
            // emitted (a later read must not block on a fifo).
            if file_type.is_symlink()
                || (!file_type.is_dir() && !file_type.is_file())
            {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            names.push((name.to_string(), path, file_type.is_dir()));
        }
        names.sort();
        for (name, path, is_dir) in names {
            let child = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if is_dir {
                walk(&path, &child, out)?;
            } else {
                out.push(child);
            }
        }
        Ok(())
    }

    let abs = scope_dir_abs(notebook_root, folder)?;
    let scope_rel = if folder.is_empty() {
        String::new()
    } else {
        confine(folder)
            .filter(|r| !r.is_empty())
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    walk(&abs, &scope_rel, &mut out)?;
    out.sort();
    Ok(out)
}

/// Checklist items of a todo file body, in file order: `true` is
/// closed. Only top-level `- [ ]` / `- [x]` / `- [X]` markers
/// count; anything else is body text.
pub(crate) fn todo_items(source: &[u8]) -> Vec<bool> {
    let mut out = Vec::new();
    for line in source.split(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.starts_with(b"- [ ]") {
            out.push(false);
        } else if line.starts_with(b"- [x]") || line.starts_with(b"- [X]") {
            out.push(true);
        }
    }
    out
}

/// Direct-child files of a scope folder, lexical by relative path.
pub(crate) fn scope_files(
    notebook_root: &Path,
    folder: &str,
) -> Result<Vec<String>, NbError> {
    scope_entries(notebook_root, folder, true)
}

/// Direct-child directories of a scope folder, lexical by path.
pub(crate) fn scope_dirs(
    notebook_root: &Path,
    folder: &str,
) -> Result<Vec<String>, NbError> {
    scope_entries(notebook_root, folder, false)
}
