//! Cross-platform resolution of the `nb` executable for child-process
//! spawning.
//!
//! Unix: `nb` is a shebang bash script; `Command::new("nb")` resolves it
//! through `PATH` exactly as the shell would, so no extra work is needed.
//!
//! Windows: `nb` is installed as a `.cmd`/`.bat` launcher (npm's, or the
//! Git Bash shim from our `setup-nb` CI action) or a native `.exe`.
//! Rust's `std::process::Command` on Windows only searches for `.exe`
//! files and does **not** honor `PATHEXT`, so `Command::new("nb")` would
//! report "program not found" even when `nb.cmd` is on `PATH`. Both the
//! test fixture (`NbTestEnv`) and the production client therefore resolve
//! `nb` to an explicit spawnable path on Windows.
//!
//! See `nb-api:todos/api/9` and the `spike/windows-nb-viability` branch
//! for the empirical findings (npm `nb.cmd` segfault, bash.exe MSYS path
//! trap, `autocrlf=false` requirement).

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
#[cfg(windows)]
use std::path::Path;
#[cfg(windows)]
use std::sync::OnceLock;

/// `PATHEXT` fallback when the environment variable is unset (matches
/// cmd.exe's default). Used by the Windows resolver; compiled on all
/// platforms so the unit tests below can exercise the parsing anywhere.
#[cfg_attr(not(windows), allow(dead_code))]
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// Candidate file names for the `nb` executable within a single PATH
/// directory, in the order a Windows shell would try them (PATHEXT
/// order). Unix yields only the bare `nb` shebang script.
pub(crate) fn nb_candidate_names() -> Vec<OsString> {
    #[cfg(windows)]
    {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| DEFAULT_PATHEXT.to_string());
        pathext_names(&pathext)
    }
    #[cfg(not(windows))]
    {
        vec![OsString::from("nb")]
    }
}

/// Parse a `PATHEXT`-style value into `nb` + extension candidate names
/// (e.g. `.COM;.EXE;.BAT;.CMD` → `nb.com`, `nb.exe`, `nb.bat`,
/// `nb.cmd`). Not cfg-gated so the parsing is unit-testable on any host;
/// dead-code-allow on Unix where the resolver is absent.
#[cfg_attr(not(windows), allow(dead_code))]
fn pathext_names(pathext: &str) -> Vec<OsString> {
    pathext
        .split(';')
        .map(|ext| ext.trim())
        .filter(|ext| !ext.is_empty())
        .map(|ext| OsString::from(format!("nb{}", ext.to_lowercase())))
        .collect()
}

/// Program argument to pass to `Command::new` when spawning `nb`.
///
/// Unix: returns `"nb"` (PATH resolution, unchanged behaviour).
/// Windows: returns the absolute path of the first existing PATHEXT
/// candidate (`nb.cmd`, `nb.exe`, …) on `PATH`, honoring an absolute
/// `NB_API_TEST_NB` override; falls back to `"nb"` when nothing is found
/// so the caller's existing `ExecutableNotFound` error still fires.
pub(crate) fn nb_program() -> Cow<'static, OsStr> {
    #[cfg(windows)]
    {
        static RESOLVED: OnceLock<Option<OsString>> = OnceLock::new();
        match RESOLVED.get_or_init(resolve_nb_program_windows).as_ref() {
            Some(path) => Cow::Owned(path.clone()),
            None => Cow::Borrowed(OsStr::new("nb")),
        }
    }
    #[cfg(not(windows))]
    {
        Cow::Borrowed(OsStr::new("nb"))
    }
}

/// Windows-only: resolve `nb` to a spawnable absolute path via PATH +
/// PATHEXT (or `NB_API_TEST_NB`).
#[cfg(windows)]
fn resolve_nb_program_windows() -> Option<OsString> {
    if let Some(explicit) = std::env::var_os("NB_API_TEST_NB") {
        let path = Path::new(&explicit);
        if path.is_absolute() && path.is_file() {
            return Some(path.to_path_buf().into_os_string());
        }
    }
    let names = nb_candidate_names();
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for name in &names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate.into_os_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod nb_program_tests {
    use super::*;

    #[test]
    fn pathext_names_default_order() {
        assert_eq!(
            pathext_names(".COM;.EXE;.BAT;.CMD"),
            vec!["nb.com", "nb.exe", "nb.bat", "nb.cmd"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn pathext_names_tolerates_spacing_and_empty_segments() {
        assert_eq!(
            pathext_names("  .CMD ;; .BAT ;"),
            vec!["nb.cmd", "nb.bat"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn pathext_names_empty_yields_no_candidates() {
        assert!(pathext_names("").is_empty());
    }
}
