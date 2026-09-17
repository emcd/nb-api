use std::collections::HashSet;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use crate::error::NbError;
use crate::gate;
use crate::git;
use crate::types::{CommitOutcome, OpOutcome};

use super::apply::{OpMeta, PlanEffects, validate_and_apply_virtual};
use super::index::{
    PendingIndexEdit, acquire_index_lock, apply_index_edits, index_id_on_disk,
    index_rel_for_folder, is_index_rel, numeric_selector, reap_stale_index_lock,
    split_folder_basename,
};
use super::plan::annotate_plan_index;
use super::virtual_tree::{
    VirtualNode, VirtualTree, normalize_rel, parent_path, refuse_symlink_ancestors,
};

use super::plan::Transaction;

impl Transaction {
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
