//! Fingerprint scheme for [`NoteDocument`].
//!
//! A [`Fingerprint`] is a versioned public token that
//! authenticates exactly the concatenated bytes in
//! [`NoteDocument::body_ranges`](crate::parser::NoteDocument) (in
//! source order). It establishes nothing about excluded partition
//! ranges (prefix, title, tags_prefix, url, tag_section,
//! separators).
//!
//! Canonical form: `b3:<64 lowercase hex>` (BLAKE3-256).
//!
//! Every public construction path validates the input and
//! returns [`NbError::InvalidFingerprint`] on failure. The
//! `FromStr::Err` type IS `NbError::InvalidFingerprint` per the
//! P1 specification. The internal BLAKE3 hashing path bypasses
//! string validation entirely (it produces the canonical form
//! directly from binary output), so no unchecked public
//! constructor can produce an out-of-spec `Fingerprint`.
//!
//! See the `add-note-document-model` (P1) specification for the
//! full contract and the FromStr rejection rules.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::NbError;
use crate::parser::NoteDocument;

const PREFIX: &str = "b3:";
const HEX_LEN: usize = 64;

/// A versioned public fingerprint token.
///
/// `Fingerprint(String)` carries the canonical
/// `b3:<64 lowercase hex>` form. Construct via:
/// - [`Fingerprint::from_str`] (rejects malformed inputs)
/// - `fingerprint(&NoteDocument)` (computes from a parsed
///   document; the only `Fingerprint`-returning path that
///   bypasses string validation, since the BLAKE3 output is
///   guaranteed canonical).
/// - [`Fingerprint::from_json`] (typed deserialization from a
///   JSON-encoded string)
///
/// The standard `Deserialize` impl also validates and rejects
/// via `D::Error`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Fingerprint(String);

impl Fingerprint {
    /// Parse a fingerprint from a JSON-encoded string.
    ///
    /// JSON parse errors map to [`NbError::JsonParseError`];
    /// format errors map to [`NbError::InvalidFingerprint`].
    pub fn from_json(s: &str) -> Result<Self, NbError> {
        let inner: String = serde_json::from_str(s).map_err(|e| NbError::JsonParseError {
            source: e.to_string(),
        })?;
        inner.parse()
    }

    /// The raw hex portion of the fingerprint (without the
    /// `b3:` prefix).
    pub fn as_hex(&self) -> &str {
        &self.0
    }

    /// The full canonical string (including the `b3:` prefix).
    pub fn as_str(&self) -> String {
        let mut buf = String::with_capacity(PREFIX.len() + self.0.len());
        buf.push_str(PREFIX);
        buf.push_str(&self.0);
        buf
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(PREFIX)?;
        f.write_str(&self.0)
    }
}

impl FromStr for Fingerprint {
    /// `FromStr` returns [`NbError::InvalidFingerprint`] on
    /// malformed input per the P1 specification. The
    /// `reason` field carries a machine-readable category
    /// (e.g., `"empty"`, `"unknown_algorithm_prefix"`,
    /// `"uppercase_hex"`).
    type Err = NbError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        validate(s)?;
        Ok(Fingerprint(s[PREFIX.len()..].to_ascii_lowercase()))
    }
}

fn validate(s: &str) -> Result<(), NbError> {
    if s.is_empty() {
        return Err(NbError::InvalidFingerprint {
            reason: "empty".to_string(),
        });
    }
    if s.starts_with(|c: char| c.is_whitespace()) {
        return Err(NbError::InvalidFingerprint {
            reason: "leading_whitespace".to_string(),
        });
    }
    if s.ends_with(|c: char| c.is_whitespace()) {
        return Err(NbError::InvalidFingerprint {
            reason: "trailing_whitespace".to_string(),
        });
    }
    if !s.starts_with(PREFIX) {
        return Err(NbError::InvalidFingerprint {
            reason: "unknown_algorithm_prefix".to_string(),
        });
    }
    let hex = &s[PREFIX.len()..];
    if hex.len() != HEX_LEN {
        return Err(NbError::InvalidFingerprint {
            reason: "wrong_length".to_string(),
        });
    }
    for c in hex.chars() {
        if !c.is_ascii_hexdigit() {
            return Err(NbError::InvalidFingerprint {
                reason: "invalid_hex".to_string(),
            });
        }
        if c.is_ascii_uppercase() {
            return Err(NbError::InvalidFingerprint {
                reason: "uppercase_hex".to_string(),
            });
        }
    }
    Ok(())
}

impl Serialize for Fingerprint {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{PREFIX}{}", self.0))
    }
}

impl<'de> Deserialize<'de> for Fingerprint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for Fingerprint {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("Fingerprint")
    }
    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": "^b3:[0-9a-f]{64}$"
        })
    }
}

/// Compute the fingerprint of a `NoteDocument`.
///
/// Hashes body_ranges bytes in source order with BLAKE3-256.
/// Returns the canonical `b3:<64 hex>` token. The BLAKE3 hex
/// output is always 64 lowercase hex digits, so this path
/// always produces a valid `Fingerprint` without needing string
/// validation.
pub fn fingerprint(doc: &NoteDocument) -> Fingerprint {
    let mut hasher = blake3::Hasher::new();
    for fragment in doc.body() {
        hasher.update(fragment);
    }
    let hash = hasher.finalize();
    let hex = hash.to_hex(); // 64 lowercase hex chars by default
    Fingerprint(hex.to_string())
}
