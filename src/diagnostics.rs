//! Stderr diagnostic normalization and absence classifiers.

pub(crate) fn append_warning(mut output: String, warning: String) -> String {
    if !output.trim().is_empty() {
        if !output.ends_with('\n') {
            output.push('\n');
        }
        output.push('\n');
    }
    output.push_str(&warning);
    output
}

/// Returns `true` if `stderr` from an `nb show <sel>` invocation
/// matches the exact `nb 7.24.0` selector-absence diagnostic
/// shape for the given expected selector. The complete
/// normalized diagnostic must equal `! Not found: <expected>`,
/// with no other content (no appended text, no mismatched
/// selector name, no other failure-mode substring).
///
/// Diagnostic normalization: ANSI escape sequences and
/// singleton control bytes (e.g., the Shift-In byte `\x0f`)
/// that `nb 7.24.0` emits between the diagnostic-bang and the
/// visible text are dropped. The criterion applies to the
/// resulting printable form.
///
/// Verified failure modes that MUST NOT match even though they
/// mention a "not found" string:
/// - `"Permission denied: file not found: /etc/x"` (real
///   subprocess failure with a non-existent path substring).
/// - `"Notebook not found: scratch"` (notebook-absence shape,
///   different classifier).
/// - `"! error: not found during expansion"` (real error with
///   "not found" mid-sentence, not the canonical shape).
///
/// Note: this helper is a transient bridge while `NbClient`
/// still wraps `nb` as a subprocess. The native rewrite
/// (`nb-api:todos/2`) will let `show_note` and friends read
/// the on-disk file directly via the P1 note-document-model,
/// removing the need to classify `nb` stderr at all. Whether
/// this helper is deleted is a separate change when the
/// native rewrite lands; the deletion is NOT automatic.
pub(crate) fn is_selector_not_found(stderr: &str, expected_selector: &str) -> bool {
    exact_normalized_diagnostic(stderr, "Not found: ", expected_selector)
}

/// Returns `true` if `stderr` from `nb notebooks show <name>
/// --path` matches the exact `nb 7.24.0` notebook-absence
/// diagnostic shape for the given expected notebook name. The
/// complete normalized diagnostic must equal
/// `! Notebook not found: <expected>`. See
/// [`is_selector_not_found`] for normalization details and
/// verified failure-mode negatives.
pub(crate) fn is_notebook_not_found(stderr: &str, expected_notebook: &str) -> bool {
    exact_normalized_diagnostic(stderr, "Notebook not found: ", expected_notebook)
}

/// Verify the normalized diagnostic for `nb show` / `nb
/// notebooks show` ABSENCE shapes. Returns `true` only when
/// the printable form, with the pinned normalization sequence
/// applied, equals the literal template `<keyword><expected>`.
///
/// Normalization sequence (pinned; do not reorder):
///   1. Reduce to printable form via [`printable_form`] (drops
///      ANSI escapes and singleton control bytes except
///      newline / carriage-return / tab).
///   2. Trim trailing `\n` / `\r` bytes (`trim_end_matches`).
///   3. Strip exactly one leading `!` diagnostic-bang
///      (`strip_prefix('!')`).
///   4. Strip leading ASCII whitespace (`trim_start`).
///   5. Compare byte-exact against the literal template
///      `<keyword><expected>`.
///
/// Pathological stderr (multiline, trailing garbage, foreign
/// diagnostic styles) is rejected because the criterion is
/// byte-exact post-normalization. This prevents appended
/// failures (e.g., a retry that succeeds after a missing-
/// selector error) from being absorbed as a "not found" when
/// they were actually transient.
pub(crate) fn exact_normalized_diagnostic(
    stderr: &str,
    keyword: &str,
    expected_value: &str,
) -> bool {
    let printable = printable_form(stderr);
    let trimmed = printable.trim_end_matches(&['\n', '\r'][..]);
    let after_bang = match trimmed.strip_prefix('!').map(str::trim_start) {
        Some(rest) => rest,
        None => return false,
    };
    let expected = format!("{keyword}{expected_value}");
    after_bang == expected
}

/// Reduce `stderr` to its printable-canonical form by
/// dropping ANSI escape sequences and singleton control
/// characters while preserving spaces, tabs, and newlines.
///
/// `nb 7.24.0` decorates its error diagnostics with mixed
/// ANSI/control bytes between the diagnostic-bang and the
/// visible text (e.g., `!<ANSI reset><SI> Not found: ...`).
/// Without this normalization, the byte-exact classifiers
/// above cannot reach the visible prefix. The function is
/// intentionally narrow: it only normalizes the bytes that
/// `nb 7.24.0` is known to emit; non-printable bytes inside
/// an unrelated error's message will be dropped, which is
/// acceptable because the classifiers compare against the
/// pinned literal template and would not match a different
/// message anyway.
pub(crate) fn printable_form(stderr: &str) -> String {
    let mut out = String::with_capacity(stderr.len());
    let mut chars = stderr.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip an ANSI control sequence: CSI (`ESC [` ... )
            // or Fe (`ESC <byte>`).
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&nc) = chars.peek() {
                    chars.next();
                    if (0x40..=0x7e).contains(&(nc as u32)) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
            continue;
        }
        // Drop singleton control bytes (SI/SO/...) while
        // preserving tab/newline. nb's diagnostic decoration
        // uses these; ordinary messages do not contain
        // unprintable bytes in the visible region.
        let code = c as u32;
        if code < 0x20 && c != '\t' && c != '\n' && c != '\r' {
            continue;
        }
        out.push(c);
    }
    out
}
