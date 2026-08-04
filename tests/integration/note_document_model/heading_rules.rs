//! Exact reserved-heading and role-boundary rules for Todo/Bookmark Tags.

use std::path::PathBuf;

use nb_api::parser::{DocumentKind, ParseContext, parse};

use crate::common::find_subsequence;

// ---------- terminal Tags after Content/Source (R3-F1 coverage) ----------

/// Frozen scenario added by R3-F1: Tags terminal AFTER Content
/// SHALL canonicalize the Tags section as metadata. The
/// bookmark canonical-Tags rule picks the FIRST Tags that
/// precedes any Content/Source; if none precedes, it falls
/// through to the LAST Tags section in the file (which here is
/// the sole-terminal Tags after Content).
///
/// Pre-R3-F1 this case was untested; the cycle-3 implementation
/// already handles it correctly per the spec pseudocode, so the
/// failing-first test pins the behavior.
#[test]
fn bookmark_tags_terminal_after_content_is_canonical_metadata() {
    let bytes: &[u8] = b"# Bm\n\n<URL>\n\n## Content\n\nbody\n\n## Tags\n\n#a\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("bm.bookmark.md")),
    )
    .expect("parse must succeed for valid bookmark");

    let tags_start = find_subsequence(bytes, b"## Tags").expect("## Tags in input");
    let expected_tags = &bytes[tags_start..];
    assert_eq!(
        doc.tag_section(),
        Some(expected_tags),
        "Tags terminal after Content SHALL be canonical metadata"
    );

    let bodies: Vec<&[u8]> = doc.body().collect();
    let tags_pat = expected_tags;
    for (i, body) in bodies.iter().enumerate() {
        assert!(
            !body.windows(tags_pat.len()).any(|w| w == tags_pat),
            "Tags bytes must NOT appear inside body: body[{i}]={body:?}"
        );
    }

    let content_offset = find_subsequence(bytes, b"## Content").expect("## Content in input");
    let content_pat_len = b"## Content".len();
    let content_pat = &bytes[content_offset..content_offset + content_pat_len];
    assert!(
        bodies
            .iter()
            .any(|b| b.windows(content_pat_len).any(|w| w == content_pat)),
        "Content section should appear in some body fragment"
    );

    assert_eq!(doc.kind(), DocumentKind::Bookmark);
}

#[test]
fn bookmark_tags_terminal_after_source_is_canonical_metadata() {
    let bytes: &[u8] = b"# Bm\n\n<URL>\n\n## Source\n\n```html\n<p>raw</p>\n```\n\n## Tags\n\n#a\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("bm.bookmark.md")),
    )
    .expect("parse must succeed for bookmark with Source");

    let tags_start = find_subsequence(bytes, b"## Tags").expect("## Tags in input");
    let expected_tags = &bytes[tags_start..];
    assert_eq!(
        doc.tag_section(),
        Some(expected_tags),
        "Tags terminal after Source SHALL be canonical metadata"
    );

    let bodies: Vec<&[u8]> = doc.body().collect();
    for (i, body) in bodies.iter().enumerate() {
        assert!(
            !body
                .windows(expected_tags.len())
                .any(|w| w == expected_tags),
            "Tags bytes must NOT appear inside body: body[{i}]={body:?}"
        );
    }

    assert_eq!(doc.kind(), DocumentKind::Bookmark);
}

// ---------- Decision 17 boundary: rename HeadingRole ----------

/// Decision 17 Revision 3 replaces the boolean
/// `H2Section::is_inner_body_h2` (and the kebab-case
/// `is_inner_body_h2` consumer check at the bookmark body
/// cursor) with the explicit `HeadingRole::{SectionBoundary,
/// InternalBody}` enum. The rename is observable only via the
/// internal type; the public spec is unchanged but the
/// type-system clarity around outer section boundaries vs.
/// inner-body H2s is materially better.
#[test]
#[allow(non_snake_case)]
fn bookmark_assembler_distinguishes_section_boundary_from_internal_body_role() {
    let bytes: &[u8] =
        b"# Bookmark\n\n<URL>\n\n## Tags\n\n#official\n\n## Content\n\n## Tags in body\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("bm.bookmark.md")),
    )
    .expect("parse must succeed");

    let tags_start = find_subsequence(bytes, b"## Tags").expect("## Tags in input");
    let first_tags_section_end = tags_start + b"## Tags\n\n#official\n".len();
    let expected_tags = &bytes[tags_start..first_tags_section_end];
    assert_eq!(
        doc.tag_section(),
        Some(expected_tags),
        "Canonical Tags is the FIRST Tags before Content"
    );

    let inner_tags_in_body_offset =
        find_subsequence(bytes, b"## Tags in body").expect("inner Tags in input");
    let inner_pat_len = b"## Tags in body".len();
    let inner_pat = &bytes[inner_tags_in_body_offset..inner_tags_in_body_offset + inner_pat_len];
    let bodies: Vec<&[u8]> = doc.body().collect();
    assert!(
        bodies
            .iter()
            .any(|b| b.windows(inner_pat_len).any(|w| w == inner_pat)),
        "Inner '## Tags in body' H2 must be body content"
    );
}

// ---------- Cycle-4e: negative regressions for non-exact reserved headings ----------

/// `### Tags` is an H3 heading, not an H2 reserved marker. The
/// bookmark classifier must NOT classify it as `SectionBoundary`
/// — `tag_section` MUST be `None` for this fixture.
#[test]
#[allow(non_snake_case)]
fn cycle_4e_h3_tags_does_not_become_canonical_tag_section() {
    let bytes: &[u8] = b"# Bookmark\n<U>\n\n### Tags\n\n#alpha\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .expect("parse must succeed");
    assert!(
        doc.tag_section().is_none(),
        "### Tags must NOT be promoted to canonical tag_section; classifier \
         must treat H3+ as InternalBody. got tag_section={:?}",
        doc.tag_section().map(std::str::from_utf8)
    );
    assert_eq!(doc.emit(), bytes);
}

/// ` ## Tags` has a single leading space, so it is a CommonMark
/// ATX H2 with leading indent but NOT an exact reserved
/// `## Tags` candidate (the frozen rule requires no leading
/// whitespace). `tag_section` MUST be `None`.
#[test]
#[allow(non_snake_case)]
fn cycle_4e_indented_tags_does_not_become_canonical_tag_section() {
    let bytes: &[u8] = b"# Bookmark\n<U>\n\n ## Tags\n\n#alpha\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .expect("parse must succeed");
    assert!(
        doc.tag_section().is_none(),
        "leading-indent ' ## Tags' must NOT be promoted to canonical \
         tag_section; the rule is exact `## Tags` only. got tag_section={:?}",
        doc.tag_section().map(std::str::from_utf8)
    );
    assert_eq!(doc.emit(), bytes);
}

/// `##\tTags` uses a tab character (not space) after `##`. The
/// CommonMark ATX-2 detector accepts it, but the frozen exact-
/// reserved rule requires a single ASCII space. `tag_section`
/// MUST be `None`.
#[test]
#[allow(non_snake_case)]
fn cycle_4e_tab_after_hashes_does_not_become_canonical_tag_section() {
    let bytes: &[u8] = b"# Bookmark\n<U>\n\n##\tTags\n\n#alpha\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .expect("parse must succeed");
    assert!(
        doc.tag_section().is_none(),
        "`##\\tTags` must NOT be promoted to canonical tag_section; the \
         exact-reserved rule requires a single ASCII space, not a tab. \
         got tag_section={:?}",
        doc.tag_section().map(std::str::from_utf8)
    );
    assert_eq!(doc.emit(), bytes);
}

/// `## Tags ##` carries a closing-hash sequence after the
/// heading text. The CommonMark ATX heading *text* normalizes
/// to `Tags`, but the exact-reserved rule treats the closing
/// sequence as a non-exact variant. `tag_section` MUST be
/// `None`.
#[test]
#[allow(non_snake_case)]
fn cycle_4e_closing_hashes_does_not_become_canonical_tag_section() {
    let bytes: &[u8] = b"# Bookmark\n<U>\n\n## Tags ##\n\n#alpha\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .expect("parse must succeed");
    assert!(
        doc.tag_section().is_none(),
        "`## Tags ##` must NOT be promoted to canonical tag_section; the \
         exact-reserved rule excludes closing-hash sequences. got \
         tag_section={:?}",
        doc.tag_section().map(std::str::from_utf8)
    );
    assert_eq!(doc.emit(), bytes);
}

// ---------- Cycle-4e: EOF-unterminated regression coverage ----------

/// EOF-unterminated Bookmark: title at byte 0..10, URL line at
/// byte 11..14 with no trailing newline. The tokenizer must
/// emit a `Line` whose `terminator == b""` and `content`
/// carries `<U>`; the bookmark assembler must still recognize
/// the URL line.
#[test]
#[allow(non_snake_case)]
fn cycle_4e_eof_unterminated_bookmark_title_and_url() {
    let bytes: &[u8] = b"# Bookmark\n<U>";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .expect("parse must succeed");
    let title = doc
        .title_str()
        .expect("title must be present")
        .expect("title must be valid UTF-8");
    assert_eq!(title, "# Bookmark\n");
    let url = doc
        .url_str()
        .expect("url must be present")
        .expect("url must be valid UTF-8");
    assert_eq!(url, "<U>");
    assert_eq!(doc.emit(), bytes);
}

// ---------- Cycle-4f: Todo exact-candidate role check ----------

/// `### Tags` is an H3 heading, not the literal exact `## Tags`
/// canonical form. The Todo assembler MUST require
/// `text == "Tags" && role == SectionBoundary`. Without the
/// role check, the closing H3 of the document would be picked
/// as the canonical terminal Tags section just because its
/// normalized heading text is `Tags`. Per cycle-4e verdict,
/// this regression confirms `tag_section` is `None`.
#[test]
#[allow(non_snake_case)]
fn cycle_4f_todo_h3_tags_does_not_become_terminal_metadata() {
    let bytes: &[u8] = b"# [ ] Task\n\nbody\n\n### Tags\n\n#alpha\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("x.todo.md")))
        .expect("parse must succeed");
    assert!(
        doc.tag_section().is_none(),
        "### Tags (terminal H3 with normalized text 'Tags') must NOT be \
         promoted to canonical tag_section; the assembler must require \
         text=='Tags' && role==SectionBoundary. got tag_section={:?}",
        doc.tag_section().map(std::str::from_utf8)
    );
    assert_eq!(doc.emit(), bytes);
}

/// ` ## Tags` has a single leading space. The Todo classifier
/// marks it with `role = InternalBody` (the exact-form rule
/// requires literal `## Tags` with no leading indent). The
/// assembler must NOT promote it to canonical tag_section.
#[test]
#[allow(non_snake_case)]
fn cycle_4f_todo_indented_tags_does_not_become_terminal_metadata() {
    let bytes: &[u8] = b"# [ ] Task\n\nbody\n\n ## Tags\n\n#alpha\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("x.todo.md")))
        .expect("parse must succeed");
    assert!(
        doc.tag_section().is_none(),
        "leading-indent ' ## Tags' must NOT be promoted to canonical \
         tag_section; the rule is literal exact `## Tags` only. \
         got tag_section={:?}",
        doc.tag_section().map(std::str::from_utf8)
    );
    assert_eq!(doc.emit(), bytes);
}

/// `##\tTags` uses a tab (not space) after `##`. The Todo
/// classifier marks it with `role = InternalBody`. The
/// assembler must NOT promote it to canonical tag_section.
#[test]
#[allow(non_snake_case)]
fn cycle_4f_todo_tab_after_hashes_does_not_become_terminal_metadata() {
    let bytes: &[u8] = b"# [ ] Task\n\nbody\n\n##\tTags\n\n#alpha\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("x.todo.md")))
        .expect("parse must succeed");
    assert!(
        doc.tag_section().is_none(),
        "`##\\tTags` must NOT be promoted to canonical tag_section; the \
         exact-form rule requires a single ASCII space, not a tab. \
         got tag_section={:?}",
        doc.tag_section().map(std::str::from_utf8)
    );
    assert_eq!(doc.emit(), bytes);
}

/// `## Tags ##` carries a closing-hash sequence. The Todo
/// classifier marks it with `role = InternalBody`. The
/// assembler must NOT promote it to canonical tag_section.
#[test]
#[allow(non_snake_case)]
fn cycle_4f_todo_closing_hashes_does_not_become_terminal_metadata() {
    let bytes: &[u8] = b"# [ ] Task\n\nbody\n\n## Tags ##\n\n#alpha\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("x.todo.md")))
        .expect("parse must succeed");
    assert!(
        doc.tag_section().is_none(),
        "`## Tags ##` must NOT be promoted to canonical tag_section; the \
         exact-form rule excludes closing-hash sequences. \
         got tag_section={:?}",
        doc.tag_section().map(std::str::from_utf8)
    );
    assert_eq!(doc.emit(), bytes);
}

/// Positive coverage retained: an exact terminal `## Tags`
/// heading WITHOUT any trailing interfering H2 must still be
/// promoted to canonical tag_section with full extent.
#[test]
#[allow(non_snake_case)]
fn cycle_4f_todo_exact_terminal_tags_positive_coverage() {
    let bytes: &[u8] = b"# [ ] Task\n\nbody\n\n## Tags\n\n#alpha\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("x.todo.md")))
        .expect("parse must succeed");
    let tag_section = doc
        .tag_section()
        .expect("exact terminal `## Tags` must remain canonical");
    assert_eq!(
        tag_section,
        &bytes[bytes.len() - (b"## Tags\n\n#alpha\n".len())..],
        "exact terminal `## Tags` retains its full section extent"
    );
    assert_eq!(doc.emit(), bytes);
}
