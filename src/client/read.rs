use std::path::{Path, PathBuf};

use crate::argv::search_command_args;
use crate::diagnostics::is_selector_not_found;
use crate::error::NbError;
use crate::fingerprint;
use crate::gate;
use crate::lines::{
    document_lines, note_line_from_body_line, require_contiguous_body, search_lines,
};
use crate::output::strip_empty_result_hint;
use crate::parser::{ParseContext, parse};
use crate::transaction;
use crate::types::{BodyFragment, NoteTarget, SearchNoteLines, ShowNote, ShowNoteLines};
use crate::validate::validate_folder_option;
use crate::{NbClient, SearchMode};

use super::core::ShowClassification;

impl NbClient {
    /// Returns status information about the resolved notebook.
    pub async fn show_notebook_status(&self, notebook: Option<&str>) -> Result<String, NbError> {
        let notebook = self.resolve_notebook(notebook).await?;
        self.exec_vec(vec![format!("{}:", notebook), "status".to_string()])
            .await
    }

    /// Lists available notebooks.
    pub async fn list_notebooks(&self) -> Result<String, NbError> {
        let _g = gate::acquire_global(self.gate_timeout).await?;
        // Use --no-color to avoid ANSI escape codes
        self.exec(&["notebooks", "--no-color"]).await
    }

    /// Returns the path for a notebook.
    pub async fn show_notebook_path(&self, notebook: Option<&str>) -> Result<PathBuf, NbError> {
        let notebook = self.resolve_notebook_name(notebook)?;
        self.ensure_notebook(&notebook).await?;
        self.with_notebook_gate(
            &notebook,
            self.show_notebook_path_unguarded(Some(&notebook)),
        )
        .await
    }

    /// Shows a note as a structured [`ShowNote`] (gate-held).
    pub async fn show_note(&self, id: &str, notebook: Option<&str>) -> Result<ShowNote, NbError> {
        let (notebook, selector) = self.resolve_target_selector(id, notebook).await?;
        self.with_notebook_gate(&notebook, async {
            self.show_note_inner(&notebook, &selector).await
        })
        .await
    }

    async fn show_note_inner(&self, notebook: &str, selector: &str) -> Result<ShowNote, NbError> {
        match self.probe_show_classification(selector).await {
            ShowClassification::NonTextual { actual_type } => {
                return Err(NbError::UnsupportedShowTarget {
                    selector: selector.to_string(),
                    actual_type,
                });
            }
            ShowClassification::Textual | ShowClassification::ProbeFailure => {}
        }
        let path = self.resolve_item_path(selector).await?;
        let root = self.show_notebook_path_unguarded(Some(notebook)).await?;
        let rel = path_relative_to(&root, &path)?;
        let source = std::fs::read(&path).map_err(|e| NbError::Io {
            path: path.clone(),
            source: e.into(),
        })?;
        let doc = match parse(&source, ParseContext::FromPath(PathBuf::from(&rel))) {
            Ok(doc) => doc,
            // nb may classify additional textual extensions as showable; map
            // unrecognized formats to a Note partition for structured show.
            Err(NbError::UnsupportedDocumentFormat { .. }) => {
                parse(&source, ParseContext::Explicit(crate::DocumentKind::Note))?
            }
            Err(err) => return Err(err),
        };
        let kind_str = format!("{:?}", doc.kind()).to_lowercase();
        let non_utf8 = |path: String| NbError::NonUtf8 {
            selector: selector.to_string(),
            path,
            kind: kind_str.clone(),
            mime_hint: None,
        };
        let source_str = std::str::from_utf8(&source)
            .map_err(|_| non_utf8(rel.clone()))?
            .to_string();
        // Fragment offsets into `source`: body_ranges are source-absolute.
        let ranges = doc.body_ranges();
        let mut fragments: Vec<BodyFragment> = Vec::with_capacity(ranges.len());
        for (i, span) in ranges.iter().enumerate() {
            let bytes = std::str::from_utf8(&source[span.clone()])
                .map_err(|_| non_utf8(rel.clone()))?
                .to_string();
            fragments.push(BodyFragment {
                index: i as u32,
                bytes,
                start_byte: span.start as u32,
                end_byte: span.end as u32,
            });
        }
        let body_contiguous = fragments.len() <= 1;
        let body_bytes = doc.body_bytes();
        let body = std::str::from_utf8(&body_bytes)
            .map_err(|_| non_utf8(rel.clone()))?
            .to_string();
        // Lossy convenience strings; raw title bytes remain authoritative.
        let tags: Vec<String> = doc
            .tags()
            .map(|t| {
                String::from_utf8_lossy(t)
                    .trim_start_matches('#')
                    .to_string()
            })
            .collect();
        let title = doc
            .title()
            .map(|bytes| std::str::from_utf8(bytes).map(str::to_string))
            .transpose()
            .map_err(|_| non_utf8(rel.clone()))?;
        let title_text = doc.title().map(|bytes| {
            let s = String::from_utf8_lossy(bytes);
            s.trim_end_matches('\n')
                .trim_start_matches('#')
                .trim()
                .to_string()
        });
        Ok(ShowNote {
            selector: numeric_selector_for_path(notebook, &root, &rel, selector),
            path: rel.clone(),
            kind: doc.kind(),
            todo_state: doc.todo_state(),
            title,
            title_text,
            tags,
            body_fragments: fragments,
            body_contiguous,
            body,
            fingerprint: fingerprint::fingerprint(&doc),
            source: source_str,
            numeric_id: numeric_id_for_path(&root, &rel),
        })
    }

    /// Raw-bytes escape hatch: full file bytes without UTF-8 validation.
    ///
    /// This is the only public read that returns `Vec<u8>`; every other
    /// structured read is text-only and returns `NonUtf8` on invalid UTF-8.
    pub async fn read_note_source_bytes(
        &self,
        target: NoteTarget,
        notebook: Option<&str>,
    ) -> Result<Vec<u8>, NbError> {
        let (notebook, selector) = self.resolve_note_target(&target, notebook).await?;
        self.with_notebook_gate(&notebook, async {
            let path = self.resolve_item_path(&selector).await?;
            std::fs::read(&path).map_err(|e| NbError::Io {
                path: path.clone(),
                source: e.into(),
            })
        })
        .await
    }

    /// Enumerate body lines for a contiguous-body note.
    pub async fn show_note_lines(
        &self,
        target: NoteTarget,
        offset: Option<u32>,
        limit: Option<u32>,
        notebook: Option<&str>,
    ) -> Result<ShowNoteLines, NbError> {
        let (notebook, selector) = self.resolve_note_target(&target, notebook).await?;
        self.with_notebook_gate(&notebook, async {
            let shown = self.show_note_inner(&notebook, &selector).await?;
            let source = shown.source.as_bytes().to_vec();
            let doc = parse(&source, ParseContext::FromPath(PathBuf::from(&shown.path)))?;
            let body = require_contiguous_body(&doc)?;
            let (eol, has_final_eol, lines) = document_lines(&body);
            let total_lines = lines.len() as u32;
            let offset = offset.unwrap_or(1);
            let limit = limit.unwrap_or(100);
            if offset == 0 {
                return Err(NbError::InvalidLineWindow {
                    offset,
                    limit,
                    total_lines,
                });
            }
            if total_lines == 0 {
                if offset != 1 {
                    return Err(NbError::InvalidLineWindow {
                        offset,
                        limit,
                        total_lines,
                    });
                }
                return Ok(ShowNoteLines {
                    selector: shown.selector,
                    path: shown.path,
                    kind: shown.kind,
                    total_lines: 0,
                    offset,
                    limit,
                    next_offset: None,
                    lines: Vec::new(),
                    title: shown.title,
                    tags: shown.tags,
                    body_fingerprint: shown.fingerprint,
                    eol: None,
                    has_final_eol: false,
                    numeric_id: shown.numeric_id,
                });
            }
            if offset > total_lines + 1 {
                return Err(NbError::InvalidLineWindow {
                    offset,
                    limit,
                    total_lines,
                });
            }
            let start_idx = (offset - 1) as usize;
            let end_idx = (start_idx + limit as usize).min(lines.len());
            let window: Vec<_> = lines[start_idx..end_idx]
                .iter()
                .map(|l| note_line_from_body_line(l, &body))
                .collect();
            let next_offset = if end_idx < lines.len() {
                Some(offset + window.len() as u32)
            } else {
                None
            };
            Ok(ShowNoteLines {
                selector: shown.selector,
                path: shown.path,
                kind: shown.kind,
                total_lines,
                offset,
                limit,
                next_offset,
                lines: window,
                title: shown.title,
                tags: shown.tags,
                body_fingerprint: shown.fingerprint,
                eol,
                has_final_eol,
                numeric_id: shown.numeric_id,
            })
        })
        .await
    }

    /// Search body line texts for a pattern (contiguous body only).
    pub async fn search_note_lines(
        &self,
        target: NoteTarget,
        pattern: &str,
        notebook: Option<&str>,
    ) -> Result<SearchNoteLines, NbError> {
        let (notebook, selector) = self.resolve_note_target(&target, notebook).await?;
        self.with_notebook_gate(&notebook, async {
            let shown = self.show_note_inner(&notebook, &selector).await?;
            let source = shown.source.as_bytes().to_vec();
            let doc = parse(&source, ParseContext::FromPath(PathBuf::from(&shown.path)))?;
            let body = require_contiguous_body(&doc)?;
            let hits = search_lines(&body, pattern.as_bytes())?;
            Ok(SearchNoteLines {
                selector: shown.selector,
                path: shown.path,
                kind: shown.kind,
                hits,
                body_fingerprint: shown.fingerprint,
            })
        })
        .await
    }

    pub(super) async fn resolve_item_path(&self, selector: &str) -> Result<PathBuf, NbError> {
        let output = self
            .exec_vec(vec![
                "show".to_string(),
                selector.to_string(),
                "--path".to_string(),
                "--no-color".to_string(),
            ])
            .await
            .map_err(|err| match err {
                NbError::CommandFailed { stderr, .. }
                    if is_selector_not_found(&stderr, selector) =>
                {
                    NbError::NotFound {
                        selector: selector.to_string(),
                    }
                }
                other => other,
            })?;
        let path = output.trim();
        if path.is_empty() {
            return Err(NbError::CommandFailed {
                command: format!("nb show {selector} --path"),
                stderr: "empty path".into(),
                exit_code: None,
            });
        }
        Ok(PathBuf::from(path))
    }

    async fn resolve_note_target(
        &self,
        target: &NoteTarget,
        notebook: Option<&str>,
    ) -> Result<(String, String), NbError> {
        match target {
            NoteTarget::Selector { value } => self.resolve_target_selector(value, notebook).await,
            NoteTarget::Path { value } => {
                let notebook = self.resolve_notebook(notebook).await?;
                Ok((notebook.clone(), format!("{notebook}:{value}")))
            }
        }
    }

    /// Probe the textual classification of a selector via `nb`'s
    /// native `--type` mechanism.
    ///
    /// Two-step probe: first `nb show <selector> --type text` to
    /// ask `nb` whether the type is text. If yes, return
    /// [`ShowClassification::Textual`]. If no, follow up with
    /// `nb show <selector> --type` to recover the `actual_type`
    /// for the error diagnostic. If the follow-up also fails
    /// (selector not found, internal error), return
    /// [`ShowClassification::ProbeFailure`] so the caller can
    /// fall through to the original show path.
    async fn probe_show_classification(&self, selector: &str) -> ShowClassification {
        let textual = self
            .exec_vec(vec![
                "show".to_string(),
                selector.to_string(),
                "--type".to_string(),
                "text".to_string(),
                "--no-color".to_string(),
            ])
            .await;
        if textual.is_ok() {
            return ShowClassification::Textual;
        }
        match self
            .exec_vec(vec![
                "show".to_string(),
                selector.to_string(),
                "--type".to_string(),
                "--no-color".to_string(),
            ])
            .await
        {
            Ok(stdout) => {
                let trimmed = stdout.trim();
                if trimmed.is_empty() {
                    ShowClassification::ProbeFailure
                } else {
                    ShowClassification::NonTextual {
                        actual_type: trimmed.to_string(),
                    }
                }
            }
            Err(_) => ShowClassification::ProbeFailure,
        }
    }

    /// Lists notes in a notebook or folder.
    pub async fn list_notes(
        &self,
        folder: Option<&str>,
        tags: &[String],
        limit: Option<u32>,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        let mut args = Vec::new();
        validate_folder_option(folder)?;

        let notebook = self.resolve_notebook(notebook).await?;
        let cmd = match folder {
            Some(f) => format!("{}:{}/", notebook, f),
            None => format!("{}:", notebook),
        };

        args.push("list".to_string());
        args.push(cmd);

        // No color for parsing
        args.push("--no-color".to_string());

        // Limit
        if let Some(n) = limit {
            args.push("-n".to_string());
            args.push(n.to_string());
        }

        // Tags filter
        for tag in tags {
            args.push("--tags".to_string());
            let tag_str = if tag.starts_with('#') {
                tag.clone()
            } else {
                format!("#{}", tag)
            };
            args.push(tag_str);
        }

        // Strip the trailing usage/help hint block from empty
        // results (`0 items.` followed by `Add a note:`,
        // `Import a file:`, `Help information:`). Detection
        // keys off the empty-result signal per the
        // `output-behavior` specification. See `output.rs`
        // for the helper's contract.
        self.exec_vec(args)
            .await
            .map(|output| strip_empty_result_hint(&output))
    }

    /// Searches notes.
    pub async fn search_notes(
        &self,
        queries: &[String],
        mode: SearchMode,
        tags: &[String],
        folder: Option<&str>,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        validate_folder_option(folder)?;
        if queries.is_empty() {
            return Err(NbError::ValidationError {
                reason: "at least one search query is required".to_string(),
                location: None,
            });
        }

        let notebook = self.resolve_notebook(notebook).await?;
        let scope = match folder {
            Some(f) => format!("{}:{}/", notebook, f),
            None => format!("{}:", notebook),
        };
        let args = search_command_args(scope, queries, mode, tags);
        self.exec_vec(args).await
    }
}

/// Numeric `.index` id for a notebook-relative path, if the basename is
/// listed in its folder `.index` (blank/deleted lines never match).
fn numeric_id_for_path(notebook_root: &Path, rel: &str) -> Option<u32> {
    let (folder, basename) = match rel.rfind('/') {
        Some(i) => (rel[..i].to_string(), rel[i + 1..].to_string()),
        None => (String::new(), rel.to_string()),
    };
    transaction::index_id_in_folder(notebook_root, &folder, &basename)
}

/// `ShowNote`/`ShowNoteLines` selector: numeric `<folder>/<id>` form when the
/// path resolves via `.index`, else the qualified selector that was read.
fn numeric_selector_for_path(
    notebook: &str,
    notebook_root: &Path,
    rel: &str,
    fallback: &str,
) -> String {
    match numeric_id_for_path(notebook_root, rel) {
        Some(id) => match rel.rfind('/') {
            Some(i) => format!("{notebook}:{}/{}", &rel[..i], id),
            None => format!("{notebook}:{id}"),
        },
        None => fallback.to_string(),
    }
}

pub(super) fn notebook_dir_from_env(notebook: &str) -> Option<PathBuf> {
    let nb_dir = std::env::var_os("NB_DIR")?;
    let path = PathBuf::from(nb_dir).join(notebook);
    if path.is_dir() && path.join(".git").exists() {
        Some(path)
    } else {
        None
    }
}

pub(super) fn path_relative_to(root: &Path, path: &Path) -> Result<String, NbError> {
    let root = root.canonicalize().map_err(|e| NbError::Io {
        path: root.to_path_buf(),
        source: e.into(),
    })?;
    let path = path.canonicalize().map_err(|e| NbError::Io {
        path: path.to_path_buf(),
        source: e.into(),
    })?;
    let rel = path
        .strip_prefix(&root)
        .map_err(|_| NbError::ValidationError {
            reason: format!(
                "path {} is not under notebook root {}",
                path.display(),
                root.display()
            ),
            location: None,
        })?;
    Ok(rel.to_string_lossy().replace('\\', "/"))
}
