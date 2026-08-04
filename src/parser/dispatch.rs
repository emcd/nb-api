//! Format-dispatch: extension tables and path routing.

use super::document::DocumentKind;

// --- Format-dispatch constants (R3 revision) ---

/// Source-of-truth list of supported document extensions for
/// format dispatch. The constant is **declared in the `parser`
/// module and re-exported at the crate root** as
/// `nb_api::SUPPORTED_DOCUMENT_EXTENSIONS` per the spec.
///
/// Iteration order in this array is **not** the matching
/// precedence — the matcher sorts by length descending for the
/// longest-first precedence required by the R3 spec. The
/// owned-string form is populated from this constant at
/// `NbError::UnsupportedDocumentFormat` construction time so the
/// error's JSON round-trip is independent of internal ordering.
///
/// The list is intentionally narrow; future formats (Org,
/// AsciiDoc, LaTeX) land in P3+ as separate parser modules per
/// `nb-api:todos/format/{1,2,3}`.
pub const SUPPORTED_DOCUMENT_EXTENSIONS: &[&str] = &["md", "markdown", "todo.md", "bookmark.md"];

/// Recognized-but-unsupported dotted suffixes (R3 revision).
/// Files whose final filename ends with one of these are
/// rejected via `NbError::UnsupportedDocumentFormat` BEFORE
/// byte parsing runs. The set is disjoint from
/// [`SUPPORTED_DOCUMENT_EXTENSIONS`].
///
/// The set coincides with `nb` CLI's format recognition
/// (regexes `(\.latex$|\.tex$)`, `\.org$`,
/// `(\.adoc$|\.asciidoc$)`) — these are formats `nb` itself
/// recognizes but P1 cannot parse.
pub(crate) const REJECTED_DOCUMENT_EXTENSIONS: &[&str] =
    &["org", "latex", "tex", "adoc", "asciidoc"];

/// Result of format dispatch on `ParseContext::FromPath`.
///
/// Format dispatch precedes byte parsing per the R3 spec:
/// `Rejected(_)` short-circuits `parse` with
/// `NbError::UnsupportedDocumentFormat`; `Supported(_)` and
/// `MarkdownNote` proceed to byte parsing with the dispatched
/// `DocumentKind`.
pub(crate) enum FormatDispatch {
    /// Final filename matched a supported suffix; byte parsing
    /// proceeds with this kind.
    SupportedKind(DocumentKind),
    /// Final filename matched a recognized-but-unsupported
    /// suffix; `parse` returns `UnsupportedDocumentFormat`
    /// without consuming any bytes. The string is the matched
    /// lowercase dotted suffix WITHOUT the leading dot.
    Rejected(String),
    /// Final filename is absent, non-UTF-8, or matched no
    /// recognized suffix; permissive Markdown Note fallback per
    /// the spec.
    MarkdownNote,
}

/// Perform format dispatch on `path`.
///
/// The spec mandates ASCII case-insensitive matching against
/// the lowercased final filename with literal dotted
/// boundaries and longest-first precedence. The matcher
/// delegates the precedence by selecting the longest matching
/// suffix across both the supported and rejected sets rather
/// than sorting a list (the supported and rejected sets are
/// disjoint by construction, so a single pass is sufficient).
///
/// Falls through to `MarkdownNote` for:
/// - paths with no final filename component (e.g., root path);
/// - paths whose final filename is not valid UTF-8;
/// - paths whose final filename has no recognized suffix.
///
/// Neither `Explicit(DocumentKind)` nor paths that match a
/// supported suffix reach this function.
pub(crate) fn format_dispatch(path: &std::path::Path) -> FormatDispatch {
    let Some(name) = path.file_name() else {
        return FormatDispatch::MarkdownNote;
    };
    let Some(name) = name.to_str() else {
        return FormatDispatch::MarkdownNote;
    };
    let lower = name.to_ascii_lowercase();

    // First pass: find the LONGEST supported suffix matching
    // this filename. Multi-dot suffixes (e.g., `.bookmark.md`)
    // must beat single-dot suffixes (`.md`) so that
    // `foo.bookmark.md` resolves to Bookmark, not Note.
    let mut best_supported: Option<(usize, DocumentKind)> = None;
    for ext in SUPPORTED_DOCUMENT_EXTENSIONS {
        let suffix_len = ext.len() + 1; // include the leading dot
        if lower.len() < suffix_len {
            continue;
        }
        if !lower.ends_with(&format!(".{ext}")) {
            continue;
        }
        let kind = match *ext {
            "md" | "markdown" => DocumentKind::Note,
            "todo.md" => DocumentKind::Todo,
            "bookmark.md" => DocumentKind::Bookmark,
            _ => continue,
        };
        if best_supported.is_none() || suffix_len > best_supported.unwrap().0 {
            best_supported = Some((suffix_len, kind));
        }
    }
    if let Some((_, kind)) = best_supported {
        return FormatDispatch::SupportedKind(kind);
    }

    // Second pass: recognized-but-unsupported suffixes. We test
    // all rejected suffixes and pick the longest match. In
    // practice this matters for compound inputs like
    // `xxx.latex.tex` (would be `Rejected("tex")` if the user
    // uses an unusual filename); supported-then-rejected
    // precedence ensures no overlap.
    let mut best_rejected: Option<(usize, &str)> = None;
    for ext in REJECTED_DOCUMENT_EXTENSIONS {
        let suffix_len = ext.len() + 1;
        if lower.len() < suffix_len {
            continue;
        }
        if !lower.ends_with(&format!(".{ext}")) {
            continue;
        }
        if best_rejected.is_none() || suffix_len > best_rejected.unwrap().0 {
            best_rejected = Some((suffix_len, ext));
        }
    }
    if let Some((_, ext)) = best_rejected {
        return FormatDispatch::Rejected(ext.to_string());
    }

    FormatDispatch::MarkdownNote
}
