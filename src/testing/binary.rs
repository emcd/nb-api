use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::nb_program::nb_candidate_names;

use super::home::login_home_dir;

/// System / platform directories always injected into fixture child
/// `PATH` so shebang interpreters (`env bash`) and common `nb`
/// install locations resolve under PATH poison tests.
///
/// Includes Apple Silicon Homebrew (`/opt/homebrew/bin`) and
/// Intel Homebrew / manual prefixes (`/usr/local/bin`).
pub(super) const SAFE_PATH_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

static NB_BINARY: OnceLock<PathBuf> = OnceLock::new();
static CHILD_PATH: OnceLock<OsString> = OnceLock::new();

/// Absolute canonical path to the `nb` executable used by fixtures.
///
/// Resolution order: absolute `NB_API_TEST_NB`, fixed install
/// locations (incl. Homebrew), login-home `~/.local/bin/nb` (macOS
/// `id -P` / `dscl`, then `/etc/passwd`), then absolute non-poisoned
/// `PATH` entries. Relative override/PATH candidates are rejected.
/// Concurrent tests may poison process `PATH` (`issues/api/7`);
/// discovery does not rely on it alone.
pub fn nb_binary() -> &'static Path {
    NB_BINARY.get_or_init(discover_nb_binary).as_path()
}

/// `PATH` value applied to every fixture-spawned child.
pub fn fixture_child_path() -> &'static OsString {
    CHILD_PATH.get_or_init(|| {
        let nb = nb_binary();
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(parent) = nb.parent() {
            dirs.push(parent.to_path_buf());
        }
        for d in SAFE_PATH_DIRS {
            let p = PathBuf::from(d);
            if !dirs.iter().any(|x| x == &p) {
                dirs.push(p);
            }
        }
        std::env::join_paths(dirs).unwrap_or_else(|_| OsString::from("/usr/bin:/bin"))
    })
}

fn discover_nb_binary() -> PathBuf {
    if let Some(explicit) = std::env::var_os("NB_API_TEST_NB") {
        return resolve_nb_override(Path::new(&explicit)).unwrap_or_else(|reason| {
            panic!("nb-api testing: invalid NB_API_TEST_NB: {reason}");
        });
    }

    // Candidate file names per directory. On Windows this is the PATHEXT
    // set (`nb.cmd`, `nb.exe`, …) because CreateProcess cannot spawn an
    // extensionless bash script; on Unix it is the bare `nb`.
    let names = nb_candidate_names();

    let mut candidates: Vec<PathBuf> = Vec::new();
    for d in SAFE_PATH_DIRS {
        for name in &names {
            candidates.push(PathBuf::from(d).join(name));
        }
    }

    if let Some(home) = login_home_dir() {
        for name in &names {
            candidates.push(home.join(".local/bin").join(name));
        }
    }

    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            if path_dir_looks_poisoned(&dir) || !dir.is_absolute() {
                continue;
            }
            for name in &names {
                candidates.push(dir.join(name));
            }
        }
    }

    let mut tried = Vec::new();
    for c in &candidates {
        tried.push(c.clone());
        if let Some(abs) = canonicalize_executable(c) {
            return abs;
        }
    }

    panic!(
        "nb-api testing: could not locate an executable `nb` binary. \
         Install nb 7.24.0 or set NB_API_TEST_NB to its absolute path. \
         candidates tried: {tried:?}"
    );
}

/// Validate `NB_API_TEST_NB`: must be absolute, exist, be executable,
/// and is returned in canonical form.
pub(super) fn resolve_nb_override(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!(
            "path must be absolute (got relative {:?}); \
             relative overrides break after fixture current_dir changes",
            path
        ));
    }
    canonicalize_executable(path).ok_or_else(|| {
        format!(
            "path is not an executable file after canonicalize: {:?}",
            path
        )
    })
}

/// Canonical absolute executable, or `None` if `path` is relative,
/// missing, or not executable.
pub(super) fn canonicalize_executable(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let canonical = path.canonicalize().ok()?;
    if !is_executable_file(&canonical) {
        return None;
    }
    Some(canonical)
}

fn path_dir_looks_poisoned(dir: &Path) -> bool {
    let s = dir.to_string_lossy();
    s.contains("poisoned")
        || s.contains("nb-shim-")
        || s == "/nonexistent"
        || s.starts_with("/nonexistent")
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(windows)]
    {
        // CreateProcess cannot spawn an extensionless bash script; only
        // accept files with a PATHEXT-style executable extension
        // (nb.cmd, nb.exe, nb.bat, nb.com). See `nb-api:todos/api/9`.
        has_spawnable_windows_extension(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        true
    }
}

/// True when `path`'s file name carries a CreateProcess-spawnable
/// extension from the PATHEXT set (case-insensitive).
#[cfg(windows)]
fn has_spawnable_windows_extension(path: &Path) -> bool {
    path.extension().is_some_and(|ext| {
        let ext = ext.to_string_lossy().to_ascii_lowercase();
        matches!(ext.as_str(), "com" | "exe" | "bat" | "cmd")
    })
}
