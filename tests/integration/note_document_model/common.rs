//! Shared helpers for note_document_model integration tests.

use std::path::PathBuf;

use nb_api::NoteDocument;
use nb_api::parser::{ParseContext, parse};

pub(crate) fn check_partition(bytes: &[u8], path: &str) -> NoteDocument {
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from(path)))
        .unwrap_or_else(|e| panic!("parse({path}) failed: {e}"));
    assert_eq!(doc.emit(), bytes, "{path}: emit round-trip");
    doc.verify_partition()
        .unwrap_or_else(|e| panic!("{path}: partition invariants violated: {e}"));
    doc
}

pub(crate) fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
