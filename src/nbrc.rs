//! Notebook rc (`~/.nbrc`) parsing and store resolution.
//!
//! Replicates the `nb` CLI startup contract without executing
//! anything: `nb` sources `$NBRC_PATH` (default `~/.nbrc`) and then
//! applies `NB_DIR="${NB_DIR:-$HOME/.nb}"`. Sourcing an arbitrary rc
//! file would execute shell code, so this module parses a narrow,
//! documented subset instead and fails visibly on anything outside
//! it. The resolution chain is explicit process `NB_DIR`, else the
//! parsed rc value, else `~/.nb`; empty values count as unset
//! (`:-` semantics). Nothing here mutates process state, caches rc
//! contents, or spawns a subprocess.

use std::path::{Path, PathBuf};

use crate::error::NbError;

/// Parse the last supported top-level `NB_DIR` assignment from rc
/// text, expanded against `home`.
///
/// Supported lines (and only these affect the result):
/// - `NB_DIR=<value>` and `export NB_DIR=<value>`, where `<value>`
///   is unquoted (no whitespace), single-quoted (fully literal), or
///   double-quoted (backslash escapes plus `$HOME`/`${HOME}`
///   expansion; any other `$` expansion is rejected).
/// - `unset NB_DIR`, which clears any earlier assignment.
/// - Blank lines and `#` comments (`#` at line start or after
///   whitespace; a `#` glued to other text is literal).
/// - Anything else is ignored, unless it references `NB_DIR` in an
///   unsupported form (command substitution, backquotes,
///   conditionals, `+=`, spaced `=`, multi-assignment, `~user`, ...),
///   which fails instead of silently resolving elsewhere.
///
/// Expansion: a leading `~/` (or bare `~`) resolves against `home`;
/// `$HOME` and `${HOME}` expand anywhere outside single quotes.
/// An empty expanded value counts as unset. The returned string may
/// still be relative (for example a literal single-quoted `~/x`);
/// the resolver rejects non-absolute selections.
pub(crate) fn parse_nb_dir_assignment(
    input: &str,
    home: &Path,
) -> Result<Option<String>, NbError> {
    let mut selected: Option<String> = None;
    for (index, raw_line) in input.lines().enumerate() {
        let line_no = index + 1;
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let code = cut_comment(line).trim();
        if code.is_empty() {
            continue;
        }
        if code == "unset NB_DIR" {
            selected = None;
            continue;
        }
        let mut rest = code;
        if let Some(after) = strip_export(rest) {
            rest = after;
        }
        if !mentions_nb_dir(rest) {
            continue;
        }
        let fail = |why: &str| NbError::ValidationError {
            reason: format!(
                "unsupported NB_DIR syntax in notebook rc (line {line_no}): {why}"
            ),
            location: None,
        };
        if rest == "NB_DIR" {
            continue;
        }
        if rest.starts_with("unset") {
            return Err(fail("only plain `unset NB_DIR` is supported"));
        }
        let Some(rhs) = rest.strip_prefix("NB_DIR=") else {
            return Err(fail(
                "only top-level `NB_DIR=<value>` assignments are supported",
            ));
        };
        if rhs.contains("$(") || rhs.contains('`') {
            return Err(fail("command substitution is not supported"));
        }
        let expanded = expand_value(rhs, home).map_err(fail)?;
        selected = if expanded.is_empty() {
            None
        } else {
            Some(expanded)
        };
    }
    Ok(selected)
}

/// Resolve the notebook store directory: explicit non-empty
/// `NB_DIR` wins without any rc I/O; with `strict` set, an unset
/// `NB_DIR` is a configuration failure instead. Otherwise read
/// `$NBRC_PATH` (default `~/.nbrc`), falling back to `~/.nb` when
/// the file is absent. Missing `HOME`, an unreadable rc file, or an
/// unsupported or relative rc value fails instead of resolving
/// elsewhere. The returned directory may not exist; absence is
/// reported downstream, never switched to another notebook.
pub(crate) fn resolve_store_dir(strict: bool) -> Result<PathBuf, NbError> {
    let nb_dir = std::env::var_os("NB_DIR");
    let home = std::env::var_os("HOME");
    let nbrc_path = std::env::var_os("NBRC_PATH");
    if let Some(dir) = nb_dir.as_ref().filter(|v| !v.is_empty()) {
        tracing::debug!("notebook store from explicit NB_DIR");
        return Ok(PathBuf::from(dir));
    }
    if strict {
        return Err(NbError::ValidationError {
            reason: "notebook store is not configured (`NB_DIR` is unset) and strict resolution refuses the file default"
                .to_string(),
            location: None,
        });
    }
    let home = home.filter(|h| !h.is_empty()).ok_or_else(|| {
        NbError::ValidationError {
            reason: "home directory is not available, so the default notebook store cannot be resolved"
                .to_string(),
            location: None,
        }
    })?;
    let home_path = PathBuf::from(home);
    let rc_path = match nbrc_path.as_ref().filter(|v| !v.is_empty()) {
        Some(custom) => {
            let path = PathBuf::from(custom);
            if !path.is_absolute() {
                return Err(NbError::ValidationError {
                    reason: "`NBRC_PATH` must be absolute when set"
                        .to_string(),
                    location: None,
                });
            }
            path
        },
        None => home_path.join(".nbrc"),
    };
    let store = match std::fs::read_to_string(&rc_path) {
        Ok(text) => match parse_nb_dir_assignment(&text, &home_path)? {
            Some(raw) => PathBuf::from(raw),
            None => home_path.join(".nb"),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!("notebook store from default ~/.nb (no rc file)");
            home_path.join(".nb")
        },
        Err(e) => {
            return Err(NbError::Io {
                path: rc_path.clone(),
                source: e.into(),
            });
        },
    };
    if !store.is_absolute() {
        return Err(NbError::ValidationError {
            reason: format!(
                "notebook store from `{}` must be absolute, got `{}`",
                rc_path.display(),
                store.display()
            ),
            location: None,
        });
    }
    tracing::debug!(
        "notebook store `{}` selected via {}",
        store.display(),
        rc_path.display()
    );
    Ok(store)
}

/// Cut a `#` comment: a `#` outside quotes starts a comment when
/// it opens the line or follows whitespace.
fn cut_comment(line: &str) -> &str {
    let mut quote = None;
    let mut prev_was_space = true;
    for (index, ch) in line.char_indices() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                }
                prev_was_space = false;
            },
            None => {
                if ch == '\'' || ch == '"' {
                    quote = Some(ch);
                    prev_was_space = false;
                } else if ch == '#' && prev_was_space {
                    return &line[..index];
                } else {
                    prev_was_space = ch.is_whitespace();
                }
            },
        }
    }
    line
}

/// Strip one leading `export` keyword; `None` when absent.
fn strip_export(code: &str) -> Option<&str> {
    code.strip_prefix("export ")
        .or_else(|| code.strip_prefix("export\t"))
}

/// Whether `NB_DIR` appears as a standalone word (not inside a
/// longer identifier such as `MY_NB_DIR`).
fn mentions_nb_dir(code: &str) -> bool {
    let mut start = 0;
    while let Some(index) = code[start..].find("NB_DIR") {
        let index = start + index;
        let before = code[..index].chars().next_back();
        let after = code[index + "NB_DIR".len()..].chars().next();
        let boundary = |c: Option<char>| {
            c.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
        };
        if boundary(before) && boundary(after) {
            return true;
        }
        start = index + 1;
    }
    false
}

/// Expand one raw right-hand side against `home`. Returns the
/// expanded value or a static reason for rejection.
///
/// Quoting follows shell rules for this narrow subset:
/// single-quoted values are fully literal; double-quoted values
/// honor backslash escapes and `$HOME` expansion but not `~`
/// expansion; unquoted values additionally expand a leading `~/`.
/// A trailing backslash, unterminated quotes, quote concatenation,
/// and any `$` expansion other than `$HOME`/`${HOME}` are
/// rejected instead of guessed at.
fn expand_value(rhs: &str, home: &Path) -> Result<String, &'static str> {
    let rhs = rhs.trim_end();
    if let Some(inner) = rhs
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        if inner.contains('\'') {
            return Err("single-quoted values must be exactly quoted");
        }
        return Ok(inner.to_string());
    }
    if rhs.starts_with('\'') {
        return Err("single-quoted values must be exactly quoted");
    }
    if let Some(body) = rhs
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        if has_bare_quote(body) {
            return Err("double-quoted values must be exactly quoted");
        }
        return expand_scalar(body, home, true);
    }
    if rhs.starts_with('"') {
        return Err("double-quoted values must be exactly quoted");
    }
    if rhs.chars().any(|c| c == ' ' || c == '\t') {
        return Err("unquoted values cannot contain whitespace");
    }
    if rhs.chars().any(|c| c == '\'' || c == '"') {
        return Err("unquoted values cannot contain quote characters");
    }
    expand_scalar(rhs, home, false)
}

/// Whether `body` holds a `"` that is not backslash-escaped.
fn has_bare_quote(body: &str) -> bool {
    let mut escaped = false;
    for ch in body.chars() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return true;
        }
    }
    false
}

/// Expand escapes, `~`, and `$HOME` in one scalar value. `quoted`
/// selects double-quoted rules (no `~` expansion, backslash only
/// special before `$`, backquote, `"`, `\`) versus unquoted rules.
fn expand_scalar(
    body: &str,
    home: &Path,
    quoted: bool,
) -> Result<String, &'static str> {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    let mut first = true;
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let Some(next) = chars.next() else {
                return Err("dangling backslash");
            };
            if quoted && !matches!(next, '$' | '`' | '"' | '\\') {
                out.push('\\');
            }
            out.push(next);
        } else if ch == '~' && first && !quoted {
            match chars.peek() {
                None => out.push_str(&home.to_string_lossy()),
                Some('/') => out.push_str(&home.to_string_lossy()),
                _ => {
                    return Err(
                        "only ~/ (current home) expansion is supported",
                    );
                },
            }
        } else if ch == '$' {
            match chars.peek() {
                Some('{') => {
                    chars.next();
                    let mut name = String::new();
                    for c in chars.by_ref() {
                        if c == '}' {
                            break;
                        }
                        name.push(c);
                    }
                    if name != "HOME" {
                        return Err("only $HOME expansion is supported");
                    }
                    out.push_str(&home.to_string_lossy());
                },
                Some(c) if c.is_ascii_alphabetic() || *c == '_' => {
                    let mut name = String::new();
                    while let Some(&c) = chars.peek() {
                        if c.is_ascii_alphanumeric() || c == '_' {
                            name.push(c);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    if name != "HOME" {
                        return Err("only $HOME expansion is supported");
                    }
                    out.push_str(&home.to_string_lossy());
                },
                _ => out.push('$'),
            }
        } else {
            out.push(ch);
        }
        first = false;
    }
    Ok(out)
}

#[cfg(test)]
mod nbrc_tests {
    use super::parse_nb_dir_assignment;

    /// Parser contract: supported forms resolve (last wins, empty
    /// clears, comments/quotes/ordering honored), unsupported
    /// `NB_DIR` syntax fails visibly, unrelated lines never matter.
    #[test]
    fn nb_dir_assignment_matrix() {
        let home = std::path::Path::new("/home/test");
        let cases: &[(&str, Result<Option<&str>, &str>)] = &[
            ("", Ok(None)),
            ("# nothing here\nFOO=bar\n", Ok(None)),
            ("NB_DIR=/data/nb\n", Ok(Some("/data/nb"))),
            ("export NB_DIR=/data/nb\n", Ok(Some("/data/nb"))),
            ("NB_DIR=/old\nNB_DIR=/new\n", Ok(Some("/new"))),
            ("NB_DIR=/new\nNB_DIR=\n", Ok(None)),
            ("NB_DIR=/new\nunset NB_DIR\n", Ok(None)),
            ("NB_DIR=/data # comment\n", Ok(Some("/data"))),
            ("NB_DIR=/data#kept\n", Ok(Some("/data#kept"))),
            ("NB_DIR='/a b'\n", Ok(Some("/a b"))),
            ("NB_DIR=\"/a $HOME\"\n", Ok(Some("/a /home/test"))),
            ("NB_DIR=$HOME/.nb\n", Ok(Some("/home/test/.nb"))),
            ("NB_DIR=${HOME}/x\n", Ok(Some("/home/test/x"))),
            ("NB_DIR=~/x\n", Ok(Some("/home/test/x"))),
            ("NB_DIR=\n", Ok(None)),
            ("NB_DIR=\"\"\n", Ok(None)),
            ("export NB_DIR\n", Ok(None)),
            ("MY_NB_DIR=/x\n", Ok(None)),
            ("echo hi\n", Ok(None)),
            ("NB_DIR=$(hostname)\n", Err("command substitution")),
            ("NB_DIR=`hostname`\n", Err("command substitution")),
            ("NB_DIR=\"$(hostname)\"\n", Err("command substitution")),
            ("if [ x ]; then NB_DIR=/x; fi\n", Err("top-level")),
            ("NB_DIR =/x\n", Err("top-level")),
            ("NB_DIR+=:/x\n", Err("top-level")),
            ("export A=1 NB_DIR=/x\n", Err("top-level")),
            ("NB_DIR=/a b\n", Err("whitespace")),
            ("NB_DIR=/x # trailing\n", Ok(Some("/x"))),
            ("NB_DIR=/win\r\n", Ok(Some("/win"))),
            ("unset -v NB_DIR\n", Err("plain `unset NB_DIR`")),
            ("NB_DIR=\"a\"b\n", Err("exactly quoted")),
            ("NB_DIR='a' 'b'\n", Err("exactly quoted")),
            ("NB_DIR=\"~/x\"\n", Ok(Some("~/x"))),
            ("NB_DIR='~/x'\n", Ok(Some("~/x"))),
            ("NB_DIR=\"a\\qb\"\n", Ok(Some("a\\qb"))),
            ("NB_DIR=a\\qb\n", Ok(Some("aqb"))),
            ("NB_DIR='unterminated\n", Err("single-quoted")),
            ("NB_DIR=\"unterminated\n", Err("exactly quoted")),
            ("NB_DIR=~other/x\n", Err("~/")),
            ("NB_DIR=$OTHER/x\n", Err("$HOME")),
            ("NB_DIR=${OTHER}\n", Err("$HOME")),
            ("NB_DIR=$NB_DIR/x\n", Err("$HOME")),
            ("readonly NB_DIR=/x\n", Err("top-level")),
            ("NB_DIR='a'b\n", Err("single-quoted")),
        ];
        for (input, expected) in cases {
            let result = parse_nb_dir_assignment(input, home);
            match (result, expected) {
                (Ok(got), Ok(want)) => {
                    assert_eq!(got.as_deref(), *want, "input {input:?}");
                },
                (Err(got), Err(want)) => {
                    assert!(
                        format!("{got:?}").contains(want),
                        "input {input:?} error {got:?} misses {want:?}"
                    );
                },
                (got, want) => {
                    panic!("input {input:?}: got {got:?}, want {want:?}");
                },
            }
        }
    }
}
