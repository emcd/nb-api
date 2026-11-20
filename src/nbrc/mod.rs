//! Notebook rc (`~/.nbrc`) parsing and store resolution for the
//! native line.
//!
//! Mirrors the `nb` CLI startup contract: `nb` sources
//! `$NBRC_PATH` (default `~/.nbrc`) unconditionally at startup, so
//! the rc is always parsed when present — an explicit `NB_DIR`
//! is only the starting value its self-defaults apply on top of.
//! `ParseNbrc` parses the rc with a real shell grammar
//! (`brush-parser`, AST walk, no execution) over a documented
//! subset: unconditional top-level `NB_DIR` assignments (bare and
//! `export`), last wins, with a variable table tracking `$NB_DIR`
//! self-defaults (`:-`, `:=`, `:+` and friends, with empty kept
//! distinct from unset exactly as the shell distinguishes them).
//! Anything undeterminable without execution selects a warned
//! fallback to `~/.nb` instead of being guessed at.
//! `EvaluateNbrc` instead sources the rc in a private in-memory
//! shell and takes the resulting variables (opt-in execution,
//! exactly what `nb` itself does). `IgnoreNbrc` never reads the rc.
//! Nothing here mutates process state, caches rc contents, or
//! spawns `nb` or git; only `EvaluateNbrc` executes rc code, and
//! only inside its private shell.

use std::path::{Path, PathBuf};

use crate::error::NbError;

use walk::{ItemOutcome, program_opaque_reason, walk_item};

mod evaluate;
mod walk;

/// How the caller wants `~/.nbrc` handled during store resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResolutionMode {
    /// Use only the process environment; never read the rc file
    /// (not even to check its existence). Hermetic escape hatch;
    /// reserves a future `ExplicitVariables { vars }` payload.
    IgnoreNbrc,
    /// Always parse the rc when present (declared AST subset,
    /// never executed); warn-and-fallback outside the subset.
    /// The default: `nb` parity without execution.
    #[default]
    ParseNbrc,
    /// Source the rc in a private in-memory shell and take the
    /// resulting variables. Opt-in execution, exactly as `nb`
    /// itself does.
    EvaluateNbrc,
}

/// Select the notebook store from rc text, expanded against
/// `home`, using a real shell parse (`brush-parser`).
///
/// `starting` is the pre-existing `NB_DIR` value (`None` when
/// unset; `Some("")` stays set-but-empty so `${VAR-...}` and
/// `${VAR:-...}` evaluate exactly as the shell distinguishes
/// them). `home` is `None` when `HOME` is unavailable; home
/// expansions then fail as undeterminable instead of expanding
/// to garbage.
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

pub(crate) fn select_nb_dir(
    input: &str,
    home: Option<&Path>,
    starting: Option<String>,
) -> NbrcSelection {
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
    // empty — preserved distinctly so `-` and `:-` evaluate as the
    // shell does). The table starts from the caller's `starting`
    // value and only tracks what the rc itself assigns on top.
    // The table is the single authoritative state: `walk_item`
    // applies every mutation exactly once as it walks, and this
    // loop only records the last direct assignment outcome for the
    // final selection — it never rewrites the table, so a side
    // effect later in the same command cannot be clobbered.
    let mut nb_dir: Option<String> = starting;
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
        // (Bash leaves it set), or the starting value survived a
        // silent rc. Empty still counts as unset.
        NbrcSelection::Assign(value)
    } else {
        NbrcSelection::Absent
    }
}

/// Default store (`~/.nb`) or a configuration error when no
/// default is computable (missing `HOME`).
fn default_store(home: Option<&Path>) -> Result<PathBuf, NbError> {
    let Some(home) = home else {
        return Err(NbError::ValidationError {
            reason: "home directory is not available, so the default notebook store cannot be resolved"
                .to_string(),
            location: None,
        });
    };
    Ok(home.join(".nb"))
}

/// Resolve the notebook store per the caller-selected mode.
/// Never mutates process state; only `EvaluateNbrc` executes rc
/// code, inside its private shell.
pub(crate) async fn resolve_nbrc_store(
    mode: ResolutionMode,
) -> Result<PathBuf, NbError> {
    let raw_explicit = std::env::var_os("NB_DIR");
    let home = home_dir();
    let nbrc_path = std::env::var_os("NBRC_PATH");
    resolve_with(mode, raw_explicit, home, nbrc_path).await
}

/// Resolve against explicit inputs (pure apart from rc file I/O
/// and, in `EvaluateNbrc`, private-shell execution). Separated
/// from the environment read so the precedence matrix is
/// unit-testable without mutating process state.
pub(crate) async fn resolve_with(
    mode: ResolutionMode,
    raw_explicit: Option<std::ffi::OsString>,
    home: Option<PathBuf>,
    nbrc_path: Option<std::ffi::OsString>,
) -> Result<PathBuf, NbError> {
    let home_path = home.as_deref();
    // Every arm below produces the store; the absolute-path gate
    // at the end covers all modes uniformly — no mode bypasses it.
    let store = if mode == ResolutionMode::IgnoreNbrc {
        if let Some(dir) = raw_explicit.filter(|v| !v.is_empty()) {
            tracing::debug!("notebook store from explicit NB_DIR");
            PathBuf::from(dir)
        } else {
            default_store(home_path)?
        }
    } else {
        // ParseNbrc / EvaluateNbrc: the rc is always parsed when
        // present, even with NB_DIR already set.
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
                Some(path)
            },
            None => home_path.map(|h| h.join(".nbrc")),
        };
        let starting: Option<String> = raw_explicit
            .as_ref()
            .map(|v| v.to_string_lossy().into_owned());
        let text = match rc_path {
            None => None,
            Some(ref path) => match std::fs::read_to_string(path) {
                Ok(text) => Some(text),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if nbrc_path.as_ref().is_some_and(|v| !v.is_empty()) {
                        return Err(NbError::ValidationError {
                            reason: format!(
                                "`NBRC_PATH` points at a missing file: `{}`",
                                path.display()
                            ),
                            location: None,
                        });
                    }
                    tracing::debug!(
                        "notebook store without rc file (default rc absent)"
                    );
                    None
                },
                Err(e) => {
                    tracing::warn!(
                        "notebook rc `{}` unreadable ({}); treating as absent",
                        path.display(),
                        e
                    );
                    None
                },
            },
        };
        match (mode, text) {
            (_, None) => match starting.filter(|v| !v.is_empty()) {
                Some(value) => PathBuf::from(value),
                None => default_store(home_path)?,
            },
            (ResolutionMode::EvaluateNbrc, Some(text)) => {
                let cwd =
                    std::env::current_dir().map_err(|e| NbError::Io {
                        path: PathBuf::from("."),
                        source: e.into(),
                    })?;
                let rc_path =
                    rc_path.expect("rc path resolved when text is present");
                // Sourced from the file itself (not the text read above)
                // so top-level `return` gets a real source frame; the
                // pre-read above only classifies existence. A concurrent
                // edit between the two reads is accepted: anyone who can
                // write the rc already owns the account.
                let _read = text;
                match evaluate::evaluate_nbrc(&rc_path, &cwd, starting).await?
                {
                    Some(value) => PathBuf::from(value),
                    None => default_store(home_path)?,
                }
            },
            (_, Some(text)) => {
                let rc_path =
                    rc_path.expect("rc path resolved when text is present");
                match select_nb_dir(&text, home_path, starting) {
                    NbrcSelection::Assign(raw) => PathBuf::from(raw),
                    NbrcSelection::Fallback(reason) => {
                        tracing::warn!(
                            "notebook rc `{}` is ambiguous ({}); falling back to the default store",
                            rc_path.display(),
                            reason
                        );
                        default_store(home_path)?
                    },
                    NbrcSelection::Absent => {
                        tracing::debug!(
                            "notebook store from default ~/.nb (no NB_DIR in rc)"
                        );
                        default_store(home_path)?
                    },
                }
            },
        }
    };
    if !store.is_absolute() {
        return Err(NbError::ValidationError {
            reason: format!(
                "notebook store must be absolute, got `{}`",
                store.display()
            ),
            location: None,
        });
    }
    tracing::debug!("notebook store `{}` selected", store.display());
    Ok(store)
}

/// Process `HOME` when set and non-empty; `None` otherwise.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests;
