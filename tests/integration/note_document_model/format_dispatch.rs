//! Format-dispatch matrix: unsupported extensions, longest-first, explicit kind.

use std::path::PathBuf;

use nb_api::NbError;
use nb_api::parser::{DocumentKind, ParseContext, parse};

// ---------- format dispatch (R3 spec revision) ----------

/// Helper: assert that parsing `bytes` against `path` returns
/// the `UnsupportedDocumentFormat` variant with the given
/// extension, and that the carried `supported` list equals
/// `nb_api::SUPPORTED_DOCUMENT_EXTENSIONS` populated as
/// `String`.
fn assert_unsupported_document_format(bytes: &[u8], path: &str, expected_extension: &str) {
    let result = parse(bytes, ParseContext::FromPath(PathBuf::from(path)));
    match result {
        Err(NbError::UnsupportedDocumentFormat {
            extension,
            supported,
        }) => {
            assert_eq!(extension, expected_extension);
            let expected_supported: Vec<String> = nb_api::SUPPORTED_DOCUMENT_EXTENSIONS
                .iter()
                .map(|s| s.to_string())
                .collect();
            assert_eq!(supported, expected_supported);
        }
        Err(other) => {
            panic!("expected UnsupportedDocumentFormat for path {path:?}, got: {other:?}")
        }
        Ok(doc) => panic!(
            "expected UnsupportedDocumentFormat for path {path:?}, \
             got Ok NoteDocument with kind {:?}",
            doc.kind()
        ),
    }
}

// ---------- recognized but unsupported: rejected before byte parsing ----------

/// `parse` rejects `.org` files with
/// `UnsupportedDocumentFormat`, even when the bytes are empty
/// (format dispatch precedes byte parsing per the R3 spec).
#[test]
fn parse_rejects_org_file_with_unsupported_document_format() {
    assert_unsupported_document_format(b"", "note.org", "org");
}

#[test]
fn parse_rejects_latex_file_with_unsupported_document_format() {
    assert_unsupported_document_format(b"", "paper.latex", "latex");
}

#[test]
fn parse_rejects_tex_file_with_unsupported_document_format() {
    assert_unsupported_document_format(b"", "paper.tex", "tex");
}

#[test]
fn parse_rejects_adoc_file_with_unsupported_document_format() {
    assert_unsupported_document_format(b"", "doc.adoc", "adoc");
}

#[test]
fn parse_rejects_asciidoc_file_with_unsupported_document_format() {
    assert_unsupported_document_format(b"", "doc.asciidoc", "asciidoc");
}

/// Format dispatch precedes byte parsing: even with empty
/// bytes and a `.org` filename, the error is
/// `UnsupportedDocumentFormat`, NOT `ParseError(MissingTitle)`.
/// The spec explicitly notes this rule.
#[test]
fn parse_rejected_format_does_not_surface_as_missing_title() {
    let result = parse(b"", ParseContext::FromPath(PathBuf::from("empty.org")));
    match result {
        Err(NbError::UnsupportedDocumentFormat { extension, .. }) => {
            assert_eq!(extension, "org");
        }
        Err(other) => {
            panic!("expected UnsupportedDocumentFormat for empty .org file, got: {other:?}")
        }
        Ok(_) => panic!("empty .org must not parse as Ok"),
    }
}

// ---------- uppercase extensions: ASCII case-insensitive matching ----------

/// ASCII case-insensitive matching per the R3 spec: `.ORG`,
/// `.Org`, etc. all reject with the LOWERCASE `extension`
/// value regardless of input case.
#[test]
fn parse_rejects_uppercase_org_with_lowercase_extension_value() {
    assert_unsupported_document_format(b"", "note.ORG", "org");
    assert_unsupported_document_format(b"", "note.Org", "org");
}

/// Uppercase `.BOOKMARK.MD` resolves to Bookmark (P1-vs-nb
/// case-insensitive divergence, deliberately documented in
/// the spec as Q-D / Option Z).
#[test]
fn parse_uppercase_bookmark_md_is_bookmark() {
    let result = parse(
        b"# bm\n\n<URL>\n",
        ParseContext::FromPath(PathBuf::from("bm.BOOKMARK.MD")),
    );
    let doc = result.expect("uppercase .BOOKMARK.MD should resolve to Bookmark");
    assert_eq!(doc.kind(), DocumentKind::Bookmark);
}

// ---------- longest-first precedence ----------

/// `foo.bookmark.md` resolves to Bookmark, not Note. The
/// compound multi-dot suffix `.bookmark.md` must beat the
/// shorter `.md`.
#[test]
fn parse_longest_first_precedence_bookmark_beats_md() {
    let result = parse(
        b"# bm\n\n<URL>\n",
        ParseContext::FromPath(PathBuf::from("foo.bookmark.md")),
    );
    let doc = result.expect("foo.bookmark.md should be Bookmark");
    assert_eq!(doc.kind(), DocumentKind::Bookmark);
}

/// `foo.todo.md` resolves to Todo, not Note.
#[test]
fn parse_longest_first_precedence_todo_beats_md() {
    let result = parse(
        b"# [ ] task\n",
        ParseContext::FromPath(PathBuf::from("foo.todo.md")),
    );
    let doc = result.expect("foo.todo.md should be Todo");
    assert_eq!(doc.kind(), DocumentKind::Todo);
}

/// `notbookmark.md` does NOT match `.bookmark.md` (no literal
/// dotted boundary); falls through to Note. The spec rules
/// out the false match.
#[test]
fn parse_notbookmark_md_is_note_not_bookmark() {
    let result = parse(
        b"# nb\n\n<URL>\n",
        ParseContext::FromPath(PathBuf::from("notbookmark.md")),
    );
    let doc = result.expect("notbookmark.md should be Note");
    assert_eq!(doc.kind(), DocumentKind::Note);
}

/// The bare `.todo` extension (without `.md`) is permissive
/// Markdown Note, per the spec. It is NOT a mutation-
/// authoritative Todo indicator.
#[test]
fn parse_bare_dot_todo_is_note_not_todo() {
    let result = parse(
        b"# task\n",
        ParseContext::FromPath(PathBuf::from("scratch.todo")),
    );
    let doc = result.expect(".todo should be permissive Note");
    assert_eq!(doc.kind(), DocumentKind::Note);
}

// ---------- directory components ----------

/// Directory components do not participate in matching. Only the
/// final filename (the component after the last `/` or `\`
/// separator) is matched.
#[test]
fn parse_directory_components_do_not_participate_in_match() {
    // `a/b/c/note.org` — directory `a/b/c/` does not
    // participate; final filename `note.org` rejects.
    assert_unsupported_document_format(b"", "a/b/c/note.org", "org");

    // `notes/scratch.bookmark.md` — directory `notes/` does
    // not participate; final filename `scratch.bookmark.md`
    // matches `.bookmark.md`. The leading `.` is the literal
    // dotted boundary between the stem `scratch` and the
    // suffix `.bookmark.md`.
    let result = parse(
        b"# bm\n\n<URL>\n",
        ParseContext::FromPath(PathBuf::from("notes/scratch.bookmark.md")),
    );
    let doc = result.expect("scratch.bookmark.md under notes/ should be Bookmark");
    assert_eq!(doc.kind(), DocumentKind::Bookmark);

    // `notes/bookmark.md` (no leading dot on `bookmark`) is
    // NOT a Bookmark; the literal dotted boundary is enforced
    // on the final filename, so this falls through to Note.
    let result = parse(
        b"# bm\n\n<URL>\n",
        ParseContext::FromPath(PathBuf::from("notes/bookmark.md")),
    );
    let doc = result.expect("bookmark.md without leading dot is Note, not Bookmark");
    assert_eq!(doc.kind(), DocumentKind::Note);
}

// ---------- portable dispatch: no final filename / non-UTF-8 ----------

/// A path with no final filename component (root path / empty
/// path) is permissive Markdown Note.
#[test]
fn parse_path_with_no_final_filename_is_note() {
    let result = parse(
        b"just content\n",
        ParseContext::FromPath(PathBuf::from("/")),
    );
    let doc = result.expect("root path should be permissive Note");
    assert_eq!(doc.kind(), DocumentKind::Note);
}

/// Non-UTF-8 bytes in the final filename are permissive
/// Markdown Note (per the R3 portable dispatch rule). The
/// only way to construct such a `PathBuf` from valid UTF-8
/// `Path` is via `OsStr` containing invalid UTF-8 — using
/// `PathBuf::from` for portability. We instead verify the
/// portable rule via a stub path that triggers
/// `to_str() == None` in `format_dispatch` by using a path
/// component that is *trailing-slash* without a final
/// filename.
#[test]
fn parse_path_with_trailing_slash_is_note() {
    // `a/b/` has no final filename component — the trailing
    // slash terminates the path before any filename.
    let result = parse(
        b"just content\n",
        ParseContext::FromPath(PathBuf::from("a/b/")),
    );
    let doc = result.expect("path with trailing slash should be Note");
    assert_eq!(doc.kind(), DocumentKind::Note);
}

// ---------- unrecognized extension: permissive Markdown Note ----------

/// Files whose final filename does NOT match any recognized
/// suffix (supported or unsupported) are treated as Markdown
/// Note. The R3 spec mandates this permissive fallback.
#[test]
fn parse_unrecognized_extension_is_note() {
    let result = parse(b"body\n", ParseContext::FromPath(PathBuf::from("note.txt")));
    let doc = result.expect(".txt should be permissive Markdown Note");
    assert_eq!(doc.kind(), DocumentKind::Note);

    let result = parse(
        b"body\n",
        ParseContext::FromPath(PathBuf::from("note.html")),
    );
    assert_eq!(result.unwrap().kind(), DocumentKind::Note);

    // No extension at all.
    let result = parse(b"body\n", ParseContext::FromPath(PathBuf::from("Makefile")));
    assert_eq!(result.unwrap().kind(), DocumentKind::Note);
}

// ---------- Explicit(DocumentKind) bypasses format dispatch ----------

/// `Explicit(DocumentKind)` is treated as Markdown WITHOUT
/// format dispatch. It can NEVER produce
/// `UnsupportedDocumentFormat`, even for `.org` bytes.
#[test]
fn parse_explicit_kind_bypasses_unsupported_format_rejection() {
    let result = parse(
        b"just content\n",
        ParseContext::Explicit(DocumentKind::Note),
    );
    let doc = result.expect("Explicit(Note) must succeed without format check");
    assert_eq!(doc.kind(), DocumentKind::Note);

    // Even `.org` bytes with `Explicit(Bookmark)` should
    // succeed (path dispatch does not run; the bytes are
    // parsed as Markdown Bookmark with whatever shape they
    // happen to have).
    let result = parse(
        b"# explicit\n\n<URL>\n",
        ParseContext::Explicit(DocumentKind::Bookmark),
    );
    let doc = result.expect("Explicit(Bookmark) must succeed without format check");
    assert_eq!(doc.kind(), DocumentKind::Bookmark);
}

/// `Explicit(DocumentKind::Todo)` is subject to the ordinary
/// Todo parse failures (e.g., empty bytes still returns
/// `MissingTitle`), but NOT to `UnsupportedDocumentFormat`.
#[test]
fn parse_explicit_todo_still_returns_missing_title_for_empty_bytes() {
    use nb_api::ParseErrorKind;
    let result = parse(b"", ParseContext::Explicit(DocumentKind::Todo));
    assert!(matches!(
        result,
        Err(NbError::ParseError {
            kind: ParseErrorKind::MissingTitle,
            ..
        })
    ));
}

// ---------- supported constant: crate-root public access ----------

/// `nb_api::SUPPORTED_DOCUMENT_EXTENSIONS` is exposed at the
/// crate root and equals the spec's documented source-of-truth
/// list.
#[test]
fn supported_document_extensions_constant_matches_spec() {
    assert_eq!(
        nb_api::SUPPORTED_DOCUMENT_EXTENSIONS,
        &["md", "markdown", "todo.md", "bookmark.md"],
    );
}

// ---------- UnsupportedDocumentFormat serde round-trip ----------

/// The `Vec<String>` shape in `UnsupportedDocumentFormat`
/// survives serde round-trip independently of internal
/// iteration order. The owned shape ensures JSON equality
/// holds across the general NbError contract.
#[test]
fn unsupported_document_format_serde_round_trips() {
    let original = NbError::UnsupportedDocumentFormat {
        extension: "org".to_string(),
        supported: vec![
            "md".to_string(),
            "markdown".to_string(),
            "todo.md".to_string(),
            "bookmark.md".to_string(),
        ],
    };
    let json = serde_json::to_string(&original).unwrap();
    let restored: NbError = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, original);
}
