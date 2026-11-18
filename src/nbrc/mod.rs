//! Notebook rc (`~/.nbrc`) parsing and store resolution.
//!
//! Replicates the `nb` CLI startup contract without executing
//! anything: `nb` sources `$NBRC_PATH` (default `~/.nbrc`) and then
//! applies `NB_DIR="${NB_DIR:-$HOME/.nb}"`. Sourcing an arbitrary rc
//! file would execute shell code, so this module parses the rc
//! with a real shell grammar (`brush-parser`, AST walk, no
//! execution) over a documented subset: unconditional top-level
//! `NB_DIR` assignments (bare and `export`), last wins, with a
//! variable table tracking `$NB_DIR` self-defaults (`:-`, `:=`,
//! `:+` and friends). Anything undeterminable without execution
//! selects a warned fallback to `~/.nb` instead of being guessed
//! at. The resolution chain is explicit process `NB_DIR`, else the
//! parsed rc value, else `~/.nb`; empty values count as unset
//! (`:-` semantics). Nothing here mutates process state, caches rc
//! contents, or spawns a subprocess.

use std::path::{Path, PathBuf};

use crate::error::NbError;

use walk::{ItemOutcome, program_opaque_reason, walk_item};

mod walk;

/// Select the notebook store from rc text, expanded against
/// `home`, using a real shell parse (`brush-parser`).
///
/// Nothing is executed. Only unconditional top-level `NB_DIR`
/// assignments count: bare `NB_DIR=<value>` and
/// `export NB_DIR=<value>`, with the last assignment winning, and
/// `unset NB_DIR` clearing. Anything that makes the value
/// undeterminable without execution — conditionals, loops,
/// function bodies, backgrounded or piped commands, command
/// substitution, non-`$HOME` expansions, array assignments, `+=`,
/// `source`/`.`, `eval`-family builtins touching `NB_DIR`, or any
/// other `NB_DIR` mention outside the supported forms — selects
/// `Fallback`, which resolves to the default with a warning.
/// Files that never mention `NB_DIR` select `Absent` (quiet
/// default). A literal `~` or `$HOME` expands against `home`;
/// anything else undeterminable falls back instead of being
/// guessed at.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NbrcSelection {
    /// Use this literal store path (still subject to the
    /// absolute-path check in resolution).
    Assign(String),
    /// Fall back to the default store; warn with the reason.
    Fallback(&'static str),
    /// No `NB_DIR` involvement; fall back quietly.
    Absent,
}

pub(crate) fn select_nb_dir(input: &str, home: &Path) -> NbrcSelection {
    // Normalize Windows line endings before parsing: a trailing
    // CR is a transport artifact, not part of the value.
    let normalized = input.replace("\r\n", "\n");
    let options = brush_parser::ParserOptions::default();
    let mut parser =
        brush_parser::Parser::new(normalized.as_bytes(), &options);
    let program = match parser.parse_program() {
        Ok(program) => program,
        Err(_) => return NbrcSelection::Fallback("unparseable rc"),
    };
    // Opaque dispatch anywhere — sourcing, re-dispatching
    // (`builtin`/`command`), in-shell execution (`eval`/`exec`),
    // dynamic names, or names mentioning `NB_DIR` — top level,
    // behind a guard, or inside any compound command or function
    // body — may set `NB_DIR` from a file that is never read here
    // or through a dispatch this walker does not model. Detect it
    // structurally by effective command identity (quotes and
    // concatenation resolved, not spellings listed) and fall back
    // for the whole file rather than keep a store that could have
    // been overridden. Sourced files are never read.
    let mut scan_state: Option<String> = None;
    if let Some(reason) =
        program_opaque_reason(&program, home, &mut scan_state)
    {
        return NbrcSelection::Fallback(reason);
    }
    // Current known `NB_DIR` value while walking: `None` is unset
    // (including after `unset`), `Some(value)` is set (possibly
    // empty). Process `NB_DIR` is unset here — resolution returns
    // early otherwise — so the table starts empty and only tracks
    // what the rc itself assigns. Empty counts as unset for `:-`
    // but stays set-but-null for bare `+/ -` tests. The table is
    // the single authoritative state: `walk_item` applies every
    // mutation exactly once as it walks, and this loop only
    // records the last direct assignment outcome for the final
    // selection — it never rewrites the table, so a side effect
    // later in the same command cannot be clobbered.
    let mut nb_dir: Option<String> = None;
    let mut selected: Option<String> = None;
    let mut undetermined: Option<&'static str> = None;
    for complete in &program.complete_commands {
        for item in &complete.0 {
            match walk_item(item, home, &mut nb_dir) {
                ItemOutcome::Assign(value) => {
                    selected =
                        if value.is_empty() { None } else { Some(value) };
                },
                ItemOutcome::Clear => {
                    selected = None;
                },
                ItemOutcome::Undetermined(reason) => {
                    if undetermined.is_none() {
                        undetermined = Some(reason);
                    }
                },
                ItemOutcome::Ignore => {},
            }
        }
    }
    if let Some(reason) = undetermined {
        NbrcSelection::Fallback(reason)
    } else if let Some(value) = selected {
        NbrcSelection::Assign(value)
    } else if let Some(value) = nb_dir.clone().filter(|v| !v.is_empty()) {
        // No direct `NB_DIR` assignment, but a `${NB_DIR:=...}`
        // side effect through another variable persisted one
        // (Bash leaves it set). Empty still counts as unset.
        NbrcSelection::Assign(value)
    } else {
        NbrcSelection::Absent
    }
}

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
        Ok(text) => match select_nb_dir(&text, &home_path) {
            NbrcSelection::Assign(raw) => PathBuf::from(raw),
            NbrcSelection::Fallback(reason) => {
                tracing::warn!(
                    "notebook rc `{}` is ambiguous ({}); falling back to the default store",
                    rc_path.display(),
                    reason
                );
                home_path.join(".nb")
            },
            NbrcSelection::Absent => {
                tracing::debug!(
                    "notebook store from default ~/.nb (no NB_DIR in rc)"
                );
                home_path.join(".nb")
            },
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

#[cfg(test)]
mod tests;
