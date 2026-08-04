//! Unit tests for the lossless emitter.
//!
//! `NoteDocument::emit()` returns a slice into the retained
//! `source` bytes, byte-identical to the original input. These
//! tests assert the round-trip identity invariant for every
//! pinned case.

use std::path::PathBuf;

use nb_api::parser::{ParseContext, parse};

// Every frozen W1 and E1-E11 case from the Resolution Table
// plus a few invariant edge cases. Round-trip identity must hold
// for each.

#[test]
fn round_trip_w1_1() {
    let bytes: &[u8] = b"# Writer Note\n\n#alpha #beta\n\nWriter body\nsecond line\n\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_w1_2() {
    let bytes: &[u8] = b"#alpha #beta\n\nWriter titleless body\n\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_w1_3() {
    let bytes: &[u8] =
        b"# [ ] Writer Todo\n\n## Description\n\nWriter description\n\n## Tags\n\n#alpha #beta\n\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("todo.todo.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_w1_4() {
    let bytes: &[u8] =
        b"# Writer Bookmark (example.com)\n\n<https://example.com>\n\n## Tags\n\n#beta\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("bookmark.bookmark.md")),
    )
    .unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_w1_5() {
    let bytes: &[u8] = b"# (example.org)\n\n<https://example.org/no-title>\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("bookmark.bookmark.md")),
    )
    .unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e1_1() {
    let bytes: &[u8] = b"# Title\n\nBody\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e1_2() {
    let bytes: &[u8] = b"Just content.\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e1_3() {
    let bytes: &[u8] = b"#Title\n\nBody\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e1_4() {
    let bytes: &[u8] = b"    # Title\n\nBody\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e1_5() {
    let bytes: &[u8] = b"Title\n=====\n\nBody\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e2() {
    let bytes: &[u8] = b"\n\n# Title\n\nBody\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e3_3() {
    let bytes: &[u8] = b"Text\n\n## Tags\n\n#alpha #beta\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e4_1_bom() {
    let bytes: &[u8] = b"\xef\xbb\xbf# Title\n\nBody\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e4_2_cr_only() {
    let bytes: &[u8] = b"# Title\r\rBody\rSecond\r";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e4_3_invalid_utf8() {
    let bytes: &[u8] = b"# Title\n\nBody \xff\xfe\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e6() {
    let bytes: &[u8] = b"# Task\n\nBody\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("x.todo.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e7_1() {
    let bytes: &[u8] = b"# [ ] Task\n\n## Tags\n\n#alpha\n\n## Description\n\nBody\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("x.todo.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e7_2() {
    let bytes: &[u8] =
        b"# [ ] Task\n\n## Tags\n\n#first\n\n## Description\n\nBody\n\n## Tags\n\n#last\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("x.todo.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e8_1() {
    let bytes: &[u8] = b"# Bookmark\n\n<https://example.com>\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e8_2() {
    let bytes: &[u8] = b"<https://example.com>\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e8_3() {
    let bytes: &[u8] = b"# Bookmark\n\nBody\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e9() {
    let bytes: &[u8] = b"# Bookmark\n\n<URL>\n\n## Description\n\nDesc\n\n## Tags\n\n#alpha\n\n## Content\n\nContent body\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e10_1() {
    let bytes: &[u8] =
        b"# Bookmark\n\n<URL>\n\n## Tags\n\n#official\n\n## Content\n\n## Tags in body\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .unwrap();
    assert_eq!(doc.emit(), bytes);
}

#[test]
fn round_trip_e10_2() {
    let bytes: &[u8] = b"# Bookmark\n\n<URL>\n\n## Tags\n\n#official\n\n## Source\n\n```html\n## Tags\n<p>raw</p>\n```\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .unwrap();
    assert_eq!(doc.emit(), bytes);
}

// Empty source round-trips for Note only (Todo/Bookmark refuse).
#[test]
fn round_trip_empty_note() {
    let bytes: &[u8] = b"";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("x.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
}

// ---------- EOF-unterminated final line ----------

/// Tokenizer regression: an EOF-unterminated line
/// with content must produce a `Line` whose `terminator ==
/// b""` and whose `content` carries the bytes; the prior
/// implementation misclassified the content as a zero-content
/// line and lost the title. Without the fix, the round-trip
/// check passes vacuously because the title is missing.
#[test]
fn round_trip_eof_unterminated_note_title_only() {
    let bytes: &[u8] = b"# Title";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("x.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
    let title_str = doc.title_str().expect("title must be present");
    let title_str = title_str.expect("EOF-unterminated title must be valid UTF-8");
    assert_eq!(
        title_str, "# Title",
        "EOF-unterminated Note must still recognize the trailing line as a title"
    );
}

#[test]
fn round_trip_eof_unterminated_todo_title_only() {
    let bytes: &[u8] = b"# [ ] Task";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("x.todo.md"))).unwrap();
    assert_eq!(doc.emit(), bytes);
    let title_str = doc.title_str().expect("title must be present");
    let title_str = title_str.expect("EOF-unterminated title must be valid UTF-8");
    assert_eq!(
        title_str, "# [ ] Task",
        "EOF-unterminated Todo must still recognize the trailing line as a title"
    );
}

#[test]
fn round_trip_eof_unterminated_bookmark_title_and_url() {
    let bytes: &[u8] = b"# Bookmark\n<U>";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .unwrap();
    assert_eq!(doc.emit(), bytes);
    let title_str = doc
        .title_str()
        .expect("title must be present")
        .expect("title must be valid UTF-8");
    assert_eq!(
        title_str, "# Bookmark\n",
        "EOF-unterminated Bookmark must still recognize the first line as a title"
    );
    let url_str = doc
        .url_str()
        .expect("url must be present")
        .expect("url must be valid UTF-8");
    assert_eq!(
        url_str, "<U>",
        "EOF-unterminated Bookmark must still recognize the trailing line as a URL"
    );
}
