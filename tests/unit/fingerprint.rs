//! Unit tests for the `Fingerprint` type and the
//! `fingerprint(&NoteDocument)` function.
//!
//! Covers the FromStr rejection rules, the canonical
//! `b3:<64 hex>` format, BLAKE3-256 hashing over body_ranges
//! bytes, and the `from_json` typed deserialization.

use std::path::PathBuf;
use std::str::FromStr;

use nb_api::NbError;
use nb_api::fingerprint::{Fingerprint, fingerprint};
use nb_api::parser::{ParseContext, parse};

// ---------- Canonical form ----------

#[test]
fn fingerprint_format_is_b3_with_64_lowercase_hex() {
    let bytes: &[u8] = b"line1\nline2\n";
    let doc = parse(bytes, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    let fp = fingerprint(&doc);
    let s = fp.to_string();
    assert!(s.starts_with("b3:"));
    assert_eq!(s.len(), 3 + 64);
    let hex = &s[3..];
    assert!(
        hex.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
}

// ---------- BLAKE3 of empty body ----------

#[test]
fn fingerprint_of_empty_body_is_blake3_empty() {
    let doc = parse(b"", ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    let fp = fingerprint(&doc);
    assert_eq!(
        fp.to_string(),
        "b3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
    );
}

// ---------- Fragmented body hashes in source order ----------

#[test]
fn fingerprint_hashes_fragments_in_source_order() {
    // Construct a Bookmark where body_ranges has multiple
    // fragments (Content section plus post-Tags content).
    let bytes: &[u8] =
        b"# Bookmark\n\n<URL>\n\n## Tags\n\n#a\n\n## Content\n\nContent body\n\nTrailer\n";
    let doc = parse(
        bytes,
        ParseContext::FromPath(PathBuf::from("x.bookmark.md")),
    )
    .unwrap();
    // Independently compute the expected BLAKE3 hash of the
    // concatenated body fragments.
    let mut hasher = blake3::Hasher::new();
    for fragment in doc.body() {
        hasher.update(fragment);
    }
    let expected_hex = hasher.finalize().to_hex().to_string();
    let fp = fingerprint(&doc);
    assert_eq!(fp.as_hex(), expected_hex);
}

// ---------- FromStr rejection (returns NbError::InvalidFingerprint) ----------

fn assert_invalid(err: NbError, reason: &str) {
    match err {
        NbError::InvalidFingerprint { reason: r } => assert_eq!(r, reason),
        other => panic!("expected InvalidFingerprint({reason}), got {other:?}"),
    }
}

#[test]
fn from_str_rejects_empty() {
    assert_invalid(Fingerprint::from_str("").unwrap_err(), "empty");
}

#[test]
fn from_str_rejects_leading_whitespace() {
    assert_invalid(
        Fingerprint::from_str(" b3:af1349b9...").unwrap_err(),
        "leading_whitespace",
    );
}

#[test]
fn from_str_rejects_trailing_whitespace() {
    assert_invalid(
        Fingerprint::from_str("b3:af1349b9... ").unwrap_err(),
        "trailing_whitespace",
    );
}

#[test]
fn from_str_rejects_unknown_prefix() {
    assert_invalid(
        Fingerprint::from_str(
            "sha3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
        )
        .unwrap_err(),
        "unknown_algorithm_prefix",
    );
}

#[test]
fn from_str_rejects_wrong_length() {
    assert_invalid(
        Fingerprint::from_str("b3:af1349").unwrap_err(),
        "wrong_length",
    );
}

#[test]
fn from_str_rejects_invalid_hex() {
    // Replace one hex char with 'g' (non-hex).
    let mut s = String::from("b3:");
    s.push_str("gf1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262");
    assert_invalid(Fingerprint::from_str(&s).unwrap_err(), "invalid_hex");
}

#[test]
fn from_str_rejects_uppercase_hex() {
    // One uppercase hex char.
    let mut s = String::from("b3:");
    s.push_str("Af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262");
    assert_invalid(Fingerprint::from_str(&s).unwrap_err(), "uppercase_hex");
}

#[test]
fn from_str_accepts_canonical_lowercase_hex() {
    let s = "b3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";
    let fp = Fingerprint::from_str(s).unwrap();
    assert_eq!(fp.to_string(), s);
}

// ---------- from_json typed deserialization ----------

#[test]
fn from_json_returns_nb_error_on_malformed_json() {
    // The string is not valid JSON (no surrounding quotes).
    let result = Fingerprint::from_json("b3:af1349b9...");
    assert!(matches!(result, Err(NbError::JsonParseError { .. })));
}

#[test]
fn from_json_returns_invalid_fingerprint_on_format_error() {
    // Valid JSON string but the inner content is malformed.
    let result = Fingerprint::from_json(
        "\"b3:afg349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262\"",
    );
    assert!(
        matches!(result, Err(NbError::InvalidFingerprint { reason }) if reason == "invalid_hex")
    );
}

#[test]
fn from_json_accepts_canonical_form() {
    let canonical = "b3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";
    let result = Fingerprint::from_json(&format!("\"{canonical}\"")).unwrap();
    assert_eq!(result.to_string(), canonical);
}

// ---------- Body fingerprint authenticates only body_ranges ----------

#[test]
fn fingerprint_depends_only_on_body_ranges() {
    // Two Note documents with the same body but different
    // titles should produce the same fingerprint. The tags
    // prefix line must contain 2+ tokens per the spec; both
    // notes have the same multi-token tags prefix, but with
    // different individual tokens.
    let bytes_a: &[u8] = b"# Title A\n\n#alpha #gamma\n\nShared body\n";
    let bytes_b: &[u8] = b"# Title B\n\n#beta #gamma\n\nShared body\n";
    let doc_a = parse(bytes_a, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    let doc_b = parse(bytes_b, ParseContext::FromPath(PathBuf::from("note.md"))).unwrap();
    let fp_a = fingerprint(&doc_a);
    let fp_b = fingerprint(&doc_b);
    assert_eq!(fp_a, fp_b);
}

// ---------- Display / Eq / Hash ----------

#[test]
fn fingerprint_eq_and_hash() {
    let s = "b3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";
    let a = Fingerprint::from_str(s).unwrap();
    let b = Fingerprint::from_str(s).unwrap();
    assert_eq!(a, b);
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut ha = DefaultHasher::new();
    let mut hb = DefaultHasher::new();
    a.hash(&mut ha);
    b.hash(&mut hb);
    assert_eq!(ha.finish(), hb.finish());
}

// ---------- Serialize / Deserialize JSON ----------

#[test]
fn fingerprint_serializes_to_canonical_string() {
    let s = "b3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";
    let fp = Fingerprint::from_str(s).unwrap();
    let json = serde_json::to_string(&fp).unwrap();
    assert_eq!(json, format!("\"{s}\""));
}

#[test]
fn fingerprint_deserializes_from_canonical_string() {
    let s = "b3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";
    let json = format!("\"{s}\"");
    let fp: Fingerprint = serde_json::from_str(&json).unwrap();
    assert_eq!(fp.to_string(), s);
}

#[test]
fn fingerprint_rejects_garbage_via_deserialize() {
    let result: Result<Fingerprint, _> = serde_json::from_str("\"not a fingerprint\"");
    assert!(result.is_err());
}
