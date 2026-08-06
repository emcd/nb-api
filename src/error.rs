//! Structured error types for `nb-api`.
//!
//! This module hosts [`NbError`] and its associated types
//! ([`ParseErrorKind`], [`IoError`], [`IoErrorKind`]) as defined by
//! the `add-note-document-model` (P1) specification. All variants
//! derive `Serialize`/`Deserialize` so errors round-trip cleanly
//! through JSON-encoded wire protocols (MCP tool responses, etc.).
//!
//! [`NbError`] also derives `Display` and implements
//! [`std::error::Error`], so existing textual error reporting paths
//! continue to work unchanged.
//!
//! # Migration (from `nb-api` 0.2.x to 0.3.0)
//!
//! | Old variant | New variant | Migration |
//! |---|---|---|
//! | `CommandFailed(String)` | `CommandFailed { command, stderr, exit_code }` | capture fields at call sites; map validation/policy failures to `ValidationError` |
//! | `NotFound` (unit) | `ExecutableNotFound { path }` | the unit means `nb` binary missing; track path |
//! | `Io(std::io::Error)` | `Io { path, source: IoError }` | track path in callers; convert via `From`; reverse is lossy |
//! | `UnsupportedShowTarget { selector, actual_type }` | Same | Unchanged |
//! | `DuplicateTitleHeading { title, heading }` | Same | Unchanged |
//!
//! New variants introduced in 0.3.0: `ParseError`, `InvalidFingerprint`,
//! `JsonParseError`, `ValidationError`, `ExecutableNotFound`,
//! `UnsupportedDocumentFormat`.
//!
//! ## New `UnsupportedDocumentFormat` variant (R3 revision)
//!
//! This is a NEW variant. There is no `nb-api 0.2.x` predecessor.
//! Consumers upgrading to `nb-api 0.3.0` SHALL add the variant to
//! their match arms. **Adding a variant to an exhaustive enum
//! match produces a compile error, not a warning; wildcard
//! matches continue compiling.** The constant
//! [`SUPPORTED_DOCUMENT_EXTENSIONS`] (crate-root re-export of the
//! source-of-truth declared in `parser`) is the basis for the
//! `supported` payload at construction time.

use std::error::Error as _; // `.source()` on `std::io::Error`
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Errors raised by `nb-api` operations.
///
/// All variants are serializable so they can traverse MCP tool
/// response envelopes without lossy string flattening. The
/// [`std::error::Error`] implementation remains for ergonomic
/// interop with code that uses `?` and standard error reporting.
///
/// `std::error::Error` is implemented MANUALLY below rather than
/// via `thiserror::Error` derive. The derive's autodetect
/// treats any field named `source` as a `#[source]` chain link
/// and tries to convert it to `dyn Error`, which fails for
/// fields of type `String` (e.g., `JsonParseError { source:
/// String }` per the P1 specification). The manual impl
/// preserves the field name and only sets a source-chain link
/// for variants whose source field is itself an `Error`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NbError {
    /// An `nb` subprocess invocation failed (non-zero exit, or
    /// spawning failed for a non-NotFound reason).
    CommandFailed {
        /// The argument vector (joined) that was executed.
        command: String,
        /// Captured stderr (or stdout when stderr was empty).
        stderr: String,
        /// Process exit code, if the process started.
        exit_code: Option<i32>,
    },

    /// The `nb` executable could not be located on `PATH` (or in
    /// the resolved location) when a subprocess was spawned.
    ExecutableNotFound {
        /// The resolved path the spawn attempt targeted.
        path: String,
    },

    /// `nb` reported that the requested selector was not found.
    NotFound {
        /// The selector that was not resolved.
        selector: String,
    },

    /// An I/O error occurred while accessing a notebook file or
    /// the on-disk notebook directory. The `source` field captures
    /// the original `std::io::Error` chain as a serializable
    /// snapshot (recursive [`IoError`]).
    Io {
        /// Path that triggered the I/O error.
        path: PathBuf,
        /// Snapshot of the `std::io::Error` chain.
        source: IoError,
    },

    /// `nb show` was invoked on a selector whose type is not
    /// classified as text by `nb` itself (per the
    /// `add-0-2-0-foundation` public-API spec). Folders, archives,
    /// audio, video, image, and any other non-textual type reach
    /// this path. The classification delegates to
    /// `nb show <selector> --type text` so forward compatibility
    /// is automatic when `nb` adds new textual types. Probe
    /// failure (e.g., selector not found) does NOT route here;
    /// it falls through to the original `CommandFailed` error
    /// from the content-read path.
    UnsupportedShowTarget {
        selector: String,
        actual_type: String,
    },

    /// `nb add` was called with a `title` and `content` where the
    /// first nonblank line of `content` is an exact Markdown ATX H1
    /// duplicating the title. The validation runs in the caller
    /// process before any subprocess invocation or notebook side
    /// effect (including `resolve_notebook`); the rejection
    /// happens entirely in-process. `heading` carries the exact
    /// detected source line (including the leading `#` and any
    /// surrounding whitespace) for actionable diagnostics. See
    /// `add-0-2-0-foundation` public-API specification.
    DuplicateTitleHeading { title: String, heading: String },

    /// A structural parse failure.
    ParseError {
        kind: ParseErrorKind,
        location: std::ops::Range<usize>,
    },

    /// A fingerprint string failed structural validation (wrong
    /// prefix, wrong length, mixed case, etc.). `reason` carries a
    /// machine-readable category (e.g., `"empty"`,
    /// `"unknown_algorithm_prefix"`, `"uppercase_hex"`).
    InvalidFingerprint { reason: String },

    /// JSON deserialization failed when constructing a typed
    /// wrapper (e.g., [`crate::fingerprint::Fingerprint::from_json`]).
    JsonParseError {
        // Field is named `source` per the P1 specification.
        source: String,
    },

    /// A semantic validation error raised by an operation boundary
    /// (e.g., rejecting input that is structurally valid but
    /// semantically illegal). `location` is `None` when the
    /// failure is not byte-anchored.
    ValidationError {
        reason: String,
        location: Option<std::ops::Range<usize>>,
    },

    /// Format-dispatch refusal (R3 revision).
    ///
    /// `parse(bytes, FromPath(path))` returned this error because
    /// the final filename matched one of the recognized-but-
    /// unsupported dotted suffixes (e.g., `.org`, `.latex`,
    /// `.tex`, `.adoc`, `.asciidoc`). Format dispatch precedes
    /// byte parsing, so no bytes were consumed and no
    /// parse-error location is meaningful — this is a
    /// contextual configuration error, not a parse failure.
    ///
    /// `extension` carries the lowercase dotted suffix WITHOUT
    /// the leading dot (e.g., `"org"`, `"latex"`).
    /// `supported` is the owned canonical list of supported
    /// extensions, populated at construction time from
    /// `crate::SUPPORTED_DOCUMENT_EXTENSIONS`. The owned
    /// `Vec<String>` shape is required for the derived
    /// `Serialize` / `Deserialize` round-trip contract on
    /// `NbError`.
    UnsupportedDocumentFormat {
        extension: String,
        supported: Vec<String>,
    },

    /// Process-shared gate queue wait exceeded. Notebook was not mutated.
    GateTimeout { gate: String, timeout_ms: u64 },

    /// `Transaction::commit` refused because the notebook worktree/index is dirty.
    DirtyBaseline { guidance: String },

    /// Plan-op path collision against snapshot or earlier plan ops.
    PathCollision {
        path: String,
        plan_index: Option<u32>,
    },

    /// Plan targets or would leave durable state on a Git-ignored path in a
    /// way that cannot be represented in the single checkpoint (existing
    /// ignored entry mutation), or requires force-staging (reported only when
    /// force-stage is unavailable). Prefer rejecting existing-ignored mutators.
    PathIgnored {
        path: String,
        guidance: String,
        plan_index: Option<u32>,
    },

    /// Validation/apply failure tied to a plan entry.
    PlanValidation {
        kind: String,
        message: String,
        plan_index: Option<u32>,
    },

    /// Commit completion is unknown (e.g. transport timeout after checkpoint may have started).
    IndeterminateCommit {
        pre_revision: String,
        post_revision_observed: Option<String>,
        guidance: String,
    },

    /// Cleanup after a known failed apply could not verify clean `pre_revision`.
    RecoveryRequired {
        pre_revision: String,
        post_revision_observed: Option<String>,
        status_observed: Option<String>,
        preserved_paths: Option<Vec<String>>,
        guidance: String,
    },

    /// Body fingerprint precondition failed.
    FingerprintMismatch {
        target: crate::types::NoteTarget,
        guidance: String,
    },

    /// Line number/anchor precondition failed.
    AnchorMismatch {
        target: String,
        number: u32,
        guidance: String,
    },

    /// Substring `expected_count` did not match actual matches.
    OccurrenceMismatch { expected: u32, actual: u32 },

    /// Overlapping line edits in one batch.
    OverlappingEdits { indices: Vec<u32> },

    /// Invalid `show_note_lines` window.
    InvalidLineWindow {
        offset: u32,
        limit: u32,
        total_lines: u32,
    },

    /// Empty substring pattern refused at enqueue/apply.
    EmptySubstringPattern,

    /// Contiguous-body-only op invoked on a multi-fragment document.
    FragmentedBody {
        fragment_count: u32,
        guidance: String,
    },

    /// Document structure is not supported for the requested op.
    UnsupportedStructure { reason: String },
}

impl std::fmt::Display for NbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CommandFailed {
                command,
                stderr,
                exit_code,
            } => write!(
                f,
                "command {command:?} failed (exit {exit_code:?}): {stderr}"
            ),
            Self::ExecutableNotFound { path } => {
                write!(f, "`nb` executable not found at path {path:?}")
            }
            Self::NotFound { selector } => write!(f, "not found: {selector:?}"),
            Self::Io { path, source } => write!(f, "I/O error at {path:?}: {source}"),
            Self::UnsupportedShowTarget {
                selector,
                actual_type,
            } => write!(
                f,
                "selector `{selector}` resolved to non-textual type `{actual_type}`; \
                 `nb show` does not display non-textual content"
            ),
            Self::DuplicateTitleHeading { title, heading } => write!(
                f,
                "title `{title}` duplicates the first H1 in content (`{heading}`); \
                 remove the duplicate heading to avoid double-rendering"
            ),
            Self::ParseError { kind, location } => {
                write!(f, "parse error: {kind:?} at {location:?}")
            }
            Self::InvalidFingerprint { reason } => {
                write!(f, "invalid fingerprint: {reason:?}")
            }
            Self::JsonParseError { source } => {
                write!(f, "JSON parse error: {source:?}")
            }
            Self::ValidationError {
                reason,
                location: _,
            } => {
                write!(f, "validation error: {reason:?}")
            }
            Self::UnsupportedDocumentFormat {
                extension,
                supported,
            } => {
                write!(
                    f,
                    "unsupported document format: {extension:?} (supported: {supported:?})"
                )
            }
            Self::GateTimeout { gate, timeout_ms } => {
                write!(f, "gate timeout waiting on {gate} after {timeout_ms}ms")
            }
            Self::DirtyBaseline { guidance } => {
                write!(f, "dirty baseline: {guidance}")
            }
            Self::PathCollision { path, plan_index } => {
                write!(f, "path collision at {path:?} (plan_index={plan_index:?})")
            }
            Self::PathIgnored {
                path,
                guidance,
                plan_index,
            } => write!(
                f,
                "ignored path {path:?} (plan_index={plan_index:?}): {guidance}"
            ),
            Self::PlanValidation {
                kind,
                message,
                plan_index,
            } => write!(
                f,
                "plan validation ({kind}) at index {plan_index:?}: {message}"
            ),
            Self::IndeterminateCommit {
                pre_revision,
                post_revision_observed,
                guidance,
            } => write!(
                f,
                "indeterminate commit (pre={pre_revision}, post={post_revision_observed:?}): {guidance}"
            ),
            Self::RecoveryRequired {
                pre_revision,
                guidance,
                ..
            } => write!(
                f,
                "recovery required (pre_revision={pre_revision}): {guidance}"
            ),
            Self::FingerprintMismatch { target, guidance } => {
                write!(f, "fingerprint mismatch for {target:?}: {guidance}")
            }
            Self::AnchorMismatch {
                target,
                number,
                guidance,
            } => write!(f, "anchor mismatch for {target} line {number}: {guidance}"),
            Self::OccurrenceMismatch { expected, actual } => {
                write!(
                    f,
                    "occurrence mismatch: expected={expected} actual={actual}"
                )
            }
            Self::OverlappingEdits { indices } => {
                write!(f, "overlapping edits at indices {indices:?}")
            }
            Self::InvalidLineWindow {
                offset,
                limit,
                total_lines,
            } => write!(
                f,
                "invalid line window offset={offset} limit={limit} total_lines={total_lines}"
            ),
            Self::EmptySubstringPattern => write!(f, "empty substring pattern"),
            Self::FragmentedBody {
                fragment_count,
                guidance,
            } => write!(
                f,
                "fragmented body ({fragment_count} fragments): {guidance}"
            ),
            Self::UnsupportedStructure { reason } => {
                write!(f, "unsupported structure: {reason}")
            }
        }
    }
}

impl std::error::Error for NbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // Only `Io { source: IoError }` carries a true source-chain
        // link. Every other variant's `source` field (where
        // present) is a diagnostic String or a non-Error type, so
        // we explicitly return `None` for them.
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Narrow refusal contract for the structural parser.
///
/// `parse` accepts every input that can be losslessly partitioned
/// into a [`crate::parser::NoteDocument`], including the frozen
/// W1 and E1-E11 noncanonical forms. The ONLY refusal cases at
/// the parse layer are:
///
/// - Todo with no non-blank line at all: [`ParseErrorKind::MissingTitle`].
/// - Bookmark with no non-blank line at all: [`ParseErrorKind::MissingTitle`].
///
/// Notes are permissive on empty source. Title-shape errors belong
/// to a later strict validator (P5+), not to the parser.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum ParseErrorKind {
    /// The first non-blank line was missing entirely (Todo/Bookmark
    /// only; Notes are permissive).
    MissingTitle,
}

/// Serializable snapshot of a [`std::io::Error`] chain.
///
/// `IoError` captures the **chain structure** (as a tree of
/// snapshots) but does NOT preserve the original `std::io::Error`
/// identity. The forward conversion [`From<std::io::Error>`] walks
/// [`std::error::Error::source`] and recurses for each
/// `std::io::Error` link; nested non-`io::Error` sources are
/// stringified into [`IoErrorKind::Other`].
///
/// The reverse conversion [`From<IoError>`] for `std::io::Error`
/// is **explicitly lossy** — see [`IoError`]'s `From` impl.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, thiserror::Error)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[error("{kind:?}: {message}")]
pub struct IoError {
    pub kind: IoErrorKind,
    pub message: String,
    pub os_error: Option<i32>,
    pub source: Option<Box<IoError>>,
}

/// Discriminator for the `std::io::ErrorKind` taxonomy, lifted into
/// a serializable form. Marked `#[non_exhaustive]` so future
/// versions can add new kinds without breaking downstream
/// `match` arms that include a default branch.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum IoErrorKind {
    NotFound,
    PermissionDenied,
    ConnectionRefused,
    ConnectionReset,
    ConnectionAborted,
    NotConnected,
    AddrInUse,
    AddrNotAvailable,
    BrokenPipe,
    AlreadyExists,
    WouldBlock,
    InvalidInput,
    InvalidData,
    TimedOut,
    WriteZero,
    Interrupted,
    UnexpectedEof,
    OutOfMemory,
    /// Catch-all for nested sources that are not themselves
    /// `std::io::Error` (e.g., a third-party crate's error type
    /// captured as `source`).
    Other,
}

impl IoErrorKind {
    /// Map a `std::io::ErrorKind` into the serializable variant.
    pub fn from_std(kind: std::io::ErrorKind) -> Self {
        match kind {
            std::io::ErrorKind::NotFound => Self::NotFound,
            std::io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            std::io::ErrorKind::ConnectionRefused => Self::ConnectionRefused,
            std::io::ErrorKind::ConnectionReset => Self::ConnectionReset,
            std::io::ErrorKind::ConnectionAborted => Self::ConnectionAborted,
            std::io::ErrorKind::NotConnected => Self::NotConnected,
            std::io::ErrorKind::AddrInUse => Self::AddrInUse,
            std::io::ErrorKind::AddrNotAvailable => Self::AddrNotAvailable,
            std::io::ErrorKind::BrokenPipe => Self::BrokenPipe,
            std::io::ErrorKind::AlreadyExists => Self::AlreadyExists,
            std::io::ErrorKind::WouldBlock => Self::WouldBlock,
            std::io::ErrorKind::InvalidInput => Self::InvalidInput,
            std::io::ErrorKind::InvalidData => Self::InvalidData,
            std::io::ErrorKind::TimedOut => Self::TimedOut,
            std::io::ErrorKind::WriteZero => Self::WriteZero,
            std::io::ErrorKind::Interrupted => Self::Interrupted,
            std::io::ErrorKind::UnexpectedEof => Self::UnexpectedEof,
            std::io::ErrorKind::OutOfMemory => Self::OutOfMemory,
            _ => Self::Other,
        }
    }
}

impl From<std::io::ErrorKind> for IoErrorKind {
    fn from(kind: std::io::ErrorKind) -> Self {
        Self::from_std(kind)
    }
}

impl From<std::io::Error> for IoError {
    /// Capture an `io::Error` chain as a tree of serializable
    /// snapshots.
    ///
    /// The implementation recursively walks the
    /// [`std::error::Error::source`] chain on BORROWED
    /// `&dyn Error` references. This is critical: `std::io::Error`
    /// is not `Clone`, so we cannot reconstruct an inner error
    /// from a borrow without losing its `raw_os_error()` and its
    /// own source chain. Walking the borrow preserves the chain
    /// STRUCTURE exactly.
    ///
    /// At each level: if the source is itself an `io::Error`,
    /// recurse on its borrow; otherwise, capture the source as
    /// an [`IoErrorKind::Other`] snapshot (preserving the
    /// `Display` text and walking its own source chain for
    /// nested non-io links).
    fn from(err: std::io::Error) -> Self {
        snapshot_io_error(err)
    }
}

/// Build an `IoError` snapshot from an `io::Error` value, then
/// walk its source chain via the borrowed `dyn Error` to
/// preserve chain structure exactly.
fn snapshot_io_error(err: std::io::Error) -> IoError {
    let kind = IoErrorKind::from_std(err.kind());
    let os_error = err.raw_os_error();
    let message = err.to_string();
    let source = walk_source_chain(err.source());
    IoError {
        kind,
        message,
        os_error,
        source,
    }
}

/// Walk a `&(dyn Error + 'static)` chain, building a list of
/// `IoError` snapshots. Each link appears EXACTLY ONCE in the
/// chain (no duplication). Each `std::io::Error` link has its
/// `raw_os_error()` preserved verbatim via the recursive walk
/// on the borrowed `dyn Error`.
///
/// The chain is built by snapshotting the CURRENT node with its
/// `source` field set to the recursive walk of the NEXT node.
/// This avoids the A→B→B duplication that would arise from both
/// recursing into the source AND iterating/appending the same
/// source.
fn walk_source_chain(source: Option<&(dyn std::error::Error + 'static)>) -> Option<Box<IoError>> {
    let head = source?;
    Some(Box::new(snapshot_link(head)))
}

/// Snapshot a single link in the source chain. The link's `source`
/// field is set to the recursive snapshot of the next link.
fn snapshot_link(current: &(dyn std::error::Error + 'static)) -> IoError {
    if let Some(io_err) = current.downcast_ref::<std::io::Error>() {
        IoError {
            kind: IoErrorKind::from_std(io_err.kind()),
            message: io_err.to_string(),
            os_error: io_err.raw_os_error(),
            // The recursive walk captures the io_err's own
            // chain. Each io_err-level appears exactly once.
            source: walk_source_chain(io_err.source()),
        }
    } else {
        IoError {
            kind: IoErrorKind::Other,
            message: current.to_string(),
            os_error: None,
            // Recurse on the next link. Because `walk_source_chain`
            // snapshots its OWN first link and recurses, the
            // outer iteration does NOT see the next link — no
            // duplication.
            source: walk_source_chain(current.source()),
        }
    }
}

impl From<IoError> for std::io::Error {
    /// Reverse conversion is **explicitly lossy**.
    ///
    /// The implementation prefers [`std::io::Error::from_raw_os_error`]
    /// when [`IoError::os_error`] is `Some`, which LOSES the
    /// snapshot message and any source chain. Otherwise, it
    /// constructs a new `io::Error::new(kind, message)`, which
    /// loses `raw_os_error()`.
    ///
    /// The forward conversion ([`From<std::io::Error>`] for
    /// `IoError`) preserves chain STRUCTURE; this reverse
    /// conversion does not — the snapshot tree is collapsed to a
    /// single `std::io::Error`.
    fn from(snapshot: IoError) -> Self {
        if let Some(code) = snapshot.os_error {
            std::io::Error::from_raw_os_error(code)
        } else {
            let kind = match snapshot.kind {
                IoErrorKind::NotFound => std::io::ErrorKind::NotFound,
                IoErrorKind::PermissionDenied => std::io::ErrorKind::PermissionDenied,
                IoErrorKind::ConnectionRefused => std::io::ErrorKind::ConnectionRefused,
                IoErrorKind::ConnectionReset => std::io::ErrorKind::ConnectionReset,
                IoErrorKind::ConnectionAborted => std::io::ErrorKind::ConnectionAborted,
                IoErrorKind::NotConnected => std::io::ErrorKind::NotConnected,
                IoErrorKind::AddrInUse => std::io::ErrorKind::AddrInUse,
                IoErrorKind::AddrNotAvailable => std::io::ErrorKind::AddrNotAvailable,
                IoErrorKind::BrokenPipe => std::io::ErrorKind::BrokenPipe,
                IoErrorKind::AlreadyExists => std::io::ErrorKind::AlreadyExists,
                IoErrorKind::WouldBlock => std::io::ErrorKind::WouldBlock,
                IoErrorKind::InvalidInput => std::io::ErrorKind::InvalidInput,
                IoErrorKind::InvalidData => std::io::ErrorKind::InvalidData,
                IoErrorKind::TimedOut => std::io::ErrorKind::TimedOut,
                IoErrorKind::WriteZero => std::io::ErrorKind::WriteZero,
                IoErrorKind::Interrupted => std::io::ErrorKind::Interrupted,
                IoErrorKind::UnexpectedEof => std::io::ErrorKind::UnexpectedEof,
                IoErrorKind::OutOfMemory => std::io::ErrorKind::OutOfMemory,
                IoErrorKind::Other => std::io::ErrorKind::Other,
            };
            std::io::Error::new(kind, snapshot.message)
        }
    }
}
