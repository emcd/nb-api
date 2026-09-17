use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::NbError;
use crate::fingerprint::{self, Fingerprint};
use crate::lines::{
    apply_line_edits, apply_substring, require_contiguous_body, splice_body, splice_title,
};
use crate::parser::{DocumentKind, NoteDocument, ParseContext, parse};

use super::index::{
    PendingIndexEdit, append_index_edit, blank_index_edit, move_index_edits, split_folder_basename,
};
use super::plan::PlanOp;
use super::virtual_tree::{
    VirtualTree, outcome_selector, refuse_create_collision, resolve_move_destination,
    resolve_target_path,
};

pub(super) struct OpMeta {
    pub(super) path: Option<String>,
    pub(super) selector: Option<String>,
    pub(super) noop: bool,
    pub(super) fingerprint: Option<Fingerprint>,
    pub(super) index_edits: Vec<PendingIndexEdit>,
}

/// Paths the plan dirties. Materialization touches ONLY these paths: files
/// created after the snapshot (e.g. an external `nb add` racing our commit)
/// are never deleted, and files deleted externally are never resurrected.
#[derive(Debug, Default)]
pub(super) struct PlanEffects {
    /// Delete/move sources: the only paths materialize may remove.
    pub(super) removed: Vec<String>,
    /// Created or modified file paths: the only files materialize writes
    /// (`.index` rels excluded — applied separately under the index lock).
    pub(super) written: Vec<String>,
    /// Paths that must NOT exist on disk at materialize time (creates and
    /// move destinations): a file appearing after the snapshot means an
    /// external writer raced us — refuse with `PathCollision` instead of
    /// overwriting it.
    pub(super) created: Vec<String>,
    /// Folders created by the plan.
    pub(super) mkdirs: Vec<String>,
}

pub(super) fn validate_and_apply_virtual(
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
