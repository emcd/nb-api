//! Native (nb-spawn-free) read-path resolution and classification.
//!
//! Implements the pinned selector precedence table and nb's textual
//! classification without spawning `nb`, for the `native_reads`
//! feature path. All reads are side-effect-free filesystem plus
//! `.index` scans under the notebook gate.
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
use crate::parser::{ParseContext, parse};

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

/// Classify one existing path without spawning `nb`.
///
/// Directories are folders. Extension sets decide before any content
/// sniff, exactly like nb. Unknown extensions fall back to a content
/// sniff (non-empty, valid UTF-8, no NUL bytes reads as text);
/// empty files read as non-text, matching `file`'s `inode/x-empty`.
pub(crate) fn classify_path(path: &Path) -> NativeClass {
    if path.is_dir() {
        return NativeClass::Folder;
    }
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let first = ext_after_first_dot(&name);
    let last = ext_after_last_dot(&name);
    if TEXT_EXTENSIONS.contains(&last) {
        return NativeClass::Text;
    }
    if is_known_non_text(last) {
        return NativeClass::NonTextual { ext: first };
    }
    let text = std::fs::read(path).is_ok_and(|bytes| {
        !bytes.is_empty()
            && bytes.iter().all(|&b| b != 0)
            && std::str::from_utf8(&bytes).is_ok()
    });
    if text {
        NativeClass::Text
    } else {
        NativeClass::NonTextual { ext: first }
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

/// Basename listed at one-based `.index` slot `id` in `folder`.
/// Blanked and out-of-range slots yield `None` (dead, non-terminal).
fn id_to_basename(
    notebook_root: &Path,
    folder: &str,
    id: u32,
) -> Option<String> {
    if id == 0 {
        return None;
    }
    let rel = if folder.is_empty() {
        ".index".to_string()
    } else {
        format!("{folder}/.index")
    };
    let bytes = std::fs::read(notebook_root.join(rel)).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
        .get(id as usize - 1)
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
}

/// Display title of one file for root-scope title matching.
/// Mirrors the `title_text` derivation in `show_note`: trailing
/// newline stripped, leading `#` run stripped, trimmed.
fn file_title(notebook_root: &Path, rel: &str) -> Option<String> {
    let bytes = std::fs::read(notebook_root.join(rel)).ok()?;
    let doc =
        parse(&bytes, ParseContext::FromPath(PathBuf::from(rel))).ok()?;
    let title = doc.title()?;
    let s = String::from_utf8_lossy(title);
    Some(
        s.trim_end_matches('\n')
            .trim_start_matches('#')
            .trim()
            .to_string(),
    )
}

/// Root-scope files eligible for title matching: regular files with
/// a parseable (supported-extension) document shape.
fn root_scoped_files(notebook_root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(notebook_root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        // Only documents nb would title-match: skip formats the
        // parser refuses outright (e.g. `.org`).
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if parse(&bytes, ParseContext::FromPath(PathBuf::from(name))).is_ok() {
            out.push(name.to_string());
        }
    }
    out.sort();
    out
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
    let input = input.trim_matches('/');
    if input.is_empty() {
        return Err(NbError::NotFound {
            selector: input.to_string(),
        });
    }
    // Step 0: exact existing entry.
    let direct = notebook_root.join(input);
    if direct.is_file() || direct.is_dir() {
        return Ok(input.replace('\\', "/"));
    }
    let slashed = input.contains('/');
    // Step 1: numeric id, bare or folder-qualified.
    if let Some((folder, id_str)) = split_folder_id(input)
        && let Ok(id) = id_str.parse::<u32>()
        && let Some(basename) = id_to_basename(notebook_root, &folder, id)
    {
        return Ok(if folder.is_empty() {
            basename
        } else {
            format!("{folder}/{basename}")
        });
    }
    if slashed {
        return Err(NbError::NotFound {
            selector: input.to_string(),
        });
    }
    // Step 2: root-scope exact title; duplicates are ambiguous.
    let mut titled = Vec::new();
    for rel in root_scoped_files(notebook_root) {
        if file_title(notebook_root, &rel).as_deref() == Some(input) {
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
