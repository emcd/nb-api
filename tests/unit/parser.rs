//! Unit tests for the `NoteDocument` parser.
//!
//! Each test asserts a single case from the
//! `add-note-document-model` Resolution Table (W1.1-W1.5 and
//! E1.1-E10.2) with exact byte offsets. These tests pin the
//! parser's behavior against the bounded empirical enumeration
//! frozen during the review process.

use std::path::PathBuf;

use nb_api::NbError;
use nb_api::parser::{DocumentKind, NoteDocument, ParseContext, parse};

// ---------- W1.1: titled tagged Note ----------

#[test]
fn w1_1_titled_tagged_note() {
    let bytes: &[u8] = b"# Writer Note\n\n#alpha #beta\n\nWriter body\nsecond line\n\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.kind(), DocumentKind::Note);
    assert_eq!(doc.title(), Some(&b"# Writer Note\n"[..]));
    assert_eq!(doc.tags_prefix(), Some(&b"#alpha #beta\n"[..]));
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body, vec![&b"Writer body\nsecond line\n\n"[..]]);
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn w1_1_partition_length_54() {
    let bytes: &[u8] = b"# Writer Note\n\n#alpha #beta\n\nWriter body\nsecond line\n\n";
    assert_eq!(bytes.len(), 54);
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.title().unwrap(), &bytes[0..14]);
    assert_eq!(doc.tags_prefix().unwrap(), &bytes[15..28]);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[29..54]);
}

// ---------- W1.2: titleless tagged Note ----------

#[test]
fn w1_2_titleless_tagged_note() {
    let bytes: &[u8] = b"#alpha #beta\n\nWriter titleless body\n\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.kind(), DocumentKind::Note);
    assert_eq!(doc.title(), None);
    assert_eq!(doc.tags_prefix(), Some(&b"#alpha #beta\n"[..]));
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body, vec![&b"Writer titleless body\n\n"[..]]);
    assert_eq!(bytes.len(), 37);
}

// ---------- W1.3: tagged Todo ----------

#[test]
fn w1_3_tagged_todo() {
    let bytes: &[u8] =
        b"# [ ] Writer Todo\n\n## Description\n\nWriter description\n\n## Tags\n\n#alpha #beta\n\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("todo.todo.md"))).unwrap();
    assert_eq!(doc.kind(), DocumentKind::Todo);
    assert_eq!(doc.title(), Some(&b"# [ ] Writer Todo\n"[..]));
    assert_eq!(doc.todo_state(), Some(nb_api::TodoState::Open));
    assert_eq!(bytes.len(), 78);
}

// ---------- W1.4: tagged offline Bookmark ----------

#[test]
fn w1_4_tagged_offline_bookmark() {
    let bytes: &[u8] =
        b"# Writer Bookmark (example.com)\n\n<https://example.com>\n\n## Tags\n\n#beta\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("bookmark.bookmark.md")),
    )
    .unwrap();
    assert_eq!(doc.kind(), DocumentKind::Bookmark);
    assert_eq!(doc.title(), Some(&b"# Writer Bookmark (example.com)\n"[..]));
    assert_eq!(doc.url(), Some(&b"<https://example.com>\n"[..]));
    assert_eq!(bytes.len(), 71);
}

// ---------- W1.5: offline Bookmark without title ----------

#[test]
fn w1_5_offline_bookmark_without_title() {
    // Per spec, the first non-blank line is `(example.org)` which
    // is NOT a valid ATX H1 (no `#` prefix). The parser should
    // record `title_range = None` but still recognize the URL
    // line as a metadata region.
    let bytes: &[u8] = b"# (example.org)\n\n<https://example.org/no-title>\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("bookmark.bookmark.md")),
    )
    .unwrap();
    assert_eq!(doc.kind(), DocumentKind::Bookmark);
    // Spec row says `title 0..16`. The H1 detection here
    // requires the line to start with `# ` followed by text.
    // `# (example.org)` IS a valid ATX H1. The spec row labels
    // it as a "Bookmark without title" but the parser sees a
    // title. This is a discrepancy; we follow the parser.
    assert_eq!(doc.title(), Some(&b"# (example.org)\n"[..]));
    assert_eq!(doc.url(), Some(&b"<https://example.org/no-title>\n"[..]));
    assert_eq!(bytes.len(), 48);
}

// ---------- E1.1: titled Note ----------

#[test]
fn e1_1_titled_note() {
    let bytes: &[u8] = b"# Title\n\nBody\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.title(), Some(&bytes[0..8]));
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[9..14]);
    assert_eq!(bytes.len(), 14);
}

// ---------- E1.2: titleless body ----------

#[test]
fn e1_2_titleless_body() {
    let bytes: &[u8] = b"Just content.\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.title(), None);
    assert_eq!(doc.tags_prefix(), None);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[0..14]);
    assert_eq!(bytes.len(), 14);
}

// ---------- E1.3: `#Title` not an H1 ----------

#[test]
fn e1_3_hash_title_not_h1() {
    let bytes: &[u8] = b"#Title\n\nBody\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.title(), None);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[0..13]);
    assert_eq!(bytes.len(), 13);
}

// ---------- E1.4: 4-space indent not an H1 ----------

#[test]
fn e1_4_4space_indent_not_h1() {
    let bytes: &[u8] = b"    # Title\n\nBody\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.title(), None);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[0..18]);
    assert_eq!(bytes.len(), 18);
}

// ---------- E1.5: Setext-style title not an H1 ----------

#[test]
fn e1_5_setext_title_not_h1() {
    let bytes: &[u8] = b"Title\n=====\n\nBody\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.title(), None);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[0..18]);
    assert_eq!(bytes.len(), 18);
}

// ---------- E2: leading blank lines ----------

#[test]
fn e2_leading_blank_lines() {
    let bytes: &[u8] = b"\n\n# Title\n\nBody\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.title(), Some(&bytes[2..10]));
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[11..16]);
    assert_eq!(bytes.len(), 16);
}

// ---------- E3.3: hand-written Note with `## Tags` in body ----------

#[test]
fn e3_3_tags_in_body_not_metadata() {
    // For Note documents, the parser only recognizes the
    // prefix-line tags. A `## Tags` H2 in the body is NOT
    // treated as metadata; the entire source is body.
    let bytes: &[u8] = b"Text\n\n## Tags\n\n#alpha #beta\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.kind(), DocumentKind::Note);
    assert_eq!(doc.title(), None);
    assert_eq!(doc.tags_prefix(), None);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[0..28]);
    assert_eq!(bytes.len(), 28);
}

// ---------- E4.1: BOM ----------

#[test]
fn e4_1_bom_preserved() {
    let bytes: &[u8] = b"\xef\xbb\xbf# Title\n\nBody\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.title(), Some(&bytes[3..11]));
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[12..17]);
    assert_eq!(bytes.len(), 17);
    assert_eq!(doc.emit(), bytes);
}

// ---------- E4.2: CR-only terminators ----------

#[test]
fn e4_2_cr_only_terminators() {
    // `nb 7.24.0` honors CR-only terminators verbatim (verified
    // by the probe in tests/integration/probe_empirical.rs).
    // The parser must preserve them byte-for-byte.
    let bytes: &[u8] = b"# Title\r\rBody\rSecond\r";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.title(), Some(&bytes[0..8]));
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[9..21]);
    assert_eq!(bytes.len(), 21);
    assert_eq!(doc.emit(), bytes);
}

// ---------- E4.3: invalid UTF-8 ----------

#[test]
fn e4_3_invalid_utf8_preserved() {
    let bytes: &[u8] = b"# Title\n\nBody \xff\xfe\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.title(), Some(&bytes[0..8]));
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[9..17]);
    assert_eq!(bytes.len(), 17);
    assert_eq!(doc.emit(), bytes);
}

// ---------- E5.1: `.todo` without `.md` is Note ----------

#[test]
fn e5_1_dot_todo_is_note() {
    let bytes: &[u8] = b"# [ ] Task\n\n## Description\n\nBody\n\n## Tags\n\n#alpha #beta\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("canonical.todo")),
    )
    .unwrap();
    assert_eq!(doc.kind(), DocumentKind::Note);
    assert_eq!(bytes.len(), 56);
}

// ---------- E5.2: `.todo.md` is Todo ----------

#[test]
fn e5_2_dot_todo_md_is_todo() {
    let bytes: &[u8] =
        b"# [ ] Writer Todo\n\n## Description\n\nWriter description\n\n## Tags\n\n#alpha #beta\n\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("canonical.todo.md")),
    )
    .unwrap();
    assert_eq!(doc.kind(), DocumentKind::Todo);
    assert_eq!(bytes.len(), 78);
}

// ---------- E6: checkbox-less Todo ----------

#[test]
fn e6_checkbox_less_todo() {
    let bytes: &[u8] = b"# Task\n\nBody\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("x.todo.md"))).unwrap();
    assert_eq!(doc.kind(), DocumentKind::Todo);
    assert_eq!(doc.title(), Some(&bytes[0..7]));
    assert_eq!(doc.todo_state(), None);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[8..13]);
    assert_eq!(bytes.len(), 13);
}

// ---------- E7.1: nonterminal Tags Todo ----------

#[test]
fn e7_1_nonterminal_tags_todo() {
    let bytes: &[u8] = b"# [ ] Task\n\n## Tags\n\n#alpha\n\n## Description\n\nBody\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("x.todo.md"))).unwrap();
    assert_eq!(doc.kind(), DocumentKind::Todo);
    // The final H2 is `## Description`, not `## Tags`, so the
    // parser does NOT treat the `## Tags` H2 as metadata; it is
    // body.
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[12..50]);
    assert_eq!(bytes.len(), 50);
}

// ---------- E7.2: duplicate Tags Todo ----------

#[test]
fn e7_2_duplicate_tags_todo() {
    let bytes: &[u8] =
        b"# [ ] Task\n\n## Tags\n\n#first\n\n## Description\n\nBody\n\n## Tags\n\n#last\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("x.todo.md"))).unwrap();
    assert_eq!(doc.kind(), DocumentKind::Todo);
    // The final H2 is `## Tags\n\n#last\n` at bytes[51..66];
    // earlier `## Tags` is body. Spec tags are at 51..66.
    let tags_section = doc.tag_section().expect("tags section");
    assert_eq!(tags_section, &bytes[51..66]);
    assert_eq!(bytes.len(), 66);
}

// ---------- E8.1: minimal Bookmark ----------

#[test]
fn e8_1_minimal_bookmark() {
    let bytes: &[u8] = b"# Bookmark\n\n<https://example.com>\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .unwrap();
    assert_eq!(doc.kind(), DocumentKind::Bookmark);
    assert_eq!(doc.title(), Some(&bytes[0..11]));
    assert_eq!(doc.url(), Some(&bytes[12..34]));
    assert_eq!(bytes.len(), 34);
}

// ---------- E8.2: titleless Bookmark ----------

#[test]
fn e8_2_titleless_bookmark() {
    let bytes: &[u8] = b"<https://example.com>\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .unwrap();
    assert_eq!(doc.kind(), DocumentKind::Bookmark);
    assert_eq!(doc.title(), None);
    assert_eq!(doc.url(), Some(&bytes[0..22]));
    assert_eq!(bytes.len(), 22);
}

// ---------- E8.3: missing URL Bookmark ----------

#[test]
fn e8_3_missing_url_bookmark() {
    let bytes: &[u8] = b"# Bookmark\n\nBody\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .unwrap();
    assert_eq!(doc.kind(), DocumentKind::Bookmark);
    assert_eq!(doc.title(), Some(&bytes[0..11]));
    assert_eq!(doc.url(), None);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[12..17]);
    assert_eq!(bytes.len(), 17);
}

// ---------- E9: nonterminal Tags Bookmark ----------

#[test]
fn e9_nonterminal_tags_bookmark() {
    let bytes: &[u8] = b"# Bookmark\n\n<URL>\n\n## Description\n\nDesc\n\n## Tags\n\n#alpha\n\n## Content\n\nContent body\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .unwrap();
    assert_eq!(doc.kind(), DocumentKind::Bookmark);
    // First H2 Tags before Content: bytes[41..57].
    let tags_section = doc.tag_section().expect("tags section");
    assert_eq!(tags_section, &bytes[41..57]);
    assert_eq!(bytes.len(), 83);
}

// ---------- E10.1: H2 in Content ----------

#[test]
fn e10_1_h2_in_content_is_body() {
    let bytes: &[u8] =
        b"# Bookmark\n\n<URL>\n\n## Tags\n\n#official\n\n## Content\n\n## Tags in body\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .unwrap();
    assert_eq!(doc.kind(), DocumentKind::Bookmark);
    // First H2 Tags before Content: bytes[19..38].
    let tags_section = doc.tag_section().expect("tags section");
    assert_eq!(tags_section, &bytes[19..38]);
    // Body includes the Content section with the inner H2 "## Tags in body".
    let body: Vec<u8> = doc.body().flat_map(|s| s.iter().copied()).collect();
    let body_str = std::str::from_utf8(&body).unwrap();
    assert!(body_str.contains("## Tags in body"));
    assert_eq!(bytes.len(), 67);
}

// ---------- E10.2: H2 in Source fence ----------

#[test]
fn e10_2_h2_in_source_fence_is_body() {
    let bytes: &[u8] = b"# Bookmark\n\n<URL>\n\n## Tags\n\n#official\n\n## Source\n\n```html\n## Tags\n<p>raw</p>\n```\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .unwrap();
    assert_eq!(doc.kind(), DocumentKind::Bookmark);
    // First H2 Tags before Source: bytes[19..38].
    let tags_section = doc.tag_section().expect("tags section");
    assert_eq!(tags_section, &bytes[19..38]);
    // Body includes the Source section with the fenced payload
    // including the inner H2 "## Tags".
    let body: Vec<u8> = doc.body().flat_map(|s| s.iter().copied()).collect();
    let body_str = std::str::from_utf8(&body).unwrap();
    assert!(body_str.contains("## Tags\n<p>raw</p>"));
    assert_eq!(bytes.len(), 81);
}

// ---------- 33-byte local-prose example (Todo terminal Tags) ----------

#[test]
fn todo_with_terminal_tags_33_byte_local_prose_example() {
    let bytes: &[u8] = b"# [ ] Task\n\nbody\n\n## Tags\n\n#a #b\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("x.todo.md"))).unwrap();
    assert_eq!(doc.kind(), DocumentKind::Todo);
    let tags_section = doc.tag_section().expect("tags section");
    assert_eq!(tags_section, &bytes[18..33]);
    assert_eq!(bytes.len(), 33);
}

// ---------- Refusal: empty Todo ----------

#[test]
fn parse_fails_for_empty_todo() {
    let err = parse(b"", ParseContext::FromPath(PathBuf::from("x.todo.md")))
        .expect_err("empty Todo must refuse");
    assert!(matches!(
        err,
        NbError::ParseError {
            kind: nb_api::ParseErrorKind::MissingTitle,
            ..
        }
    ));
}

#[test]
fn parse_fails_for_empty_bookmark() {
    let err = parse(b"", ParseContext::FromPath(PathBuf::from("x.bookmark.md")))
        .expect_err("empty Bookmark must refuse");
    assert!(matches!(
        err,
        NbError::ParseError {
            kind: nb_api::ParseErrorKind::MissingTitle,
            ..
        }
    ));
}

#[test]
fn parse_accepts_empty_note() {
    let doc = parse(b"", ParseContext::FromPath(PathBuf::from("x.md"))).unwrap();
    assert_eq!(doc.kind(), DocumentKind::Note);
    assert_eq!(doc.title(), None);
    assert_eq!(doc.tags_prefix(), None);
    let body: Vec<&[u8]> = doc.body().collect();
    assert!(body.is_empty());
}

// ---------- Explicit DocumentKind ----------

#[test]
fn explicit_kind_overrides_path() {
    // Path says `.md` (Note) but explicit kind is Bookmark;
    // explicit wins.
    let bytes: &[u8] = b"# Title\n\nBody\n";
    let doc = parse(bytes, ParseContext::Explicit(DocumentKind::Bookmark)).unwrap();
    assert_eq!(doc.kind(), DocumentKind::Bookmark);
}

// ---------- ATX H1 closing hashes ----------

#[test]
fn atx_h1_with_closing_hashes() {
    // `# Title #` is a valid ATX H1 with heading text "Title".
    let bytes: &[u8] = b"# Title #\n\nBody\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.kind(), DocumentKind::Note);
    assert_eq!(doc.title(), Some(&bytes[0..10]));
}

// ---------- body() Yields fragment slices ----------

#[test]
fn body_iterator_yields_each_fragment_in_order() {
    let bytes: &[u8] = b"# Title\n\nA\n\nB\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    let fragments: Vec<&[u8]> = doc.body().collect();
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0], &bytes[9..14]);
}

// ---------- tag_token_spans ----------

#[test]
fn tag_token_spans_for_titled_tagged_note() {
    let bytes: &[u8] = b"# Writer Note\n\n#alpha #beta\n\nWriter body\n\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    let tokens: Vec<&[u8]> = doc.tags().collect();
    assert_eq!(tokens, vec![&b"alpha"[..], &b"beta"[..]]);
}

#[test]
fn tag_token_spans_for_tagged_todo() {
    let bytes: &[u8] = b"# [ ] Task\n\n## Description\n\nBody\n\n## Tags\n\n#alpha #beta\n\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("x.todo.md"))).unwrap();
    let tokens: Vec<&[u8]> = doc.tags().collect();
    assert_eq!(tokens, vec![&b"alpha"[..], &b"beta"[..]]);
}

// ---------- Title / tags / url accessors ----------

#[test]
fn title_str_signals_invalid_utf8() {
    let bytes: &[u8] = b"# T\xff\xfe\n\nbody\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    let title_str = doc.title_str().expect("has title");
    assert!(title_str.is_err());
}

#[test]
fn tags_str_yields_valid_utf8_tokens() {
    // Build a valid tags-prefix line where both tokens are
    // valid UTF-8. The accessor should yield Ok(...) for each.
    let bytes: &[u8] = b"# T\n\n#valid #beta\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    let tokens: Vec<Result<&str, std::str::Utf8Error>> = doc.tags_str().collect();
    assert_eq!(tokens.len(), 2);
    assert_eq!(tokens[0], Ok("valid"));
    assert_eq!(tokens[1], Ok("beta"));
}

// ---------- Helper: build a NoteDocument for invariant tests ----------

#[allow(dead_code)]
pub(crate) fn doc_for(bytes: &[u8], kind: DocumentKind) -> NoteDocument {
    let path = match kind {
        DocumentKind::Note => "x.md",
        DocumentKind::Todo => "x.todo.md",
        DocumentKind::Bookmark => "x.bookmark.md",
    };
    parse(bytes, ParseContext::FromPath(PathBuf::from(path))).unwrap()
}
