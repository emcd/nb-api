//! Property-based round-trips and independent expected-domain builders.

use std::path::PathBuf;

use nb_api::parser::{ParseContext, parse};
use proptest::{prop_assert, prop_assert_eq, proptest};

// ---------- Property-based round-trip (every kind) ----------

use proptest::prelude::*;

use super::proptest_helpers::*;

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
    /// the adjacent-section ownership regression. The expected
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

// ---------- Independent-from-doc.body() generators ----------
//
// The previous proptest block computed the expected fingerprint
// from `doc.body()` — the production view. Expected domains must
// be derived from generation INPUTS (independently) so a bug in
// body selection is caught. The proptests below build byte
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
    /// Input-derived body and fingerprint
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
