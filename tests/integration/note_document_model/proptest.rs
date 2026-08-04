//! Property-based round-trips and independent expected-domain builders.

use std::path::PathBuf;

use nb_api::parser::{ParseContext, parse};
use proptest::{prop_assert, prop_assert_eq, proptest};

// ---------- Property-based round-trip (every kind) ----------

use proptest::prelude::*;

#[allow(dead_code)]
fn ascii_printable() -> impl Strategy<Value = String> {
    "[ -~]{0,24}"
}

fn short_title() -> impl Strategy<Value = String> {
    "[A-Za-z][A-Za-z ]{0,7}"
}

fn short_tags() -> impl Strategy<Value = Vec<String>> {
    // At least 2 tags are required so the tags-prefix line is
    // recognized by the parser (the regex requires
    // `^#[a-zA-Z0-9_-]+(\s+#[a-zA-Z0-9_-]+)+$` — at least 2
    // tokens). With fewer tags the parser treats the leading
    // `#tag` as body content, which would change the body
    // selection unpredictably.
    prop::collection::vec("[a-z]{1,4}", 2..4)
}

/// Pick a random line terminator. The strategy exercises LF,
/// CR, and CRLF so the parser is verified against all three.
fn any_terminator() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        Just(b"\n".to_vec()),
        Just(b"\r".to_vec()),
        Just(b"\r\n".to_vec()),
    ]
}

/// Pick a count of blank-line separators in [1, 4]. The
/// strategy varies separator patterns so the parser is
/// exercised against single, double, triple, and quadruple
/// blank lines between sections.
fn blank_separator_count() -> impl Strategy<Value = usize> {
    1usize..5
}

/// A non-empty body line strategy that excludes the characters
/// reserved for line terminators and blank lines, so the
/// generated body content survives a round-trip regardless
/// of terminator choice.
fn nonblank_body_line() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9][a-zA-Z0-9 ,._-]{0,30}"
}

proptest! {
    /// Note: every generated document round-trips losslessly,
    /// the partition is invariant, and the fingerprint matches
    /// BLAKE3-256 over the INDEPENDENTLY-derived expected body.
    /// The terminator and blank-line separator count vary.
    #[test]
    fn prop_note_round_trip(
        title in short_title(),
        tags in short_tags(),
        body in nonblank_body_line(),
        terminator in any_terminator(),
        blank_count in blank_separator_count(),
    ) {
        let bytes = build_note_bytes_v2(&title, &tags, &body, &terminator, blank_count);
        let doc = parse(&bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
        prop_assert_eq!(doc.emit(), bytes.as_slice());
        prop_assert!(
            doc.verify_partition().is_ok(),
            "partition invariants violated"
        );
        // INDEPENDENTLY-derived expected body: just `body`
        // followed by one terminator. Derived from generation
        // inputs, NOT from `doc.body()`. A bug in the parser's
        // body selection would cause `doc.body()` to differ
        // from this expected value and the test would fail.
        let mut expected_body = body.into_bytes();
        expected_body.extend_from_slice(&terminator);
        let mut hasher = blake3::Hasher::new();
        hasher.update(&expected_body);
        let expected_fp = hasher.finalize().to_hex().to_string();
        let fp = nb_api::fingerprint::fingerprint(&doc);
        prop_assert_eq!(
            fp.as_hex(),
            expected_fp,
            "fingerprint must match BLAKE3-256 over the independently-derived expected body"
        );
        // Body fragments must match the INDEPENDENTLY-derived
        // expected body.
        let body_fragments: Vec<&[u8]> = doc.body().collect();
        prop_assert_eq!(body_fragments.len(), 1, "expected 1 body fragment");
        prop_assert_eq!(body_fragments[0], expected_body.as_slice());
    }

    /// Todo: every generated document round-trips losslessly,
    /// the partition is invariant, and the fingerprint matches
    /// BLAKE3-256 over the INDEPENDENTLY-derived expected body.
    /// The terminator and blank-line separator count vary.
    #[test]
    fn prop_todo_round_trip(
        title in short_title(),
        body in nonblank_body_line(),
        terminator in any_terminator(),
        blank_count in blank_separator_count(),
    ) {
        let bytes = build_todo_bytes_v2(&title, &body, &terminator, blank_count);
        let doc = parse(&bytes, ParseContext::FromPath(PathBuf::from("x.todo.md"))).unwrap();
        prop_assert_eq!(doc.emit(), bytes.as_slice());
        prop_assert!(
            doc.verify_partition().is_ok(),
            "partition invariants violated"
        );
        // INDEPENDENTLY-derived expected body: just `body`
        // followed by one terminator.
        let mut expected_body = body.into_bytes();
        expected_body.extend_from_slice(&terminator);
        let mut hasher = blake3::Hasher::new();
        hasher.update(&expected_body);
        let expected_fp = hasher.finalize().to_hex().to_string();
        let fp = nb_api::fingerprint::fingerprint(&doc);
        prop_assert_eq!(
            fp.as_hex(),
            expected_fp,
            "fingerprint must match BLAKE3-256 over the independently-derived expected body"
        );
        let body_fragments: Vec<&[u8]> = doc.body().collect();
        prop_assert_eq!(body_fragments.len(), 1, "expected 1 body fragment");
        prop_assert_eq!(body_fragments[0], expected_body.as_slice());
    }

    /// Bookmark: every generated document round-trips
    /// losslessly, the partition is invariant, and the
    /// fingerprint matches BLAKE3-256 over the
    /// INDEPENDENTLY-derived expected body. The terminator
    /// and blank-line separator count vary.
    #[test]
    fn prop_bookmark_round_trip(
        title in short_title(),
        body in nonblank_body_line(),
        terminator in any_terminator(),
        blank_count in blank_separator_count(),
    ) {
        let bytes = build_bookmark_bytes_v2(&title, &body, &terminator, blank_count);
        let doc = parse(&bytes, ParseContext::FromPath(PathBuf::from("x.bookmark.md"))).unwrap();
        prop_assert_eq!(doc.emit(), bytes.as_slice());
        prop_assert!(
            doc.verify_partition().is_ok(),
            "partition invariants violated"
        );
        // INDEPENDENTLY-derived expected body: just `body`
        // followed by one terminator.
        let mut expected_body = body.into_bytes();
        expected_body.extend_from_slice(&terminator);
        let mut hasher = blake3::Hasher::new();
        hasher.update(&expected_body);
        let expected_fp = hasher.finalize().to_hex().to_string();
        let fp = nb_api::fingerprint::fingerprint(&doc);
        prop_assert_eq!(
            fp.as_hex(),
            expected_fp,
            "fingerprint must match BLAKE3-256 over the independently-derived expected body"
        );
        let body_fragments: Vec<&[u8]> = doc.body().collect();
        prop_assert_eq!(body_fragments.len(), 1, "expected 1 body fragment");
        prop_assert_eq!(body_fragments[0], expected_body.as_slice());
    }

    /// Bookmark with `## Tags` AFTER body (terminal metadata).
    /// Body ends WITHOUT a blank line before `## Tags` —
    /// the C4B-P1-1 adjacent-section regression. The expected
    /// body is derived INDEPENDENTLY from the generation
    /// inputs and the terminator and separator count vary.
    #[test]
    fn prop_bookmark_tags_after_body_adjacent(
        title in short_title(),
        body in nonblank_body_line(),
        tags in tag_tokens(),
        terminator in any_terminator(),
        blank_count in blank_separator_count(),
    ) {
        let bytes = build_bookmark_tags_after_body_adjacent_bytes(
            &title, &body, &tags, &terminator, blank_count,
        );
        let doc = parse(
            &bytes,
            ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
        )
        .unwrap();
        prop_assert_eq!(doc.emit(), bytes.as_slice());
        prop_assert!(
            doc.verify_partition().is_ok(),
            "partition invariants violated"
        );

        // The expected body is the Content section bytes
        // constructed from the generation inputs directly.
        // The body fragment starts at the `## Content` heading
        // line and ends at the trailing terminator of the body
        // line. `## Tags` (after body) is metadata and
        // excluded.
        let mut expected_body = Vec::new();
        expected_body.extend_from_slice(b"## Content");
        expected_body.extend_from_slice(&terminator);
        for _ in 0..blank_count {
            expected_body.extend_from_slice(&terminator);
        }
        expected_body.extend_from_slice(body.as_bytes());
        expected_body.extend_from_slice(&terminator);
        let body_fragments: Vec<&[u8]> = doc.body().collect();
        prop_assert_eq!(
            body_fragments.len(),
            1,
            "expected 1 body fragment, got {}",
            body_fragments.len()
        );
        prop_assert_eq!(
            body_fragments[0],
            expected_body.as_slice(),
            "body fragment must match the independently-derived expected body"
        );

        // Fingerprint from INDEPENDENTLY-derived expected body.
        let mut hasher = blake3::Hasher::new();
        hasher.update(&expected_body);
        let expected_fp = hasher.finalize().to_hex().to_string();
        let fp = nb_api::fingerprint::fingerprint(&doc);
        prop_assert_eq!(
            fp.as_hex(),
            expected_fp,
            "fingerprint must match BLAKE3-256 over the independently-derived expected body"
        );
    }
}

// ---------- R3-F3: independent-from-doc.body() generators ----------
//
// The previous proptest block computed the expected fingerprint
// from `doc.body()` — the production view. Per the R3-F3
// finding, the test must compute expected values from the
// generation INPUTS (independently) so a bug in the body
// selection is caught. The proptests below build byte
// sequences with known Tags/Content/Source shapes, compute
// expected `tag_section` and body bytes from the inputs
// (NOT from the parser output), and assert equality.

proptest! {
    /// Todo: every generated document with terminal `## Tags`
    /// round-trips losslessly, the partition is invariant,
    /// `tag_section` matches the INDEPENDENTLY-derived expected
    /// value, the body bytes match the INDEPENDENTLY-derived
    /// expected body, and the fingerprint matches
    /// BLAKE3-256 over those body bytes. The terminator and
    /// blank-line separator count vary.
    #[test]
    fn prop_todo_terminal_tags(
        title in prop::string::string_regex("[a-zA-Z][a-zA-Z0-9 ]{0,30}").unwrap(),
        body in body_line(),
        tags in tag_tokens(),
        terminator in any_terminator(),
        blank_count in blank_separator_count(),
    ) {
        let bytes = build_todo_terminal_tags_bytes_v3(
            &title, &body, &tags, &terminator, blank_count,
        );
        let doc = parse(
            &bytes,
            ParseContext::FromPath(PathBuf::from("x.todo.md")),
        )
        .unwrap();
        prop_assert_eq!(doc.emit(), bytes.as_slice());
        prop_assert!(
            doc.verify_partition().is_ok(),
            "partition invariants violated"
        );

        // Independent expected values: derived from generation
        // inputs, NOT from `doc.body()`.
        let expected_tag =
            expected_todo_terminal_tags_v3(&body, &tags, &terminator, blank_count);
        prop_assert_eq!(
            doc.tag_section(),
            Some(expected_tag.as_slice()),
            "tag_section must match the independently-derived expected value"
        );

        let expected_body =
            expected_todo_terminal_body_v3(&body, &terminator, blank_count);
        let body_fragments: Vec<&[u8]> = doc.body().collect();
        prop_assert_eq!(
            body_fragments.len(),
            1,
            "expected 1 body fragment for terminal-Tags Todo, got {}",
            body_fragments.len()
        );
        prop_assert_eq!(
            body_fragments[0], expected_body.as_slice(),
            "body fragment must match the independently-derived expected body"
        );

        // Fingerprint from the INDEPENDENTLY-derived expected body.
        let mut hasher = blake3::Hasher::new();
        hasher.update(expected_body.as_slice());
        let expected_fp = hasher.finalize().to_hex().to_string();
        let fp = nb_api::fingerprint::fingerprint(&doc);
        prop_assert_eq!(
            fp.as_hex(),
            expected_fp,
            "fingerprint must match BLAKE3-256 over the independently-derived body"
        );
    }

    /// Todo: when `## Tags` is NOT terminal (followed by another
    /// H2), the typed `NotFound` for `tag_section` is the
    /// correct behavior — Tags is body content, not metadata.
    /// The terminator and blank-line separator count vary.
    /// Per C4B-F3-1 closure: input-derived body and fingerprint
    /// assertions are also added (not just `tag_section().is_none()`).
    #[test]
    fn prop_todo_nonterminal_tags(
        title in prop::string::string_regex("[a-zA-Z][a-zA-Z0-9 ]{0,30}").unwrap(),
        body in body_line(),
        tags in tag_tokens(),
        terminator in any_terminator(),
        blank_count in blank_separator_count(),
    ) {
        let bytes = build_todo_nonterminal_tags_bytes_v3(
            &title, &body, &tags, &terminator, blank_count,
        );
        let doc = parse(
            &bytes,
            ParseContext::FromPath(PathBuf::from("x.todo.md")),
        )
        .unwrap();
        prop_assert_eq!(doc.emit(), bytes.as_slice());
        prop_assert!(
            doc.verify_partition().is_ok(),
            "partition invariants violated"
        );

        // Independent expected: when the final H2 is something
        // other than `## Tags`, the Tags section is body
        // content and `tag_section` is None.
        prop_assert!(
            doc.tag_section().is_none(),
            "tag_section must be None when ## Tags is not terminal, got {:?}",
            doc.tag_section()
        );

        // Independent expected body: derived from the generation
        // INPUTS, NOT from `doc.body()`. The non-terminal Tags
        // test has NO `## Tags` heading at the end of the source
        // (the final H2 is `## Description`), so the body
        // fragment is everything after the title and post-title
        // blanks. The expected body bytes are constructed from
        // the generation inputs directly: the `## Tags` section
        // + all separators + the `## Description` section + body
        // line.
        let mut expected_body = Vec::new();
        // ## Tags heading line
        expected_body.extend_from_slice(b"## Tags");
        expected_body.extend_from_slice(&terminator);
        // All blank_count blank lines after Tags heading
        for _ in 0..blank_count {
            expected_body.extend_from_slice(&terminator);
        }
        // Tag tokens line
        for (i, t) in tags.iter().enumerate() {
            if i > 0 {
                expected_body.push(b' ');
            }
            expected_body.push(b'#');
            expected_body.extend_from_slice(t.as_bytes());
        }
        expected_body.extend_from_slice(&terminator);
        // All blank_count blank lines after tag line
        for _ in 0..blank_count {
            expected_body.extend_from_slice(&terminator);
        }
        // ## Description heading line
        expected_body.extend_from_slice(b"## Description");
        expected_body.extend_from_slice(&terminator);
        // All blank_count blank lines after Description heading
        for _ in 0..blank_count {
            expected_body.extend_from_slice(&terminator);
        }
        // Body line
        expected_body.extend_from_slice(body.as_bytes());
        expected_body.extend_from_slice(&terminator);

        let body_fragments: Vec<&[u8]> = doc.body().collect();
        prop_assert_eq!(
            body_fragments.len(),
            1,
            "expected 1 body fragment when ## Tags is non-terminal, got {}",
            body_fragments.len()
        );
        prop_assert_eq!(
            body_fragments[0],
            expected_body.as_slice(),
            "body fragment must match the independently-derived expected body \
             (including ## Tags heading + tags + ## Description heading + body line)"
        );

        // Independent expected fingerprint: BLAKE3-256 over the
        // input-derived expected body (NOT over `doc.body()`).
        // If the parser picks a different body, the
        // fingerprint diverges.
        let mut hasher = blake3::Hasher::new();
        hasher.update(expected_body.as_slice());
        let expected_fp = hasher.finalize().to_hex().to_string();
        let fp = nb_api::fingerprint::fingerprint(&doc);
        prop_assert_eq!(
            fp.as_hex(),
            expected_fp,
            "fingerprint must match BLAKE3-256 over the input-derived expected body"
        );
    }

    /// Bookmark: `## Tags` BEFORE `## Content` is canonical. The
    /// expected `tag_section` and body bytes are derived
    /// INDEPENDENTLY from the generation inputs. The terminator
    /// and blank-line separator count vary.
    #[test]
    fn prop_bookmark_tags_before_content(
        title in prop::string::string_regex("[a-zA-Z][a-zA-Z0-9 ]{0,30}").unwrap(),
        url in prop::string::string_regex("https?://[a-zA-Z0-9.-]{1,30}/[a-zA-Z0-9]{0,10}").unwrap(),
        tags in tag_tokens(),
        content in body_line(),
        terminator in any_terminator(),
        blank_count in blank_separator_count(),
    ) {
        let bytes = build_bookmark_tags_before_content_bytes_v3(
            &title, &url, &tags, &content, &terminator, blank_count,
        );
        let doc = parse(
            &bytes,
            ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
        )
        .unwrap();
        prop_assert_eq!(doc.emit(), bytes.as_slice());
        prop_assert!(
            doc.verify_partition().is_ok(),
            "partition invariants violated"
        );

        // Independent expected values. Tags is followed by
        // ## Content, so it is NOT the last heading.
        let expected_tag = expected_bookmark_tags_section_v3(
            &tags, &terminator, blank_count, false,
        );
        prop_assert_eq!(
            doc.tag_section(),
            Some(expected_tag.as_slice()),
            "tag_section must match the independently-derived expected value"
        );

        let expected_body = expected_bookmark_content_section_v3(
            &content, &terminator, blank_count,
        );
        let body_fragments: Vec<&[u8]> = doc.body().collect();
        prop_assert_eq!(
            body_fragments.len(),
            1,
            "expected 1 body fragment for Tags-before-Content, got {}",
            body_fragments.len()
        );
        prop_assert_eq!(
            body_fragments[0], expected_body.as_slice(),
            "body fragment must match the independently-derived expected body"
        );

        // Fingerprint from INDEPENDENTLY-derived expected body.
        let mut hasher = blake3::Hasher::new();
        hasher.update(expected_body.as_slice());
        let expected_fp = hasher.finalize().to_hex().to_string();
        let fp = nb_api::fingerprint::fingerprint(&doc);
        prop_assert_eq!(
            fp.as_hex(),
            expected_fp,
            "fingerprint must match BLAKE3-256 over the independently-derived body"
        );
    }

    /// Bookmark: `## Tags` terminal AFTER `## Content`. The
    /// expected `tag_section` and body bytes are derived
    /// INDEPENDENTLY from the generation inputs. The terminator
    /// and blank-line separator count vary.
    #[test]
    fn prop_bookmark_tags_after_content_terminal(
        title in prop::string::string_regex("[a-zA-Z][a-zA-Z0-9 ]{0,30}").unwrap(),
        url in prop::string::string_regex("https?://[a-zA-Z0-9.-]{1,30}/[a-zA-Z0-9]{0,10}").unwrap(),
        content in body_line(),
        tags in tag_tokens(),
        terminator in any_terminator(),
        blank_count in blank_separator_count(),
    ) {
        let bytes = build_bookmark_tags_after_content_terminal_bytes_v3(
            &title, &url, &content, &tags, &terminator, blank_count,
        );
        let doc = parse(
            &bytes,
            ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
        )
        .unwrap();
        prop_assert_eq!(doc.emit(), bytes.as_slice());
        prop_assert!(
            doc.verify_partition().is_ok(),
            "partition invariants violated"
        );

        // Expected `tag_section` is the trailing Tags section.
        // Tags IS the last heading (no next section after Tags).
        let expected_tag = expected_bookmark_tags_section_v3(
            &tags, &terminator, blank_count, true,
        );
        prop_assert_eq!(
            doc.tag_section(),
            Some(expected_tag.as_slice()),
            "tag_section must match the independently-derived expected value"
        );

        // Expected body is the Content section. The bookmark
        // partition's body fragment starts at the `## Content`
        // heading and ends at the trailing terminator of the
        // content line. Tags (after Content) is metadata and
        // excluded.
        let expected_body = expected_bookmark_content_with_trailing_term_v3(
            &content, &terminator, blank_count,
        );
        let body_fragments: Vec<&[u8]> = doc.body().collect();
        prop_assert_eq!(
            body_fragments.len(),
            1,
            "expected 1 body fragment for Tags-after-Content, got {}",
            body_fragments.len()
        );
        prop_assert_eq!(
            body_fragments[0], expected_body.as_slice(),
            "body fragment must match the independently-derived expected body"
        );

        // Fingerprint from INDEPENDENTLY-derived expected body.
        let mut hasher = blake3::Hasher::new();
        hasher.update(expected_body.as_slice());
        let expected_fp = hasher.finalize().to_hex().to_string();
        let fp = nb_api::fingerprint::fingerprint(&doc);
        prop_assert_eq!(
            fp.as_hex(),
            expected_fp,
            "fingerprint must match BLAKE3-256 over the independently-derived body"
        );
    }

    /// Bookmark: `## Source` (with fenced HTML payload) followed
    /// by terminal `## Tags`. The expected `tag_section` and
    /// body bytes are derived INDEPENDENTLY from the
    /// generation inputs. The terminator and blank-line
    /// separator count vary.
    #[test]
    fn prop_bookmark_source_terminal_tags(
        title in prop::string::string_regex("[a-zA-Z][a-zA-Z0-9 ]{0,30}").unwrap(),
        url in prop::string::string_regex("https?://[a-zA-Z0-9.-]{1,30}/[a-zA-Z0-9]{0,10}").unwrap(),
        source_payload in prop::string::string_regex("[a-zA-Z0-9 ,._<>/-]{1,40}").unwrap(),
        tags in tag_tokens(),
        terminator in any_terminator(),
        blank_count in blank_separator_count(),
    ) {
        let bytes = build_bookmark_source_terminal_tags_bytes_v3(
            &title, &url, &source_payload, &tags, &terminator, blank_count,
        );
        let doc = parse(
            &bytes,
            ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
        )
        .unwrap();
        prop_assert_eq!(doc.emit(), bytes.as_slice());
        prop_assert!(
            doc.verify_partition().is_ok(),
            "partition invariants violated"
        );

        // Expected `tag_section` is the trailing Tags section.
        // Tags IS the last heading (no next section after Tags).
        let expected_tag = expected_bookmark_tags_section_v3(
            &tags, &terminator, blank_count, true,
        );
        prop_assert_eq!(
            doc.tag_section(),
            Some(expected_tag.as_slice()),
            "tag_section must match the independently-derived expected value"
        );

        // Expected body is the Source section.
        let expected_body = expected_bookmark_source_section_v3(
            &source_payload, &terminator, blank_count,
        );
        let body_fragments: Vec<&[u8]> = doc.body().collect();
        prop_assert_eq!(
            body_fragments.len(),
            1,
            "expected 1 body fragment for Source+Tags, got {}",
            body_fragments.len()
        );
        prop_assert_eq!(
            body_fragments[0], expected_body.as_slice(),
            "body fragment must match the independently-derived expected body"
        );

        // Fingerprint from INDEPENDENTLY-derived expected body.
        let mut hasher = blake3::Hasher::new();
        hasher.update(expected_body.as_slice());
        let expected_fp = hasher.finalize().to_hex().to_string();
        let fp = nb_api::fingerprint::fingerprint(&doc);
        prop_assert_eq!(
            fp.as_hex(),
            expected_fp,
            "fingerprint must match BLAKE3-256 over the independently-derived body"
        );
    }
}

/// A small printable body line. The regex requires at least
/// one non-whitespace character so the generator never
/// produces whitespace-only or empty bodies, which the parser
/// would not recognize as a body fragment.
fn body_line() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-zA-Z0-9][a-zA-Z0-9 ,._-]{0,39}").unwrap()
}

/// A single tag token. We restrict to simple identifiers to
/// avoid whitespace and punctuation edge cases.
fn tag_token() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-z][a-z0-9_-]{0,7}").unwrap()
}

/// A small list of tag tokens.
fn tag_tokens() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(tag_token(), 1..4)
}

// ---------- C4B-F3-1: proptest v3 builders and expected helpers ----------
//
// The `_v3` builders and `expected_*_v3` helpers extend the
// original v1 (fixed LF, fixed double-blank) shapes with two
// randomized dimensions:
//
//   - `terminator` (LF / CR / CRLF): every line terminator in
//     the document uses the chosen terminator. The parser must
//     accept all three.
//   - `blank_count` (1..5): the number of blank-line separators
//     between sections. The parser must accept single, double,
//     triple, and quadruple blank lines.
//
// Every expected value (`expected_*_v3`) is derived
// INDEPENDENTLY from the generation inputs — terminator,
// blank_count, and the content strings. A bug in the parser's
// section/body selection that affects only some terminators
// or only some blank counts will be caught by the proptest.

/// Write `count` blank-line terminators as separators.
fn write_blank_lines(bytes: &mut Vec<u8>, count: usize, terminator: &[u8]) {
    for _ in 0..count {
        bytes.extend_from_slice(terminator);
    }
}

/// Build a Note byte sequence with the chosen terminator and
/// blank-line separator count.
///
/// Layout:
/// ```
/// # <title>
/// <blank_count x terminator>
/// #<tag> #<tag> ...
/// <blank_count x terminator>
/// <body>
/// <terminator>
/// ```
fn build_note_bytes_v2(
    title: &str,
    tags: &[String],
    body: &str,
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    if !title.is_empty() {
        bytes.extend_from_slice(b"# ");
        bytes.extend_from_slice(title.as_bytes());
        bytes.extend_from_slice(terminator);
        write_blank_lines(&mut bytes, blank_count, terminator);
        if !tags.is_empty() {
            for (i, tag) in tags.iter().enumerate() {
                if i > 0 {
                    bytes.push(b' ');
                }
                bytes.push(b'#');
                bytes.extend_from_slice(tag.as_bytes());
            }
            bytes.extend_from_slice(terminator);
        }
    }
    // If the title is empty, do not emit a tags-prefix line.
    // The parser treats a leading `#tag` (single-token, no
    // preceding H1) as body content, not as a tags prefix, so
    // including a tags-prefix line under an empty title would
    // shift the body selection unpredictably. The round-trip
    // is permitted when title is non-empty.
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(body.as_bytes());
    bytes.extend_from_slice(terminator);
    bytes
}

/// Build a Todo byte sequence with the chosen terminator and
/// blank-line separator count.
///
/// Layout:
/// ```
/// # [ ] <title>
/// <blank_count x terminator>
/// <body>
/// <terminator>
/// ```
fn build_todo_bytes_v2(title: &str, body: &str, terminator: &[u8], blank_count: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"# [ ] ");
    bytes.extend_from_slice(title.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(body.as_bytes());
    bytes.extend_from_slice(terminator);
    bytes
}

/// Build a Bookmark byte sequence with the chosen terminator
/// and blank-line separator count.
///
/// Layout:
/// ```
/// # <title>
/// <blank_count x terminator>
/// <URL>
/// <blank_count x terminator>
/// <body>
/// <terminator>
/// ```
fn build_bookmark_bytes_v2(
    title: &str,
    body: &str,
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"# ");
    bytes.extend_from_slice(title.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"<https://example.com>\n");
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(body.as_bytes());
    bytes.extend_from_slice(terminator);
    let _ = title;
    bytes
}

/// Build a Todo byte sequence with terminal `## Tags` and the
/// chosen terminator and blank-line separator count.
///
/// Layout:
/// ```
/// # [ ] <title>
/// <blank_count x terminator>
/// <body>
/// <terminator>
/// <blank_count x terminator>
/// ## Tags
/// <blank_count x terminator>
/// #<tag> #<tag> ...
/// <terminator>
/// ```
fn build_todo_terminal_tags_bytes_v3(
    title: &str,
    body: &str,
    tags: &[String],
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"# [ ] ");
    bytes.extend_from_slice(title.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(body.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Tags");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    for (i, t) in tags.iter().enumerate() {
        if i > 0 {
            bytes.push(b' ');
        }
        bytes.push(b'#');
        bytes.extend_from_slice(t.as_bytes());
    }
    bytes.extend_from_slice(terminator);
    bytes
}

/// Build a Todo byte sequence where `## Tags` is NOT terminal
/// with the chosen terminator and blank-line separator count.
/// Per the spec, terminal Tags is the LAST H2 — when the last
/// H2 is something else, the Tags section is body content, not
/// metadata.
fn build_todo_nonterminal_tags_bytes_v3(
    title: &str,
    body: &str,
    tags: &[String],
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"# [ ] ");
    bytes.extend_from_slice(title.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Tags");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    for (i, t) in tags.iter().enumerate() {
        if i > 0 {
            bytes.push(b' ');
        }
        bytes.push(b'#');
        bytes.extend_from_slice(t.as_bytes());
    }
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Description");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(body.as_bytes());
    bytes.extend_from_slice(terminator);
    bytes
}

/// Build a Bookmark with `## Tags` before `## Content`, with the
/// chosen terminator and blank-line separator count. The
/// canonical-selection rule picks the FIRST Tags before
/// Content; the body fragment is the Content section.
fn build_bookmark_tags_before_content_bytes_v3(
    title: &str,
    url: &str,
    tags: &[String],
    content: &str,
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"# ");
    bytes.extend_from_slice(title.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.push(b'<');
    bytes.extend_from_slice(url.as_bytes());
    bytes.push(b'>');
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Tags");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    for (i, t) in tags.iter().enumerate() {
        if i > 0 {
            bytes.push(b' ');
        }
        bytes.push(b'#');
        bytes.extend_from_slice(t.as_bytes());
    }
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Content");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(content.as_bytes());
    bytes.extend_from_slice(terminator);
    bytes
}

/// Build a Bookmark with `## Tags` AFTER `## Content` (terminal),
/// with the chosen terminator and blank-line separator count.
/// The body fragment is the Content section only.
fn build_bookmark_tags_after_content_terminal_bytes_v3(
    title: &str,
    url: &str,
    content: &str,
    tags: &[String],
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"# ");
    bytes.extend_from_slice(title.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.push(b'<');
    bytes.extend_from_slice(url.as_bytes());
    bytes.push(b'>');
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Content");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(content.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Tags");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    for (i, t) in tags.iter().enumerate() {
        if i > 0 {
            bytes.push(b' ');
        }
        bytes.push(b'#');
        bytes.extend_from_slice(t.as_bytes());
    }
    bytes.extend_from_slice(terminator);
    bytes
}

/// Build a Bookmark with `## Source` (fenced HTML payload)
/// followed by terminal `## Tags`, with the chosen terminator
/// and blank-line separator count.
fn build_bookmark_source_terminal_tags_bytes_v3(
    title: &str,
    url: &str,
    source_payload: &str,
    tags: &[String],
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"# ");
    bytes.extend_from_slice(title.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.push(b'<');
    bytes.extend_from_slice(url.as_bytes());
    bytes.push(b'>');
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Source");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"```html");
    bytes.extend_from_slice(terminator);
    bytes.extend_from_slice(source_payload.as_bytes());
    bytes.extend_from_slice(terminator);
    bytes.extend_from_slice(b"```");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Tags");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    for (i, t) in tags.iter().enumerate() {
        if i > 0 {
            bytes.push(b' ');
        }
        bytes.push(b'#');
        bytes.extend_from_slice(t.as_bytes());
    }
    bytes.extend_from_slice(terminator);
    bytes
}

/// Build a Bookmark with `## Content` (body section) directly
/// followed by terminal `## Tags` with NO blank-line
/// separator. This is the C4B-P1-1 adjacent-section regression
/// shape: the parser must preserve the body's trailing
/// terminator as part of the body, not strip it into a
/// separator.
fn build_bookmark_tags_after_body_adjacent_bytes(
    title: &str,
    body: &str,
    tags: &[String],
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"# ");
    bytes.extend_from_slice(title.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Content");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(body.as_bytes());
    bytes.extend_from_slice(terminator);
    // No blank line between body and Tags — adjacent sections.
    bytes.extend_from_slice(b"## Tags");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    for (i, t) in tags.iter().enumerate() {
        if i > 0 {
            bytes.push(b' ');
        }
        bytes.push(b'#');
        bytes.extend_from_slice(t.as_bytes());
    }
    bytes.extend_from_slice(terminator);
    bytes
}

/// Expected `tag_section` bytes for a Bookmark with `## Tags`,
/// derived INDEPENDENTLY from the generation inputs.
///
/// `is_last_heading` distinguishes two cases:
///
/// - `true` (Tags is the LAST heading, no trailing section):
///   the section extends to `source.len()` and includes only
///   the heading + heading terminator + `blank_count` blank
///   line terminators (between heading and tag line) + tag
///   line + tag-line terminator. There are no blank line
///   terminators AFTER the tag line because the source ends
///   there.
/// - `false` (Tags is followed by another heading, e.g.
///   Content): the section's end is computed by stripping the
///   LAST blank-line terminator; only `blank_count - 1` blank
///   line terminators after the tag-token line are absorbed
///   into the section.
fn expected_bookmark_tags_section_v3(
    tags: &[String],
    terminator: &[u8],
    blank_count: usize,
    is_last_heading: bool,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"## Tags");
    bytes.extend_from_slice(terminator);
    // All `blank_count` blank-line terminators between heading
    // and tag line are absorbed.
    for _ in 0..blank_count {
        bytes.extend_from_slice(terminator);
    }
    for (i, t) in tags.iter().enumerate() {
        if i > 0 {
            bytes.push(b' ');
        }
        bytes.push(b'#');
        bytes.extend_from_slice(t.as_bytes());
    }
    bytes.extend_from_slice(terminator);
    // Blank-line terminators after the tag line: 0 if Tags is
    // last (the source ends with the tag-line terminator);
    // (blank_count - 1) otherwise (the Nth is the separator,
    // stripped).
    let trailing_blanks = if is_last_heading {
        0
    } else {
        blank_count.saturating_sub(1)
    };
    for _ in 0..trailing_blanks {
        bytes.extend_from_slice(terminator);
    }
    bytes
}

/// Expected body bytes for a Bookmark with `## Content` as the
/// LAST heading, derived INDEPENDENTLY from the generation
/// inputs. The body starts at the `## Content` heading and
/// extends to the source end; ALL `blank_count` blank-line
/// terminators are absorbed into the section (no separator
/// stripping because Content is the last heading).
fn expected_bookmark_content_section_v3(
    content: &str,
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"## Content");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(content.as_bytes());
    bytes.extend_from_slice(terminator);
    bytes
}

/// Expected body bytes for a Bookmark with `## Content`
/// followed by terminal `## Tags`. The section absorbs ALL
/// `blank_count` blank-line terminators between the Content
/// heading and the content line, the content line, its
/// terminator, and `blank_count - 1` blank-line terminators
/// between the content line and the Tags heading (the LAST
/// one is stripped as separator).
fn expected_bookmark_content_with_trailing_term_v3(
    content: &str,
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"## Content");
    bytes.extend_from_slice(terminator);
    // All `blank_count` blank-line terminators between Content
    // heading and content line are absorbed.
    for _ in 0..blank_count {
        bytes.extend_from_slice(terminator);
    }
    bytes.extend_from_slice(content.as_bytes());
    bytes.extend_from_slice(terminator);
    // (blank_count - 1) blank-line terminators after the
    // content line are absorbed; the LAST one is the separator.
    for _ in 0..blank_count.saturating_sub(1) {
        bytes.extend_from_slice(terminator);
    }
    bytes
}

/// Expected body bytes for a Bookmark with `## Source` and a
/// fenced HTML payload, followed by terminal `## Tags`. The
/// section absorbs `blank_count` blank-line terminators
/// between Source heading and the opening fence, all bytes
/// inside the fenced payload, and `blank_count - 1` blank-line
/// terminators between the closing fence and the Tags heading
/// (the LAST one is stripped as separator).
fn expected_bookmark_source_section_v3(
    source_payload: &str,
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"## Source");
    bytes.extend_from_slice(terminator);
    for _ in 0..blank_count {
        bytes.extend_from_slice(terminator);
    }
    bytes.extend_from_slice(b"```html");
    bytes.extend_from_slice(terminator);
    bytes.extend_from_slice(source_payload.as_bytes());
    bytes.extend_from_slice(terminator);
    bytes.extend_from_slice(b"```");
    bytes.extend_from_slice(terminator);
    for _ in 0..blank_count.saturating_sub(1) {
        bytes.extend_from_slice(terminator);
    }
    bytes
}

/// Expected `tag_section` bytes for a Todo with terminal
/// `## Tags`, derived INDEPENDENTLY from the generation inputs.
fn expected_todo_terminal_tags_v3(
    _body: &str,
    tags: &[String],
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"## Tags");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    for (i, t) in tags.iter().enumerate() {
        if i > 0 {
            bytes.push(b' ');
        }
        bytes.push(b'#');
        bytes.extend_from_slice(t.as_bytes());
    }
    bytes.extend_from_slice(terminator);
    bytes
}

/// Expected body bytes for a Todo with terminal `## Tags`.
/// The parser's pre-Tags separator handling emits only ONE
/// separator (the LAST blank line terminator); the other
/// `blank_count - 1` blank line terminators are absorbed into
/// the body fragment. This matches the cycle-4a R2-F1 verdict:
/// a "complete blank line" between body and Tags is a
/// separator, but the pre-Tags separator handling does not
/// split multiple blank lines into multiple separator ranges.
fn expected_todo_terminal_body_v3(body: &str, terminator: &[u8], blank_count: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(body.as_bytes());
    bytes.extend_from_slice(terminator);
    for _ in 0..blank_count.saturating_sub(1) {
        bytes.extend_from_slice(terminator);
    }
    bytes
}
