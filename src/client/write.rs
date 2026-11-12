use std::collections::VecDeque;

use crate::argv::{
    child_folder_names, empty_tasks_signal, is_empty_tasks_error,
    normalize_folder, tasks_scope, todos_tasks_command_args,
};
use crate::error::NbError;
use crate::fingerprint::Fingerprint;
use crate::output::strip_empty_result_hint;
use crate::transaction;
use crate::transaction::Transaction;
use crate::types::{CommitOutcome, LineEdit, NoteTarget, Occurrence};
use crate::validate::{
    detect_duplicate_title_heading, validate_destination,
    validate_folder_option, validate_folder_path,
};
use crate::{NbClient, TaskStatus};

impl NbClient {
    /// Creates a new note (one-shot transaction; nb-mangled filename).
    pub async fn add_note(
        &self,
        title: Option<&str>,
        content: &str,
        tags: &[String],
        folder: Option<&str>,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        if let Some(t) = title
            && let Some(heading) = detect_duplicate_title_heading(t, content)
        {
            return Err(NbError::DuplicateTitleHeading {
                title: t.to_string(),
                heading,
            });
        }
        self.require_folder_for_new_note(folder)?;
        validate_folder_option(folder)?;
        let filename = transaction::filename_for_title(title, "md");
        self.create_with_retry(notebook, folder, filename, |tx, path| {
            tx.add_note(&path, title, content, tags)
        })
        .await
    }

    /// One-shot create with `-N` collision retry on the auto filename.
    ///
    /// Explicit-path `Transaction` plans keep hard `PathCollision` errors;
    /// the mangled/titleless one-shot names instead gain `-1`, `-2`, …
    /// until free (basenames stay unique per folder, which keeps post-write
    /// `.index` lookup unambiguous).
    async fn create_with_retry(
        &self,
        notebook: Option<&str>,
        folder: Option<&str>,
        filename: String,
        plan: impl Fn(&mut Transaction, String) -> Result<(), NbError>,
    ) -> Result<CommitOutcome, NbError> {
        let mut name = filename.clone();
        for attempt in 0..100u32 {
            if attempt > 0 {
                name = transaction::suffixed_filename(&filename, attempt);
            }
            let path = transaction::join_folder_file(folder, &name);
            let mut tx = self.transaction(notebook).await?;
            plan(&mut tx, path)?;
            match tx.commit().await {
                Ok(outcome) => return Ok(outcome),
                Err(NbError::PathCollision { .. }) => continue,
                Err(other) => return Err(other),
            }
        }
        Err(NbError::PathCollision {
            path: transaction::join_folder_file(folder, &filename),
            plan_index: None,
        })
    }

    /// Deletes a note (one-shot transaction).
    pub async fn delete_note(
        &self,
        id: &str,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        let (nb, selector) =
            self.resolve_target_selector(id, notebook).await?;
        let target = self.note_target_for_selector(&nb, &selector).await?;
        let mut tx = self.transaction(Some(&nb)).await?;
        tx.delete_note(target)?;
        tx.commit().await
    }

    /// Deletes a folder (one-shot transaction).
    ///
    /// `recursive = false` refuses non-empty folders, listing their
    /// contents; `recursive = true` removes the whole subtree. There
    /// is no `force` flag: deletion is explicit by call.
    pub async fn delete_folder(
        &self,
        id: &str,
        recursive: bool,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        let (nb, selector) =
            self.resolve_target_selector(id, notebook).await?;
        let target = self.note_target_for_selector(&nb, &selector).await?;
        let mut tx = self.transaction(Some(&nb)).await?;
        tx.delete_folder(target, recursive)?;
        tx.commit().await
    }

    /// Moves or renames a note (one-shot transaction).
    pub async fn move_note(
        &self,
        id: &str,
        destination: &str,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        validate_destination(destination)?;
        let (nb, selector) =
            self.resolve_target_selector(id, notebook).await?;
        let target = self.note_target_for_selector(&nb, &selector).await?;
        let mut tx = self.transaction(Some(&nb)).await?;
        tx.move_note(target, destination)?;
        tx.commit().await
    }

    /// Resolve a write one-shot selector to a [`NoteTarget`]:
    /// direct filesystem hit first, else the native pinned table
    /// (numeric id, then root title) inside one no-ensure gate hold.
    /// Spawns zero `nb` subprocesses; notebook existence is ensured
    /// upstream by the caller. Missing inputs are terminal
    /// `NotFound` with the original selector.
    async fn note_target_for_selector(
        &self,
        notebook: &str,
        selector: &str,
    ) -> Result<NoteTarget, NbError> {
        let root = self.show_notebook_path_unguarded(Some(notebook)).await?;
        let stripped = selector
            .rsplit_once(':')
            .map(|(_, rest)| rest)
            .unwrap_or(selector);
        if stripped.is_empty() {
            return Ok(NoteTarget::path(stripped));
        }
        let direct = root.join(stripped);
        if direct.is_file() || direct.is_dir() {
            return Ok(NoteTarget::path(stripped));
        }
        self.with_notebook_gate_native(&root, async {
            let rel = crate::native::resolve_selector(&root, stripped)
                .map_err(|e| match e {
                    NbError::NotFound { .. } => NbError::NotFound {
                        selector: selector.to_string(),
                    },
                    other => other,
                })?;
            Ok(NoteTarget::path(rel))
        })
        .await
    }

    /// Creates a todo item (one-shot transaction; nb-mangled filename).
    pub async fn add_todo(
        &self,
        title: &str,
        description: Option<&str>,
        tasks: &[String],
        tags: &[String],
        folder: Option<&str>,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        self.require_folder_for_new_note(folder)?;
        validate_folder_option(folder)?;
        let filename = transaction::filename_for_title(Some(title), "todo.md");
        self.create_with_retry(notebook, folder, filename, |tx, path| {
            tx.add_todo(&path, title, description, tasks, tags)
        })
        .await
    }

    /// Marks a todo as done (one-shot transaction).
    pub async fn mark_task_done(
        &self,
        id: &str,
        task_number: Option<u32>,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        let (nb, selector) =
            self.resolve_target_selector(id, notebook).await?;
        let target = self.note_target_for_selector(&nb, &selector).await?;
        let mut tx = self.transaction(Some(&nb)).await?;
        tx.mark_task_done(target, task_number)?;
        tx.commit().await
    }

    /// Marks a todo as not done (one-shot transaction).
    pub async fn unmark_task_done(
        &self,
        id: &str,
        task_number: Option<u32>,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        let (nb, selector) =
            self.resolve_target_selector(id, notebook).await?;
        let target = self.note_target_for_selector(&nb, &selector).await?;
        let mut tx = self.transaction(Some(&nb)).await?;
        tx.unmark_task_done(target, task_number)?;
        tx.commit().await
    }

    /// Replace contiguous body text (one-shot).
    pub async fn replace_note_body(
        &self,
        target: NoteTarget,
        new_body: &str,
        fingerprint: Fingerprint,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        let mut tx = self.transaction(notebook).await?;
        tx.replace_note_body(target, new_body, fingerprint)?;
        tx.commit().await
    }

    /// Substring edit on contiguous body (one-shot).
    #[allow(clippy::too_many_arguments)]
    pub async fn edit_note_substring(
        &self,
        target: NoteTarget,
        pattern: &str,
        replacement: &str,
        occurrence: Occurrence,
        expected_count: u32,
        fingerprint: Option<Fingerprint>,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        let mut tx = self.transaction(notebook).await?;
        tx.edit_note_substring(
            target,
            pattern,
            replacement,
            occurrence,
            expected_count,
            fingerprint,
        )?;
        tx.commit().await
    }

    /// Line-oriented body edit batch (one-shot).
    pub async fn edit_note_lines(
        &self,
        target: NoteTarget,
        edits: Vec<LineEdit>,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        let mut tx = self.transaction(notebook).await?;
        tx.edit_note_lines(target, edits)?;
        tx.commit().await
    }

    /// Retitle a note without changing path (one-shot).
    pub async fn retitle_note(
        &self,
        target: NoteTarget,
        title: &str,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        let mut tx = self.transaction(notebook).await?;
        tx.retitle_note(target, title)?;
        tx.commit().await
    }

    /// Add/remove tags (one-shot).
    pub async fn edit_note_tags(
        &self,
        target: NoteTarget,
        add: &[String],
        remove: &[String],
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        let mut tx = self.transaction(notebook).await?;
        tx.edit_note_tags(target, add, remove)?;
        tx.commit().await
    }

    /// Lists checklist items within todos.
    ///
    /// Invokes `nb todos tasks` per scope. The method enumerates
    /// the checklist items within todos (and recursively into
    /// subfolders when `recursive = true`), filtered by
    /// `status` if provided. An empty scope returns the empty
    /// signal (`0 open tasks.`) rather than an error. A future
    /// `list_todos` method for the todo **container** listing
    /// (invoking `nb todos`) is tracked at
    /// `nb-api:todos/api/5` (deferred to `0.5.0`).
    pub async fn list_tasks(
        &self,
        folder: Option<&str>,
        status: Option<TaskStatus>,
        recursive: bool,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        validate_folder_option(folder)?;
        let notebook = self.resolve_notebook(notebook).await?;
        let folder = folder.map(normalize_folder);
        let scopes = if recursive {
            self.tasks_scopes_recursive(&notebook, folder.as_deref())
                .await?
        } else {
            vec![tasks_scope(&notebook, folder.as_deref())]
        };

        let mut outputs: Vec<String> = Vec::new();
        let mut saw_empty = false;
        let mut empty_signal: Option<String> = None;
        for scope in scopes {
            match self.exec_vec(todos_tasks_command_args(scope, status)).await
            {
                Ok(output) => {
                    let output = output.trim();
                    if !output.is_empty() {
                        outputs.push(output.to_string());
                    }
                },
                Err(NbError::CommandFailed { stderr, .. })
                    if is_empty_tasks_error(&stderr) =>
                {
                    saw_empty = true;
                    empty_signal = Some(empty_tasks_signal(&stderr));
                },
                Err(err) => return Err(err),
            }
        }
        if outputs.is_empty() && saw_empty {
            // An empty scope is an empty list, not a failure: return
            // the normalized empty signal (`0 open tasks.`) so
            // callers can enumerate todos notebook-wide without
            // treating "no tasks here" as an error.
            return Ok(empty_signal.unwrap_or_default());
        }
        Ok(outputs.join("\n"))
    }

    /// Lists todo checklist items natively: one [`crate::ListEntry`]
    /// per `- [ ]` / `- [x]` / `- [X]` item in `.todo.md` files
    /// under the scope (recursive iff set), in file order with
    /// 1-based task positions. Selectors append `#task-<n>` to the
    /// shadow-aware canonical file selector; kind is `Todo` and
    /// `numeric_id` rides along from the file. `status` filters by
    /// item state (`None` keeps all). Scopes skip dotfiles and
    /// symlinks; a missing scope is terminal `NotFound`. Zero `nb`
    /// spawns.
    pub async fn list_tasks_typed(
        &self,
        folder: Option<&str>,
        status: Option<TaskStatus>,
        recursive: bool,
        notebook: Option<&str>,
    ) -> Result<Vec<crate::ListEntry>, NbError> {
        validate_folder_option(folder)?;
        let notebook = self.resolve_notebook_name(notebook)?;
        let root = self.native_notebook_root(&notebook).await?;
        let scope = folder
            .map(|f| f.trim_matches('/').to_string())
            .filter(|f| !f.is_empty())
            .unwrap_or_default();
        let want_closed = status.map(|s| matches!(s, TaskStatus::Closed));
        self.with_notebook_gate_native(&root, async {
            let rels = if recursive {
                crate::scope::scope_files_recursive(&root, &scope)?
            } else {
                crate::scope::scope_files(&root, &scope)?
            };
            let mut out = Vec::new();
            for rel in rels {
                if crate::native::document_kind_for_rel(&rel)
                    != crate::DocumentKind::Todo
                {
                    continue;
                }
                let abs = root.join(&rel);
                let bytes = std::fs::read(&abs).map_err(|e| NbError::Io {
                    path: abs.clone(),
                    source: e.into(),
                })?;
                let file = self.list_entry_for(&notebook, &root, &rel).await?;
                for (i, closed) in
                    crate::scope::todo_items(&bytes).into_iter().enumerate()
                {
                    if let Some(want) = want_closed
                        && want != closed
                    {
                        continue;
                    }
                    out.push(crate::ListEntry {
                        selector: format!("{}#task-{}", file.selector, i + 1),
                        path: file.path.clone(),
                        kind: crate::EntityKind::Todo,
                        numeric_id: file.numeric_id,
                    });
                }
            }
            Ok(out)
        })
        .await
    }

    /// Lists todo files as summaries: one [`crate::TodoSummary`] per
    /// `.todo.md` file under the scope (recursive iff set) with task
    /// counts and derived state (`Done` iff at least one item exists
    /// and all are completed, else `Open`). `state` filters files
    /// (`None` keeps all). Titles are checkbox-stripped and trimmed,
    /// falling back to the filename. Zero `nb` spawns.
    pub async fn list_todos(
        &self,
        folder: Option<&str>,
        state: Option<TaskStatus>,
        recursive: bool,
        notebook: Option<&str>,
    ) -> Result<Vec<crate::TodoSummary>, NbError> {
        use crate::parser::{ParseContext, parse};

        validate_folder_option(folder)?;
        let notebook = self.resolve_notebook_name(notebook)?;
        let root = self.native_notebook_root(&notebook).await?;
        let scope = folder
            .map(|f| f.trim_matches('/').to_string())
            .filter(|f| !f.is_empty())
            .unwrap_or_default();
        self.with_notebook_gate_native(&root, async {
            let rels = if recursive {
                crate::scope::scope_files_recursive(&root, &scope)?
            } else {
                crate::scope::scope_files(&root, &scope)?
            };
            let mut out = Vec::new();
            for rel in rels {
                if crate::native::document_kind_for_rel(&rel)
                    != crate::DocumentKind::Todo
                {
                    continue;
                }
                let abs = root.join(&rel);
                let bytes = std::fs::read(&abs).map_err(|e| NbError::Io {
                    path: abs.clone(),
                    source: e.into(),
                })?;
                let items = crate::scope::todo_items(&bytes);
                let completed = items.iter().filter(|&&c| c).count() as u32;
                let total = items.len() as u32;
                let file_state = if total > 0 && completed == total {
                    crate::TodoState::Done
                } else {
                    crate::TodoState::Open
                };
                let is_open = file_state == crate::TodoState::Open;
                match state {
                    Some(TaskStatus::Closed) if is_open => continue,
                    Some(TaskStatus::Open) if !is_open => continue,
                    _ => {},
                }
                let title =
                    parse(&bytes, ParseContext::FromPath(rel.clone().into()))
                        .ok()
                        .and_then(|doc| {
                            doc.title().map(|t| {
                                let s = String::from_utf8_lossy(t);
                                let s = s
                                    .trim_end_matches('\n')
                                    .trim_start_matches('#')
                                    .trim();
                                let s = s
                                    .strip_prefix("[ ]")
                                    .or_else(|| s.strip_prefix("[x]"))
                                    .or_else(|| s.strip_prefix("[X]"))
                                    .unwrap_or(s);
                                s.trim().to_string()
                            })
                        })
                        .filter(|t| !t.is_empty())
                        .unwrap_or_else(|| {
                            rel.rsplit('/').next().unwrap_or(&rel).to_string()
                        });
                let file = self.list_entry_for(&notebook, &root, &rel).await?;
                out.push(crate::TodoSummary {
                    id: file.selector,
                    title,
                    path: rel,
                    task_count: total,
                    completed_count: completed,
                    state: file_state,
                });
            }
            Ok(out)
        })
        .await
    }

    async fn tasks_scopes_recursive(
        &self,
        notebook: &str,
        folder: Option<&str>,
    ) -> Result<Vec<String>, NbError> {
        let notebook_root = self.show_notebook_path(Some(notebook)).await?;
        let start = folder.unwrap_or_default().to_string();
        let mut queue = VecDeque::new();
        queue.push_back(start.clone());

        let mut scopes = vec![tasks_scope(notebook, folder)];
        while let Some(current) = queue.pop_front() {
            let base = if current.is_empty() {
                notebook_root.clone()
            } else {
                notebook_root.join(&current)
            };
            let children = child_folder_names(&base)?;
            for child in children {
                let next = if current.is_empty() {
                    child
                } else {
                    format!("{}/{}", current, child)
                };
                scopes.push(tasks_scope(notebook, Some(&next)));
                queue.push_back(next);
            }
        }
        Ok(scopes)
    }

    /// Creates a bookmark (one-shot transaction; nb-mangled filename).
    pub async fn add_bookmark(
        &self,
        url: &str,
        title: Option<&str>,
        tags: &[String],
        comment: Option<&str>,
        folder: Option<&str>,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        self.require_folder_for_new_note(folder)?;
        validate_folder_option(folder)?;
        let filename = transaction::filename_for_title(title, "bookmark.md");
        self.create_with_retry(notebook, folder, filename, |tx, path| {
            tx.add_bookmark(&path, url, title, tags, comment)
        })
        .await
    }

    /// Lists folders in a notebook.
    pub async fn list_folders(
        &self,
        parent: Option<&str>,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        let mut args = vec!["list".to_string()];
        validate_folder_option(parent)?;

        let notebook = self.resolve_notebook(notebook).await?;
        let path = match parent {
            Some(p) => format!("{}:{}/", notebook, p),
            None => format!("{}:", notebook),
        };
        args.push(path);

        // Filter to only show folders
        args.push("--type".to_string());
        args.push("folder".to_string());
        args.push("--no-color".to_string());

        // Strip the trailing usage/help hint block from empty
        // results (`0 folders.` followed by `Import a file:`,
        // `Help information:`). Detection keys off the
        // empty-result signal per the `output-behavior`
        // specification. See `output.rs` for the helper's
        // contract.
        self.exec_vec(args)
            .await
            .map(|output| strip_empty_result_hint(&output))
    }

    /// Lists folders in a notebook as typed entries: one
    /// [`crate::ListEntry`] per direct child directory of the
    /// scope (non-recursive), excluding dot-directories, ordered
    /// lexically by relative path. Each entry is kind `Folder`
    /// with the exact-path selector and no numeric id. A missing
    /// scope is terminal `NotFound`. Zero `nb` spawns.
    pub async fn list_folders_typed(
        &self,
        parent: Option<&str>,
        notebook: Option<&str>,
    ) -> Result<Vec<crate::ListEntry>, NbError> {
        validate_folder_option(parent)?;
        let notebook = self.resolve_notebook_name(notebook)?;
        let root = self.native_notebook_root(&notebook).await?;
        let scope = parent
            .map(|f| f.trim_matches('/').to_string())
            .filter(|f| !f.is_empty())
            .unwrap_or_default();
        self.with_notebook_gate_native(&root, async {
            let mut out = Vec::new();
            for rel in crate::scope::scope_dirs(&root, &scope)? {
                out.push(self.list_entry_for(&notebook, &root, &rel).await?);
            }
            Ok(out)
        })
        .await
    }

    /// Creates a folder (one-shot transaction).
    pub async fn add_folder(
        &self,
        path: &str,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        validate_folder_path(path)?;
        let mut tx = self.transaction(notebook).await?;
        tx.add_folder(path)?;
        tx.commit().await
    }

    /// Imports a file or URL into the notebook as a note.
    ///
    /// One-shot only under the process-shared notebook gate. Not a
    /// [`Transaction`] plan op in 0.3.0.
    ///
    /// NOTE (0.4.3, evaluated): `nb import` accepts no `--wait`
    /// flag — an unknown flag would be misparsed as an import
    /// path — and a standalone `nb git checkpoint --wait` cannot
    /// join the import's already-backgrounded checkpoint job, so
    /// there is no `--wait` containment for the import race window.
    /// Containment waits on 0.5.0 (native import with proper
    /// locking); meanwhile the commit guard absorbs index-only
    /// droppings and note-content leftovers still refuse loudly.
    pub async fn import_note(
        &self,
        source: &str,
        folder: Option<&str>,
        filename: Option<&str>,
        convert: bool,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        let mut args = Vec::new();
        self.require_folder_for_new_note(folder)?;
        validate_folder_option(folder)?;

        let notebook = self.resolve_notebook(notebook).await?;
        self.with_notebook_gate(&notebook, async {
            let cmd = format!("{}:import", notebook);
            args.push(cmd);
            args.push(source.to_string());
            if convert {
                args.push("--convert".to_string());
            }
            if folder.is_some() || filename.is_some() {
                let dest = match (folder, filename) {
                    (Some(f), Some(n)) => format!("{}/{}", f, n),
                    (Some(f), None) => format!("{}/", f),
                    (None, Some(n)) => n.to_string(),
                    (None, None) => unreachable!(),
                };
                args.push(dest);
            }
            self.exec_vec(args)
                .await
                .map(|output| self.append_notebook_warning(output, &notebook))
        })
        .await
    }
}
