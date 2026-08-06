//! Collect-then-commit [`Transaction`] plan and apply engine.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::NbClient;
use crate::error::NbError;
use crate::fingerprint::{self, Fingerprint};
use crate::gate::{self, DEFAULT_GATE_TIMEOUT};
use crate::git;
use crate::lines::{
    apply_line_edits, apply_substring, require_contiguous_body, splice_body, splice_title,
};
use crate::parser::{DocumentKind, NoteDocument, ParseContext, parse};
use crate::types::{ByteString, CommitOutcome, LineEdit, NoteTarget, Occurrence, OpOutcome};

/// In-memory plan bound to one notebook. Drop discards; no begin/rollback.
pub struct Transaction {
    client: NbClient,
    notebook: String,
    plan: Vec<PlanOp>,
    gate_timeout: Duration,
}

#[derive(Debug, Clone)]
enum PlanOp {
    AddNote {
        path: String,
        title: Option<String>,
        content: String,
        tags: Vec<String>,
    },
    AddTodo {
        path: String,
        title: String,
        description: Option<String>,
        tasks: Vec<String>,
        tags: Vec<String>,
    },
    AddBookmark {
        path: String,
        url: String,
        title: Option<String>,
        tags: Vec<String>,
        comment: Option<String>,
    },
    AddFolder {
        path: String,
    },
    DeleteNote {
        target: NoteTarget,
    },
    MoveNote {
        target: NoteTarget,
        destination: String,
    },
    MarkTaskDone {
        target: NoteTarget,
        task_number: Option<u32>,
    },
    UnmarkTaskDone {
        target: NoteTarget,
        task_number: Option<u32>,
    },
    ReplaceNoteBody {
        target: NoteTarget,
        new_body: Vec<u8>,
        fingerprint: Fingerprint,
    },
    EditNoteSubstring {
        target: NoteTarget,
        pattern: Vec<u8>,
        replacement: Vec<u8>,
        occurrence: Occurrence,
        expected_count: u32,
        fingerprint: Option<Fingerprint>,
    },
    EditNoteLines {
        target: NoteTarget,
        edits: Vec<LineEdit>,
    },
    RetitleNote {
        target: NoteTarget,
        title: Vec<u8>,
    },
    EditNoteTags {
        target: NoteTarget,
        add: Vec<String>,
        remove: Vec<String>,
    },
}

#[derive(Debug, Clone)]
enum VirtualNode {
    File(Vec<u8>),
    Folder,
}

struct VirtualTree {
    /// Notebook-relative paths using `/` separators.
    nodes: HashMap<String, VirtualNode>,
}

impl VirtualTree {
    fn from_disk(root: &Path) -> Result<Self, NbError> {
        let mut nodes = HashMap::new();
        let paths = git::list_notebook_paths(root)?;
        for rel in paths {
            let abs = root.join(&rel);
            if abs.is_dir() {
                nodes.insert(normalize_rel(&rel), VirtualNode::Folder);
            } else if abs.is_file() {
                let bytes = std::fs::read(&abs).map_err(|e| NbError::Io {
                    path: abs,
                    source: e.into(),
                })?;
                nodes.insert(normalize_rel(&rel), VirtualNode::File(bytes));
            }
        }
        Ok(Self { nodes })
    }

    fn exists(&self, path: &str) -> bool {
        self.nodes.contains_key(path)
    }

    fn get_file(&self, path: &str) -> Option<&[u8]> {
        match self.nodes.get(path) {
            Some(VirtualNode::File(b)) => Some(b),
            _ => None,
        }
    }

    fn insert_file(&mut self, path: String, bytes: Vec<u8>) {
        // Ensure parent folders exist virtually.
        if let Some(parent) = parent_path(&path) {
            self.ensure_folder_chain(&parent);
        }
        self.nodes.insert(path, VirtualNode::File(bytes));
    }

    fn insert_folder(&mut self, path: String) {
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

    fn remove(&mut self, path: &str) {
        self.nodes.remove(path);
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), NbError> {
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

impl Transaction {
    pub(crate) fn new(client: NbClient, notebook: String) -> Self {
        Self {
            client,
            notebook,
            plan: Vec::new(),
            gate_timeout: DEFAULT_GATE_TIMEOUT,
        }
    }

    pub fn add_note(
        &mut self,
        path: &str,
        title: Option<&str>,
        content: &str,
        tags: &[String],
    ) -> Result<(), NbError> {
        let path = validate_create_path(path, true)?;
        if let Some(t) = title
            && let Some(heading) = crate::validate::detect_duplicate_title_heading(t, content)
        {
            return Err(NbError::DuplicateTitleHeading {
                title: t.to_string(),
                heading,
            });
        }
        self.plan.push(PlanOp::AddNote {
            path,
            title: title.map(str::to_string),
            content: content.to_string(),
            tags: tags.to_vec(),
        });
        Ok(())
    }

    pub fn add_todo(
        &mut self,
        path: &str,
        title: &str,
        description: Option<&str>,
        tasks: &[String],
        tags: &[String],
    ) -> Result<(), NbError> {
        let path = validate_create_path(path, true)?;
        if title.trim().is_empty() {
            return Err(NbError::ValidationError {
                reason: "todo title must not be empty".into(),
                location: None,
            });
        }
        self.plan.push(PlanOp::AddTodo {
            path,
            title: title.to_string(),
            description: description.map(str::to_string),
            tasks: tasks.to_vec(),
            tags: tags.to_vec(),
        });
        Ok(())
    }

    pub fn add_bookmark(
        &mut self,
        path: &str,
        url: &str,
        title: Option<&str>,
        tags: &[String],
        comment: Option<&str>,
    ) -> Result<(), NbError> {
        let path = validate_create_path(path, true)?;
        if url.trim().is_empty() {
            return Err(NbError::ValidationError {
                reason: "bookmark url must not be empty".into(),
                location: None,
            });
        }
        self.plan.push(PlanOp::AddBookmark {
            path,
            url: url.to_string(),
            title: title.map(str::to_string),
            tags: tags.to_vec(),
            comment: comment.map(str::to_string),
        });
        Ok(())
    }

    pub fn add_folder(&mut self, path: &str) -> Result<(), NbError> {
        let path = validate_create_path(path, false)?;
        self.plan.push(PlanOp::AddFolder { path });
        Ok(())
    }

    pub fn delete_note(&mut self, target: NoteTarget) -> Result<(), NbError> {
        validate_target(&target)?;
        self.plan.push(PlanOp::DeleteNote { target });
        Ok(())
    }

    pub fn move_note(&mut self, target: NoteTarget, destination: &str) -> Result<(), NbError> {
        validate_target(&target)?;
        crate::validate::validate_destination(destination)?;
        let destination = normalize_rel(destination);
        if destination.is_empty() || destination.contains("..") {
            return Err(NbError::ValidationError {
                reason: "invalid move destination".into(),
                location: None,
            });
        }
        self.plan.push(PlanOp::MoveNote {
            target,
            destination,
        });
        Ok(())
    }

    pub fn mark_task_done(
        &mut self,
        target: NoteTarget,
        task_number: Option<u32>,
    ) -> Result<(), NbError> {
        validate_target(&target)?;
        self.plan.push(PlanOp::MarkTaskDone {
            target,
            task_number,
        });
        Ok(())
    }

    pub fn unmark_task_done(
        &mut self,
        target: NoteTarget,
        task_number: Option<u32>,
    ) -> Result<(), NbError> {
        validate_target(&target)?;
        self.plan.push(PlanOp::UnmarkTaskDone {
            target,
            task_number,
        });
        Ok(())
    }

    pub fn replace_note_body(
        &mut self,
        target: NoteTarget,
        new_body: impl AsRef<[u8]>,
        fingerprint: Fingerprint,
    ) -> Result<(), NbError> {
        validate_target(&target)?;
        self.plan.push(PlanOp::ReplaceNoteBody {
            target,
            new_body: new_body.as_ref().to_vec(),
            fingerprint,
        });
        Ok(())
    }

    pub fn edit_note_substring(
        &mut self,
        target: NoteTarget,
        pattern: impl AsRef<[u8]>,
        replacement: impl AsRef<[u8]>,
        occurrence: Occurrence,
        expected_count: u32,
        fingerprint: Option<Fingerprint>,
    ) -> Result<(), NbError> {
        validate_target(&target)?;
        if pattern.as_ref().is_empty() {
            return Err(NbError::EmptySubstringPattern);
        }
        self.plan.push(PlanOp::EditNoteSubstring {
            target,
            pattern: pattern.as_ref().to_vec(),
            replacement: replacement.as_ref().to_vec(),
            occurrence,
            expected_count,
            fingerprint,
        });
        Ok(())
    }

    pub fn edit_note_lines(
        &mut self,
        target: NoteTarget,
        edits: Vec<LineEdit>,
    ) -> Result<(), NbError> {
        validate_target(&target)?;
        if edits.is_empty() {
            return Err(NbError::ValidationError {
                reason: "edit_note_lines requires a non-empty edits batch".into(),
                location: None,
            });
        }
        self.plan.push(PlanOp::EditNoteLines { target, edits });
        Ok(())
    }

    pub fn retitle_note(
        &mut self,
        target: NoteTarget,
        title: impl AsRef<[u8]>,
    ) -> Result<(), NbError> {
        validate_target(&target)?;
        self.plan.push(PlanOp::RetitleNote {
            target,
            title: title.as_ref().to_vec(),
        });
        Ok(())
    }

    pub fn edit_note_tags(
        &mut self,
        target: NoteTarget,
        add: &[String],
        remove: &[String],
    ) -> Result<(), NbError> {
        validate_target(&target)?;
        let add_set: HashSet<&str> = add.iter().map(|s| s.trim_start_matches('#')).collect();
        for r in remove {
            let key = r.trim_start_matches('#');
            if add_set.contains(key) {
                return Err(NbError::ValidationError {
                    reason: format!("contradictory tag add/remove for `{key}`"),
                    location: None,
                });
            }
        }
        self.plan.push(PlanOp::EditNoteTags {
            target,
            add: add.to_vec(),
            remove: remove.to_vec(),
        });
        Ok(())
    }

    /// Validate-all, apply-all, at most one Git checkpoint.
    pub async fn commit(self) -> Result<CommitOutcome, NbError> {
        // Resolve root without invoking `nb` when possible (avoids nb
        // auto-checkpoint clearing dirty state before the baseline check).
        let notebook_root = self
            .client
            .show_notebook_path_unguarded(Some(&self.notebook))
            .await?;
        let gate_key = gate::git_common_dir_realpath(&notebook_root)?;
        let _hold = gate::acquire_notebook(gate_key, self.gate_timeout, false).await?;

        if git::notebook_is_dirty(&notebook_root)? {
            return Err(NbError::DirtyBaseline {
                guidance: "commit or clean the notebook worktree/index before Transaction::commit"
                    .into(),
            });
        }

        let pre_revision = git::notebook_head(&notebook_root)?;
        if self.plan.is_empty() {
            return Ok(CommitOutcome {
                commit_created: false,
                revision_id: None,
                pre_revision,
                ops: Vec::new(),
            });
        }

        let mut tree = VirtualTree::from_disk(&notebook_root)?;
        let mut op_meta: Vec<OpMeta> = Vec::with_capacity(self.plan.len());

        for (index, op) in self.plan.iter().enumerate() {
            match validate_and_apply_virtual(&self.notebook, &mut tree, op, index) {
                Ok(meta) => op_meta.push(meta),
                Err(err) => {
                    return Err(annotate_plan_index(err, index as u32));
                }
            }
        }

        // Materialize to disk under gate, then single checkpoint.
        let apply_result = (|| -> Result<bool, NbError> {
            materialize_tree(&notebook_root, &tree)?;
            git::notebook_commit_all(
                &notebook_root,
                &format!("nb-api transaction ({} ops)", self.plan.len()),
            )
        })();

        match apply_result {
            Ok(created) => {
                let revision_id = if created {
                    Some(git::notebook_head(&notebook_root)?)
                } else {
                    None
                };
                let ops = op_meta
                    .into_iter()
                    .enumerate()
                    .map(|(i, meta)| OpOutcome {
                        index: i as u32,
                        path: meta.path,
                        selector: meta.selector,
                        noop: meta.noop,
                        fingerprint: meta.fingerprint,
                    })
                    .collect();
                Ok(CommitOutcome {
                    commit_created: created,
                    revision_id,
                    pre_revision,
                    ops,
                })
            }
            Err(err) => {
                let post = git::notebook_head(&notebook_root).ok();
                if post.as_deref().is_some_and(|h| h != pre_revision.as_str()) {
                    return Err(NbError::IndeterminateCommit {
                        pre_revision,
                        post_revision_observed: post,
                        guidance: "checkpoint may have completed; re-read HEAD/status and do not retry the same plan blindly".into(),
                    });
                }
                match try_restore(&notebook_root, &pre_revision) {
                    Ok(()) => Err(err),
                    Err(recovery) => Err(recovery),
                }
            }
        }
    }
}

fn try_restore(notebook_root: &Path, pre_revision: &str) -> Result<(), NbError> {
    if let Err(e) = git::notebook_reset_clean(notebook_root, pre_revision) {
        let post = git::notebook_head(notebook_root).ok();
        let status = git::notebook_status_porcelain(notebook_root).ok();
        return Err(NbError::RecoveryRequired {
            pre_revision: pre_revision.to_string(),
            post_revision_observed: post,
            status_observed: status,
            preserved_paths: None,
            guidance: format!(
                "cleanup after failed commit could not be verified; inspect HEAD/status before retry. underlying: {e}"
            ),
        });
    }
    let head = git::notebook_head(notebook_root)?;
    let dirty = git::notebook_is_dirty(notebook_root)?;
    if head != pre_revision || dirty {
        return Err(NbError::RecoveryRequired {
            pre_revision: pre_revision.to_string(),
            post_revision_observed: Some(head),
            status_observed: git::notebook_status_porcelain(notebook_root).ok(),
            preserved_paths: None,
            guidance: "cleanup ran but HEAD/status verification failed; do not retry blindly"
                .into(),
        });
    }
    Ok(())
}

fn materialize_tree(root: &Path, tree: &VirtualTree) -> Result<(), NbError> {
    // Remove tracked files not present as files in the virtual tree (deleted/moved).
    let disk_paths = git::list_notebook_paths(root)?;
    for rel in &disk_paths {
        let key = normalize_rel(rel);
        let abs = root.join(rel);
        if abs.is_file() {
            match tree.nodes.get(&key) {
                Some(VirtualNode::File(_)) => {}
                _ => {
                    std::fs::remove_file(&abs).map_err(|e| NbError::Io {
                        path: abs,
                        source: e.into(),
                    })?;
                }
            }
        }
    }
    for (rel, node) in &tree.nodes {
        let abs = root.join(rel);
        match node {
            VirtualNode::Folder => {
                std::fs::create_dir_all(&abs).map_err(|e| NbError::Io {
                    path: abs.clone(),
                    source: e.into(),
                })?;
            }
            VirtualNode::File(bytes) => {
                if let Some(parent) = abs.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| NbError::Io {
                        path: parent.to_path_buf(),
                        source: e.into(),
                    })?;
                }
                std::fs::write(&abs, bytes).map_err(|e| NbError::Io {
                    path: abs,
                    source: e.into(),
                })?;
            }
        }
    }
    Ok(())
}

struct OpMeta {
    path: Option<String>,
    selector: Option<String>,
    noop: bool,
    fingerprint: Option<Fingerprint>,
}

fn validate_and_apply_virtual(
    notebook: &str,
    tree: &mut VirtualTree,
    op: &PlanOp,
    _index: usize,
) -> Result<OpMeta, NbError> {
    match op {
        PlanOp::AddNote {
            path,
            title,
            content,
            tags,
        } => {
            if tree.exists(path) {
                return Err(NbError::PathCollision {
                    path: path.clone(),
                    plan_index: None,
                });
            }
            let bytes = build_note_bytes(title.as_deref(), content, tags);
            let fp = fingerprint_bytes(&bytes, path)?;
            tree.insert_file(path.clone(), bytes);
            Ok(OpMeta {
                path: Some(path.clone()),
                selector: Some(format!("{notebook}:{path}")),
                noop: false,
                fingerprint: Some(fp),
            })
        }
        PlanOp::AddTodo {
            path,
            title,
            description,
            tasks,
            tags,
        } => {
            if tree.exists(path) {
                return Err(NbError::PathCollision {
                    path: path.clone(),
                    plan_index: None,
                });
            }
            let bytes = build_todo_bytes(title, description.as_deref(), tasks, tags);
            let fp = fingerprint_bytes(&bytes, path)?;
            tree.insert_file(path.clone(), bytes);
            Ok(OpMeta {
                path: Some(path.clone()),
                selector: Some(format!("{notebook}:{path}")),
                noop: false,
                fingerprint: Some(fp),
            })
        }
        PlanOp::AddBookmark {
            path,
            url,
            title,
            tags,
            comment,
        } => {
            if tree.exists(path) {
                return Err(NbError::PathCollision {
                    path: path.clone(),
                    plan_index: None,
                });
            }
            let bytes = build_bookmark_bytes(url, title.as_deref(), tags, comment.as_deref());
            let fp = fingerprint_bytes(&bytes, path)?;
            tree.insert_file(path.clone(), bytes);
            Ok(OpMeta {
                path: Some(path.clone()),
                selector: Some(format!("{notebook}:{path}")),
                noop: false,
                fingerprint: Some(fp),
            })
        }
        PlanOp::AddFolder { path } => {
            if tree.exists(path) {
                return Err(NbError::PathCollision {
                    path: path.clone(),
                    plan_index: None,
                });
            }
            tree.insert_folder(path.clone());
            Ok(OpMeta {
                path: Some(path.clone()),
                selector: None,
                noop: false,
                fingerprint: None,
            })
        }
        PlanOp::DeleteNote { target } => {
            let path = resolve_target_path(tree, target)?;
            if !tree.exists(&path) {
                return Err(NbError::NotFound {
                    selector: target.value().to_string(),
                });
            }
            tree.remove(&path);
            Ok(OpMeta {
                path: Some(path),
                selector: Some(format!("{notebook}:{}", target.value())),
                noop: false,
                fingerprint: None,
            })
        }
        PlanOp::MoveNote {
            target,
            destination,
        } => {
            let from = resolve_target_path(tree, target)?;
            let to = resolve_move_destination(&from, destination);
            if tree.exists(&to) {
                return Err(NbError::PathCollision {
                    path: to,
                    plan_index: None,
                });
            }
            tree.rename(&from, &to)?;
            Ok(OpMeta {
                path: Some(to.clone()),
                selector: Some(format!("{notebook}:{to}")),
                noop: false,
                fingerprint: None,
            })
        }
        PlanOp::MarkTaskDone {
            target,
            task_number,
        } => {
            let path = resolve_target_path(tree, target)?;
            let bytes = tree
                .get_file(&path)
                .ok_or_else(|| NbError::NotFound {
                    selector: target.value().to_string(),
                })?
                .to_vec();
            let new_bytes = set_todo_state(&bytes, &path, true, *task_number)?;
            let noop = new_bytes == bytes;
            let fp = fingerprint_bytes(&new_bytes, &path)?;
            tree.insert_file(path.clone(), new_bytes);
            Ok(OpMeta {
                path: Some(path),
                selector: Some(format!("{notebook}:{}", target.value())),
                noop,
                fingerprint: Some(fp),
            })
        }
        PlanOp::UnmarkTaskDone {
            target,
            task_number,
        } => {
            let path = resolve_target_path(tree, target)?;
            let bytes = tree
                .get_file(&path)
                .ok_or_else(|| NbError::NotFound {
                    selector: target.value().to_string(),
                })?
                .to_vec();
            let new_bytes = set_todo_state(&bytes, &path, false, *task_number)?;
            let noop = new_bytes == bytes;
            let fp = fingerprint_bytes(&new_bytes, &path)?;
            tree.insert_file(path.clone(), new_bytes);
            Ok(OpMeta {
                path: Some(path),
                selector: Some(format!("{notebook}:{}", target.value())),
                noop,
                fingerprint: Some(fp),
            })
        }
        PlanOp::ReplaceNoteBody {
            target,
            new_body,
            fingerprint: expected,
        } => {
            let path = resolve_target_path(tree, target)?;
            let bytes = tree
                .get_file(&path)
                .ok_or_else(|| NbError::NotFound {
                    selector: target.value().to_string(),
                })?
                .to_vec();
            let doc = parse_doc(&bytes, &path)?;
            let current = fingerprint::fingerprint(&doc);
            if &current != expected {
                return Err(NbError::FingerprintMismatch {
                    target: target.clone(),
                    guidance: "body fingerprint does not match; re-read and retry".into(),
                });
            }
            require_contiguous_body(&doc)?;
            let new_bytes = splice_body(&doc, new_body)?;
            let fp = fingerprint_bytes(&new_bytes, &path)?;
            let noop = new_bytes == bytes;
            tree.insert_file(path.clone(), new_bytes);
            Ok(OpMeta {
                path: Some(path),
                selector: Some(format!("{notebook}:{}", target.value())),
                noop,
                fingerprint: Some(fp),
            })
        }
        PlanOp::EditNoteSubstring {
            target,
            pattern,
            replacement,
            occurrence,
            expected_count,
            fingerprint: expected,
        } => {
            let path = resolve_target_path(tree, target)?;
            let bytes = tree
                .get_file(&path)
                .ok_or_else(|| NbError::NotFound {
                    selector: target.value().to_string(),
                })?
                .to_vec();
            let doc = parse_doc(&bytes, &path)?;
            if let Some(exp) = expected {
                let current = fingerprint::fingerprint(&doc);
                if &current != exp {
                    return Err(NbError::FingerprintMismatch {
                        target: target.clone(),
                        guidance: "body fingerprint does not match; re-read and retry".into(),
                    });
                }
            }
            let body = require_contiguous_body(&doc)?;
            let new_body =
                apply_substring(&body, pattern, replacement, occurrence, *expected_count)?;
            let new_bytes = splice_body(&doc, &new_body)?;
            let fp = fingerprint_bytes(&new_bytes, &path)?;
            let noop = new_bytes == bytes;
            tree.insert_file(path.clone(), new_bytes);
            Ok(OpMeta {
                path: Some(path),
                selector: Some(format!("{notebook}:{}", target.value())),
                noop,
                fingerprint: Some(fp),
            })
        }
        PlanOp::EditNoteLines { target, edits } => {
            let path = resolve_target_path(tree, target)?;
            let bytes = tree
                .get_file(&path)
                .ok_or_else(|| NbError::NotFound {
                    selector: target.value().to_string(),
                })?
                .to_vec();
            let doc = parse_doc(&bytes, &path)?;
            let body = require_contiguous_body(&doc)?;
            let new_body = apply_line_edits(&body, edits)?;
            let new_bytes = splice_body(&doc, &new_body)?;
            let fp = fingerprint_bytes(&new_bytes, &path)?;
            let noop = new_bytes == bytes;
            tree.insert_file(path.clone(), new_bytes);
            Ok(OpMeta {
                path: Some(path),
                selector: Some(format!("{notebook}:{}", target.value())),
                noop,
                fingerprint: Some(fp),
            })
        }
        PlanOp::RetitleNote { target, title } => {
            let path = resolve_target_path(tree, target)?;
            let bytes = tree
                .get_file(&path)
                .ok_or_else(|| NbError::NotFound {
                    selector: target.value().to_string(),
                })?
                .to_vec();
            let doc = parse_doc(&bytes, &path)?;
            let mut title_line = title.clone();
            if !title_line.starts_with(b"#") {
                let mut prefixed = b"# ".to_vec();
                prefixed.extend_from_slice(&title_line);
                title_line = prefixed;
            }
            let new_bytes = splice_title(&doc, &title_line)?;
            let fp = fingerprint_bytes(&new_bytes, &path)?;
            let noop = new_bytes == bytes;
            tree.insert_file(path.clone(), new_bytes);
            Ok(OpMeta {
                path: Some(path),
                selector: Some(format!("{notebook}:{}", target.value())),
                noop,
                fingerprint: Some(fp),
            })
        }
        PlanOp::EditNoteTags {
            target,
            add,
            remove,
        } => {
            let path = resolve_target_path(tree, target)?;
            let bytes = tree
                .get_file(&path)
                .ok_or_else(|| NbError::NotFound {
                    selector: target.value().to_string(),
                })?
                .to_vec();
            let doc = parse_doc(&bytes, &path)?;
            let new_bytes = apply_tag_edit(&doc, &path, add, remove)?;
            let fp = fingerprint_bytes(&new_bytes, &path)?;
            let noop = new_bytes == bytes;
            tree.insert_file(path.clone(), new_bytes);
            Ok(OpMeta {
                path: Some(path),
                selector: Some(format!("{notebook}:{}", target.value())),
                noop,
                fingerprint: Some(fp),
            })
        }
    }
}

fn resolve_target_path(tree: &VirtualTree, target: &NoteTarget) -> Result<String, NbError> {
    match target {
        NoteTarget::Path { value } => {
            let path = normalize_rel(value);
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
            if tree.exists(&candidate) {
                return Ok(candidate);
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
                [] => Err(NbError::NotFound {
                    selector: value.clone(),
                }),
                _ => Err(NbError::ValidationError {
                    reason: format!("selector `{value}` is ambiguous across multiple paths"),
                    location: None,
                }),
            }
        }
    }
}

fn resolve_move_destination(from: &str, destination: &str) -> String {
    let dest = normalize_rel(destination);
    if dest.ends_with('/') {
        let base = Path::new(from)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(from);
        return normalize_rel(&format!("{dest}{base}"));
    }
    // Destination is a folder path without slash if no extension and exists as folder —
    // treat as basename rename when it has a file extension or contains a dot filename.
    dest
}

fn parse_doc(bytes: &[u8], path: &str) -> Result<NoteDocument, NbError> {
    parse(bytes, ParseContext::FromPath(PathBuf::from(path)))
}

fn fingerprint_bytes(bytes: &[u8], path: &str) -> Result<Fingerprint, NbError> {
    let doc = parse_doc(bytes, path)?;
    Ok(fingerprint::fingerprint(&doc))
}

fn build_note_bytes(title: Option<&str>, content: &str, tags: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(t) = title {
        out.extend_from_slice(format!("# {t}\n\n").as_bytes());
    }
    if !tags.is_empty() {
        let mut first = true;
        for tag in tags {
            if !first {
                out.push(b' ');
            }
            first = false;
            if tag.starts_with('#') {
                out.extend_from_slice(tag.as_bytes());
            } else {
                out.push(b'#');
                out.extend_from_slice(tag.as_bytes());
            }
        }
        out.extend_from_slice(b"\n\n");
    }
    out.extend_from_slice(content.as_bytes());
    out
}

fn build_todo_bytes(
    title: &str,
    description: Option<&str>,
    tasks: &[String],
    tags: &[String],
) -> Vec<u8> {
    let mut out = Vec::new();
    let title_line = if title.contains('[') {
        format!("# {title}\n\n")
    } else {
        format!("# [ ] {title}\n\n")
    };
    out.extend_from_slice(title_line.as_bytes());
    if !tasks.is_empty() {
        out.extend_from_slice(b"## Tasks\n\n");
        for task in tasks {
            out.extend_from_slice(format!("- [ ] {task}\n").as_bytes());
        }
        out.push(b'\n');
    }
    if let Some(desc) = description {
        out.extend_from_slice(b"## Description\n\n");
        out.extend_from_slice(desc.as_bytes());
        if !desc.ends_with('\n') {
            out.push(b'\n');
        }
        out.push(b'\n');
    }
    if !tags.is_empty() {
        out.extend_from_slice(b"## Tags\n\n");
        let mut first = true;
        for tag in tags {
            if !first {
                out.push(b' ');
            }
            first = false;
            if tag.starts_with('#') {
                out.extend_from_slice(tag.as_bytes());
            } else {
                out.push(b'#');
                out.extend_from_slice(tag.as_bytes());
            }
        }
        out.push(b'\n');
    }
    out
}

fn build_bookmark_bytes(
    url: &str,
    title: Option<&str>,
    tags: &[String],
    comment: Option<&str>,
) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(t) = title {
        out.extend_from_slice(format!("# {t}\n\n").as_bytes());
    }
    out.extend_from_slice(format!("<{url}>\n\n").as_bytes());
    if let Some(c) = comment {
        out.extend_from_slice(b"## Content\n\n");
        out.extend_from_slice(c.as_bytes());
        if !c.ends_with('\n') {
            out.push(b'\n');
        }
        out.push(b'\n');
    }
    if !tags.is_empty() {
        out.extend_from_slice(b"## Tags\n\n");
        let mut first = true;
        for tag in tags {
            if !first {
                out.push(b' ');
            }
            first = false;
            if tag.starts_with('#') {
                out.extend_from_slice(tag.as_bytes());
            } else {
                out.push(b'#');
                out.extend_from_slice(tag.as_bytes());
            }
        }
        out.push(b'\n');
    }
    out
}

fn set_todo_state(
    bytes: &[u8],
    path: &str,
    done: bool,
    task_number: Option<u32>,
) -> Result<Vec<u8>, NbError> {
    let doc = parse_doc(bytes, path)?;
    if doc.kind() != DocumentKind::Todo {
        return Err(NbError::UnsupportedStructure {
            reason: "mark_task_done/unmark_task_done require a Todo document".into(),
        });
    }
    let mut out = bytes.to_vec();
    if let Some(n) = task_number {
        // Flip Nth `- [ ]` / `- [x]` checklist item (1-based).
        let mut count = 0u32;
        let mut i = 0usize;
        while i + 5 < out.len() {
            if out[i] == b'-' && out[i + 1] == b' ' && out[i + 2] == b'[' {
                let close = out[i + 4];
                if close == b']' {
                    count += 1;
                    if count == n {
                        let mark = if done { b'x' } else { b' ' };
                        out[i + 3] = mark;
                        return Ok(out);
                    }
                }
            }
            i += 1;
        }
        return Err(NbError::ValidationError {
            reason: format!("task_number {n} not found"),
            location: None,
        });
    }
    // Title checkbox.
    if let Some(range) = doc.title_byte_range() {
        let title = &out[range.clone()];
        let replaced = if done {
            replace_checkbox(title, true)
        } else {
            replace_checkbox(title, false)
        };
        out.splice(range, replaced);
    }
    Ok(out)
}

fn replace_checkbox(title: &[u8], done: bool) -> Vec<u8> {
    let s = String::from_utf8_lossy(title);
    let mark = if done { "[x]" } else { "[ ]" };
    let new = if s.contains("[x]") || s.contains("[X]") {
        s.replacen("[x]", mark, 1).replacen("[X]", mark, 1)
    } else if s.contains("[ ]") {
        s.replacen("[ ]", mark, 1)
    } else {
        // Insert after `# `
        if let Some(rest) = s.strip_prefix("# ") {
            format!("# {mark} {rest}")
        } else {
            s.into_owned()
        }
    };
    new.into_bytes()
}

fn apply_tag_edit(
    doc: &NoteDocument,
    path: &str,
    add: &[String],
    remove: &[String],
) -> Result<Vec<u8>, NbError> {
    let mut tags: Vec<String> = doc
        .tags_str()
        .filter_map(|t| t.ok().map(|s| s.trim_start_matches('#').to_string()))
        .collect();
    for r in remove {
        let key = r.trim_start_matches('#');
        tags.retain(|t| t != key);
    }
    for a in add {
        let key = a.trim_start_matches('#').to_string();
        if !tags.iter().any(|t| t == &key) {
            tags.push(key);
        }
    }
    let source = doc.source();
    let result = match doc.kind() {
        DocumentKind::Note => {
            let tag_line = if tags.is_empty() {
                None
            } else {
                Some(
                    tags.iter()
                        .map(|t| format!("#{t}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                        + "\n",
                )
            };
            if let Some(range) = doc.tags_byte_range() {
                let mut out = source.to_vec();
                match tag_line {
                    Some(line) => {
                        out.splice(range, line.into_bytes());
                    }
                    None => {
                        out.splice(range, std::iter::empty::<u8>());
                    }
                }
                Ok(out)
            } else if let Some(line) = tag_line {
                // Insert after title or at start.
                let insert_at = doc.title_byte_range().map(|r| r.end).unwrap_or(0);
                let mut block = Vec::new();
                if insert_at > 0 {
                    block.push(b'\n');
                }
                block.extend_from_slice(line.as_bytes());
                block.push(b'\n');
                let mut out = source.to_vec();
                out.splice(insert_at..insert_at, block);
                Ok(out)
            } else {
                Ok(source.to_vec())
            }
        }
        DocumentKind::Todo | DocumentKind::Bookmark => {
            let section = if tags.is_empty() {
                None
            } else {
                Some(format!(
                    "## Tags\n\n{}\n",
                    tags.iter()
                        .map(|t| format!("#{t}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                ))
            };
            if let Some(range) = doc.tags_byte_range() {
                let mut out = source.to_vec();
                let replacement = section.map(|s| s.into_bytes()).unwrap_or_default();
                out.splice(range, replacement);
                Ok(out)
            } else if let Some(s) = section {
                let mut out = source.to_vec();
                if !out.ends_with(b"\n") {
                    out.push(b'\n');
                }
                out.push(b'\n');
                out.extend_from_slice(s.as_bytes());
                Ok(out)
            } else {
                Ok(source.to_vec())
            }
        }
    };
    let _ = path;
    result
}

fn annotate_plan_index(err: NbError, index: u32) -> NbError {
    match err {
        NbError::PathCollision { path, .. } => NbError::PathCollision {
            path,
            plan_index: Some(index),
        },
        NbError::FingerprintMismatch { .. }
        | NbError::AnchorMismatch { .. }
        | NbError::OccurrenceMismatch { .. }
        | NbError::OverlappingEdits { .. }
        | NbError::FragmentedBody { .. }
        | NbError::EmptySubstringPattern
        | NbError::NotFound { .. }
        | NbError::UnsupportedStructure { .. } => err,
        NbError::PlanValidation {
            kind,
            message,
            plan_index: _,
        } => NbError::PlanValidation {
            kind,
            message,
            plan_index: Some(index),
        },
        other => NbError::PlanValidation {
            kind: "op_failed".into(),
            message: other.to_string(),
            plan_index: Some(index),
        },
    }
}

fn validate_target(target: &NoteTarget) -> Result<(), NbError> {
    if target.value().trim().is_empty() {
        return Err(NbError::ValidationError {
            reason: "note target must not be empty".into(),
            location: None,
        });
    }
    Ok(())
}

fn validate_create_path(path: &str, file: bool) -> Result<String, NbError> {
    let path = normalize_rel(path);
    if path.is_empty() {
        return Err(NbError::ValidationError {
            reason: "path must not be empty".into(),
            location: None,
        });
    }
    if path.starts_with('/') || path.contains('\\') {
        return Err(NbError::ValidationError {
            reason: "path must be notebook-relative without absolute or backslash form".into(),
            location: None,
        });
    }
    if path
        .split('/')
        .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(NbError::ValidationError {
            reason: "path must not contain empty, `.`, or `..` segments".into(),
            location: None,
        });
    }
    if file {
        let name = Path::new(&path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if name.is_empty() {
            return Err(NbError::ValidationError {
                reason: "create path must include a filename".into(),
                location: None,
            });
        }
    }
    Ok(path)
}

fn normalize_rel(path: &str) -> String {
    path.trim()
        .trim_start_matches("./")
        .trim_matches('/')
        .replace('\\', "/")
}

fn parent_path(path: &str) -> Option<String> {
    let p = Path::new(path).parent()?;
    let s = p.to_string_lossy().replace('\\', "/");
    if s.is_empty() || s == "." {
        None
    } else {
        Some(s)
    }
}

/// Generate an nb-style timestamp basename for one-shot auto-name creates.
pub(crate) fn auto_filename(extension: &str) -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{secs}-{n}.{extension}")
}

pub(crate) fn join_folder_file(folder: Option<&str>, filename: &str) -> String {
    match folder {
        Some(f) if !f.trim().is_empty() => {
            format!("{}/{}", normalize_rel(f), filename.trim_start_matches('/'))
        }
        _ => filename.to_string(),
    }
}

// Silence unused import warning if ByteString only used in tests later.
#[allow(dead_code)]
fn _bytestring_touch(b: ByteString) -> ByteString {
    b
}
