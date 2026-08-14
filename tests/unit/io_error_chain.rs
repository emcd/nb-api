//! Unit tests for the IoError snapshot chain captured from
//! `std::io::Error` source chains via the public
//! `From<std::io::Error> for IoError` entry point.
//!
//! The walker is exercised end-to-end by constructing
//! `std::io::Error` chains via `std::io::Error::other`. A
//! subtlety: `std::io::Error::source()` on a `Custom`
//! representation returns `c.error.source()` (NOT `&c.error`
//! itself), so the immediate Custom wrapper is transparent.
//! To produce a multi-link chain visible end-to-end through
//! `IoError::from`, the chain needs non-`io::Error` wrappers
//! whose `source()` returns the next link — each wrapper
//! contributes one chain link.
//!
//! These tests are whitebox (no widening of any types);
//! `IoError`, `IoErrorKind`, and `IoError::from(io::Error)`
//! are all on the public surface. The tests construct their
//! own `std::io::Error` chains via the public API of the std
//! library.

use std::error::Error as StdError;

use nb_api::{IoError, IoErrorKind};

// ---------- test fixtures: error wrappers ----------

/// A non-`io::Error` wrapper that exposes its `inner` as
/// `source()`. Each layer in a chain of `DeepWrap`s contributes
/// one link to the IoError snapshot.
struct DeepWrap {
    inner: Box<dyn StdError + Send + Sync + 'static>,
}

impl std::fmt::Debug for DeepWrap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DeepWrap")
    }
}

impl std::fmt::Display for DeepWrap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DeepWrap")
    }
}

impl StdError for DeepWrap {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&*self.inner)
    }
}

// ---------- helper: chain depth walk ----------

fn chain_depth(err: &IoError) -> usize {
    let mut depth = 1;
    let mut cursor = err;
    while let Some(next) = cursor.source.as_deref() {
        depth += 1;
        cursor = next;
    }
    depth
}

// ---------- single-link leaves (production-reachable shapes) ----------

/// Forward-conversion of a leaf `std::io::Error` (no source)
/// produces an `IoError` with `source = None`. This is the
/// shape production can actually produce today; a real `nb`
/// subprocess failure surfaces as a single-leaf io::Error.
#[test]
fn leaf_io_error_with_raw_code_captures_os_error() {
    let io_err = std::io::Error::from_raw_os_error(7);
    let snapshot = IoError::from(io_err);
    // Note: `from_raw_os_error(7)` on Linux maps to E2BIG
    // (`ArgumentListTooLong`), which has no dedicated
    // IoErrorKind and falls through to `Other`. We assert
    // the raw OS code preservation rather than the kind map,
    // since the kind is platform-specific.
    assert_eq!(snapshot.os_error, Some(7));
    assert!(snapshot.source.is_none());
}

/// Forward-conversion of a `std::io::Error` constructed via
/// `Error::new` with an explicit known `ErrorKind` preserves
/// the `kind` mapping and leaves `os_error = None`. The reverse
/// `From<IoError> for io::Error` is lossy here (per the spec).
#[test]
fn leaf_io_error_without_raw_code_preserves_kind_drops_os_error() {
    let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
    let snapshot = IoError::from(io_err);
    assert_eq!(snapshot.kind, IoErrorKind::PermissionDenied);
    assert_eq!(snapshot.os_error, None);
    assert!(snapshot.source.is_none());
}

// ---------- multi-link chains via Wrap layers ----------

/// Build a 3-link chain through `io::Error::other` with TWO
/// `DeepWrap` layers (outer + inner) and a leaf `io_err`:
///
///   top_io_err (Custom of outer Wrap)
///     → top.source() = outer_W.source()
///       → inner_W → inner_W.source() = leaf_io_err
///
/// `io::Error::other`'s Custom layer is transparent: it calls
/// `c.error.source()` rather than returning `Some(&c.error)`.
/// Therefore the FIRST Wrap layer is not visible as a chain
/// link. The Inner Wrap layer IS visible (its `source()`
/// returns `&leaf_io`, which the walker downcasts and
/// captures as the 3rd link).
///
///   Visible chain: top_io_err → inner_W → leaf_io_err (3 links)
///
/// Each non-`io::Error` link is captured as `IoErrorKind::Other`
/// because the walker takes the `else` branch. The leaf is
/// captured via downcast and preserves its kind and
/// `raw_os_error()` verbatim.
#[test]
fn io_only_three_link_chain_via_two_wrap_layers() {
    let leaf = std::io::Error::from_raw_os_error(11);
    let inner_wrap: DeepWrap = DeepWrap {
        inner: Box::new(leaf),
    };
    let outer_wrap: DeepWrap = DeepWrap {
        inner: Box::new(inner_wrap),
    };
    let top = std::io::Error::other(outer_wrap);

    let snapshot = IoError::from(top);

    // Root: outer io_err captured as Other. Custom is
    // transparent; level 2 is the INNER Wrap.
    assert_eq!(snapshot.kind, IoErrorKind::Other);
    assert!(snapshot.os_error.is_none());

    // Level 2: the inner Wrap layer (captured as non-io / Other).
    let level2 = snapshot.source.as_deref().expect("level 2 link");
    assert_eq!(level2.kind, IoErrorKind::Other);
    assert_eq!(level2.message, "DeepWrap");
    assert!(level2.os_error.is_none());

    // Level 3: leaf io_err with raw_os_error=11 preserved verbatim.
    let level3 = level2.source.as_deref().expect("level 3 link");
    assert_eq!(level3.os_error, Some(11));
    assert!(level3.source.is_none());

    assert_eq!(chain_depth(&snapshot), 3);
}

// ---------- mixed chain: io → Wrap → Wrap → non-io leaf ----------

/// Mixed chain with a non-`io::Error` leaf. The transparent
/// Custom makes the OUTER Wrap invisible, so 3 links are
/// visible: top_io_err → inner_W → PlainMsg.
#[test]
fn mixed_chain_non_io_leaf_via_two_wrap_layers() {
    #[derive(Debug)]
    struct PlainMsg(&'static str);
    impl std::fmt::Display for PlainMsg {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }
    impl StdError for PlainMsg {}

    let leaf = PlainMsg("innermost-leaf");
    let inner_wrap: DeepWrap = DeepWrap {
        inner: Box::new(leaf),
    };
    let outer_wrap: DeepWrap = DeepWrap {
        inner: Box::new(inner_wrap),
    };
    let top = std::io::Error::other(outer_wrap);
    let snapshot = IoError::from(top);
    assert_eq!(chain_depth(&snapshot), 3);

    // Root: kind=Other, message="DeepWrap" (Custom delegates
    // Display to inner via `c.error.fmt()`).
    assert_eq!(snapshot.kind, IoErrorKind::Other);
    assert_eq!(snapshot.message, "DeepWrap");

    // Level 2: inner Wrap captured as Other.
    let level2 = snapshot.source.as_deref().expect("level 2");
    assert_eq!(level2.kind, IoErrorKind::Other);
    assert_eq!(level2.message, "DeepWrap");

    // Level 3: PlainMsg with its own message.
    let level3 = level2.source.as_deref().expect("level 3");
    assert_eq!(level3.kind, IoErrorKind::Other);
    assert_eq!(level3.message, "innermost-leaf");
    assert!(level3.source.is_none());
}

// ---------- serde round-trip + non-duplication invariants ----------

/// The serialized JSON of a 3-link chain (top → DeepWrap →
/// leaf io_err) survives the deserialize round-trip EXACTLY
/// (verified via equality).
#[test]
fn three_link_chain_serde_round_trip_preserves_structure() {
    let leaf = std::io::Error::from_raw_os_error(11);
    let inner_wrap: DeepWrap = DeepWrap {
        inner: Box::new(leaf),
    };
    let outer_wrap: DeepWrap = DeepWrap {
        inner: Box::new(inner_wrap),
    };
    let top = std::io::Error::other(outer_wrap);
    let snapshot = IoError::from(top);

    let json = serde_json::to_string(&snapshot).unwrap();
    let restored: IoError = serde_json::from_str(&json).unwrap();

    assert_eq!(restored, snapshot);
    assert_eq!(chain_depth(&restored), 3);
}

/// A → B → B duplication would inflate the chain depth. The
/// walker must visit each link EXACTLY ONCE. We construct a
/// 3-link chain and verify depth is preserved and the leaf's
/// raw OS code round-trips through JSON.
///
/// The leaf's `IoErrorKind` for raw code 11 is platform-dependent
/// (Linux EAGAIN ⇒ `WouldBlock`; macOS EDEADLK / Windows
/// ERROR_BAD_FORMAT ⇒ `Other`). Assert structure only — never a
/// fixed global `"Other"` string count (that fails when the leaf
/// is also `Other`).
#[test]
fn three_link_chain_has_no_duplication() {
    let leaf = std::io::Error::from_raw_os_error(11);
    let inner_wrap: DeepWrap = DeepWrap {
        inner: Box::new(leaf),
    };
    let outer_wrap: DeepWrap = DeepWrap {
        inner: Box::new(inner_wrap),
    };
    let top = std::io::Error::other(outer_wrap);
    let snapshot = IoError::from(top);

    let json = serde_json::to_string(&snapshot).unwrap();
    let restored: IoError = serde_json::from_str(&json).unwrap();

    // Chain depth: root (top Custom → Other) → DeepWrap (Other) → leaf.
    assert_eq!(chain_depth(&snapshot), 3, "snapshot depth");
    assert_eq!(chain_depth(&restored), 3, "restored depth");
    assert_eq!(restored, snapshot, "serde round-trip identity");

    assert_eq!(snapshot.kind, IoErrorKind::Other, "root Custom is Other");
    let mid = snapshot
        .source
        .as_deref()
        .expect("mid DeepWrap link must be reachable");
    assert_eq!(mid.kind, IoErrorKind::Other, "DeepWrap snapshot is Other");
    assert_eq!(mid.message, "DeepWrap");

    let leaf_link = mid.source.as_deref().expect("leaf link must be reachable");
    assert_eq!(
        leaf_link.os_error,
        Some(11),
        "leaf io_err raw_os_error must round-trip verbatim through the chain walker"
    );
    assert!(
        !leaf_link.message.is_empty(),
        "leaf message must be non-empty"
    );
    assert!(
        leaf_link.message.contains("11") || leaf_link.message.contains("os error"),
        "leaf message should reference the OS error; got {:?}",
        leaf_link.message
    );
    assert!(
        json.contains("\"os_error\":11"),
        "JSON must carry leaf os_error=11; got {json}"
    );
}
