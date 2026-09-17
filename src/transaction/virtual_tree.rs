use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::error::NbError;
use crate::git;
use crate::types::NoteTarget;

use super::index::{index_rel_for_folder, parse_index_lines};

#[derive(Debug, Clone)]
pub(super) enum VirtualNode {
    File(Vec<u8>),
    Folder,
}

pub(super) struct VirtualTree {
    /// Notebook-relative paths using `/` separators.
    pub(super) nodes: HashMap<String, VirtualNode>,
}

impl VirtualTree {
    pub(super) fn from_disk(root: &Path) -> Result<Self, NbError> {
        let mut nodes = HashMap::new();
        let paths = git::list_notebook_paths(root)?;
        for rel in paths {
            let key = normalize_rel(&rel);
            let abs = root.join(&rel);
            let meta = std::fs::symlink_metadata(&abs).map_err(|e| NbError::Io {
                path: abs.clone(),
                source: e.into(),
            })?;
            if meta.file_type().is_symlink() {
                return Err(NbError::UnsupportedStructure {
                    reason: format!(
                        "notebook path `{key}` is a symlink; transactions refuse symlink snapshot/materialization"
                    ),
                });
            }
            if meta.is_dir() {
                nodes.insert(key, VirtualNode::Folder);
            } else if meta.is_file() {
                let bytes = std::fs::read(&abs).map_err(|e| NbError::Io {
                    path: abs,
                    source: e.into(),
                })?;
                // Non-empty directories may be absent from the path list
                // (only their children are listed); chain the parents so a
                // create/move onto an existing directory path collides
                // instead of materializing a file over the directory.
                if let Some(parent) = parent_path(&key) {
                    let mut acc = String::new();
                    for part in parent.split('/').filter(|p| !p.is_empty()) {
                        if !acc.is_empty() {
                            acc.push('/');
                        }
                        acc.push_str(part);
                        nodes.entry(acc.clone()).or_insert(VirtualNode::Folder);
                    }
                }
                nodes.insert(key, VirtualNode::File(bytes));
            }
        }
        Ok(Self { nodes })
    }

    pub(super) fn exists(&self, path: &str) -> bool {
        self.nodes.contains_key(path)
    }

    pub(super) fn get_file(&self, path: &str) -> Option<&[u8]> {
        match self.nodes.get(path) {
            Some(VirtualNode::File(b)) => Some(b),
            _ => None,
        }
    }

    pub(super) fn insert_file(&mut self, path: String, bytes: Vec<u8>) {
        // Ensure parent folders exist virtually.
        if let Some(parent) = parent_path(&path) {
            self.ensure_folder_chain(&parent);
        }
        self.nodes.insert(path, VirtualNode::File(bytes));
    }

    pub(super) fn insert_folder(&mut self, path: String) {
        self.ensure_folder_chain(&path);
        self.nodes.insert(path, VirtualNode::Folder);
    }

    fn ensure_folder_chain(&mut self, path: &str) {
        let mut acc = String::new();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(part);
            self.nodes.entry(acc.clone()).or_insert(VirtualNode::Folder);
        }
    }

    pub(super) fn remove(&mut self, path: &str) {
        self.nodes.remove(path);
    }

    pub(super) fn rename(&mut self, from: &str, to: &str) -> Result<(), NbError> {
        let node = self.nodes.remove(from).ok_or_else(|| NbError::NotFound {
            selector: from.to_string(),
        })?;
        if let Some(parent) = parent_path(to) {
            self.ensure_folder_chain(&parent);
        }
        self.nodes.insert(to.to_string(), node);
        Ok(())
    }
}

/// Reject `rel` when any existing path component under `root` is a symlink.
///
/// Uses no-follow metadata so ignored directory symlinks (absent from the
/// virtual snapshot) cannot redirect `create_dir_all` / `write` outside the
/// notebook root.
pub(super) fn refuse_symlink_ancestors(root: &Path, rel: &str) -> Result<(), NbError> {
    let mut acc = PathBuf::new();
    for part in rel.split('/').filter(|p| !p.is_empty()) {
        acc.push(part);
        let abs = root.join(&acc);
        match std::fs::symlink_metadata(&abs) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let key = acc.to_string_lossy().replace('\\', "/");
                return Err(NbError::UnsupportedStructure {
                    reason: format!(
                        "refusing path through symlink `{key}`; transactions do not follow symlinks"
                    ),
                });
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => {
                return Err(NbError::Io {
                    path: abs,
                    source: e.into(),
                });
            }
        }
    }
    Ok(())
}

pub(super) fn outcome_selector(notebook: &str, target: &NoteTarget, resolved_path: &str) -> String {
    match target {
        NoteTarget::Selector { value } => {
            if value.contains(':') {
                value.clone()
            } else {
                format!("{notebook}:{value}")
            }
        }
        NoteTarget::Path { .. } => format!("{notebook}:{resolved_path}"),
    }
}

pub(super) fn refuse_create_collision(
    notebook_root: &Path,
    tree: &VirtualTree,
    ignored_existing: &HashSet<String>,
    path: &str,
) -> Result<(), NbError> {
    if ignored_existing.contains(path) {
        return Err(NbError::PathIgnored {
            path: path.to_string(),
            guidance: "path exists as a Git-ignored file; un-ignore or remove it outside the transaction before creating here".into(),
            plan_index: None,
        });
    }
    // Ignored directory/file symlinks are absent from the virtual tree; reject
    // creates whose prefix is an ignored symlink (or any on-disk symlink).
    refuse_symlink_ancestors(notebook_root, path)?;
    for ig in ignored_existing {
        if path == ig || path.starts_with(&format!("{ig}/")) {
            let abs = notebook_root.join(ig);
            if let Ok(meta) = std::fs::symlink_metadata(&abs)
                && meta.file_type().is_symlink()
            {
                return Err(NbError::UnsupportedStructure {
                    reason: format!(
                        "refusing path through symlink `{ig}`; transactions do not follow symlinks"
                    ),
                });
            }
        }
    }
    if tree.exists(path) {
        return Err(NbError::PathCollision {
            path: path.to_string(),
            plan_index: None,
        });
    }
    Ok(())
}

pub(super) fn refuse_existing_ignored(
    ignored_existing: &HashSet<String>,
    path: &str,
) -> Result<(), NbError> {
    if ignored_existing.contains(path) {
        return Err(NbError::PathIgnored {
            path: path.to_string(),
            guidance:
                "cannot edit, delete, or move an existing Git-ignored path inside a transaction"
                    .into(),
            plan_index: None,
        });
    }
    Ok(())
}

pub(super) fn resolve_target_path(
    tree: &VirtualTree,
    ignored_existing: &HashSet<String>,
    target: &NoteTarget,
) -> Result<String, NbError> {
    match target {
        NoteTarget::Path { value } => {
            let path = normalize_rel(value);
            refuse_existing_ignored(ignored_existing, &path)?;
            if tree.exists(&path) {
                return Ok(path);
            }
            // Allow selector-like numeric ids only via Selector variant.
            Err(NbError::NotFound {
                selector: value.clone(),
            })
        }
        NoteTarget::Selector { value } => {
            // Prefer exact path match when value looks like a path.
            let stripped = value
                .rsplit_once(':')
                .map(|(_, rest)| rest)
                .unwrap_or(value.as_str());
            let candidate = normalize_rel(stripped);
            refuse_existing_ignored(ignored_existing, &candidate)?;
            if tree.exists(&candidate) {
                return Ok(candidate);
            }
            // Numeric `<folder>/<id>` / `<id>` via the virtual `.index`
            // (blank lines are dead ids -> NotFound, never reused).
            if let Some(resolved) = resolve_numeric_in_tree(tree, &candidate) {
                return resolved;
            }
            // Match by basename or unique suffix.
            let matches: Vec<_> = tree
                .nodes
                .iter()
                .filter_map(|(p, n)| match n {
                    VirtualNode::File(_)
                        if p == &candidate
                            || p.ends_with(&format!("/{candidate}"))
                            || Path::new(p).file_stem().and_then(|s| s.to_str())
                                == Some(stripped)
                            || p.ends_with(stripped) =>
                    {
                        Some(p.clone())
                    }
                    _ => None,
                })
                .collect();
            match matches.as_slice() {
                [one] => Ok(one.clone()),
                [] => {
                    // Existing ignored paths are not in the editable tree.
                    if ignored_existing.contains(&candidate) {
                        return Err(NbError::PathIgnored {
                            path: candidate,
                            guidance: "cannot edit, delete, or move an existing Git-ignored path inside a transaction".into(),
                            plan_index: None,
                        });
                    }
                    for ig in ignored_existing {
                        if ig.ends_with(&format!("/{candidate}")) || ig.ends_with(stripped) {
                            return Err(NbError::PathIgnored {
                                path: ig.clone(),
                                guidance: "cannot edit, delete, or move an existing Git-ignored path inside a transaction".into(),
                                plan_index: None,
                            });
                        }
                    }
                    Err(NbError::NotFound {
                        selector: value.clone(),
                    })
                }
                _ => Err(NbError::ValidationError {
                    reason: format!("selector `{value}` is ambiguous across multiple paths"),
                    location: None,
                }),
            }
        }
    }
}

/// Resolve `<folder>/<id>` or `<id>` against the virtual `.index`.
///
/// Returns `Some(Ok(path))` on a live entry, `Some(Err(NotFound))` when the
/// line exists but is blank (deleted id) or out of range, and `None` when
/// the value is not numeric-id shaped (caller falls through to name
/// matching).
fn resolve_numeric_in_tree(tree: &VirtualTree, candidate: &str) -> Option<Result<String, NbError>> {
    let (folder, id_str) = match candidate.rfind('/') {
        Some(i) => (candidate[..i].to_string(), candidate[i + 1..].to_string()),
        None => (String::new(), candidate.to_string()),
    };
    let id: usize = match id_str.parse() {
        Ok(n) => n,
        Err(_) => return None,
    };
    if id == 0 {
        return Some(Err(NbError::NotFound {
            selector: candidate.to_string(),
        }));
    }
    let rel = index_rel_for_folder(&folder);
    let bytes = match tree.nodes.get(&rel) {
        Some(VirtualNode::File(b)) => b.clone(),
        _ => return None,
    };
    let lines = parse_index_lines(&bytes);
    match lines.get(id - 1) {
        Some(name) if !name.trim().is_empty() => {
            let path = if folder.is_empty() {
                name.clone()
            } else {
                format!("{folder}/{name}")
            };
            Some(Ok(path))
        }
        _ => Some(Err(NbError::NotFound {
            selector: candidate.to_string(),
        })),
    }
}

pub(super) fn resolve_move_destination(from: &str, destination: &str) -> String {
    // A trailing slash means "into folder" (basename kept). Check the raw
    // form: `normalize_rel` trims slashes, so a post-normalize check would
    // be dead code.
    let into_folder = destination.trim_end().ends_with('/');
    let dest = normalize_rel(destination);
    if into_folder {
        let base = Path::new(from)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(from);
        return normalize_rel(&format!("{dest}/{base}"));
    }
    // Destination is a folder path without slash if no extension and exists as folder —
    // treat as basename rename when it has a file extension or contains a dot filename.
    dest
}

pub(super) fn normalize_rel(path: &str) -> String {
    path.trim()
        .trim_start_matches("./")
        .trim_matches('/')
        .replace('\\', "/")
}

pub(super) fn parent_path(path: &str) -> Option<String> {
    let p = Path::new(path).parent()?;
    let s = p.to_string_lossy().replace('\\', "/");
    if s.is_empty() || s == "." {
        None
    } else {
        Some(s)
    }
}
