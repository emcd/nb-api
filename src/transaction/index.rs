use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::error::NbError;

use super::virtual_tree::normalize_rel;

/// One `.index` mutation derived from a validated plan op.
///
/// `file_rel` is the notebook-relative `.index` path (`.index` at root,
/// `<folder>/.index` otherwise). Applied to fresh on-disk content under the
/// `.nb-api-index.lock`, in plan order, after non-index files materialize.
#[derive(Debug, Clone)]
pub(super) struct PendingIndexEdit {
    pub(super) file_rel: String,
    pub(super) kind: PendingIndexKind,
}

#[derive(Debug, Clone)]
pub(super) enum PendingIndexKind {
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

/// Slug a title the way `nb` 7.24.0 does (ASCII locale): ASCII `A-Z` is
/// lowercased, non-ASCII is preserved verbatim, ASCII non-alphanumeric maps
/// to `_`, runs collapse, leading/trailing `_` trim.
fn mangle_stem(title: &str) -> String {
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
fn titleless_stem() -> String {
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
pub(super) fn split_folder_basename(path: &str) -> (String, String) {
    match path.rfind('/') {
        Some(i) => (path[..i].to_string(), path[i + 1..].to_string()),
        None => (String::new(), path.to_string()),
    }
}

/// Notebook-relative `.index` path for a folder (`""` -> `.index`).
pub(super) fn index_rel_for_folder(folder: &str) -> String {
    if folder.is_empty() {
        ".index".to_string()
    } else {
        format!("{folder}/.index")
    }
}

pub(super) fn append_index_edit(path: &str) -> PendingIndexEdit {
    let (folder, basename) = split_folder_basename(path);
    PendingIndexEdit {
        file_rel: index_rel_for_folder(&folder),
        kind: PendingIndexKind::Append { basename },
    }
}

pub(super) fn blank_index_edit(path: &str) -> PendingIndexEdit {
    let (folder, basename) = split_folder_basename(path);
    PendingIndexEdit {
        file_rel: index_rel_for_folder(&folder),
        kind: PendingIndexKind::Blank { basename },
    }
}

pub(super) fn move_index_edits(from: &str, to: &str) -> Vec<PendingIndexEdit> {
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
pub(super) fn parse_index_lines(bytes: &[u8]) -> Vec<String> {
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

pub(super) fn render_index_lines(lines: &[String]) -> Vec<u8> {
    if lines.is_empty() {
        return Vec::new();
    }
    let mut out = lines.join("\n").into_bytes();
    out.push(b'\n');
    out
}

/// Numeric selector for a post-write placement (`nb:3`, `nb:work/3`).
pub(super) fn numeric_selector(notebook: &str, folder: &str, id: u32) -> String {
    if folder.is_empty() {
        format!("{notebook}:{id}")
    } else {
        format!("{notebook}:{folder}/{id}")
    }
}

/// Notebook-relative `.index` files are managed post-materialize under the
/// `.nb-api-index.lock`, so the generic tree write must not clobber them
/// with its stale snapshot.
pub(super) fn is_index_rel(rel: &str) -> bool {
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
pub(super) struct IndexLockGuard {
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
pub(super) fn reap_stale_index_lock(notebook_root: &Path, timeout: Duration) {
    let path = index_lock_path(notebook_root);
    if path.exists() {
        try_remove_lock_if_stale(&path, timeout);
    }
}

pub(super) async fn acquire_index_lock(
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
pub(super) struct IndexPlacement {
    pub(super) op_index: u32,
    pub(super) folder: String,
    pub(super) id: u32,
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
pub(super) fn apply_index_edits(
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
pub(super) fn index_id_on_disk(notebook_root: &Path, rel_path: &str) -> Option<u32> {
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
