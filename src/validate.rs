//! Notebook, folder, destination, and selector validation helpers.

use crate::error::NbError;

const NOTEBOOK_FIELD_MESSAGE: &str = "Invalid `notebook`: use a bare notebook name only. Use `folder` for folder paths and `id`/`selector` for note selectors.";

const FOLDER_FIELD_MESSAGE: &str = "Invalid folder path: use `folder` for folder paths only, not notebook-qualified selectors. To choose a notebook, use the separate `notebook` field.";

pub(crate) fn validate_notebook_name(name: &str) -> Result<(), NbError> {
    if name.trim().is_empty() || name.contains(':') || name.contains('/') || name.contains('\\') {
        return Err(NbError::ValidationError {
            reason: NOTEBOOK_FIELD_MESSAGE.to_string(),
            location: None,
        });
    }
    Ok(())
}

pub(crate) fn validate_folder_option(folder: Option<&str>) -> Result<(), NbError> {
    if let Some(path) = folder {
        validate_folder_path(path)?;
    }
    Ok(())
}

pub(crate) fn validate_folder_path(path: &str) -> Result<(), NbError> {
    if path.trim().is_empty() || path.contains(':') {
        return Err(NbError::ValidationError {
            reason: FOLDER_FIELD_MESSAGE.to_string(),
            location: None,
        });
    }
    Ok(())
}

pub(crate) fn validate_destination(destination: &str) -> Result<(), NbError> {
    if destination.trim().is_empty() || destination.contains(':') {
        return Err(NbError::ValidationError {
            reason:
                "Invalid destination: use a folder path or filename only, not a notebook-qualified selector."
                    .to_string(),
            location: None,
        });
    }
    Ok(())
}

pub(crate) fn parse_qualified_selector(selector: &str) -> Result<Option<(&str, &str)>, NbError> {
    let Some((notebook, path)) = selector.split_once(':') else {
        return Ok(None);
    };
    validate_notebook_name(notebook)?;
    if path.trim().is_empty() || path.contains(':') {
        return Err(NbError::ValidationError {
            reason:
                "Invalid selector: use at most one notebook qualifier, as `<notebook>:<folder>/<id>`."
                    .to_string(),
            location: None,
        });
    }
    Ok(Some((notebook, path)))
}

/// Detect a duplicate-title H1 in `content`.
///
/// Returns `Some(<exact source line>)` when `content`'s first
/// nonblank line is a CommonMark ATX H1 and the trimmed heading
/// text equals the trimmed `title`.
///
/// Per CommonMark, an ATX H1 is:
/// - 0 to 3 leading spaces (4 or more is an indented code block);
/// - exactly one opening `#` (2+ is H2 or lower);
/// - a required space, tab, or end-of-line after the opening hash
///   (`#Title` is NOT an H1);
/// - the heading text (inline content);
/// - an optional closing hash sequence: a run of `#`s preceded
///   by a space or tab and followed only by spaces or tabs to
///   end of line. Literal trailing hashes with no preceding
///   whitespace (e.g., `# C#`) are part of the heading text, NOT
///   a closing sequence.
///
/// Returns `None` when the title is empty/None, the content has
/// no nonblank line, or the first nonblank line is not a
/// duplicate H1. Used by [`NbClient::add_note`](crate::NbClient::add_note)
/// to detect the common agent-side mistake of including the title
/// H1 inside the body content.
pub(crate) fn detect_duplicate_title_heading(title: &str, content: &str) -> Option<String> {
    let trimmed_title = title.trim();
    if trimmed_title.is_empty() {
        return None;
    }
    let first_nonblank = content.lines().find(|line| !line.trim().is_empty())?;
    let bytes = first_nonblank.as_bytes();

    // 0-3 leading spaces. 4 or more is an indented code block.
    let leading_spaces = bytes.iter().take_while(|&&b| b == b' ').count();
    if leading_spaces >= 4 {
        return None;
    }
    let after_indent = &first_nonblank[leading_spaces..];

    // Exactly one opening `#`. `##` and beyond are H2+.
    if !after_indent.starts_with('#') {
        return None;
    }
    if after_indent.starts_with("##") {
        return None;
    }
    let after_hash = &after_indent[1..];

    // Required space, tab, or EOL after the opening hash. `#Title`
    // is NOT an H1 (no delimiter). A bare `#` IS an H1 with empty
    // heading text per CommonMark; the title comparison below
    // filters out the false-positive case (empty heading text
    // never matches a non-empty title).
    let after_hash_bytes = after_hash.as_bytes();
    let rest = match after_hash_bytes.first() {
        Some(b' ') | Some(b'\t') => &after_hash[1..],
        None => "",
        Some(_) => return None,
    };

    // Optional closing hash sequence per CommonMark.
    let heading_text = strip_atx_closing_hashes(rest);

    if heading_text.trim() == trimmed_title {
        Some(first_nonblank.to_string())
    } else {
        None
    }
}

/// Strip an optional CommonMark ATX closing hash sequence from
/// the end of a heading body. The closing sequence is a run of
/// one or more `#`s preceded by a space or tab and followed only
/// by spaces or tabs to end of string. If no valid closing
/// sequence is present, returns the input unchanged. Literal
/// trailing hashes with no preceding whitespace (e.g., `C#`) are
/// NOT a closing sequence and are preserved.
///
/// A closing sequence can consume the entire post-delimiter
/// content (e.g., `# #`, `# ###`); in that case the heading
/// text becomes empty per CommonMark and never matches a
/// non-empty title.
pub(crate) fn strip_atx_closing_hashes(s: &str) -> String {
    let bytes = s.as_bytes();
    let n = bytes.len();

    // Find where trailing whitespace ends.
    let mut end = n;
    while end > 0 && (bytes[end - 1] == b' ' || bytes[end - 1] == b'\t') {
        end -= 1;
    }
    if end == 0 || bytes[end - 1] != b'#' {
        return s.to_string();
    }

    // Find the start of the trailing-hash sequence.
    let mut hash_start = end;
    while hash_start > 0 && bytes[hash_start - 1] == b'#' {
        hash_start -= 1;
    }

    // The closing sequence must be preceded by a space or tab
    // within the heading body. If the run of `#`s is preceded by
    // a non-whitespace char (e.g., `C#`), it is part of the
    // heading text, NOT a closing sequence; preserve the input.
    //
    // If the run is at offset 0 of the post-delimiter body, it
    // is preceded by the delimiter space (which is NOT in the
    // body); this is a valid closing sequence that consumes the
    // entire body. The heading text becomes empty.
    if hash_start == 0 {
        return s[..0].to_string();
    }
    let prev = bytes[hash_start - 1];
    if prev != b' ' && prev != b'\t' {
        return s.to_string();
    }

    s[..hash_start].to_string()
}
