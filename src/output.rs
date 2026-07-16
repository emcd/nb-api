//! Output sanitization helpers for `nb-api` consumers.
//!
//! Strips the trailing usage/help hint block from raw `nb`
//! CLI output when an empty result is detected. Requires
//! **structural evidence of a native hint block** (signal
//! line + blank separator + at least one recognized hint
//! marker) before truncating; otherwise returns input
//! unchanged. False negatives preferred over destructive
//! output loss.
//!
//! # Background
//!
//! `nb` CLI emits a trailing usage/help hint block after
//! some empty-result signals. For example, `nb ls` on an
//! empty notebook produces:
//!
//! ```text
//! 0 items.
//!
//! Add a note:
//!   nb add scratch:
//! Add a bookmark:
//!   nb scratch: <url>
//! ...
//! ```
//!
//! Consumers integrating against `nb-api` should not see
//! the hint block; only the empty-result signal is part of
//! the API contract. The hint block is widely recognized as
//! an `nb` CLI quirk, not a stable surface.
//!
//! # Why structural evidence, not just signal matching
//!
//! An empty-result signal like `0 items.` is also a
//! legitimate prefix for user-authored content (a note
//! whose first line is `0 items.`). If the helper matched
//! on signal alone, it could corrupt user content. The
//! structural-evidence requirement — signal + blank
//! separator + recognized hint markers — ensures the
//! helper only acts when the trailing block is unambiguously
//! the native `nb` hint block.
//!
//! # Why wrapper post-processing, not a native `nb` flag
//!
//! The design note `add-0-2-0-foundation/designs/2` D2 notes
//! "investigate whether `nb` exposes a flag to suppress the
//! hint. If yes, prefer the flag over wrapper
//! post-processing." As of `nb 7.24.0` (the version pinned by
//! `qa`), `nb ls` / `nb ls --type folder` / `nb bookmark`
//! have NO flag to suppress the hint block. The wrapper
//! approach is therefore the canonical implementation. The
//! spec scenario "Hint block is preserved if nb CLI exposes
//! a flag to suppress it natively" leaves room for switching
//! to a native flag in a future `nb` release.
//!
//! # Method coverage
//!
//! The helper is applied ONLY to list-style methods that
//! empirically emit the hint block:
//! - `NbClient::list` (handles `0 items.`)
//! - `NbClient::folders` (handles `0 folders.`)
//!
//! The helper is NOT applied to:
//! - User-content methods (`show`, `add`, `edit`, `delete`,
//!   `move_note`, `import`, etc.) — these return user
//!   content, not list output, and a note whose first line is
//!   `0 items.` would be wrongly truncated.
//! - `NbClient::tasks` — single-line `! 0 ... tasks.`
//!   signals handled separately by the existing
//!   `is_empty_tasks_error` / `empty_tasks_message` logic.
//! - `NbClient::search` no-match — `! Not found in
//!   <notebook>: <query>` propagates as `CommandFailed`,
//!   preserved as a distinct contract (not a helper
//!   pass-through).
//!
//! # Server-side duplication
//!
//! The `output-behavior` specification scenario "Server-side
//! and API-side sanitization are independent" recommends
//! that `nb-mcp-server` MAY apply its own sanitization for
//! error-presentation quality (showing users clean output),
//! but should not duplicate the parser logic verbatim to
//! avoid drift from `nb-api`. If `nb-mcp-server` chooses
//! to apply server-side sanitization, it should be a thin
//! error-presentation wrapper (e.g., a server-side
//! fallback when `nb-api`'s output is wrong), not a
//! duplicate parser.

// Strip the trailing usage/help hint block from `nb` output
// when there is **structural evidence** of a native hint block:
// signal line, blank separator, at least one recognized hint
// marker. Otherwise returns input unchanged.
//
// **Detection rule:**
// 1. First line is a `0 <kind>.` signal (starts with `0 `,
//    ends with `.`).
// 2. The line following the signal is blank (a blank
//    separator, per `nb`'s hint-block convention).
// 3. At least one line in the trailing block is a recognized
//    hint marker: starts with `Add a `, `Add an `,
//    `Import a `, or `Help information:`.
//
// All three must hold. Any failure returns input unchanged.
//
// **Terminator preservation:** the helper preserves the
// signal's exact terminator (LF, CRLF, or none). When
// truncating, the result includes the signal line up to and
// including its terminator, with no characters appended.
pub(crate) fn strip_empty_result_hint(output: &str) -> String {
    let bytes = output.as_bytes();

    // Find the first line's terminator. Prefer byte-level
    // parsing so CRLF terminators are preserved exactly.
    // The prefix always extends to `pos + 1` (right after the
    // `\n`). For CRLF input, the preceding `\r` is at
    // `pos - 1`, which is naturally within `output[..pos + 1]`,
    // so the CRLF terminator is preserved exactly without
    // consuming any byte of the blank separator that follows.
    let line_end_with_terminator = match bytes.iter().position(|&b| b == b'\n') {
        Some(pos) => pos + 1,
        None => output.len(),
    };

    let prefix = &output[..line_end_with_terminator];
    let rest = &output[line_end_with_terminator..];

    // Pattern-match against the signal content. Strip
    // optional trailing \r (CRLF) and any \n so we compare
    // against the signal text only.
    // Pattern-match against the signal content. Trim any
    // trailing `\r` and `\n` (any combination: LF, CRLF, or
    // bare CR) so we compare against the signal text only.
    // `trim_end_matches` strips any combination of the listed
    // chars. For `"0 items.\r\n"`, it strips to `"0 items."`
    // (both the `\r` and the `\n`).
    let signal = prefix.trim_end_matches(&['\r', '\n'][..]);
    if !(signal.starts_with("0 ") && signal.ends_with('.')) {
        return output.to_string();
    }

    // Structural evidence: a blank separator line after the
    // signal. `nb`'s hint block is preceded by a blank line;
    // require it explicitly so content that just happens to
    // start with `0 items.` is not wrongly truncated.
    let rest_lines: Vec<&str> = rest.lines().collect();
    let has_blank_separator = rest_lines.first().is_some_and(|l| l.is_empty());
    if !has_blank_separator {
        return output.to_string();
    }

    // Structural evidence: at least one recognized hint marker.
    let has_hint_marker = rest_lines.iter().any(|line| {
        line.starts_with("Add a ")
            || line.starts_with("Add an ")
            || line.starts_with("Import a ")
            || line.starts_with("Help information:")
    });
    if !has_hint_marker {
        return output.to_string();
    }

    // Strip the hint block; preserve the signal's exact
    // terminator (return the prefix as-is, byte-for-byte).
    prefix.to_string()
}

// NOTE: AGENTS.md requires inline `#[cfg(test)]` modules to
// satisfy ALL of: (1) tested item is crate-private by design,
// (2) no existing public interface exercises the same code
// path, (3) at most ONE `#[test]` function. Condition (2)
// fails for this helper because `NbClient::list` and
// `NbClient::folders` invoke it publicly. Per the AGENTS.md
// guidance ("Do not default to inline to avoid that
// conversation; the friction is intentional"), all helper
// coverage is exercised via the public list/folders behavior
// in `tests/integration/empty_result_hints.rs`, using a
// deterministic `nb` shim in PATH (see
// `tests/integration/common/mod.rs::with_shim_nb_env`) to
// emit crafted list output for the LF / CRLF / no-terminator
// / no-recognized-marker cases that real `nb` does not
// produce.
