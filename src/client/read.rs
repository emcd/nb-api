use std::path::{Path, PathBuf};

use crate::argv::search_command_args;
use crate::diagnostics::is_selector_not_found;
use crate::error::NbError;
use crate::fingerprint;
use crate::gate;
use crate::lines::{
    document_lines, note_line_from_body_line, require_contiguous_body,
    search_lines,
};
use crate::output::strip_empty_result_hint;
use crate::parser::{ParseContext, parse};
use crate::transaction;
use crate::types::{
    BodyFragment, NoteTarget, SearchNoteLines, ShowNote, ShowNoteLines,
};
use crate::validate::validate_folder_option;
use crate::{NbClient, SearchMode};

use super::core::ShowClassification;

impl NbClient {
    /// Returns status information about the resolved notebook.
    ///
    /// When the only notebook dirt is harmless `.index`
    /// reconciliation (see `git::status_is_index_only`), an
    /// explanatory note is appended; the raw `nb` output is
    /// otherwise returned verbatim. The dirt classification runs
    /// BEFORE any `nb` subprocess in this call: `nb` fires
    /// fire-and-forget background checkpoints that could otherwise
    /// commit the very dirt being described between the check and
    /// the read. Root resolution prefers `NB_DIR` and never shells
    /// out when it is set; if the root cannot be resolved, the
    /// verbatim output is returned.
    pub async fn show_notebook_status(
        &self,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        let name = self.resolve_notebook_name(notebook)?;
        // Classify first: an `nb` invocation below may background a
        // checkpoint that reconciles the dirt away mid-call.
        // Discovery failures here must not fail the status read:
        // fall back to verbatim output when the root is unresolvable.
        let index_only = notebook_dir_from_env(&name)
            .ok()
            .flatten()
            .map(|root| {
                crate::git::notebook_status_porcelain(&root)
                    .map(|s| crate::git::status_is_index_only(&s))
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        let notebook = self.resolve_notebook(Some(&name)).await?;
        let output = self
            .exec_vec(vec![format!("{}:", notebook), "status".to_string()])
            .await?;
        if index_only {
            return Ok(format!(
                "{output}\n(note: only .index reconciliation is pending; safe to proceed — no note content affected)"
            ));
        }
        Ok(output)
    }

    /// Lists available notebooks.
    pub async fn list_notebooks(&self) -> Result<String, NbError> {
        let _g = gate::acquire_global(self.gate_timeout).await?;
        // Use --no-color to avoid ANSI escape codes
        self.exec(&["notebooks", "--no-color"]).await
    }

    /// Returns the path for a notebook.
    pub async fn show_notebook_path(
        &self,
        notebook: Option<&str>,
    ) -> Result<PathBuf, NbError> {
        let notebook = self.resolve_notebook_name(notebook)?;
        self.ensure_notebook(&notebook).await?;
        self.with_notebook_gate(
            &notebook,
            self.show_notebook_path_unguarded(Some(&notebook)),
        )
        .await
    }

    /// Shows a note as a structured [`ShowNote`] (gate-held).
    pub async fn show_note(
        &self,
        id: &str,
        notebook: Option<&str>,
    ) -> Result<ShowNote, NbError> {
        // Native read path: validate-only qualifier resolution plus
        // the pinned table, with zero `nb` spawns under `NB_DIR`
        // (flag-gated, default off through 0.5.x). Plain string ids
        // always carry selector semantics.
        if self.native_reads {
            let (notebook, display, input, exact) = self
                .resolve_read_identity(&NoteTarget::selector(id), notebook)
                .await?;
            // No-ensure root BEFORE the gate: the legacy gate helper
            // would ensure (and create) a missing notebook.
            let root = self.native_notebook_root(&notebook).await?;
            return self
                .with_notebook_gate_native(&root, async {
                    self.show_note_native_inside(
                        &notebook, &display, &input, exact, &root,
                    )
                    .await
                })
                .await;
        }
        let (notebook, selector) =
            self.resolve_target_selector(id, notebook).await?;
        self.with_notebook_gate(&notebook, async {
            self.show_note_inner(&notebook, &selector).await
        })
        .await
    }

    /// Validate-only qualifier splitting for native reads: notebook
    /// name, legacy-shaped qualified display, qualifier-stripped
    /// input, and whether the target needs exact (path) or table
    /// (selector) resolution. Performs zero I/O and spawns nothing,
    /// so it runs before the gate; every filesystem touch below
    /// happens inside one gate hold.
    async fn resolve_read_identity(
        &self,
        target: &NoteTarget,
        notebook: Option<&str>,
    ) -> Result<(String, String, String, bool), NbError> {
        // `input` borrows from `target` (which outlives the call)
        // while `name` is owned: no borrow conflict.
        let (name, input, exact): (String, &str, bool) = match target {
            NoteTarget::Selector { value } => {
                match crate::validate::parse_qualified_selector(value)? {
                    Some((embedded, path)) => {
                        if let Some(field) = notebook {
                            crate::validate::validate_notebook_name(field)?;
                            if field != embedded {
                                return Err(NbError::ValidationError {
                                    reason: format!(
                                        "ambiguous selector: id targets notebook `{embedded}`, but notebook field is `{field}`"
                                    ),
                                    location: None,
                                });
                            }
                        }
                        (embedded.to_string(), path, false)
                    },
                    None => (
                        self.resolve_notebook_name(notebook)?,
                        value.as_str(),
                        false,
                    ),
                }
            },
            NoteTarget::Path { value } => {
                (self.resolve_notebook_name(notebook)?, value.as_str(), true)
            },
        };
        let display = format!("{name}:{input}");
        Ok((name, display, input.to_string(), exact))
    }

    /// Map only genuine misses to the display string; I/O failures
    /// propagate unchanged (never degrade into `NotFound`).
    fn map_native_miss(display: &str) -> impl Fn(NbError) -> NbError + '_ {
        move |e| match e {
            NbError::NotFound { .. } => NbError::NotFound {
                selector: display.to_string(),
            },
            other => other,
        }
    }

    /// Native root lookup: `NB_DIR` only, never creates and never
    /// spawns. A missing notebook is the legacy-shaped qualified
    /// `NotFound`: reads must not create notebooks as a side effect.
    /// Genuine filesystem failures (permissions, I/O) propagate as
    /// `Io` instead of collapsing into `NotFound`.
    async fn native_notebook_root(
        &self,
        notebook: &str,
    ) -> Result<PathBuf, NbError> {
        match notebook_dir_from_env(notebook)? {
            Some(path) => Ok(path),
            None => Err(NbError::NotFound {
                selector: format!("{notebook}:"),
            }),
        }
    }

    /// Native inside-gate read: variant-preserving resolution,
    /// canonical containment, classification, then the native
    /// read-and-parse tail. The caller resolves `root` (no-ensure,
    /// no-spawn) before acquiring the no-ensure native gate;
    /// everything filesystem-touching happens inside that one gate
    /// hold: no interleaved commits can invalidate the lookup or
    /// serve stale content.
    async fn show_note_native_inside(
        &self,
        notebook: &str,
        display: &str,
        input: &str,
        exact: bool,
        root: &Path,
    ) -> Result<ShowNote, NbError> {
        let rel = if exact {
            crate::native::resolve_exact(root, input)
        } else {
            crate::native::resolve_selector(root, input)
        }
        .map_err(Self::map_native_miss(display))?;
        let abs = root.join(&rel);
        let abs = crate::native::confine_canonical(root, &abs, display)?;
        match crate::native::classify_path(&abs, &rel)? {
            crate::native::NativeClass::Folder => {
                return Err(NbError::UnsupportedShowTarget {
                    selector: display.to_string(),
                    actual_type: "folder".to_string(),
                });
            },
            crate::native::NativeClass::NonTextual { ext } => {
                return Err(NbError::UnsupportedShowTarget {
                    selector: display.to_string(),
                    actual_type: ext,
                });
            },
            crate::native::NativeClass::Text => {},
        }
        self.show_note_from_path_native(notebook, display, root, &abs)
            .await
    }

    async fn show_note_inner(
        &self,
        notebook: &str,
        selector: &str,
    ) -> Result<ShowNote, NbError> {
        // Fast path: existing directories resolve directly without
        // spawning `nb` (which misdispatches subcommand-colliding
        // names like `todos`, `notebooks`, `list`; see
        // nb-mcp-server:todos/mcp/62). Bypass also skips the probe's
        // reconciling CLI reads. Falls back to the probe + `nb show
        // --path` path for numeric ids/titles when the filesystem
        // entry is absent.
        if let Some(rel) = selector
            .rsplit_once(':')
            .map(|(_, rest)| rest)
            .filter(|rest| !rest.is_empty())
            && let Ok(root) =
                self.show_notebook_path_unguarded(Some(notebook)).await
            && root.join(rel).is_dir()
        {
            return Err(NbError::UnsupportedShowTarget {
                selector: selector.to_string(),
                actual_type: "folder".to_string(),
            });
        }
        match self.probe_show_classification(selector).await {
            ShowClassification::NonTextual { actual_type } => {
                return Err(NbError::UnsupportedShowTarget {
                    selector: selector.to_string(),
                    actual_type,
                });
            },
            ShowClassification::Textual | ShowClassification::ProbeFailure => {
            },
        }
        let path = self.resolve_item_path(selector).await?;
        if path.is_dir() {
            return Err(NbError::UnsupportedShowTarget {
                selector: selector.to_string(),
                actual_type: "folder".to_string(),
            });
        }
        let root = self.show_notebook_path_unguarded(Some(notebook)).await?;
        self.show_note_from_path(notebook, selector, &root, &path)
            .await
    }

    /// Shared read-and-parse tail: canonical rel, source bytes,
    /// document partition, and structured [`ShowNote`] assembly.
    /// Diagnostic errors (e.g. `NonUtf8`) carry the requested
    /// selector; only a successful payload carries the numeric
    /// canonical form. Native callers use
    /// `show_note_from_path_native` instead.
    async fn show_note_from_path(
        &self,
        notebook: &str,
        selector: &str,
        root: &Path,
        path: &Path,
    ) -> Result<ShowNote, NbError> {
        let rel = path_relative_to(root, path)?;
        let source = std::fs::read(path).map_err(|e| NbError::Io {
            path: path.to_path_buf(),
            source: e.into(),
        })?;
        let canonical =
            numeric_selector_for_path(notebook, root, &rel, selector);
        let numeric_id = numeric_id_for_path(root, &rel);
        Self::assemble_show_note(selector, &canonical, numeric_id, rel, source)
    }

    /// Native read-and-parse tail: same assembly, but the success
    /// selector is the shadow-checked canonical form and the numeric
    /// id comes from the confined fallible lookup (symlink-checked,
    /// I/O propagating) instead of the shared legacy helper.
    /// Diagnostics still carry the requested display selector.
    async fn show_note_from_path_native(
        &self,
        notebook: &str,
        display: &str,
        root: &Path,
        path: &Path,
    ) -> Result<ShowNote, NbError> {
        let rel = path_relative_to(root, path)?;
        let source = std::fs::read(path).map_err(|e| NbError::Io {
            path: path.to_path_buf(),
            source: e.into(),
        })?;
        let canonical =
            crate::native::canonical_selector(notebook, root, &rel)?;
        let numeric_id = crate::native::numeric_id_for_rel(root, &rel)?;
        Self::assemble_show_note(display, &canonical, numeric_id, rel, source)
    }

    /// Structured [`ShowNote`] assembly from resolved parts: document
    /// partition, fragments, titles, and fingerprint. `diagnostic`
    /// is the requested selector for error payloads; `canonical` is
    /// the success-payload selector.
    fn assemble_show_note(
        diagnostic: &str,
        canonical: &str,
        numeric_id: Option<u32>,
        rel: String,
        source: Vec<u8>,
    ) -> Result<ShowNote, NbError> {
        let doc = match parse(
            &source,
            ParseContext::FromPath(PathBuf::from(&rel)),
        ) {
            Ok(doc) => doc,
            // nb may classify additional textual extensions as showable; map
            // unrecognized formats to a Note partition for structured show.
            Err(NbError::UnsupportedDocumentFormat { .. }) => parse(
                &source,
                ParseContext::Explicit(crate::DocumentKind::Note),
            )?,
            Err(err) => return Err(err),
        };
        let kind_str = format!("{:?}", doc.kind()).to_lowercase();
        let non_utf8 = |path: String| NbError::NonUtf8 {
            selector: diagnostic.to_string(),
            path,
            kind: kind_str.clone(),
            mime_hint: None,
        };
        let source_str = std::str::from_utf8(&source)
            .map_err(|_| non_utf8(rel.clone()))?
            .to_string();
        // Fragment offsets into `source`: body_ranges are source-absolute.
        let ranges = doc.body_ranges();
        let mut fragments: Vec<BodyFragment> =
            Vec::with_capacity(ranges.len());
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
            selector: canonical.to_string(),
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
            numeric_id,
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
        // Native read path: validate-only qualifier splitting, then
        // variant-preserving resolution with canonical containment
        // inside one gate hold (escapes read as `NotFound`, never
        // foreign bytes), zero `nb` spawns.
        if self.native_reads {
            let (notebook, display, input, exact) =
                self.resolve_read_identity(&target, notebook).await?;
            // No-ensure root BEFORE the gate: the legacy gate helper
            // would ensure (and create) a missing notebook.
            let root = self.native_notebook_root(&notebook).await?;
            return self
                .with_notebook_gate_native(&root, async {
                    let rel = if exact {
                        crate::native::resolve_exact(&root, &input)
                    } else {
                        crate::native::resolve_selector(&root, &input)
                    }
                    .map_err(Self::map_native_miss(&display))?;
                    let abs = root.join(&rel);
                    let abs = crate::native::confine_canonical(
                        &root, &abs, &display,
                    )?;
                    std::fs::read(&abs).map_err(|e| NbError::Io {
                        path: abs.clone(),
                        source: e.into(),
                    })
                })
                .await;
        }
        let (notebook, selector) =
            self.resolve_note_target(&target, notebook).await?;
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
        // Native read path: validate-only qualifier splitting, then
        // variant-preserving resolution plus the shared native show
        // tail inside one gate hold, zero `nb` spawns.
        if self.native_reads {
            let (notebook, display, input, exact) =
                self.resolve_read_identity(&target, notebook).await?;
            // No-ensure root BEFORE the gate: the legacy gate helper
            // would ensure (and create) a missing notebook.
            let root = self.native_notebook_root(&notebook).await?;
            return self
                .with_notebook_gate_native(&root, async {
                    let shown = self
                        .show_note_native_inside(
                            &notebook, &display, &input, exact, &root,
                        )
                        .await?;
                    Self::show_lines_window(shown, offset, limit)
                })
                .await;
        }
        let (notebook, selector) =
            self.resolve_note_target(&target, notebook).await?;
        self.with_notebook_gate(&notebook, async {
            let shown = self.show_note_inner(&notebook, &selector).await?;
            Self::show_lines_window(shown, offset, limit)
        })
        .await
    }

    /// Window one contiguous body into [`ShowNoteLines`].
    /// Pure projection over an already-read [`ShowNote`].
    fn show_lines_window(
        shown: ShowNote,
        offset: Option<u32>,
        limit: Option<u32>,
    ) -> Result<ShowNoteLines, NbError> {
        let source = shown.source.as_bytes().to_vec();
        let doc = parse(
            &source,
            ParseContext::FromPath(PathBuf::from(&shown.path)),
        )?;
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
    }

    /// Search body line texts for a pattern (contiguous body only).
    pub async fn search_note_lines(
        &self,
        target: NoteTarget,
        pattern: &str,
        notebook: Option<&str>,
    ) -> Result<SearchNoteLines, NbError> {
        // Native read path: see `show_note_lines`.
        if self.native_reads {
            let (notebook, display, input, exact) =
                self.resolve_read_identity(&target, notebook).await?;
            // No-ensure root BEFORE the gate: the legacy gate helper
            // would ensure (and create) a missing notebook.
            let root = self.native_notebook_root(&notebook).await?;
            // `pattern` is borrowed across the gate future; copy the
            // small query string so the future owns its inputs.
            let pattern = pattern.to_string();
            return self
                .with_notebook_gate_native(&root, async {
                    let shown = self
                        .show_note_native_inside(
                            &notebook, &display, &input, exact, &root,
                        )
                        .await?;
                    Self::search_lines_hits(shown, &pattern)
                })
                .await;
        }
        let (notebook, selector) =
            self.resolve_note_target(&target, notebook).await?;
        self.with_notebook_gate(&notebook, async {
            let shown = self.show_note_inner(&notebook, &selector).await?;
            Self::search_lines_hits(shown, pattern)
        })
        .await
    }

    /// Search one contiguous body for a pattern.
    /// Pure projection over an already-read [`ShowNote`].
    fn search_lines_hits(
        shown: ShowNote,
        pattern: &str,
    ) -> Result<SearchNoteLines, NbError> {
        let source = shown.source.as_bytes().to_vec();
        let doc = parse(
            &source,
            ParseContext::FromPath(PathBuf::from(&shown.path)),
        )?;
        let body = require_contiguous_body(&doc)?;
        let hits = search_lines(&body, pattern.as_bytes())?;
        Ok(SearchNoteLines {
            selector: shown.selector,
            path: shown.path,
            kind: shown.kind,
            hits,
            body_fingerprint: shown.fingerprint,
        })
    }

    pub(super) async fn resolve_item_path(
        &self,
        selector: &str,
    ) -> Result<PathBuf, NbError> {
        // Fast path: existing filesystem entries (files AND
        // directories) resolve directly without spawning `nb`, which
        // misdispatches selectors whose trailing component collides
        // with subcommand vocabulary (`todos` -> tasks listing,
        // `notebooks` -> notebook root, `list` -> folder listing;
        // nb-mcp-server:todos/mcp/62). Falls back to `nb show --path`
        // for numeric ids/titles that have no direct filesystem form.
        if let Ok(Some((notebook, rel))) =
            crate::validate::parse_qualified_selector(selector)
            && !rel.is_empty()
            && let Ok(root) =
                self.show_notebook_path_unguarded(Some(notebook)).await
        {
            let direct = root.join(rel);
            if direct.is_file() || direct.is_dir() {
                return Ok(direct);
            }
        }
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
                },
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
            NoteTarget::Selector { value } => {
                self.resolve_target_selector(value, notebook).await
            },
            NoteTarget::Path { value } => {
                let notebook = self.resolve_notebook(notebook).await?;
                Ok((notebook.clone(), format!("{notebook}:{value}")))
            },
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
    async fn probe_show_classification(
        &self,
        selector: &str,
    ) -> ShowClassification {
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
            },
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

pub(super) fn notebook_dir_from_env(
    notebook: &str,
) -> Result<Option<PathBuf>, NbError> {
    let Some(nb_dir) = std::env::var_os("NB_DIR") else {
        return Ok(None);
    };
    let path = PathBuf::from(nb_dir).join(notebook);
    // `metadata` follows symlinks exactly like the previous
    // `is_dir` check; only genuine misses map to `None` while
    // permission and I/O failures propagate instead of collapsing
    // into `NotFound`.
    match std::fs::metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(NbError::Io {
            path: path.clone(),
            source: e.into(),
        }),
        Ok(m) if !m.is_dir() => Ok(None),
        Ok(_) => match std::fs::metadata(path.join(".git")) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(NbError::Io {
                path: path.join(".git"),
                source: e.into(),
            }),
            Ok(_) => Ok(Some(path)),
        },
    }
}

pub(super) fn path_relative_to(
    root: &Path,
    path: &Path,
) -> Result<String, NbError> {
    let root = root.canonicalize().map_err(|e| NbError::Io {
        path: root.to_path_buf(),
        source: e.into(),
    })?;
    let path = path.canonicalize().map_err(|e| NbError::Io {
        path: path.to_path_buf(),
        source: e.into(),
    })?;
    let rel =
        path.strip_prefix(&root)
            .map_err(|_| NbError::ValidationError {
                reason: format!(
                    "path {} is not under notebook root {}",
                    path.display(),
                    root.display()
                ),
                location: None,
            })?;
    // On Windows the canonical form uses `\` separators, which
    // are normalized here. On Unix a backslash is an ordinary
    // filename character (e.g. a literal `a\b` file) and must be
    // preserved verbatim.
    #[cfg(windows)]
    {
        Ok(rel.to_string_lossy().replace('\\', "/"))
    }
    #[cfg(not(windows))]
    {
        Ok(rel.to_string_lossy().into_owned())
    }
}
