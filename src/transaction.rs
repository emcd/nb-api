//! Collect-then-commit [`Transaction`] plan and apply engine.

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::NbClient;
use crate::error::NbError;
use crate::fingerprint::{self, Fingerprint};
use crate::gate;
use crate::git;
use crate::lines::{
    apply_line_edits, apply_substring, require_contiguous_body, splice_body, splice_title,
};
use crate::parser::{DocumentKind, NoteDocument, ParseContext, parse};
use crate::types::{CommitOutcome, LineEdit, NoteTarget, Occurrence, OpOutcome};

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
        new_body: String,
        fingerprint: Fingerprint,
    },
    EditNoteSubstring {
        target: NoteTarget,
        pattern: String,
        replacement: String,
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
        title: String,
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
    pub(crate) fn new(client: NbClient, notebook: String, gate_timeout: Duration) -> Self {
        Self {
            client,
            notebook,
            plan: Vec::new(),
            gate_timeout,
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
        // Preserve a trailing slash: `foo/` means "into folder foo" (basename
        // kept). `normalize_rel` trims slashes, so re-attach the marker for
        // `resolve_move_destination`.
        let into_folder = destination.trim_end().ends_with('/');
        let mut destination = normalize_rel(destination);
        if destination.is_empty() || destination.contains("..") {
            return Err(NbError::ValidationError {
                reason: "invalid move destination".into(),
                location: None,
            });
        }
        if into_folder {
            destination.push('/');
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
        new_body: &str,
        fingerprint: Fingerprint,
    ) -> Result<(), NbError> {
        validate_target(&target)?;
        self.plan.push(PlanOp::ReplaceNoteBody {
            target,
            new_body: new_body.to_string(),
            fingerprint,
        });
        Ok(())
    }

    pub fn edit_note_substring(
        &mut self,
        target: NoteTarget,
        pattern: &str,
        replacement: &str,
        occurrence: Occurrence,
        expected_count: u32,
        fingerprint: Option<Fingerprint>,
    ) -> Result<(), NbError> {
        validate_target(&target)?;
        if pattern.is_empty() {
            return Err(NbError::EmptySubstringPattern);
        }
        self.plan.push(PlanOp::EditNoteSubstring {
            target,
            pattern: pattern.to_string(),
            replacement: replacement.to_string(),
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

    pub fn retitle_note(&mut self, target: NoteTarget, title: &str) -> Result<(), NbError> {
        validate_target(&target)?;
        self.plan.push(PlanOp::RetitleNote {
            target,
            title: title.to_string(),
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

        // A crashed writer may leave its `.nb-api-index.lock` behind; an
        // untracked lock file reads as dirty, so reap a stale one first.
        reap_stale_index_lock(&notebook_root, self.gate_timeout);

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

        let ignored_existing: HashSet<String> = git::list_ignored_paths(&notebook_root)?
            .into_iter()
            .map(|p| normalize_rel(&p))
            .collect();
        let mut tree = VirtualTree::from_disk(&notebook_root)?;
        let mut op_meta: Vec<OpMeta> = Vec::with_capacity(self.plan.len());
        let mut effects = PlanEffects::default();
        // Disk keys at snapshot time: materialize refuses a planned create
        // that appears on disk afterwards (external same-path race).
        let snapshot_keys: HashSet<String> = tree.nodes.keys().cloned().collect();

        for (index, op) in self.plan.iter().enumerate() {
            match validate_and_apply_virtual(
                &self.notebook,
                &notebook_root,
                &mut tree,
                &ignored_existing,
                op,
                index,
                &mut effects,
            ) {
                Ok(meta) => op_meta.push(meta),
                Err(err) => {
                    return Err(annotate_plan_index(err, index as u32));
                }
            }
        }

        #[cfg(feature = "testing")]
        if std::env::var_os("NB_API_SIMULATE_EXTERNAL_WRITER").is_some() {
            // Deterministic stand-in for an external `nb add` racing our
            // snapshot: create AND commit a note this transaction knows
            // nothing about. Our commit must preserve it (file + index line).
            simulate_external_writer(&notebook_root)?;
        }

        // Disk baseline before materialize (for rollback of new owned outputs).
        // Virtual tree already includes planned ops; disk is still pre-apply.
        let baseline_on_disk: HashSet<String> = {
            let mut s: HashSet<String> = git::list_notebook_paths(&notebook_root)?
                .into_iter()
                .map(|p| normalize_rel(&p))
                .collect();
            s.extend(ignored_existing.iter().cloned());
            s
        };

        // Force-stage every final file path so ignore rules cannot drop
        // transaction-owned outputs (new ignored names, `.gitkeep`, etc.).
        // `.index` files are applied post-materialize (fresh read under the
        // index lock) but staged by the same checkpoint, so list them too.
        let mut force_paths: Vec<String> = tree
            .nodes
            .iter()
            .filter_map(|(p, n)| match n {
                VirtualNode::File(_) => Some(p.clone()),
                VirtualNode::Folder => None,
            })
            .collect();
        for meta in &op_meta {
            for edit in &meta.index_edits {
                if !force_paths.iter().any(|p| p == &edit.file_rel) {
                    force_paths.push(edit.file_rel.clone());
                }
            }
        }

        // Directories that already exist before materialize must never be
        // pruned during rollback (including empty ignored parents).
        let baseline_dirs = existing_dirs_under(&notebook_root);

        // Materialize non-index files, apply `.index` edits from fresh
        // on-disk content under `.nb-api-index.lock` (released before git
        // staging — never held across `git add`/`commit`), then checkpoint.
        // The lock serializes nb-api writers only; external `nb` writers are
        // covered by O_APPEND plus post-write selector derivation.
        let apply_result = async {
            materialize_tree(&notebook_root, &tree, &effects, &snapshot_keys)?;
            let mut pending: Vec<(u32, PendingIndexEdit)> = Vec::new();
            for (i, meta) in op_meta.iter().enumerate() {
                for edit in &meta.index_edits {
                    pending.push((i as u32, edit.clone()));
                }
            }
            let placements = if pending.is_empty() {
                Vec::new()
            } else {
                let _lock = acquire_index_lock(&notebook_root, self.gate_timeout).await?;
                let placements = apply_index_edits(&notebook_root, &pending, self.gate_timeout)?;
                drop(_lock);
                placements
            };
            let created = git::notebook_commit_all(
                &notebook_root,
                &format!("nb-api transaction ({} ops)", self.plan.len()),
                &force_paths,
            )?;
            Ok::<_, NbError>((created, placements))
        }
        .await;

        match apply_result {
            Ok((created, placements)) => {
                let revision_id = if created {
                    match git::notebook_head(&notebook_root) {
                        Ok(head) => Some(head),
                        Err(_) => {
                            return Err(NbError::IndeterminateCommit {
                                pre_revision,
                                post_revision_observed: None,
                                guidance: "git commit may have succeeded but HEAD could not be re-read; do not retry blindly".into(),
                            });
                        }
                    }
                } else {
                    None
                };
                let ops = op_meta
                    .into_iter()
                    .enumerate()
                    .map(|(i, meta)| {
                        // Creates/moves/deletes carry post-write placements:
                        // selector becomes the numeric `<folder>/<id>` form.
                        // A cross-folder move records both a blank (source)
                        // and an append (destination); the destination
                        // placement is last, so take the last match.
                        // All other ops keep the echoed target selector and
                        // gain a numeric id when the path is indexed.
                        let (selector, numeric_id) =
                            match placements.iter().rev().find(|p| p.op_index == i as u32) {
                                Some(p) => (
                                    Some(numeric_selector(&self.notebook, &p.folder, p.id)),
                                    Some(p.id),
                                ),
                                None => (
                                    meta.selector,
                                    meta.path
                                        .as_deref()
                                        .and_then(|p| index_id_on_disk(&notebook_root, p)),
                                ),
                            };
                        OpOutcome {
                            index: i as u32,
                            path: meta.path,
                            selector,
                            numeric_id,
                            noop: meta.noop,
                            fingerprint: meta.fingerprint,
                        }
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
                // Owned outputs that did not exist at baseline must be removed
                // even when ignored (git clean -fd keeps ignored files).
                // Baseline directories are never owned outputs: pruning one
                // would `remove_dir_all` a pre-existing folder.
                let owned_new: Vec<String> = force_paths
                    .into_iter()
                    .filter(|p| !baseline_on_disk.contains(p))
                    .filter(|p| !baseline_dirs.contains(p))
                    .collect();
                match try_restore(&notebook_root, &pre_revision, &owned_new, &baseline_dirs) {
                    Ok(()) => Err(err),
                    Err(recovery) => Err(recovery),
                }
            }
        }
    }
}

/// All directory paths under the notebook root before materialize (relative,
/// `/`-separated). Used so rollback never deletes pre-existing dirs.
fn existing_dirs_under(notebook_root: &Path) -> HashSet<String> {
    let mut dirs = HashSet::new();
    fn walk(base: &Path, rel: &Path, out: &mut HashSet<String>) {
        let entries = match std::fs::read_dir(base) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name == ".git" {
                continue;
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            // Do not follow symlinks.
            if ft.is_symlink() || !ft.is_dir() {
                continue;
            }
            let child_rel = if rel.as_os_str().is_empty() {
                PathBuf::from(&name)
            } else {
                rel.join(&name)
            };
            let key = child_rel.to_string_lossy().replace('\\', "/");
            out.insert(key);
            walk(&entry.path(), &child_rel, out);
        }
    }
    walk(notebook_root, Path::new(""), &mut dirs);
    dirs
}

fn try_restore(
    notebook_root: &Path,
    pre_revision: &str,
    remove_new_owned: &[String],
    baseline_dirs: &HashSet<String>,
) -> Result<(), NbError> {
    if let Err(e) = git::notebook_reset_clean(notebook_root, pre_revision) {
        let post = git::notebook_head(notebook_root).ok();
        let status = git::notebook_status_porcelain(notebook_root).ok();
        return Err(NbError::RecoveryRequired {
            pre_revision: pre_revision.to_string(),
            post_revision_observed: post,
            status_observed: status,
            preserved_paths: Some(remove_new_owned.to_vec()),
            guidance: format!(
                "cleanup after failed commit could not be verified; inspect HEAD/status before retry. underlying: {e}"
            ),
        });
    }
    // `git clean -fd` can remove empty untracked dirs (including ignored
    // parents) once staged children are cleared by reset --hard. Restore any
    // baseline directory that disappeared.
    for d in baseline_dirs {
        let abs = notebook_root.join(d);
        if !abs.exists()
            && let Err(e) = std::fs::create_dir_all(&abs)
        {
            return Err(NbError::RecoveryRequired {
                pre_revision: pre_revision.to_string(),
                post_revision_observed: git::notebook_head(notebook_root).ok(),
                status_observed: git::notebook_status_porcelain(notebook_root).ok(),
                preserved_paths: Some(vec![d.clone()]),
                guidance: format!(
                    "failed to restore baseline directory `{d}` after cleanup; do not retry blindly. underlying: {e}"
                ),
            });
        }
    }
    // Explicitly delete transaction-owned outputs that reset/clean leave behind
    // when they are Git-ignored untracked files.
    let mut remaining = Vec::new();
    for rel in remove_new_owned {
        let abs = notebook_root.join(rel);
        match std::fs::symlink_metadata(&abs) {
            Ok(meta) if meta.file_type().is_symlink() || meta.is_file() => {
                if let Err(e) = std::fs::remove_file(&abs) {
                    remaining.push(format!("{rel} ({e})"));
                }
            }
            Ok(meta) if meta.is_dir() => {
                if let Err(e) = std::fs::remove_dir_all(&abs) {
                    remaining.push(format!("{rel} ({e})"));
                }
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => remaining.push(format!("{rel} ({e})")),
        }
        // Prune only empty parents that the transaction created — never a
        // directory that existed at baseline (e.g. pre-existing ignored parent).
        if let Some(parent) = parent_path(rel) {
            let mut cur = parent;
            loop {
                if baseline_dirs.contains(&cur) {
                    break;
                }
                let p = notebook_root.join(&cur);
                match std::fs::remove_dir(&p) {
                    Ok(()) => {}
                    Err(_) => break,
                }
                match parent_path(&cur) {
                    Some(next) => cur = next,
                    None => break,
                }
            }
        }
    }
    if !remaining.is_empty() {
        return Err(NbError::RecoveryRequired {
            pre_revision: pre_revision.to_string(),
            post_revision_observed: git::notebook_head(notebook_root).ok(),
            status_observed: git::notebook_status_porcelain(notebook_root).ok(),
            preserved_paths: Some(remaining),
            guidance: "failed to remove transaction-owned outputs after aborted commit; do not retry blindly".into(),
        });
    }
    // Verify none of the new owned paths remain (including ignored).
    let mut still_present = Vec::new();
    for rel in remove_new_owned {
        if notebook_root.join(rel).exists() {
            still_present.push(rel.clone());
        }
    }
    let head = match restore_verify_head(notebook_root) {
        Ok(h) => h,
        Err(e) => {
            return Err(NbError::RecoveryRequired {
                pre_revision: pre_revision.to_string(),
                post_revision_observed: None,
                status_observed: git::notebook_status_porcelain(notebook_root).ok(),
                preserved_paths: if still_present.is_empty() {
                    None
                } else {
                    Some(still_present)
                },
                guidance: format!(
                    "cleanup ran but HEAD re-read failed during verification; do not retry blindly. underlying: {e}"
                ),
            });
        }
    };
    let dirty = match restore_verify_dirty(notebook_root) {
        Ok(d) => d,
        Err(e) => {
            return Err(NbError::RecoveryRequired {
                pre_revision: pre_revision.to_string(),
                post_revision_observed: Some(head),
                status_observed: git::notebook_status_porcelain(notebook_root).ok(),
                preserved_paths: if still_present.is_empty() {
                    None
                } else {
                    Some(still_present)
                },
                guidance: format!(
                    "cleanup ran but dirty-status check failed during verification; do not retry blindly. underlying: {e}"
                ),
            });
        }
    };
    if head != pre_revision || dirty || !still_present.is_empty() {
        return Err(NbError::RecoveryRequired {
            pre_revision: pre_revision.to_string(),
            post_revision_observed: Some(head),
            status_observed: git::notebook_status_porcelain(notebook_root).ok(),
            preserved_paths: if still_present.is_empty() {
                None
            } else {
                Some(still_present)
            },
            guidance:
                "cleanup ran but HEAD/status/owned-path verification failed; do not retry blindly"
                    .into(),
        });
    }
    Ok(())
}

/// HEAD observation used only in post-cleanup verification.
fn restore_verify_head(notebook_root: &Path) -> Result<String, NbError> {
    #[cfg(feature = "testing")]
    if std::env::var_os("NB_API_FAIL_RESTORE_HEAD").is_some() {
        return Err(NbError::CommandFailed {
            command: "nb-api://fail-restore-head".into(),
            stderr: "injected HEAD verify failure for rollback tests".into(),
            exit_code: Some(1),
        });
    }
    git::notebook_head(notebook_root)
}

/// Dirty-status observation used only in post-cleanup verification.
fn restore_verify_dirty(notebook_root: &Path) -> Result<bool, NbError> {
    #[cfg(feature = "testing")]
    if std::env::var_os("NB_API_FAIL_RESTORE_DIRTY").is_some() {
        return Err(NbError::CommandFailed {
            command: "nb-api://fail-restore-dirty".into(),
            stderr: "injected dirty-status verify failure for rollback tests".into(),
            exit_code: Some(1),
        });
    }
    git::notebook_is_dirty(notebook_root)
}

/// Deterministic stand-in for an external `nb add` racing our snapshot
/// (testing only, behind `NB_API_SIMULATE_EXTERNAL_WRITER`): create AND
/// commit a note plus its `.index` line that this transaction knows nothing
/// about. The commit must preserve both.
///
/// `NB_API_SIMULATE_EXTERNAL_SAME_PATH=<rel>` makes the external writer take
/// the given path instead (to test the same-path race: our commit must fail
/// `PathCollision`, never overwrite).
#[cfg(feature = "testing")]
fn simulate_external_writer(notebook_root: &Path) -> Result<(), NbError> {
    use std::io::Write as _;
    let (folder, name) = match std::env::var("NB_API_SIMULATE_EXTERNAL_SAME_PATH") {
        Ok(rel) => {
            let (folder, basename) = split_folder_basename(&normalize_rel(&rel));
            (folder, basename)
        }
        Err(_) => (String::new(), format!("external-{}.md", std::process::id())),
    };
    let rel = if folder.is_empty() {
        name.clone()
    } else {
        format!("{folder}/{name}")
    };
    if !folder.is_empty() {
        std::fs::create_dir_all(notebook_root.join(&folder)).map_err(|e| NbError::Io {
            path: notebook_root.join(&folder),
            source: e.into(),
        })?;
    }
    std::fs::write(notebook_root.join(&rel), b"# External\n\nexternal body\n").map_err(|e| {
        NbError::Io {
            path: notebook_root.join(&rel),
            source: e.into(),
        }
    })?;
    let index_abs = notebook_root.join(index_rel_for_folder(&folder));
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&index_abs)
        .map_err(|e| NbError::Io {
            path: index_abs.clone(),
            source: e.into(),
        })?;
    writeln!(f, "{name}").map_err(|e| NbError::Io {
        path: index_abs.clone(),
        source: e.into(),
    })?;
    drop(f);
    git::notebook_commit_all(
        notebook_root,
        "external writer",
        &[rel, index_rel_for_folder(&folder)],
    )?;
    Ok(())
}

/// Write the plan's effects to disk.
///
/// ONLY paths recorded in `effects` are touched: files created after the
/// snapshot by an external writer are never deleted, and files deleted
/// externally are never resurrected. `.index` rels are skipped here (applied
/// separately from fresh on-disk content under `.nb-api-index.lock`).
///
/// `snapshot_keys` are the tree keys at snapshot time. A `created` path that
/// exists on disk now but was absent from the snapshot means an external
/// writer created the same path after our collision validation — refuse with
/// `PathCollision` instead of overwriting it (which would also leave
/// ambiguous duplicate `.index` entries).
fn materialize_tree(
    root: &Path,
    tree: &VirtualTree,
    effects: &PlanEffects,
    snapshot_keys: &HashSet<String>,
) -> Result<(), NbError> {
    // Preflight: no path may traverse an existing symlink ancestor (including
    // ignored directory symlinks excluded from the virtual snapshot).
    for rel in tree.nodes.keys() {
        refuse_symlink_ancestors(root, rel)?;
    }

    // Remove ONLY plan delete/move sources that exist on disk as files.
    let mut removed: Vec<&String> = effects.removed.iter().collect();
    removed.sort();
    removed.dedup();
    for rel in removed {
        let key = normalize_rel(rel);
        let abs = root.join(&key);
        let meta = match std::fs::symlink_metadata(&abs) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(NbError::Io {
                    path: abs,
                    source: e.into(),
                });
            }
        };
        if meta.file_type().is_symlink() {
            return Err(NbError::UnsupportedStructure {
                reason: format!("refusing to materialize through symlink `{key}`"),
            });
        }
        if meta.is_file() && !matches!(tree.nodes.get(&key), Some(VirtualNode::File(_))) {
            std::fs::remove_file(&abs).map_err(|e| NbError::Io {
                path: abs,
                source: e.into(),
            })?;
        }
    }
    // Create ONLY plan folders.
    let mut mkdirs: Vec<&String> = effects.mkdirs.iter().collect();
    mkdirs.sort();
    mkdirs.dedup();
    for rel in mkdirs {
        let key = normalize_rel(rel);
        let abs = root.join(&key);
        refuse_symlink_ancestors(root, &key)?;
        std::fs::create_dir_all(&abs).map_err(|e| NbError::Io {
            path: abs.clone(),
            source: e.into(),
        })?;
    }
    // Write ONLY plan-created/modified files (never `.index`).
    let mut written: Vec<&String> = effects
        .written
        .iter()
        .filter(|rel| !is_index_rel(rel))
        .collect();
    written.sort();
    written.dedup();
    let created: HashSet<&String> = effects.created.iter().collect();
    for rel in written {
        let key = normalize_rel(rel);
        let created_contains = created.contains(rel);
        let Some(VirtualNode::File(bytes)) = tree.nodes.get(&key) else {
            return Err(NbError::ValidationError {
                reason: format!("plan effect `{key}` has no file content"),
                location: None,
            });
        };
        let abs = root.join(&key);
        // Never write through an existing symlink leaf or ancestor.
        refuse_symlink_ancestors(root, &key)?;
        if let Ok(meta) = std::fs::symlink_metadata(&abs) {
            if meta.file_type().is_symlink() {
                return Err(NbError::UnsupportedStructure {
                    reason: format!("refusing to write through symlink `{key}`"),
                });
            }
            // A planned create racing an external same-path create (file or
            // directory appearing after the snapshot): refuse instead of
            // overwriting the external note or failing obscurely on a dir.
            if created_contains && !snapshot_keys.contains(&key) {
                return Err(NbError::PathCollision {
                    path: key,
                    plan_index: None,
                });
            }
        }
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
    Ok(())
}

/// Reject `rel` when any existing path component under `root` is a symlink.
///
/// Uses no-follow metadata so ignored directory symlinks (absent from the
/// virtual snapshot) cannot redirect `create_dir_all` / `write` outside the
/// notebook root.
fn refuse_symlink_ancestors(root: &Path, rel: &str) -> Result<(), NbError> {
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

fn outcome_selector(notebook: &str, target: &NoteTarget, resolved_path: &str) -> String {
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

struct OpMeta {
    path: Option<String>,
    selector: Option<String>,
    noop: bool,
    fingerprint: Option<Fingerprint>,
    index_edits: Vec<PendingIndexEdit>,
}

/// Paths the plan dirties. Materialization touches ONLY these paths: files
/// created after the snapshot (e.g. an external `nb add` racing our commit)
/// are never deleted, and files deleted externally are never resurrected.
#[derive(Debug, Default)]
struct PlanEffects {
    /// Delete/move sources: the only paths materialize may remove.
    removed: Vec<String>,
    /// Created or modified file paths: the only files materialize writes
    /// (`.index` rels excluded — applied separately under the index lock).
    written: Vec<String>,
    /// Paths that must NOT exist on disk at materialize time (creates and
    /// move destinations): a file appearing after the snapshot means an
    /// external writer raced us — refuse with `PathCollision` instead of
    /// overwriting it.
    created: Vec<String>,
    /// Folders created by the plan.
    mkdirs: Vec<String>,
}

/// One `.index` mutation derived from a validated plan op.
///
/// `file_rel` is the notebook-relative `.index` path (`.index` at root,
/// `<folder>/.index` otherwise). Applied to fresh on-disk content under the
/// `.nb-api-index.lock`, in plan order, after non-index files materialize.
#[derive(Debug, Clone)]
struct PendingIndexEdit {
    file_rel: String,
    kind: PendingIndexKind,
}

#[derive(Debug, Clone)]
enum PendingIndexKind {
    Append {
        basename: String,
    },
    Blank {
        basename: String,
    },
    Rename {
        old_basename: String,
        new_basename: String,
    },
}

fn validate_and_apply_virtual(
    notebook: &str,
    notebook_root: &Path,
    tree: &mut VirtualTree,
    ignored_existing: &HashSet<String>,
    op: &PlanOp,
    _index: usize,
    effects: &mut PlanEffects,
) -> Result<OpMeta, NbError> {
    match op {
        PlanOp::AddNote {
            path,
            title,
            content,
            tags,
        } => {
            refuse_create_collision(notebook_root, tree, ignored_existing, path)?;
            let bytes = build_note_bytes(title.as_deref(), content, tags);
            let fp = fingerprint_bytes(&bytes, path)?;
            tree.insert_file(path.clone(), bytes);
            effects.written.push(path.clone());
            effects.created.push(path.clone());
            Ok(OpMeta {
                path: Some(path.clone()),
                selector: Some(format!("{notebook}:{path}")),
                noop: false,
                fingerprint: Some(fp),
                index_edits: vec![append_index_edit(path)],
            })
        }
        PlanOp::AddTodo {
            path,
            title,
            description,
            tasks,
            tags,
        } => {
            refuse_create_collision(notebook_root, tree, ignored_existing, path)?;
            let bytes = build_todo_bytes(title, description.as_deref(), tasks, tags);
            let fp = fingerprint_bytes(&bytes, path)?;
            tree.insert_file(path.clone(), bytes);
            effects.written.push(path.clone());
            effects.created.push(path.clone());
            Ok(OpMeta {
                path: Some(path.clone()),
                selector: Some(format!("{notebook}:{path}")),
                noop: false,
                fingerprint: Some(fp),
                index_edits: vec![append_index_edit(path)],
            })
        }
        PlanOp::AddBookmark {
            path,
            url,
            title,
            tags,
            comment,
        } => {
            refuse_create_collision(notebook_root, tree, ignored_existing, path)?;
            let bytes = build_bookmark_bytes(url, title.as_deref(), tags, comment.as_deref());
            let fp = fingerprint_bytes(&bytes, path)?;
            tree.insert_file(path.clone(), bytes);
            effects.written.push(path.clone());
            effects.created.push(path.clone());
            Ok(OpMeta {
                path: Some(path.clone()),
                selector: Some(format!("{notebook}:{path}")),
                noop: false,
                fingerprint: Some(fp),
                index_edits: vec![append_index_edit(path)],
            })
        }
        PlanOp::AddFolder { path } => {
            refuse_create_collision(notebook_root, tree, ignored_existing, path)?;
            let keep = format!("{path}/.gitkeep");
            refuse_create_collision(notebook_root, tree, ignored_existing, &keep)?;
            tree.insert_folder(path.clone());
            effects.mkdirs.push(path.clone());
            // Git cannot track empty dirs; persist a keep file so the folder
            // survives commit/checkout/clone. Force-staged at checkpoint.
            if !tree.exists(&keep) {
                tree.insert_file(keep.clone(), Vec::new());
            }
            effects.written.push(keep.clone());
            effects.created.push(keep);
            Ok(OpMeta {
                path: Some(path.clone()),
                selector: None,
                noop: false,
                fingerprint: None,
                index_edits: Vec::new(),
            })
        }
        PlanOp::DeleteNote { target } => {
            let path = resolve_target_path(tree, ignored_existing, target)?;
            if !tree.exists(&path) {
                return Err(NbError::NotFound {
                    selector: target.value().to_string(),
                });
            }
            tree.remove(&path);
            effects.removed.push(path.clone());
            Ok(OpMeta {
                path: Some(path.clone()),
                selector: Some(outcome_selector(notebook, target, &path)),
                noop: false,
                fingerprint: None,
                index_edits: vec![blank_index_edit(&path)],
            })
        }
        PlanOp::MoveNote {
            target,
            destination,
        } => {
            let from = resolve_target_path(tree, ignored_existing, target)?;
            let to = resolve_move_destination(&from, destination);
            refuse_create_collision(notebook_root, tree, ignored_existing, &to)?;
            // Same-folder renames update the `.index` line in place, which
            // is only lossless when the new name is not longer than the old
            // one (the splice prefix must not overlap the appender region).
            // A longer in-folder rename is refused: delete + recreate the
            // note (new id) instead. Cross-folder moves are blank + append
            // and are always safe.
            let (from_folder, from_base) = split_folder_basename(&from);
            let (to_folder, to_base) = split_folder_basename(&to);
            if from_folder == to_folder && to_base.len() > from_base.len() {
                return Err(NbError::ValidationError {
                    reason: format!(
                        "in-folder rename to a longer name (`{from_base}` -> `{to_base}`) would risk concurrent .index appends; delete and recreate the note instead"
                    ),
                    location: None,
                });
            }
            tree.rename(&from, &to)?;
            effects.removed.push(from.clone());
            effects.written.push(to.clone());
            effects.created.push(to.clone());
            Ok(OpMeta {
                path: Some(to.clone()),
                selector: Some(format!("{notebook}:{to}")),
                noop: false,
                fingerprint: None,
                index_edits: move_index_edits(&from, &to),
            })
        }
        PlanOp::MarkTaskDone {
            target,
            task_number,
        } => {
            let path = resolve_target_path(tree, ignored_existing, target)?;
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
            effects.written.push(path.clone());
            Ok(OpMeta {
                path: Some(path.clone()),
                selector: Some(outcome_selector(notebook, target, &path)),
                noop,
                fingerprint: Some(fp),
                index_edits: Vec::new(),
            })
        }
        PlanOp::UnmarkTaskDone {
            target,
            task_number,
        } => {
            let path = resolve_target_path(tree, ignored_existing, target)?;
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
            effects.written.push(path.clone());
            Ok(OpMeta {
                path: Some(path.clone()),
                selector: Some(outcome_selector(notebook, target, &path)),
                noop,
                fingerprint: Some(fp),
                index_edits: Vec::new(),
            })
        }
        PlanOp::ReplaceNoteBody {
            target,
            new_body,
            fingerprint: expected,
        } => {
            let path = resolve_target_path(tree, ignored_existing, target)?;
            let bytes = tree
                .get_file(&path)
                .ok_or_else(|| NbError::NotFound {
                    selector: target.value().to_string(),
                })?
                .to_vec();
            let doc = parse_doc(&bytes, &path)?;
            // Contiguity before fingerprint so fragmented bodies always surface
            // FragmentedBody even with a stale fingerprint.
            require_contiguous_body(&doc)?;
            let current = fingerprint::fingerprint(&doc);
            if &current != expected {
                return Err(NbError::FingerprintMismatch {
                    target: target.clone(),
                    guidance: "body fingerprint does not match; re-read and retry".into(),
                });
            }
            let new_bytes = splice_body(&doc, new_body.as_bytes())?;
            let fp = fingerprint_bytes(&new_bytes, &path)?;
            let noop = new_bytes == bytes;
            tree.insert_file(path.clone(), new_bytes);
            effects.written.push(path.clone());
            Ok(OpMeta {
                path: Some(path.clone()),
                selector: Some(outcome_selector(notebook, target, &path)),
                noop,
                fingerprint: Some(fp),
                index_edits: Vec::new(),
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
            let path = resolve_target_path(tree, ignored_existing, target)?;
            let bytes = tree
                .get_file(&path)
                .ok_or_else(|| NbError::NotFound {
                    selector: target.value().to_string(),
                })?
                .to_vec();
            let doc = parse_doc(&bytes, &path)?;
            let body = require_contiguous_body(&doc)?;
            if let Some(exp) = expected {
                let current = fingerprint::fingerprint(&doc);
                if &current != exp {
                    return Err(NbError::FingerprintMismatch {
                        target: target.clone(),
                        guidance: "body fingerprint does not match; re-read and retry".into(),
                    });
                }
            }
            let new_body = apply_substring(
                &body,
                pattern.as_bytes(),
                replacement.as_bytes(),
                occurrence,
                *expected_count,
            )?;
            let new_bytes = splice_body(&doc, &new_body)?;
            let fp = fingerprint_bytes(&new_bytes, &path)?;
            let noop = new_bytes == bytes;
            tree.insert_file(path.clone(), new_bytes);
            effects.written.push(path.clone());
            Ok(OpMeta {
                path: Some(path.clone()),
                selector: Some(outcome_selector(notebook, target, &path)),
                noop,
                fingerprint: Some(fp),
                index_edits: Vec::new(),
            })
        }
        PlanOp::EditNoteLines { target, edits } => {
            let path = resolve_target_path(tree, ignored_existing, target)?;
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
            effects.written.push(path.clone());
            Ok(OpMeta {
                path: Some(path.clone()),
                selector: Some(outcome_selector(notebook, target, &path)),
                noop,
                fingerprint: Some(fp),
                index_edits: Vec::new(),
            })
        }
        PlanOp::RetitleNote { target, title } => {
            let path = resolve_target_path(tree, ignored_existing, target)?;
            let bytes = tree
                .get_file(&path)
                .ok_or_else(|| NbError::NotFound {
                    selector: target.value().to_string(),
                })?
                .to_vec();
            let doc = parse_doc(&bytes, &path)?;
            let mut title_line = title.clone();
            if !title_line.starts_with('#') {
                title_line = format!("# {title_line}");
            }
            let new_bytes = splice_title(&doc, title_line.as_bytes())?;
            let fp = fingerprint_bytes(&new_bytes, &path)?;
            let noop = new_bytes == bytes;
            tree.insert_file(path.clone(), new_bytes);
            effects.written.push(path.clone());
            Ok(OpMeta {
                path: Some(path.clone()),
                selector: Some(outcome_selector(notebook, target, &path)),
                noop,
                fingerprint: Some(fp),
                index_edits: Vec::new(),
            })
        }
        PlanOp::EditNoteTags {
            target,
            add,
            remove,
        } => {
            let path = resolve_target_path(tree, ignored_existing, target)?;
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
            effects.written.push(path.clone());
            Ok(OpMeta {
                path: Some(path.clone()),
                selector: Some(outcome_selector(notebook, target, &path)),
                noop,
                fingerprint: Some(fp),
                index_edits: Vec::new(),
            })
        }
    }
}

fn refuse_create_collision(
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

fn refuse_existing_ignored(ignored_existing: &HashSet<String>, path: &str) -> Result<(), NbError> {
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

fn resolve_target_path(
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

fn resolve_move_destination(from: &str, destination: &str) -> String {
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
        NbError::PathIgnored { path, guidance, .. } => NbError::PathIgnored {
            path,
            guidance,
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
    // Validate the caller-supplied string before any normalization so
    // absolute and backslash forms cannot be silently rewritten.
    let raw = path.trim();
    if raw.is_empty() {
        return Err(NbError::ValidationError {
            reason: "path must not be empty".into(),
            location: None,
        });
    }
    if raw.starts_with('/') || raw.starts_with('\\') || Path::new(raw).is_absolute() {
        return Err(NbError::ValidationError {
            reason: "path must be notebook-relative (absolute paths refused)".into(),
            location: None,
        });
    }
    if raw.contains('\\') {
        return Err(NbError::ValidationError {
            reason: "path must not contain backslash separators".into(),
            location: None,
        });
    }
    if raw.contains('\0') {
        return Err(NbError::ValidationError {
            reason: "path must not contain NUL".into(),
            location: None,
        });
    }
    let path = normalize_rel(raw);
    if path.is_empty() {
        return Err(NbError::ValidationError {
            reason: "path must not be empty".into(),
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

/// Slug a title the way `nb` 7.24.0 does (ASCII locale): ASCII `A-Z` is
/// lowercased, non-ASCII is preserved verbatim, ASCII non-alphanumeric maps
/// to `_`, runs collapse, leading/trailing `_` trim.
pub(crate) fn mangle_stem(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut prev_underscore = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_underscore = false;
        } else if c.is_ascii() {
            if !prev_underscore {
                out.push('_');
                prev_underscore = true;
            }
        } else {
            out.push(c);
            prev_underscore = false;
        }
    }
    out.trim_matches('_').to_string()
}

/// Titleless filename stem in process-local time (`%Y%m%d%H%M%S`), matching
/// `nb`'s `date` (not UTC). Shells out to `date` so `TZ` handling matches
/// `nb` exactly; falls back to epoch seconds on failure.
pub(crate) fn titleless_stem() -> String {
    if let Ok(out) = std::process::Command::new("date")
        .arg("+%Y%m%d%H%M%S")
        .output()
        && out.status.success()
    {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.len() == 14 && s.bytes().all(|b| b.is_ascii_digit()) {
            return s;
        }
    }
    // Fallback: zero-padded epoch seconds (14 chars) + process-unique suffix.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{secs:014}-{n}")
}

/// Filename for a one-shot create: mangled title stem (or local-time
/// titleless stem when `title` is `None`/blank/fully-punctuated) + extension.
///
/// `extension` is the dotted kind suffix without leading dot (`md`,
/// `todo.md`, `bookmark.md`). Collision `-N` suffixing happens at commit
/// time via retry in the one-shot wrappers.
pub(crate) fn filename_for_title(title: Option<&str>, extension: &str) -> String {
    let stem = title
        .map(|t| mangle_stem(t.trim()))
        .filter(|s| !s.is_empty());
    match stem {
        Some(s) => format!("{s}.{extension}"),
        None => format!("{}.{extension}", titleless_stem()),
    }
}

/// Insert `-N` before the extension (`foo.md` -> `foo-1.md`).
pub(crate) fn suffixed_filename(filename: &str, n: u32) -> String {
    match filename.find('.') {
        Some(i) => format!("{}-{n}{}", &filename[..i], &filename[i..]),
        None => format!("{filename}-{n}"),
    }
}

pub(crate) fn join_folder_file(folder: Option<&str>, filename: &str) -> String {
    match folder {
        Some(f) if !f.trim().is_empty() => {
            format!("{}/{}", normalize_rel(f), filename.trim_start_matches('/'))
        }
        _ => filename.to_string(),
    }
}

/// Split a notebook-relative path into `(folder, basename)`.
fn split_folder_basename(path: &str) -> (String, String) {
    match path.rfind('/') {
        Some(i) => (path[..i].to_string(), path[i + 1..].to_string()),
        None => (String::new(), path.to_string()),
    }
}

/// Notebook-relative `.index` path for a folder (`""` -> `.index`).
fn index_rel_for_folder(folder: &str) -> String {
    if folder.is_empty() {
        ".index".to_string()
    } else {
        format!("{folder}/.index")
    }
}

fn append_index_edit(path: &str) -> PendingIndexEdit {
    let (folder, basename) = split_folder_basename(path);
    PendingIndexEdit {
        file_rel: index_rel_for_folder(&folder),
        kind: PendingIndexKind::Append { basename },
    }
}

fn blank_index_edit(path: &str) -> PendingIndexEdit {
    let (folder, basename) = split_folder_basename(path);
    PendingIndexEdit {
        file_rel: index_rel_for_folder(&folder),
        kind: PendingIndexKind::Blank { basename },
    }
}

fn move_index_edits(from: &str, to: &str) -> Vec<PendingIndexEdit> {
    let (from_folder, from_base) = split_folder_basename(from);
    let (to_folder, to_base) = split_folder_basename(to);
    if from_folder == to_folder {
        vec![PendingIndexEdit {
            file_rel: index_rel_for_folder(&from_folder),
            kind: PendingIndexKind::Rename {
                old_basename: from_base,
                new_basename: to_base,
            },
        }]
    } else {
        vec![
            PendingIndexEdit {
                file_rel: index_rel_for_folder(&from_folder),
                kind: PendingIndexKind::Blank {
                    basename: from_base,
                },
            },
            PendingIndexEdit {
                file_rel: index_rel_for_folder(&to_folder),
                kind: PendingIndexKind::Append { basename: to_base },
            },
        ]
    }
}

/// Parse `.index` bytes into positional lines.
///
/// Trailing-newline terminated content splits cleanly; a missing trailing
/// newline still yields its tail as a line. Empty content yields no lines.
fn parse_index_lines(bytes: &[u8]) -> Vec<String> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(bytes);
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

fn render_index_lines(lines: &[String]) -> Vec<u8> {
    if lines.is_empty() {
        return Vec::new();
    }
    let mut out = lines.join("\n").into_bytes();
    out.push(b'\n');
    out
}

/// Numeric selector for a post-write placement (`nb:3`, `nb:work/3`).
fn numeric_selector(notebook: &str, folder: &str, id: u32) -> String {
    if folder.is_empty() {
        format!("{notebook}:{id}")
    } else {
        format!("{notebook}:{folder}/{id}")
    }
}

/// Notebook-relative `.index` files are managed post-materialize under the
/// `.nb-api-index.lock`, so the generic tree write must not clobber them
/// with its stale snapshot.
fn is_index_rel(rel: &str) -> bool {
    rel == ".index" || rel.ends_with("/.index")
}

/// Guard for the notebook-scoped `.nb-api-index.lock`.
///
/// Serializes nb-api writers against each other for the `.index` read-append
/// critical section only (never held across `git add`/`commit`). It does NOT
/// serialize external `nb` processes; the cross-process guarantee comes from
/// `O_APPEND` plus the rewrite merge loop, with post-write re-read for the
/// selector.
///
/// The lock file carries a unique nonce (`pid`, process-unique counter,
/// timestamp). `Drop` removes the file ONLY if it still holds our nonce, so
/// a slow holder reaped as "stale" can never delete the lock file a
/// replacement writer created afterwards.
///
/// A live holder refreshes the file mtime on a `timeout/3` heartbeat, so a
/// lock that is both older than `timeout` and content-stable is genuinely
/// dead — never a live holder. The heartbeat stops itself if the file no
/// longer holds our nonce (we were reaped despite everything; refreshing a
/// foreign lock would only delay its legitimate reap).
struct IndexLockGuard {
    path: PathBuf,
    nonce: String,
    heartbeat: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for IndexLockGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.heartbeat.take() {
            handle.abort();
        }
        if std::fs::read_to_string(&self.path)
            .map(|content| content.trim_end() == self.nonce)
            .unwrap_or(false)
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn index_lock_path(notebook_root: &Path) -> PathBuf {
    notebook_root.join(".nb-api-index.lock")
}

fn new_lock_nonce() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("pid={} nonce={n} ns={nanos}", std::process::id())
}

/// Remove the lock file only if it is stale (mtime older than `timeout`) AND
/// its content is unchanged across the check, so a live holder that just
/// created (or just refreshed) the file is never reaped.
///
/// Returns true when no lock file remains (absent or reaped).
fn try_remove_lock_if_stale(path: &Path, timeout: Duration) -> bool {
    let content_before = std::fs::read_to_string(path).ok();
    let stale = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().unwrap_or(Duration::ZERO) > timeout)
        .unwrap_or(false);
    if !stale {
        return content_before.is_none();
    }
    // Re-read: only reap when the content is identical (no live writer
    // replaced it between our reads) and still stale.
    let content_after = std::fs::read_to_string(path).ok();
    if content_after != content_before {
        return false;
    }
    let still_stale = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().unwrap_or(Duration::ZERO) > timeout)
        .unwrap_or(true);
    if content_before.is_none() || !still_stale {
        return content_before.is_none();
    }
    std::fs::remove_file(path).is_ok()
}

/// Best-effort reap of a stale lock left by a crashed writer, before the
/// dirty-baseline check (an untracked lock file would otherwise read dirty).
/// Only reaps when the file is older than `timeout` with stable content; a
/// live holder's fresh lock is left alone (and then correctly fails the
/// dirty check until its owner finishes and removes it).
fn reap_stale_index_lock(notebook_root: &Path, timeout: Duration) {
    let path = index_lock_path(notebook_root);
    if path.exists() {
        try_remove_lock_if_stale(&path, timeout);
    }
}

async fn acquire_index_lock(
    notebook_root: &Path,
    timeout: Duration,
) -> Result<IndexLockGuard, NbError> {
    // The critical section is millisecond-scale local IO while `timeout`
    // defaults to 60s, so reaping below only triggers for genuinely dead
    // holders, not slow live ones.
    let path = index_lock_path(notebook_root);
    let start = Instant::now();
    loop {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                let nonce = new_lock_nonce();
                let _ = writeln!(f, "{nonce}");
                drop(f);
                // Heartbeat: a live holder must never look stale. Refresh
                // mtime every timeout/3 (at least every 30s for huge
                // timeouts, at most every 5ms to avoid spin on tiny ones).
                let beat = (timeout / 3).clamp(Duration::from_millis(5), Duration::from_secs(30));
                let heartbeat_path = path.clone();
                let heartbeat_nonce = nonce.clone();
                let heartbeat = tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(beat).await;
                        let owned = std::fs::read_to_string(&heartbeat_path)
                            .map(|content| content.trim_end() == heartbeat_nonce)
                            .unwrap_or(false);
                        if !owned {
                            break;
                        }
                        if std::fs::File::options()
                            .write(true)
                            .open(&heartbeat_path)
                            .and_then(|f| f.set_modified(SystemTime::now()))
                            .is_err()
                        {
                            break;
                        }
                    }
                });
                return Ok(IndexLockGuard {
                    path,
                    nonce,
                    heartbeat: Some(heartbeat),
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if try_remove_lock_if_stale(&path, timeout) {
                    continue;
                }
                if start.elapsed() >= timeout {
                    return Err(NbError::IndexLockTimeout {
                        path: path.to_string_lossy().into_owned(),
                        timeout_ms: timeout.as_millis() as u64,
                    });
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(e) => {
                return Err(NbError::Io {
                    path: path.clone(),
                    source: e.into(),
                });
            }
        }
    }
}

/// Placement of one `.index` line after application: 1-based line number.
#[derive(Debug, Clone)]
struct IndexPlacement {
    op_index: u32,
    folder: String,
    id: u32,
}

/// Apply pending `.index` edits to fresh on-disk content under the lock.
///
/// Pure-append files use `O_APPEND` so a concurrent external `nb add` line is
/// never clobbered. Files needing blank/rename go through a merge loop:
/// our mutations are re-applied onto the latest on-disk content until a
/// read-after-write shows no change, so external `O_APPEND` lines that land
/// mid-update are preserved, never overwritten. If the file never settles
/// (continuous concurrent writers), returns `IndexLockTimeout` instead of
/// risking a loss. Returns post-write placements (last occurrence of each
/// basename, which is unique per the collision-suffix rule).
fn apply_index_edits(
    notebook_root: &Path,
    pending: &[(u32, PendingIndexEdit)],
    contention_timeout: Duration,
) -> Result<Vec<IndexPlacement>, NbError> {
    // Group by file, preserving plan order within each file.
    let mut order: Vec<String> = Vec::new();
    let mut grouped: HashMap<String, Vec<(u32, &PendingIndexEdit)>> = HashMap::new();
    for (op_index, edit) in pending {
        grouped
            .entry(edit.file_rel.clone())
            .or_insert_with(|| {
                order.push(edit.file_rel.clone());
                Vec::new()
            })
            .push((*op_index, edit));
    }
    let mut placements: Vec<IndexPlacement> = Vec::new();
    for file_rel in &order {
        let edits = &grouped[file_rel];
        let abs = notebook_root.join(file_rel);
        let append_only = edits
            .iter()
            .all(|(_, e)| matches!(e.kind, PendingIndexKind::Append { .. }));
        if append_only {
            // O_APPEND path: concurrent external appends interleave safely.
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent).map_err(|e| NbError::Io {
                    path: parent.to_path_buf(),
                    source: e.into(),
                })?;
            }
            let mut f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&abs)
                .map_err(|e| NbError::Io {
                    path: abs.clone(),
                    source: e.into(),
                })?;
            for (op_index, edit) in edits {
                let PendingIndexKind::Append { basename } = &edit.kind else {
                    unreachable!()
                };
                writeln!(f, "{basename}").map_err(|e| NbError::Io {
                    path: abs.clone(),
                    source: e.into(),
                })?;
                let _ = op_index;
            }
        } else {
            // Lossless rewrite vs concurrent appenders (see
            // `merge_index_rewrite`): never a truncate-write-verify.
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent).map_err(|e| NbError::Io {
                    path: parent.to_path_buf(),
                    source: e.into(),
                })?;
            }
            let appends: Vec<&String> = edits
                .iter()
                .filter_map(|(_, e)| match &e.kind {
                    PendingIndexKind::Append { basename } => Some(basename),
                    _ => None,
                })
                .collect();
            let mutations: Vec<&PendingIndexEdit> = edits
                .iter()
                .map(|(_, e)| *e)
                .filter(|e| !matches!(e.kind, PendingIndexKind::Append { .. }))
                .collect();
            merge_index_rewrite(&abs, &appends, &mutations, contention_timeout)?;
        }
        // Post-write re-read: last occurrence wins (basenames unique).
        let fresh = std::fs::read(&abs).map_err(|e| NbError::Io {
            path: abs.clone(),
            source: e.into(),
        })?;
        let lines = parse_index_lines(&fresh);
        for (op_index, edit) in edits {
            let (folder, basename) = match &edit.kind {
                PendingIndexKind::Append { basename } => {
                    let (folder, _) =
                        split_folder_basename(&pending_basename_path(file_rel, basename));
                    (folder, basename.clone())
                }
                PendingIndexKind::Blank { basename } => {
                    let (folder, _) =
                        split_folder_basename(&pending_basename_path(file_rel, basename));
                    (folder, basename.clone())
                }
                PendingIndexKind::Rename { new_basename, .. } => {
                    let (folder, _) =
                        split_folder_basename(&pending_basename_path(file_rel, new_basename));
                    (folder, new_basename.clone())
                }
            };
            if let Some(pos) = lines.iter().rposition(|l| l == &basename) {
                placements.push(IndexPlacement {
                    op_index: *op_index,
                    folder,
                    id: pos as u32 + 1,
                });
            } else if let PendingIndexKind::Blank { .. } = &edit.kind {
                // Blank target missing from a fresh external index: fall back
                // to the virtual position so deletes of unindexed files still
                // report a selector echo without an id.
                let _ = folder;
            }
        }
    }
    Ok(placements)
}

/// Rewrite one `.index` file with blank/rename mutations.
///
/// Where the platform offers atomic range removal (Linux
/// `FALLOC_FL_COLLAPSE_RANGE`, verified per file at runtime), the rewrite is
/// lossless against concurrent O_APPEND writers (external `nb add`): the
/// mutated prefix (never longer than its base — same-folder growing renames
/// are refused at plan time) is overwritten in place while concurrent
/// appends live beyond it; the observed tail is copied forward and the stale
/// middle range is removed with a single atomic `collapse_range` that the
/// kernel serializes against appends. Any size movement retries the round
/// (mutations are idempotent; our basenames are unique so appends are never
/// duplicated). There is no truncate-write-verify window: no appended line
/// can be destroyed undetectably.
///
/// Where the platform lacks atomic range removal, the stale range is removed
/// with truncate: the race is narrowed to that single syscall boundary with
/// retry on any observed growth, and `IndexLockTimeout` refusal under
/// sustained contention — a documented residual micro-window, not covered by
/// the no-loss guarantee (see the note-identity specification, item 6).
///
/// A rename that still GROWS the file can only arrive via re-attached
/// appends after an external rewrite dropped ours, and falls back to a
/// whole write with post-verify — same documented residual.
///
/// Concurrent external REWRITES (another blank/rename, e.g. external `nb
/// delete`) are out of scope: without `nb` cooperation no protocol can
/// serialize them; same as nb-vs-nb. If the file never settles, returns
/// `IndexLockTimeout` instead of risking a loss.
fn merge_index_rewrite(
    abs: &Path,
    appends: &[&String],
    mutations: &[&PendingIndexEdit],
    contention_timeout: Duration,
) -> Result<(), NbError> {
    use std::io::{Read as _, Seek as _, Write as _};
    let io_err = |e: std::io::Error| NbError::Io {
        path: abs.to_path_buf(),
        source: e.into(),
    };
    let mut fd = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(abs)
        .map_err(io_err)?;
    for _ in 0..25 {
        let s0 = fd.metadata().map_err(io_err)?.len();
        fd.seek(std::io::SeekFrom::Start(0)).map_err(io_err)?;
        let mut base = Vec::new();
        fd.read_to_end(&mut base).map_err(io_err)?;
        if fd.metadata().map_err(io_err)?.len() != s0 {
            continue; // grew mid-read; nothing written yet
        }
        #[cfg(feature = "testing")]
        if let Ok(ms) = std::env::var("NB_API_INDEX_SPLICE_PAUSE_MS")
            && let Ok(ms) = ms.parse::<u64>()
        {
            // Testing only: hold the read-to-splice window open so a
            // concurrent external O_APPEND deterministically lands inside
            // it. External writers take no lock, so they are unaffected.
            std::thread::sleep(Duration::from_millis(ms));
        }
        let mut lines = parse_index_lines(&base);
        for edit in mutations {
            match &edit.kind {
                PendingIndexKind::Append { .. } => {}
                PendingIndexKind::Blank { basename } => {
                    if let Some(pos) = lines.iter().position(|l| l == basename) {
                        lines[pos] = String::new();
                    }
                }
                PendingIndexKind::Rename {
                    old_basename,
                    new_basename,
                } => {
                    if let Some(pos) = lines.iter().position(|l| l == old_basename) {
                        lines[pos] = new_basename.clone();
                    }
                }
            }
        }
        for basename in appends {
            if !lines.iter().any(|l| l == *basename) {
                lines.push((*basename).clone());
            }
        }
        let rendered = render_index_lines(&lines);
        if rendered.as_slice() == base.as_slice() {
            return Ok(()); // already applied; nothing to write
        }
        if rendered.len() as u64 > s0 {
            // Growing rewrite: only reachable via re-attached appends after
            // an external rewrite dropped ours (same-folder growing renames
            // are refused at plan time). Whole write + verify fallback with
            // a documented residual micro-window.
            fd.seek(std::io::SeekFrom::Start(0)).map_err(io_err)?;
            fd.write_all(&rendered).map_err(io_err)?;
            fd.set_len(rendered.len() as u64).map_err(io_err)?;
            if fd.metadata().map_err(io_err)?.len() == rendered.len() as u64 {
                return Ok(());
            }
            continue;
        }
        // Shrink splice: the prefix overwrite cannot reach appender bytes
        // (they live at offsets >= s0 >= rendered length).
        let prefix_len = rendered.len() as u64;
        fd.seek(std::io::SeekFrom::Start(0)).map_err(io_err)?;
        fd.write_all(&rendered).map_err(io_err)?;
        if fd.metadata().map_err(io_err)?.len() != s0 {
            continue; // grew during prefix write; their bytes untouched
        }
        // Copy the concurrent tail [s0, s1) forward behind our prefix.
        let s1 = fd.metadata().map_err(io_err)?.len();
        fd.seek(std::io::SeekFrom::Start(s0)).map_err(io_err)?;
        if fd.metadata().map_err(io_err)?.len() != s1 {
            continue;
        }
        let mut tail = Vec::new();
        let mut take = (s1 - s0) as usize;
        while take > 0 {
            let mut chunk = vec![0u8; take.min(8192)];
            let n = fd.read(&mut chunk).map_err(io_err)?;
            if n == 0 {
                break;
            }
            tail.extend_from_slice(&chunk[..n]);
            take -= n;
        }
        if fd.metadata().map_err(io_err)?.len() != s1 {
            continue;
        }
        #[cfg(feature = "testing")]
        if let Ok(ms) = std::env::var("NB_API_INDEX_SPLICE_PAUSE_MS")
            && let Ok(ms) = ms.parse::<u64>()
        {
            // Testing only: hold the tail-splice window open too, so a
            // concurrent external O_APPEND deterministically lands inside
            // it as well as the read-to-prefix window above.
            std::thread::sleep(Duration::from_millis(ms));
        }
        fd.seek(std::io::SeekFrom::Start(prefix_len))
            .map_err(io_err)?;
        fd.write_all(&tail).map_err(io_err)?;
        // Remove the stale region [prefix_len + tail, s0) and shift the
        // external tail left. collapse_range is a single syscall the kernel
        // serializes against concurrent appends: they land wholly before
        // (shifted left, preserved) or wholly after (at the new end,
        // preserved) — either way the post-check size mismatch retries
        // with their bytes present. No truncate-then-verify window.
        let tail_len = tail.len() as u64;
        let stale_len = s0.saturating_sub(prefix_len + tail_len);
        let expected = prefix_len + (s1 - s0);
        let collapsed = collapse_stale_range(&fd, prefix_len + tail_len, stale_len);
        if !collapsed {
            // Portable fallback (non-Linux, or filesystems without
            // collapse support): truncate with a documented residual
            // micro-window vs racing appends.
            fd.set_len(prefix_len + tail_len).map_err(io_err)?;
        }
        if fd.metadata().map_err(io_err)?.len() == expected {
            return Ok(());
        }
        // Size moved during removal; retry onto the new content.
    }
    Err(NbError::IndexLockTimeout {
        path: abs.to_string_lossy().into_owned(),
        timeout_ms: contention_timeout.as_millis() as u64,
    })
}

/// Remove the byte range [offset, offset+len), shifting the tail left.
///
/// Single syscall (`FALLOC_FL_COLLAPSE_RANGE`): the kernel serializes it
/// against concurrent O_APPEND writes, which land wholly before (shifted
/// left along with the tail — preserved) or wholly after (at the new end —
/// preserved). Either way a post-check size mismatch retries with their
/// bytes present, so unlike truncate-then-verify there is no window in
/// which an appended line can be destroyed undetectably.
///
/// Returns true when the range is gone (or was empty). Returns false when
/// the platform/filesystem cannot do it — caller falls back to truncate
/// with a documented residual micro-window.
#[cfg(target_os = "linux")]
fn collapse_stale_range(fd: &std::fs::File, offset: u64, len: u64) -> bool {
    use std::os::fd::AsRawFd;
    if len == 0 {
        return true;
    }
    const FALLOC_FL_COLLAPSE_RANGE: libc::c_int = 0x08;
    let r = unsafe {
        libc::fallocate(
            fd.as_raw_fd(),
            FALLOC_FL_COLLAPSE_RANGE,
            offset as libc::off_t,
            len as libc::off_t,
        )
    };
    r == 0
}

/// Non-collapse platforms always report unavailable; the caller falls back
/// to truncate with a documented residual micro-window.
#[cfg(not(target_os = "linux"))]
fn collapse_stale_range(_fd: &std::fs::File, _offset: u64, _len: u64) -> bool {
    false
}

/// Recover the folder owning `file_rel` for placement reporting.
fn pending_basename_path(file_rel: &str, basename: &str) -> String {
    if file_rel == ".index" {
        basename.to_string()
    } else if let Some(folder) = file_rel.strip_suffix("/.index") {
        format!("{folder}/{basename}")
    } else {
        basename.to_string()
    }
}

/// Scan a notebook-relative `.index` on disk for `basename`; blank lines are
/// skipped (deleted ids stay dead, never reused).
fn index_id_on_disk(notebook_root: &Path, rel_path: &str) -> Option<u32> {
    let (folder, basename) = split_folder_basename(rel_path);
    index_id_in_folder(notebook_root, &folder, &basename)
}

/// Numeric id of `basename` in `folder`'s on-disk `.index`, if listed.
pub(crate) fn index_id_in_folder(
    notebook_root: &Path,
    folder: &str,
    basename: &str,
) -> Option<u32> {
    let abs = notebook_root.join(index_rel_for_folder(folder));
    let bytes = std::fs::read(abs).ok()?;
    parse_index_lines(&bytes)
        .iter()
        .position(|l| l == basename)
        .map(|p| p as u32 + 1)
}
