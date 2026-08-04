//! Resolution-table partition assertions (W1 + E1–E10).

use nb_api::parser::DocumentKind;

use crate::common::check_partition;

// ---------- Resolution Table: exact partition assertions ----------

/// Helper: parse bytes, assert emit round-trip and partition
/// invariants, and return the doc.
#[test]
fn resolution_w1_1_titled_tagged_note_partition() {
    let bytes: &[u8] = b"# Writer Note\n\n#alpha #beta\n\nWriter body\nsecond line\n\n";
    let doc = check_partition(bytes, "note.md");
    assert_eq!(doc.title().unwrap(), &bytes[0..14]);
    assert_eq!(doc.tags_prefix().unwrap(), &bytes[15..28]);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[29..54]);
}

#[test]
fn resolution_w1_2_titleless_tagged_note_partition() {
    let bytes: &[u8] = b"#alpha #beta\n\nWriter titleless body\n\n";
    let doc = check_partition(bytes, "note.md");
    assert_eq!(doc.tags_prefix().unwrap(), &bytes[0..13]);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[14..37]);
}

#[test]
fn resolution_w1_3_tagged_todo_partition() {
    let bytes: &[u8] =
        b"# [ ] Writer Todo\n\n## Description\n\nWriter description\n\n## Tags\n\n#alpha #beta\n\n";
    let doc = check_partition(bytes, "todo.todo.md");
    assert_eq!(doc.title().unwrap(), &bytes[0..18]);
    assert_eq!(doc.tag_section().unwrap(), &bytes[55..78]);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[19..54]);
}

#[test]
fn resolution_w1_4_tagged_offline_bookmark_partition() {
    let bytes: &[u8] =
        b"# Writer Bookmark (example.com)\n\n<https://example.com>\n\n## Tags\n\n#beta\n";
    let doc = check_partition(bytes, "bookmark.bookmark.md");
    assert_eq!(doc.title().unwrap(), &bytes[0..32]);
    assert_eq!(doc.url().unwrap(), &bytes[33..55]);
    assert_eq!(doc.tag_section().unwrap(), &bytes[56..71]);
    let body: Vec<&[u8]> = doc.body().collect();
    assert!(body.is_empty(), "expected empty body, got {body:?}");
}

#[test]
fn resolution_w1_5_offline_bookmark_without_title_partition() {
    let bytes: &[u8] = b"# (example.org)\n\n<https://example.org/no-title>\n";
    let doc = check_partition(bytes, "bookmark.bookmark.md");
    assert_eq!(doc.title().unwrap(), &bytes[0..16]);
    assert_eq!(doc.url().unwrap(), &bytes[17..48]);
}

#[test]
fn resolution_e1_1_titled_note_partition() {
    let bytes: &[u8] = b"# Title\n\nBody\n";
    let doc = check_partition(bytes, "note.md");
    assert_eq!(doc.title().unwrap(), &bytes[0..8]);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[9..14]);
}

#[test]
fn resolution_e1_2_titleless_body_partition() {
    let bytes: &[u8] = b"Just content.\n";
    let doc = check_partition(bytes, "note.md");
    assert_eq!(doc.title(), None);
    assert_eq!(doc.tags_prefix(), None);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[0..14]);
}

#[test]
fn resolution_e1_3_hash_title_not_h1_partition() {
    let bytes: &[u8] = b"#Title\n\nBody\n";
    let doc = check_partition(bytes, "note.md");
    assert_eq!(doc.title(), None);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[0..13]);
}

#[test]
fn resolution_e1_4_4space_indent_not_h1_partition() {
    let bytes: &[u8] = b"    # Title\n\nBody\n";
    let doc = check_partition(bytes, "note.md");
    assert_eq!(doc.title(), None);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[0..18]);
}

#[test]
fn resolution_e1_5_setext_title_not_h1_partition() {
    let bytes: &[u8] = b"Title\n=====\n\nBody\n";
    let doc = check_partition(bytes, "note.md");
    assert_eq!(doc.title(), None);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[0..18]);
}

#[test]
fn resolution_e2_leading_blank_lines_partition() {
    let bytes: &[u8] = b"\n\n# Title\n\nBody\n";
    let doc = check_partition(bytes, "note.md");
    assert_eq!(doc.title().unwrap(), &bytes[2..10]);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[11..16]);
}

#[test]
fn resolution_e3_3_tags_in_body_partition() {
    let bytes: &[u8] = b"Text\n\n## Tags\n\n#alpha #beta\n";
    let doc = check_partition(bytes, "note.md");
    assert_eq!(doc.title(), None);
    assert_eq!(doc.tags_prefix(), None);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[0..28]);
}

#[test]
fn resolution_e4_1_bom_partition() {
    let bytes: &[u8] = b"\xef\xbb\xbf# Title\n\nBody\n";
    let doc = check_partition(bytes, "note.md");
    assert_eq!(doc.title().unwrap(), &bytes[3..11]);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[12..17]);
}

#[test]
fn resolution_e4_2_cr_only_partition() {
    let bytes: &[u8] = b"# Title\r\rBody\rSecond\r";
    let doc = check_partition(bytes, "note.md");
    assert_eq!(doc.title().unwrap(), &bytes[0..8]);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[9..21]);
}

#[test]
fn resolution_e4_3_invalid_utf8_partition() {
    let bytes: &[u8] = b"# Title\n\nBody \xff\xfe\n";
    let doc = check_partition(bytes, "note.md");
    assert_eq!(doc.title().unwrap(), &bytes[0..8]);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[9..17]);
}

#[test]
fn resolution_e5_1_dot_todo_is_note_partition() {
    let bytes: &[u8] = b"# [ ] Task\n\n## Description\n\nBody\n\n## Tags\n\n#alpha #beta\n";
    let doc = check_partition(bytes, "canonical.todo");
    assert_eq!(doc.kind(), DocumentKind::Note);
}

#[test]
fn resolution_e5_2_dot_todo_md_is_todo_partition() {
    let bytes: &[u8] =
        b"# [ ] Writer Todo\n\n## Description\n\nWriter description\n\n## Tags\n\n#alpha #beta\n\n";
    let doc = check_partition(bytes, "canonical.todo.md");
    assert_eq!(doc.kind(), DocumentKind::Todo);
}

#[test]
fn resolution_e6_checkbox_less_todo_partition() {
    let bytes: &[u8] = b"# Task\n\nBody\n";
    let doc = check_partition(bytes, "x.todo.md");
    assert_eq!(doc.kind(), DocumentKind::Todo);
    assert_eq!(doc.todo_state(), None);
}

#[test]
fn resolution_e7_1_nonterminal_tags_todo_partition() {
    let bytes: &[u8] = b"# [ ] Task\n\n## Tags\n\n#alpha\n\n## Description\n\nBody\n";
    let doc = check_partition(bytes, "x.todo.md");
    assert_eq!(doc.tag_section(), None);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[12..50]);
}

#[test]
fn resolution_e7_2_duplicate_tags_todo_partition() {
    let bytes: &[u8] =
        b"# [ ] Task\n\n## Tags\n\n#first\n\n## Description\n\nBody\n\n## Tags\n\n#last\n";
    let doc = check_partition(bytes, "x.todo.md");
    assert_eq!(doc.tag_section().unwrap(), &bytes[51..66]);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[12..50]);
}

#[test]
fn resolution_e8_1_minimal_bookmark_partition() {
    let bytes: &[u8] = b"# Bookmark\n\n<https://example.com>\n";
    let doc = check_partition(bytes, "x.bookmark.md");
    assert_eq!(doc.title().unwrap(), &bytes[0..11]);
    assert_eq!(doc.url().unwrap(), &bytes[12..34]);
}

#[test]
fn resolution_e8_2_titleless_bookmark_partition() {
    let bytes: &[u8] = b"<https://example.com>\n";
    let doc = check_partition(bytes, "x.bookmark.md");
    assert_eq!(doc.title(), None);
    assert_eq!(doc.url().unwrap(), &bytes[0..22]);
}

#[test]
fn resolution_e8_3_missing_url_bookmark_partition() {
    let bytes: &[u8] = b"# Bookmark\n\nBody\n";
    let doc = check_partition(bytes, "x.bookmark.md");
    assert_eq!(doc.title().unwrap(), &bytes[0..11]);
    assert_eq!(doc.url(), None);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(body[0], &bytes[12..17]);
}

#[test]
fn resolution_e9_bookmark_with_tags_before_content() {
    let bytes: &[u8] = b"# Bookmark\n\n<URL>\n\n## Description\n\nDesc\n\n## Tags\n\n#alpha\n\n## Content\n\nContent body\n";
    let doc = check_partition(bytes, "x.bookmark.md");
    assert_eq!(doc.tag_section().unwrap(), &bytes[41..57]);
    let body: Vec<&[u8]> = doc.body().collect();
    // Two body fragments: Description+Desc (19..40) and
    // Content+Content body (58..83), per the spec.
    assert_eq!(body.len(), 2, "expected 2 body fragments, got {body:?}");
    assert_eq!(body[0], &bytes[19..40]);
    assert_eq!(body[1], &bytes[58..83]);
}

#[test]
fn resolution_e10_1_h2_in_content_is_body() {
    // The inner `## Tags in body` H2 inside Content MUST be
    // part of a single Content body fragment (39..67), not a
    // separate body fragment. This is the assertion that
    // failed in the prior review.
    let bytes: &[u8] =
        b"# Bookmark\n\n<URL>\n\n## Tags\n\n#official\n\n## Content\n\n## Tags in body\n";
    let doc = check_partition(bytes, "x.bookmark.md");
    assert_eq!(doc.tag_section().unwrap(), &bytes[19..38]);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(
        body.len(),
        1,
        "expected single Content fragment, got {body:?}"
    );
    assert_eq!(body[0], &bytes[39..67]);
}

#[test]
fn resolution_e10_2_h2_in_source_fence_is_body() {
    let bytes: &[u8] = b"# Bookmark\n\n<URL>\n\n## Tags\n\n#official\n\n## Source\n\n```html\n## Tags\n<p>raw</p>\n```\n";
    let doc = check_partition(bytes, "x.bookmark.md");
    assert_eq!(doc.tag_section().unwrap(), &bytes[19..38]);
    let body: Vec<&[u8]> = doc.body().collect();
    assert_eq!(
        body.len(),
        1,
        "expected single Source fragment, got {body:?}"
    );
    assert_eq!(body[0], &bytes[39..81]);
}
