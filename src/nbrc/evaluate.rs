//! Opted-in rc evaluation (`EvaluateNbrc`).
//!
//! Runs the rc file in a private, in-memory `brush` shell and reads
//! back the resulting variables. This is exactly what `nb` itself
//! does at startup by sourcing `~/.nbrc` as bash — the caller opts
//! into real execution explicitly. The host process is never
//! replaced or mutated: `exec`, `umask`, and `ulimit` are
//! unregistered (process replacement, umask, and rlimits are host
//! process state, not shell state), stdio is the null device, the
//! working directory is pinned, profile/rc autoloading is skipped,
//! and only variable bindings cross back. Sourcing reads the file
//! itself (not a copy) so top-level `return` gets a real source
//! frame.

use std::collections::HashMap;
use std::path::Path;

use crate::error::NbError;

/// Evaluate the rc file at `rc_path` as shell code and return the
/// resulting value of `NB_DIR` (`None` when unset-or-empty
/// afterwards).
///
/// `starting` is applied on top of the inherited environment
/// before sourcing (see module docs): `Some` sets `NB_DIR`,
/// `None` unsets it, so the evaluated starting point is
/// deterministic. Sourcing uses the file itself (not a copy), so
/// top-level `return` behaves as it does under real sourcing.
///
/// A `Some` seed always originates from the process environment
/// (see `resolve_with`), where every entry is exported by
/// construction — so the seed is unconditionally seeded exported,
/// keeping it visible to ordinary child processes the rc spawns.
/// There is no flag to read back and no host state to consult.
///
/// A non-zero exit status (including `exit N`) is a hard
/// configuration failure, never a fallback: a half-sourced
/// environment is worse than none.
pub(crate) async fn evaluate_nbrc(
    rc_path: &Path,
    working_dir: &Path,
    starting: Option<String>,
) -> Result<Option<String>, NbError> {
    let null_stdio = || {
        let null = |what: &str| {
            brush_core::openfiles::null().map_err(|e| {
                NbError::ValidationError {
                    reason: format!(
                        "notebook rc `{}` could not be evaluated ({what}): {e}",
                        rc_path.display()
                    ),
                    location: None,
                }
            })
        };
        Ok::<_, NbError>([
            (brush_core::openfiles::OpenFiles::STDIN_FD, null("stdin")?),
            (brush_core::openfiles::OpenFiles::STDOUT_FD, null("stdout")?),
            (brush_core::openfiles::OpenFiles::STDERR_FD, null("stderr")?),
        ])
    };
    // `exec` would replace the host process image; `umask` and
    // `ulimit` mutate host process state, not shell state. None has
    // a place in a private evaluation shell. Every other builtin
    // stays: the caller opted into real shell semantics.
    let mut builtins =
        brush_builtins::default_builtins(brush_builtins::BuiltinSet::BashMode);
    for name in ["exec", "umask", "ulimit"] {
        builtins.remove(name);
    }
    let mut shell = brush_core::Shell::builder()
        .profile(brush_core::ProfileLoadBehavior::Skip)
        .rc(brush_core::RcLoadBehavior::Skip)
        .working_dir(working_dir.to_path_buf())
        .fds(HashMap::from(null_stdio()?))
        .builtins(builtins)
        .build()
        .await
        .map_err(|e| NbError::ValidationError {
            reason: format!(
                "notebook rc `{}` could not be evaluated: {e}",
                rc_path.display()
            ),
            location: None,
        })?;
    match starting {
        Some(value) => {
            // Process-environment seeds are exported by
            // construction: re-export unconditionally so ordinary
            // children the rc spawns keep seeing `NB_DIR`.
            let mut var = brush_core::ShellVariable::new(value.as_str());
            var.export();
            shell.env_mut().set_global("NB_DIR", var).map_err(|e| {
                NbError::ValidationError {
                    reason: format!("notebook rc seeding failed: {e}"),
                    location: None,
                }
            })?;
        },
        None => {
            shell.env_mut().unset("NB_DIR").map_err(|e| {
                NbError::ValidationError {
                    reason: format!("notebook rc seeding failed: {e}"),
                    location: None,
                }
            })?;
        },
    }
    let params = shell.default_exec_params();
    let result = shell
        .source_script(rc_path, std::iter::empty::<String>(), &params)
        .await
        .map_err(|e| NbError::ValidationError {
            reason: format!(
                "notebook rc `{}` failed to evaluate: {e}",
                rc_path.display()
            ),
            location: None,
        })?;
    if !matches!(result.exit_code, brush_core::ExecutionExitCode::Success) {
        return Err(NbError::ValidationError {
            reason: format!(
                "notebook rc `{}` exited unsuccessfully; refusing to resolve the store",
                rc_path.display()
            ),
            location: None,
        });
    }
    Ok(shell
        .env_str("NB_DIR")
        .map(|v| v.into_owned())
        .filter(|v| !v.is_empty()))
}
