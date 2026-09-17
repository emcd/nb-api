use std::collections::HashSet;
use std::time::Duration;

use crate::NbClient;
use crate::error::NbError;
use crate::fingerprint::Fingerprint;
use crate::types::{LineEdit, NoteTarget, Occurrence};
use std::path::Path;

use super::virtual_tree::normalize_rel;

/// In-memory plan bound to one notebook. Drop discards; no begin/rollback.
pub struct Transaction {
    pub(super) client: NbClient,
    pub(super) notebook: String,
    pub(super) plan: Vec<PlanOp>,
    pub(super) gate_timeout: Duration,
}

#[derive(Debug, Clone)]
pub(super) enum PlanOp {
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
}
pub(super) fn annotate_plan_index(err: NbError, index: u32) -> NbError {
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
