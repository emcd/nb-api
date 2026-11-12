//! Native (nb-spawn-free) read-path resolution and classification.
//!
//! Implements the pinned selector precedence table and nb's textual
//! classification without spawning `nb`. All reads are
//! side-effect-free filesystem plus `.index` scans under the notebook gate.
//!
//! Classification mirrors nb 7.24.0's `_file_is_text` decision order:
//! extension against nb's text set first (no content sniff), then the
//! known non-text sets, then a content sniff standing in for
//! `file -b --mime-type` (`^text`) on unknown extensions. Known
//! residual: UTF-16 content sniffs as non-text here while `file`
//! reports `text/plain`; the differential corpus is nb-written UTF-8,
//! so a corpus hit reopens this rule.

use std::path::{Path, PathBuf};

use crate::error::NbError;

/// nb `_TEXT_FILE_EXTENSIONS` verbatim (case-sensitive, LC_ALL=C).
const TEXT_EXTENSIONS: &[&str] = &[
    "adoc", "asciidoc", "bash", "bib", "coffee", "css", "go", "html", "js",
    "json", "jsx", "latex", "markdown", "md", "mdown", "mdwn", "mdtxt",
    "mdtext", "mkd", "mkdn", "org", "py", "rb", "Rmd", "rst", "scss", "sh",
    "sql", "tex", "text", "textile", "ts", "tsx", "txt", "xml",
];

/// nb `_ARCHIVE_FILE_EXTENSIONS` verbatim.
const ARCHIVE_EXTENSIONS: &[&str] = &[
    "7z",
    "apk",
    "bz2",
    "dmg",
    "gz",
    "lz",
    "rar",
    "s7z",
    "sit",
    "sitx",
    "sparsebundle",
    "tar",
    "tgz",
    "tbz2",
    "tlz",
    "txz",
    "xz",
    "zip",
    "zipx",
    "Z",
];

/// nb `_AUDIO_FILE_EXTENSIONS` verbatim.
const AUDIO_EXTENSIONS: &[&str] =
    &["aac", "aiff", "flac", "m4a", "mp3", "ogg", "wav"];

/// nb `_DOCUMENT_FILE_EXTENSIONS` verbatim.
const DOCUMENT_EXTENSIONS: &[&str] =
    &["doc", "docx", "odt", "pdf", "rtf", "xls", "xlsx"];

/// nb `_ENCRYPTED_FILE_EXTENSIONS` verbatim.
const ENCRYPTED_EXTENSIONS: &[&str] = &["enc"];

/// nb `_IMAGE_FILE_EXTENSIONS` verbatim.
const IMAGE_EXTENSIONS: &[&str] = &[
    "afphoto", "ai", "bmp", "gif", "heic", "ind", "indd", "jpg", "jpeg",
    "png", "psd", "raw", "svg", "svgz", "tif", "tiff", "webp",
];

/// nb `_VIDEO_FILE_EXTENSIONS` verbatim.
const VIDEO_EXTENSIONS: &[&str] = &[
    "avi", "flv", "m4p", "m4v", "mp2", "mp4", "mov", "mpeg", "mpg", "qt",
    "webm", "wmv",
];

/// Native classification of one notebook path.
pub(crate) enum NativeClass {
    /// Textual content (showable).
    Text,
    /// Non-textual file; `ext` is nb's `--type` form (after first dot).
    NonTextual { ext: String },
    /// Directory.
    Folder,
}

/// Text after the last dot, or the whole name when dotless.
/// Mirrors nb `${_file_path##*.}` (byte-exact, case-sensitive).
fn ext_after_last_dot(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

/// Text after the first dot, or the whole name when dotless.
/// Mirrors nb `${_relative_path#*.}` used for `--type` output.
fn ext_after_first_dot(name: &str) -> String {
    match name.find('.') {
        Some(i) => name[i + 1..].to_string(),
        None => name.to_string(),
    }
}

fn is_known_non_text(ext: &str) -> bool {
    ARCHIVE_EXTENSIONS.contains(&ext)
        || AUDIO_EXTENSIONS.contains(&ext)
        || DOCUMENT_EXTENSIONS.contains(&ext)
        || ENCRYPTED_EXTENSIONS.contains(&ext)
        || IMAGE_EXTENSIONS.contains(&ext)
        || VIDEO_EXTENSIONS.contains(&ext)
}

/// Magic numbers identifying binary formats regardless of
/// extension, standing in for the `file` database on unknown
/// extensions only (extension sets always win first, exactly like
/// nb). Every entry is load-bearing and verified against `file -b
/// --mime-type`: pure-printable-ASCII prefixes that `file` treats
/// as binary while the strict text rule below would accept them
/// (`%PDF-`, `GIF87a`/`GIF89a`, `BZh`, `fLaC`, `RIFF....WAVE`,
/// `RIFF....WEBP`). Short ambiguous prefixes (`BM`, `FORM`, plain
/// `MZ`) and structure-checked ones (`....ftyp`, `ustar` at 257)
/// are deliberately absent: `file` reports them as text, and the
/// strict rule agrees.
fn magic_non_text(bytes: &[u8]) -> bool {
    const MAGIC: &[&[u8]] = &[b"%PDF-", b"GIF87a", b"GIF89a", b"BZh", b"fLaC"];
    MAGIC.iter().any(|m| bytes.starts_with(m))
        || (bytes.len() > 11
            && &bytes[0..4] == b"RIFF"
            && (&bytes[8..12] == b"WAVE" || &bytes[8..12] == b"WEBP"))
}

/// Byte-order marks that `file` reports as text: downstream parsing
/// still fails them as non-UTF-8, exactly like the legacy probe path
/// (probe textual, then `NonUtf8`), so classifying them textual
/// preserves byte-identical observables.
fn has_text_bom(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xef, 0xbb, 0xbf])
        || bytes.starts_with(&[0xff, 0xfe])
        || bytes.starts_with(&[0xfe, 0xff])
        || bytes.starts_with(&[0x00, 0x00, 0xfe, 0xff])
        || bytes.starts_with(&[0xff, 0xfe, 0x00, 0x00])
}

/// Strict textual-content rule mirroring `file -b --mime-type`
/// (`^text`) as probed byte-for-byte: empty and single-byte files
/// read as non-text; UTF-8/16/32 BOMs read as text (downstream
/// parsing still yields `NonUtf8`, matching the legacy probe path);
/// NUL bytes are never text; DEL is text only when the file holds no
/// `\n`; other C0 controls are limited to tab, newline, vertical
/// tab, form feed, carriage return, BEL, BS, and ESC; every other
/// byte — including all high-bit bytes, so single-byte encodings
/// like Latin-1 read as text exactly like `file` reports them — is
/// text.
fn is_text_bytes(bytes: &[u8]) -> bool {
    if bytes.len() < 2 {
        return false;
    }
    if has_text_bom(bytes) {
        return true;
    }
    let has_newline = bytes.contains(&b'\n');
    bytes.iter().all(|&b| {
        b >= 0x80
            || b >= 0x20 && b != 0x7f
            || matches!(
                b,
                b'\t' | b'\n' | 0x0b | 0x0c | b'\r' | 0x07 | 0x08 | 0x1b
            )
            || (b == 0x7f && !has_newline)
    })
}

/// Classify one existing path without spawning `nb`.
///
/// `rel` is the notebook-relative form, used for nb's `--type`
/// spelling (text after the FIRST dot of the relative path, so a
/// dotted folder leaks into the spelling exactly like nb prints
/// it). Directories are folders. Extension sets decide before any
/// content sniff, exactly like nb. Unknown extensions consult the
/// magic table, then the strict text rule; empty files read as
/// non-text, matching `file`'s `inode/x-empty`. I/O failures
/// propagate as `Io` rather than degrading into a kind verdict.
pub(crate) fn classify_path(
    abs: &Path,
    rel: &str,
) -> Result<NativeClass, NbError> {
    if abs.is_dir() {
        return Ok(NativeClass::Folder);
    }
    let first = ext_after_first_dot(rel);
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let last = ext_after_last_dot(name);
    if TEXT_EXTENSIONS.contains(&last) {
        return Ok(NativeClass::Text);
    }
    if is_known_non_text(last) {
        return Ok(NativeClass::NonTextual { ext: first });
    }
    let bytes = std::fs::read(abs).map_err(|e| NbError::Io {
        path: abs.to_path_buf(),
        source: e.into(),
    })?;
    if magic_non_text(&bytes) || !is_text_bytes(&bytes) {
        return Ok(NativeClass::NonTextual { ext: first });
    }
    Ok(NativeClass::Text)
}

/// Confined fallible reverse `.index` lookup: the one-based slot
/// holding `rel`'s basename in its folder `.index`, if listed
/// (blank/deleted lines never match). The index file and every
/// ancestor are validated before reading: symlinks make the entry
/// unlisted (`Ok(None)`), missing indexes are unlisted, genuine I/O
/// failures propagate. This is the native replacement for the
/// shared legacy helper (which swallows I/O via `.ok()`); legacy
/// write paths are untouched.
pub(crate) fn numeric_id_for_rel(
    notebook_root: &Path,
    rel: &str,
) -> Result<Option<u32>, NbError> {
    let (folder, basename) = match rel.rfind('/') {
        Some(i) => (rel[..i].to_string(), rel[i + 1..].to_string()),
        None => (String::new(), rel.to_string()),
    };
    let index_rel = if folder.is_empty() {
        ".index".to_string()
    } else {
        format!("{folder}/.index")
    };
    // Validate root-to-leaf: ancestors first, then the index file.
    if !folder.is_empty() {
        match ensure_no_symlink_prefix(notebook_root, &folder) {
            Ok(()) => {},
            Err(NbError::NotFound { .. }) => return Ok(None),
            Err(e) => return Err(e),
        }
    }
    if is_symlinked(notebook_root, &index_rel)? {
        return Ok(None);
    }
    let abs = notebook_root.join(&index_rel);
    let bytes = match std::fs::read(&abs) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        },
        Err(e) => {
            return Err(NbError::Io {
                path: abs,
                source: e.into(),
            });
        },
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    Ok(lines
        .iter()
        .position(|l| *l == basename)
        .map(|p| p as u32 + 1))
}

/// Canonical selector for a resolved notebook-relative path: the
/// numeric `<folder>/<id>` form only when re-resolving it yields the
/// same entity (shadow-check against literal files and dead slots);
/// otherwise the exact-path form. I/O failures propagate.
pub(crate) fn canonical_selector(
    notebook: &str,
    notebook_root: &Path,
    rel: &str,
) -> Result<String, NbError> {
    let exact = format!("{notebook}:{rel}");
    match numeric_id_for_rel(notebook_root, rel)? {
        Some(id) => {
            let folder = match rel.rfind('/') {
                Some(i) => rel[..i].to_string(),
                None => String::new(),
            };
            let candidate = if folder.is_empty() {
                id.to_string()
            } else {
                format!("{folder}/{id}")
            };
            match resolve_selector(notebook_root, &candidate) {
                Ok(back) if back == rel => {
                    Ok(format!("{notebook}:{candidate}"))
                },
                Ok(_) => Ok(exact),
                Err(NbError::NotFound { .. }) => Ok(exact),
                Err(e) => Err(e),
            }
        },
        None => Ok(exact),
    }
}

/// Split `folder/id` or `id` into folder plus trailing component.
/// Returns `None` when the input has no usable trailing component.
fn split_folder_id(input: &str) -> Option<(String, String)> {
    if input.is_empty() {
        return None;
    }
    match input.rfind('/') {
        Some(i) => {
            let (folder, id) = input.split_at(i);
            let id = id.trim_start_matches('/');
            if id.is_empty() {
                None
            } else {
                Some((folder.to_string(), id.to_string()))
            }
        },
        None => Some((String::new(), input.to_string())),
    }
}

/// Whether `rel` names a todo file (nb matches `todo.md$`
/// case-insensitively for the checkbox-stripped title form).
fn file_is_todo(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    lower.ends_with("todo.md")
}

/// Strip up to the last `"] "` exactly like nb's
/// `${_title##*] }`: `[ ] Do the thing` becomes `Do the thing`.
/// No `"] "` means no stripped form (returns the input unchanged,
/// which then only matches itself).
fn strip_checkbox(title: &str) -> &str {
    match title.rfind("] ") {
        Some(i) => &title[i + 2..],
        None => title,
    }
}

/// Whether `rel` (notebook-relative) names a symlink itself,
/// without following it. Missing paths are not symlinks.
/// Genuine I/O failures propagate.
fn is_symlinked(notebook_root: &Path, rel: &str) -> Result<bool, NbError> {
    let abs = notebook_root.join(rel);
    match std::fs::symlink_metadata(&abs) {
        Ok(meta) => Ok(meta.file_type().is_symlink()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(NbError::Io {
            path: abs,
            source: e.into(),
        }),
    }
}

/// Reject symlink ancestors of a notebook-relative folder:
/// every path prefix must resolve without following a link.
/// A symlinked ancestor makes the subtree unresolvable
/// (`NotFound`); genuine I/O failures propagate. Missing prefixes
/// are skipped: a path through a missing directory is a genuine
/// miss, reported by the final existence check.
fn ensure_no_symlink_prefix(
    notebook_root: &Path,
    folder: &str,
) -> Result<(), NbError> {
    let mut prefix = PathBuf::new();
    for component in folder.split('/') {
        prefix.push(component);
        let rel = prefix.to_string_lossy().into_owned();
        match std::fs::symlink_metadata(notebook_root.join(&rel)) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(NbError::NotFound {
                    selector: folder.to_string(),
                });
            },
            Ok(_) => {},
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => {
                return Err(NbError::Io {
                    path: notebook_root.join(&rel),
                    source: e.into(),
                });
            },
        }
    }
    Ok(())
}

/// Lexically confine `input` to the notebook: reject absolute
/// paths, Windows drive-letter prefixes, and `..` escapes without
/// touching the filesystem. Returns the clean relative form; `None`
/// escapes or is empty.
///
/// Backslash handling is platform-scoped. On Unix a backslash is an
/// ordinary filename character (`a\b` is one literal component) and
/// is never a separator. On Windows the backslash IS the native
/// separator, so inputs are normalized (`\` to `/`) before the
/// lexical checks; otherwise `sub\..\..\foreign` would pass the
/// slash-only validation as one opaque component and then escape
/// through the OS path join.
fn confine(input: &str) -> Option<String> {
    // Absolute forms: POSIX root, Windows drive (with or without
    // separator), and UNC or lone-backslash roots.
    if input.starts_with(['/', '\\']) {
        return None;
    }
    let bytes = input.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return None;
    }
    #[cfg(windows)]
    let normalized = input.replace('\\', "/");
    #[cfg(windows)]
    let input = normalized.as_str();
    let mut parts: Vec<&str> = Vec::new();
    for part in input.split('/') {
        match part {
            "" | "." => {},
            ".." => {
                parts.pop()?;
            },
            _ => parts.push(part),
        }
    }
    let rel = parts.join("/");
    if rel.is_empty() {
        return None;
    }
    Some(rel)
}

/// Basename listed at one-based `.index` slot `id` in `folder`.
/// Blanked and out-of-range slots yield `Ok(None)` (dead,
/// non-terminal). Missing `.index` files also yield `Ok(None)`:
/// a folder without an index simply has no numeric addresses.
/// Crafted index lines that escape the folder are treated as dead
/// entries (`Ok(None)`), never followed. Genuine I/O failures
/// propagate.
fn id_to_basename(
    notebook_root: &Path,
    folder: &str,
    id: u32,
) -> Result<Option<String>, NbError> {
    if id == 0 {
        return Ok(None);
    }
    let rel = if folder.is_empty() {
        ".index".to_string()
    } else {
        format!("{folder}/.index")
    };
    // Validate root-to-leaf: ancestor folders first, then the
    // index file itself. A symlinked ancestor or symlinked index
    // makes the slot dead instead of reading through it. Genuine
    // I/O failures still propagate.
    if !folder.is_empty() {
        match ensure_no_symlink_prefix(notebook_root, folder) {
            Ok(()) => {},
            Err(NbError::NotFound { .. }) => return Ok(None),
            Err(e) => return Err(e),
        }
    }
    if is_symlinked(notebook_root, &rel)? {
        return Ok(None);
    }
    let abs = notebook_root.join(&rel);
    let bytes = match std::fs::read(&abs) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        },
        Err(e) => {
            return Err(NbError::Io {
                path: abs,
                source: e.into(),
            });
        },
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    let basename = lines
        .get(id as usize - 1)
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string());
    let Some(basename) = basename else {
        return Ok(None);
    };
    // Index lines are basenames; anything else is crafted or
    // corrupt and must not escape the folder.
    if basename.contains('/') || basename.contains('\\') {
        return Ok(None);
    }
    Ok(Some(basename))
}

/// Strip one CommonMark closing-hash run exactly like nb's
/// `_get_content --title`: trailing blanks, then a `#` run, kept
/// only when preceded by a blank; the preceding blanks go with it.
/// `C#` keeps its hash; `T # ` becomes `T`.
fn strip_closing_hashes(line: &str) -> &str {
    let t = line.trim_end_matches([' ', '\t']);
    let hashes = t.len() - t.trim_end_matches('#').len();
    if hashes == 0 {
        return line;
    }
    let head = &t[..t.len() - hashes];
    if head.ends_with([' ', '\t']) {
        head.trim_end_matches([' ', '\t'])
    } else {
        line
    }
}

/// Whether `name` may carry an nb-matchable title: `.md` or
/// `.markdown`, ASCII case-insensitive (nb matches with
/// `nocasematch` on).
fn has_nb_title_extension(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

/// nb-faithful title extraction for root-scope title matching.
///
/// Mirrors `_get_content --title`: only `.md` / `.markdown` files;
/// first ATX H1 outside code blocks, else front-matter `title:`,
/// else setext underline; closing-hash sequences stripped; no
/// whitespace trimming (byte-exact like nb, so trailing-space
/// titles only match trailing-space inputs). Read streams
/// line-by-line so large binaries never fully load. Vanished paths
/// yield `Ok(None)`; genuine I/O failures propagate.
fn file_title(
    notebook_root: &Path,
    rel: &str,
) -> Result<Option<String>, NbError> {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    if !has_nb_title_extension(name) {
        return Ok(None);
    }
    let abs = notebook_root.join(rel);
    let file = match std::fs::File::open(&abs) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        },
        Err(e) => {
            return Err(NbError::Io {
                path: abs,
                source: e.into(),
            });
        },
    };
    let mut reader = std::io::BufReader::new(file);
    let mut in_code = false;
    let mut in_front = false;
    let mut maybe: Option<String> = None;
    let mut chunk = Vec::new();
    loop {
        chunk.clear();
        let n = std::io::BufRead::read_until(&mut reader, b'\n', &mut chunk)
            .map_err(|e| NbError::Io {
            path: abs.clone(),
            source: e.into(),
        })?;
        if n == 0 {
            break;
        }
        if chunk.ends_with(b"\n") {
            chunk.pop();
        }
        let line = String::from_utf8_lossy(&chunk);
        let line = line.strip_suffix('\r').unwrap_or(&line);
        // ATX H1 outside code blocks (leading blanks allowed).
        let bare = line.trim_start_matches([' ', '\t', '\u{b}', '\u{c}']);
        if let Some(after_hash) = bare.strip_prefix('#')
            && !after_hash.starts_with('#')
            && !in_code
            && let Some(body) = after_hash.strip_prefix([' ', '\t'])
        {
            let body = body.trim_start_matches([' ', '\t']);
            return Ok(Some(strip_closing_hashes(body).to_string()));
        }
        if line == "---" {
            in_front = !in_front;
        } else if line.starts_with("```") {
            in_code = !in_code;
        } else if in_front && line.starts_with("title: ") {
            return Ok(Some(line["title: ".len()..].to_string()));
        } else if !in_front && !in_code {
            if maybe.is_none() && !line.is_empty() {
                maybe = Some(line.to_string());
            } else if let Some(candidate) = maybe.take() {
                let underline = line.trim();
                if !underline.is_empty() && underline.chars().all(|c| c == '=')
                {
                    return Ok(Some(
                        candidate.trim_start_matches([' ', '\t']).to_string(),
                    ));
                }
                maybe = Some(candidate);
            }
        }
    }
    Ok(None)
}

/// Root-scope files eligible for title matching: regular files
/// only — symlinks are never followed into title content.
/// Directory enumeration failures propagate; per-entry failures on
/// vanished paths are skipped, genuine I/O failures propagate.
fn root_scoped_files(notebook_root: &Path) -> Result<Vec<String>, NbError> {
    let mut out = Vec::new();
    let entries =
        std::fs::read_dir(notebook_root).map_err(|e| NbError::Io {
            path: notebook_root.to_path_buf(),
            source: e.into(),
        })?;
    for entry in entries {
        let entry = entry.map_err(|e| NbError::Io {
            path: notebook_root.to_path_buf(),
            source: e.into(),
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| NbError::Io {
            path: path.clone(),
            source: e.into(),
        })?;
        if file_type.is_symlink() || !file_type.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        // Extension gating lives in `file_title` (nb's `.md`-only
        // rule); every regular file is a candidate here.
        out.push(name.to_string());
    }
    out.sort();
    Ok(out)
}

/// Resolve one qualifier-stripped PATH to its exact notebook-relative
/// form: lexical confinement plus an existence check with no selector
/// fallback. Symlinks are never followed: notebook content is
/// regular files and directories (transactions already refuse
/// symlink paths), so a symlink never resolves. Missing, escaping,
/// or non-regular targets are `NotFound`. Used for
/// `NoteTarget::Path`, which must never consult the numeric or title
/// tables.
pub(crate) fn resolve_exact(
    notebook_root: &Path,
    input: &str,
) -> Result<String, NbError> {
    let rel = confine(input).filter(|r| !r.is_empty()).ok_or_else(|| {
        NbError::NotFound {
            selector: input.to_string(),
        }
    })?;
    // Validate root-to-leaf: ancestor components first, so
    // `linkdir/note.md` through a symlinked `linkdir` is `NotFound`
    // without ever following the link; then the final component.
    if let Some(i) = rel.rfind('/')
        && let Err(e) = ensure_no_symlink_prefix(notebook_root, &rel[..i])
    {
        return Err(match e {
            NbError::NotFound { .. } => NbError::NotFound {
                selector: input.to_string(),
            },
            other => other,
        });
    }
    let abs = notebook_root.join(&rel);
    match std::fs::symlink_metadata(&abs) {
        Ok(meta) if meta.file_type().is_symlink() => Err(NbError::NotFound {
            selector: input.to_string(),
        }),
        Ok(meta) if meta.is_file() || meta.is_dir() => Ok(rel),
        Ok(_) => Err(NbError::NotFound {
            selector: input.to_string(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(NbError::NotFound {
                selector: input.to_string(),
            })
        },
        Err(e) => Err(NbError::Io {
            path: abs,
            source: e.into(),
        }),
    }
}

/// Canonical containment: `abs` must sit under canonical `root`
/// (follows symlinks, so swapped ancestors cannot escape).
/// Escapes and vanished paths are terminal `NotFound`; genuine I/O
/// failures propagate.
pub(crate) fn confine_canonical(
    root: &Path,
    abs: &Path,
    selector: &str,
) -> Result<PathBuf, NbError> {
    let canonical_root = root.canonicalize().map_err(|e| NbError::Io {
        path: root.to_path_buf(),
        source: e.into(),
    })?;
    let canonical_abs = abs.canonicalize().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            NbError::NotFound {
                selector: selector.to_string(),
            }
        } else {
            NbError::Io {
                path: abs.to_path_buf(),
                source: e.into(),
            }
        }
    })?;
    if canonical_abs.starts_with(&canonical_root) {
        Ok(canonical_abs)
    } else {
        Err(NbError::NotFound {
            selector: selector.to_string(),
        })
    }
}

/// Resolve one qualifier-stripped selector to a notebook-relative path
/// without spawning `nb`, following the pinned precedence table:
///
/// 0. Exact existing relative path (file or directory) wins.
/// 1. Numeric id via `.index` (`<id>` or `<folder>/<id>`); dead
///    slots are non-terminal and fall through.
/// 2. Exact case-sensitive title, root scope only; duplicates are
///    ambiguous `NotFound`.
/// 3. Otherwise `NotFound`, terminal. There is no cross-folder
///    basename search: a bare basename resolves only when it names
///    an existing root entry (step 0), and folder-qualified forms
///    are exact paths (step 0). No stems, suffixes, or non-root
///    titles.
///
/// Slash-qualified inputs that miss steps 0 and 1 are terminal
/// `NotFound`: basenames never match across an explicit folder.
pub(crate) fn resolve_selector(
    notebook_root: &Path,
    input: &str,
) -> Result<String, NbError> {
    // Lexically confine first: absolute paths, drive-letter
    // prefixes, and `..` escapes are terminal `NotFound`.
    // Normalizable forms (`./x`, `sub/../x`) proceed in their
    // confined shape. Backslashes are never rewritten (see
    // `confine`).
    let Some(canonical) = confine(input) else {
        return Err(NbError::NotFound {
            selector: input.to_string(),
        });
    };
    // Step 0: exact existing entry. Only genuine misses fall
    // through; I/O failures propagate.
    match resolve_exact(notebook_root, &canonical) {
        Ok(rel) => return Ok(rel),
        Err(NbError::NotFound { .. }) => {},
        Err(other) => return Err(other),
    }
    let slashed = canonical.contains('/');
    // Step 1: numeric id, bare or folder-qualified. The candidate
    // is validated exactly like an explicit path (existence plus
    // symlink confinement): missing or dead targets fall through
    // to the title step instead of resolving to nothing, and
    // crafted index lines can never escape the folder.
    if let Some((folder, id_str)) = split_folder_id(&canonical)
        && let Ok(id) = id_str.parse::<u32>()
        && let Some(basename) = id_to_basename(notebook_root, &folder, id)?
    {
        let rel = if folder.is_empty() {
            basename
        } else {
            format!("{folder}/{basename}")
        };
        match resolve_exact(notebook_root, &rel) {
            Ok(verified) => return Ok(verified),
            Err(NbError::NotFound { .. }) => {},
            Err(other) => return Err(other),
        }
    }
    if slashed {
        return Err(NbError::NotFound {
            selector: input.to_string(),
        });
    }
    // Step 2: root-scope exact title; duplicates are ambiguous.
    // Todo files additionally match the checkbox-stripped title
    // (`Do the thing` matches `[ ] Do the thing`), mirroring nb.
    let mut titled = Vec::new();
    for rel in root_scoped_files(notebook_root)? {
        let title = file_title(notebook_root, &rel)?;
        let exact = title.as_deref() == Some(canonical.as_str());
        let checkbox = file_is_todo(&rel)
            && title.as_deref().map(strip_checkbox)
                == Some(canonical.as_str());
        if exact || checkbox {
            titled.push(rel);
        }
    }
    match titled.as_slice() {
        [one] => return Ok(one.clone()),
        [] => {},
        _ => {
            return Err(NbError::NotFound {
                selector: input.to_string(),
            });
        },
    }
    // Step 3: no cross-folder basename fallback. A bare basename
    // resolves only when it names an existing root entry, which
    // step 0 already covers; folder-qualified forms are exact
    // paths, also covered by step 0. Anything else is terminal
    // `NotFound` (no stems, suffixes, non-root titles, or
    // cross-folder basename search).
    Err(NbError::NotFound {
        selector: input.to_string(),
    })
}
