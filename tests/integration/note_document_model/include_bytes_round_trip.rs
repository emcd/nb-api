//! Pinned fixture round-trips via include_bytes!.

use std::path::PathBuf;

use nb_api::parser::{DocumentKind, ParseContext, parse};

use super::{BOOKMARK_FIXTURE, NOTE_FIXTURE, TODO_FIXTURE};

// ---------- include_bytes! round-trip ----------

#[test]
fn include_bytes_note_round_trip() {
    let doc = parse(
        NOTE_FIXTURE,
        ParseContext::FromPath(PathBuf::from("note.md")),
    )
    .unwrap();
    assert_eq!(doc.kind(), DocumentKind::Note);
    assert_eq!(doc.emit(), NOTE_FIXTURE);
}

#[test]
fn include_bytes_todo_round_trip() {
    let doc = parse(
        TODO_FIXTURE,
        ParseContext::FromPath(PathBuf::from("todo.todo.md")),
    )
    .unwrap();
    assert_eq!(doc.kind(), DocumentKind::Todo);
    assert_eq!(doc.emit(), TODO_FIXTURE);
}

#[test]
fn include_bytes_bookmark_round_trip() {
    let doc = parse(
        BOOKMARK_FIXTURE,
        ParseContext::FromPath(PathBuf::from("bookmark.bookmark.md")),
    )
    .unwrap();
    assert_eq!(doc.kind(), DocumentKind::Bookmark);
    assert_eq!(doc.emit(), BOOKMARK_FIXTURE);
}
