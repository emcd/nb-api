//! Hermetic test fixtures for integration tests that exercise the `nb`
//! CLI.
//!
//! This module is gated behind the `testing` Cargo feature so that
//! test-only dependencies (notably [`tempfile`]) do not enter the
//! build graph of normal consumers. Enable with
//! `--features testing`; add `--features testing-tokio` to also reach
//! the async helpers ([`NbTestEnv::configure_tokio`],
//! [`NbTestEnv::nb_command_async`]).
//!
//! # Quick start
//!
//! ```no_run
//! # #[cfg(feature = "testing")]
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use nb_api::testing::NbTestEnv;
//!
//! let env = NbTestEnv::new()?;
//! let mut command = env.nb_command();
//! command.arg("notebooks");
//! let output = command.output()?;
//! # Ok(())
//! # }
//! ```
//!
//! # Hermeticity
//!
//! The fixture owns an isolated [`NB_DIR`] (the data store where `nb`
//! writes notebooks) and a separate [`NbTestEnv::working_dir`]
//! (the execution cwd for spawned `nb` processes). Neither inherits
//! the caller's project-root CWD by default, though
//! [`NbTestEnvBuilder::working_directory`] can supply a caller-owned
//! path (the fixture then does not delete it on `Drop`). Cleanup
//! happens on `Drop`.
//!
//! Inherited `GIT_*` routing vars (`GIT_DIR`, `GIT_INDEX_FILE`,
//! `GIT_COMMON_DIR`, `GIT_WORK_TREE`, `GIT_OBJECT_DIRECTORY`,
//! `GIT_ALTERNATE_OBJECT_DIRECTORIES`) are stripped via
//! [`crate::git_env::scrub_git_env`] (or its `std` sibling) before
//! any intentional fixture overrides apply. See `nb-api:issues/3`.
//!
//! [`NB_DIR`]: https://github.com/xwmx/nb#environment-variables
//!
//! # `nb` binary resolution
//!
//! The fixture spawns the `nb` CLI during initialization via an
//! **absolute path** discovered once per process (see
//! [`nb_binary`]). Child processes receive a **safe `PATH`** that
//! always includes system directories so `#!/usr/bin/env bash` on
//! the `nb` script can resolve `bash` even when a concurrent test
//! has poisoned the parent process `PATH` (see `nb-api:issues/api/7`).
//!
//! The repository's `qa` workflow installs `nb` (pinned to the
//! `7.24.0` tag) before running tests. Override discovery with
//! `NB_API_TEST_NB` when needed.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, ExitStatus};
use std::sync::OnceLock;

use crate::git_env::scrub_git_env_std;
use crate::nb_program::nb_candidate_names;

#[cfg(feature = "testing-tokio")]
use tokio::process::Command as TokioCommand;

const DEFAULT_NOTEBOOK: &str = "scratch";

const GIT_AUTHOR_NAME: &str = "nb-api tests";
const GIT_AUTHOR_EMAIL: &str = "nb-api@localhost";

/// Deterministic git configuration applied to every fixture-spawned git
/// command, on top of the signing overrides. Forces `core.autocrlf=false`
/// so notebook repos are byte-identical across platforms: Git-for-Windows
/// defaults `autocrlf=true`, which renormalizes committed files to CRLF on
/// checkout and makes a fresh nb init commit appear dirty to the
/// transaction's baseline check (`nb-api:todos/api/9`, Windows CI finding).
///
/// Uses the `GIT_CONFIG_COUNT` mechanism so no global/`HOME` config file
/// is needed (the fixture HOME is a tempdir anyway).
fn apply_git_config_env(cmd: &mut impl GitEnvSetter) {
    cmd.env("GIT_CONFIG_COUNT", "4");
    cmd.env("GIT_CONFIG_KEY_0", "commit.gpgsign");
    cmd.env("GIT_CONFIG_VALUE_0", "false");
    cmd.env("GIT_CONFIG_KEY_1", "tag.gpgsign");
    cmd.env("GIT_CONFIG_VALUE_1", "false");
    cmd.env("GIT_CONFIG_KEY_2", "core.autocrlf");
    cmd.env("GIT_CONFIG_VALUE_2", "false");
    cmd.env("GIT_CONFIG_KEY_3", "core.eol");
    cmd.env("GIT_CONFIG_VALUE_3", "lf");
}

/// Minimal surface shared by [`StdCommand`] and [`TokioCommand`] so the
/// fixture's deterministic git-config env can be applied to both.
trait GitEnvSetter {
    fn env<K, V>(&mut self, key: K, value: V)
    where
        K: AsRef<std::ffi::OsStr>,
        V: AsRef<std::ffi::OsStr>;
}

impl GitEnvSetter for StdCommand {
    fn env<K, V>(&mut self, key: K, value: V)
    where
        K: AsRef<std::ffi::OsStr>,
        V: AsRef<std::ffi::OsStr>,
    {
        StdCommand::env(self, key, value);
    }
}

#[cfg(feature = "testing-tokio")]
impl GitEnvSetter for TokioCommand {
    fn env<K, V>(&mut self, key: K, value: V)
    where
        K: AsRef<std::ffi::OsStr>,
        V: AsRef<std::ffi::OsStr>,
    {
        TokioCommand::env(self, key, value);
    }
}

/// System / platform directories always injected into fixture child
/// `PATH` so shebang interpreters (`env bash`) and common `nb`
/// install locations resolve under PATH poison tests.
///
/// Includes Apple Silicon Homebrew (`/opt/homebrew/bin`) and
/// Intel Homebrew / manual prefixes (`/usr/local/bin`).
const SAFE_PATH_DIRS: &[&str] = &[
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
fn resolve_nb_override(path: &Path) -> Result<PathBuf, String> {
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
fn canonicalize_executable(path: &Path) -> Option<PathBuf> {
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

/// Login home for the current user (ignores env `HOME`, which tests
/// overwrite with fixture tempdirs).
///
/// Order (Unix):
/// 1. macOS `id -P` passwd-style line (Open Directory–backed accounts)
/// 2. macOS `dscl` `NFSHomeDirectory` for the current user name
/// 3. `/etc/passwd` by UID (Linux and legacy macOS local files)
///
/// External tools are invoked by absolute path so a poisoned process
/// `PATH` cannot block discovery.
fn login_home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        if let Some(home) = home_from_id_p() {
            return Some(home);
        }
        if let Some(home) = home_from_dscl() {
            return Some(home);
        }
        home_from_etc_passwd_uid()
    }
    #[cfg(not(unix))]
    {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
    }
}

/// macOS `id -P`: print the current user as a passwd-style entry
/// (works for Open Directory accounts that are absent from
/// `/etc/passwd`). GNU/Linux `id` typically rejects `-P`.
#[cfg(unix)]
fn home_from_id_p() -> Option<PathBuf> {
    for id_bin in ["/usr/bin/id", "/bin/id"] {
        let output = match StdCommand::new(id_bin).arg("-P").output() {
            Ok(o) => o,
            Err(_) => continue,
        };
        if !output.status.success() {
            continue;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout.lines().next()?;
        if let Some(home) = home_from_passwd_style_line(line) {
            return Some(home);
        }
    }
    None
}

/// macOS Directory Service home lookup for the current user name.
#[cfg(unix)]
fn home_from_dscl() -> Option<PathBuf> {
    let user = current_username()?;
    // Only meaningful on macOS; spawn failure is non-fatal.
    let output = match StdCommand::new("/usr/bin/dscl")
        .args([".", "-read", &format!("/Users/{user}"), "NFSHomeDirectory"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return None,
    };
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_dscl_home(&stdout)
}

#[cfg(unix)]
fn home_from_etc_passwd_uid() -> Option<PathBuf> {
    let uid = current_uid()?;
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    for line in passwd.lines() {
        let mut fields = line.split(':');
        let _name = fields.next()?;
        let _passwd = fields.next()?;
        let file_uid = fields.next()?.parse::<u32>().ok()?;
        if file_uid != uid {
            continue;
        }
        let _gid = fields.next()?;
        let _gecos = fields.next()?;
        let home = fields.next()?;
        if !home.is_empty() {
            return Some(PathBuf::from(home));
        }
    }
    None
}

/// Home directory from a passwd-style colon line.
///
/// Traditional (7 fields): `name:pw:uid:gid:gecos:home:shell` → home @ 5.
/// macOS `id -P` (10 fields):
/// `name:pw:uid:gid:class:change:expire:gecos:home:shell` → home @ 8.
#[cfg_attr(not(unix), allow(dead_code))]
fn home_from_passwd_style_line(line: &str) -> Option<PathBuf> {
    let fields: Vec<&str> = line.trim().split(':').collect();
    let home = if fields.len() >= 10 {
        fields[8]
    } else if fields.len() >= 7 {
        fields[5]
    } else {
        return None;
    };
    if home.is_empty() {
        None
    } else {
        Some(PathBuf::from(home))
    }
}

/// Parse `dscl … NFSHomeDirectory` stdout.
#[cfg_attr(not(unix), allow(dead_code))]
fn parse_dscl_home(stdout: &str) -> Option<PathBuf> {
    for line in stdout.lines() {
        let line = line.trim();
        let Some(rest) = line
            .strip_prefix("NFSHomeDirectory:")
            .or_else(|| line.strip_prefix("dsAttrTypeNative:NFSHomeDirectory:"))
        else {
            continue;
        };
        let home = rest.trim();
        if !home.is_empty() {
            return Some(PathBuf::from(home));
        }
    }
    None
}

#[cfg(unix)]
fn current_username() -> Option<String> {
    for id_bin in ["/usr/bin/id", "/bin/id"] {
        let output = match StdCommand::new(id_bin).arg("-un").output() {
            Ok(o) => o,
            Err(_) => continue,
        };
        if !output.status.success() {
            continue;
        }
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

#[cfg(unix)]
fn current_uid() -> Option<u32> {
    // Absolute id(1) — does not depend on process PATH (may be poisoned).
    for id_bin in ["/usr/bin/id", "/bin/id"] {
        let output = match StdCommand::new(id_bin).arg("-u").output() {
            Ok(o) => o,
            Err(_) => continue,
        };
        if !output.status.success() {
            continue;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Ok(uid) = stdout.trim().parse::<u32>() {
            return Some(uid);
        }
    }
    // Linux-only fallback when id(1) is unavailable.
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        let Some(rest) = line.strip_prefix("Uid:") else {
            continue;
        };
        return rest.split_whitespace().next()?.parse().ok();
    }
    None
}

#[cfg(test)]
mod nb_resolve_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn relative_nb_override_is_rejected() {
        let err = resolve_nb_override(Path::new("relative/nb")).unwrap_err();
        assert!(
            err.contains("absolute"),
            "expected absolute-path error, got {err}"
        );
    }

    #[test]
    fn relative_candidate_is_not_canonicalized() {
        assert!(canonicalize_executable(Path::new("nb")).is_none());
        assert!(canonicalize_executable(Path::new("./nb")).is_none());
    }

    #[test]
    fn safe_path_dirs_include_homebrew_prefix() {
        assert!(
            SAFE_PATH_DIRS.contains(&"/opt/homebrew/bin"),
            "Apple Silicon Homebrew must be in SAFE_PATH_DIRS for poisoned-PATH discovery"
        );
    }

    #[test]
    fn parses_macos_id_p_home_field() {
        let line = "me:********:501:20::0:0:Me:/Users/me:/bin/zsh";
        assert_eq!(
            home_from_passwd_style_line(line),
            Some(PathBuf::from("/Users/me"))
        );
    }

    #[test]
    fn parses_traditional_passwd_home_field() {
        let line = "me:x:1000:1000:Me:/home/me:/bin/bash";
        assert_eq!(
            home_from_passwd_style_line(line),
            Some(PathBuf::from("/home/me"))
        );
    }

    #[test]
    fn parses_dscl_nfs_home_directory() {
        assert_eq!(
            parse_dscl_home("NFSHomeDirectory: /Users/me\n"),
            Some(PathBuf::from("/Users/me"))
        );
        assert_eq!(
            parse_dscl_home("dsAttrTypeNative:NFSHomeDirectory: /Users/me\n"),
            Some(PathBuf::from("/Users/me"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn absolute_executable_is_canonicalized() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("fake-nb");
        fs::write(&bin, b"#!/bin/sh\n").expect("write");
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("chmod");
        let abs = bin.canonicalize().expect("canonicalize fixture path");
        let got = canonicalize_executable(&abs).expect("accept absolute executable");
        assert!(got.is_absolute());
        assert_eq!(got, abs);
    }
}

/// A captured `nb` subprocess failure: exit status, stdout, and
/// stderr preserved separately so callers can inspect all three
/// streams when a fixture-initialization command fails.
#[derive(Debug)]
pub struct NbFailure {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl std::fmt::Display for NbFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "exit status: {}\nstdout: {}\nstderr: {}",
            self.status,
            if self.stdout.is_empty() {
                "<empty>"
            } else {
                &self.stdout
            },
            if self.stderr.is_empty() {
                "<empty>"
            } else {
                &self.stderr
            },
        )
    }
}

impl std::error::Error for NbFailure {}

/// Errors raised while building or initializing an [`NbTestEnv`].
#[derive(Debug, thiserror::Error)]
pub enum NbTestError {
    #[error("io error during {context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    #[error("nb command failed during {context}: {failure}")]
    Nb {
        context: String,
        #[source]
        failure: NbFailure,
    },
}

/// Builder for [`NbTestEnv`]. Constructed via [`NbTestEnv::builder`].
#[derive(Debug, Default)]
pub struct NbTestEnvBuilder {
    notebook: Option<String>,
    working_directory: Option<PathBuf>,
}

impl NbTestEnvBuilder {
    /// Override the notebook name created during fixture initialization.
    pub fn notebook(mut self, name: impl Into<String>) -> Self {
        self.notebook = Some(name.into());
        self
    }

    /// Use a caller-owned path as the execution cwd of spawned `nb`
    /// processes. The fixture does not delete this path on `Drop`;
    /// the fixture-owned data store under the fixture's root tempdir
    /// is still cleaned up automatically.
    pub fn working_directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(path.into());
        self
    }

    /// Build the fixture, initialize its notebook, and return it.
    pub fn build(self) -> Result<NbTestEnv, NbTestError> {
        let root = tempfile::Builder::new()
            .prefix("nb-api-test-")
            .tempdir()
            .map_err(|e| NbTestError::Io {
                context: "create isolated root tempdir".to_string(),
                source: e,
            })?;
        let nb_dir = root.path().join(".nb");
        std::fs::create_dir_all(&nb_dir).map_err(|e| NbTestError::Io {
            context: format!("create nb dir at {}", nb_dir.display()),
            source: e,
        })?;
        let working_dir = match self.working_directory {
            Some(path) => path,
            None => {
                let path = root.path().join("work");
                std::fs::create_dir_all(&path).map_err(|e| NbTestError::Io {
                    context: format!("create working dir at {}", path.display()),
                    source: e,
                })?;
                path
            }
        };
        let home_dir = root.path().join("home");
        std::fs::create_dir_all(&home_dir).map_err(|e| NbTestError::Io {
            context: format!("create fixture HOME at {}", home_dir.display()),
            source: e,
        })?;
        let notebook = self
            .notebook
            .unwrap_or_else(|| DEFAULT_NOTEBOOK.to_string());
        let env = NbTestEnv {
            root,
            nb_dir,
            working_dir,
            home_dir,
            notebook,
        };
        env.initialize_notebook()?;
        Ok(env)
    }
}

/// Hermetic fixture for integration tests that exercise the `nb` CLI.
///
/// Constructed via [`NbTestEnv::new`] for the common case or
/// [`NbTestEnv::builder`] for configuration.
///
/// # Drop semantics
///
/// The fixture-owned root tempdir is removed on `Drop`. A
/// caller-supplied `working_directory` (via [`NbTestEnvBuilder`]) is
/// outside the root and is left intact.
pub struct NbTestEnv {
    /// Held for its `Drop` cleanup of the fixture-owned root
    /// tempdir; never read by name. The fixture's derived paths
    /// (`nb_dir`, `working_dir`) are stored separately so callers
    /// can inspect them without taking the root.
    #[allow(dead_code)]
    root: tempfile::TempDir,
    nb_dir: PathBuf,
    working_dir: PathBuf,
    /// Fixture-owned `$HOME` so `nb`'s `_git_required` global-config
    /// check (`git config --global --includes user.name`) finds a
    /// deterministic `user.name`/`user.email` and never falls into
    /// its interactive prompt. The fixture writes `.gitconfig`
    /// here during [`initialize_notebook`](Self::initialize_notebook).
    home_dir: PathBuf,
    notebook: String,
}

impl NbTestEnv {
    /// Build a fixture with hermetic defaults (isolated root,
    /// default notebook name, separate `nb_dir` and `working_dir`,
    /// scrubbed inherited `GIT_*`, deterministic git identity, signing
    /// disabled).
    pub fn new() -> Result<Self, NbTestError> {
        Self::builder().build()
    }

    /// Begin building a fixture with non-default knobs.
    pub fn builder() -> NbTestEnvBuilder {
        NbTestEnvBuilder::default()
    }

    /// Path of the `NB_DIR` data store where `nb` writes notebooks.
    /// Isolated git repository under the fixture-owned root.
    pub fn nb_dir(&self) -> &Path {
        &self.nb_dir
    }

    /// Path of the execution cwd for spawned `nb` processes. Distinct
    /// from [`nb_dir`](Self::nb_dir) by design.
    pub fn working_dir(&self) -> &Path {
        &self.working_dir
    }

    /// Fixture-owned `$HOME` directory. The fixture writes a
    /// deterministic `.gitconfig` here so `nb`'s `_git_required`
    /// global-config check always finds a `user.name`/`user.email`
    /// and never falls into its interactive prompt.
    pub fn home_dir(&self) -> &Path {
        &self.home_dir
    }

    /// Name of the notebook created during fixture initialization.
    pub fn notebook(&self) -> &str {
        &self.notebook
    }

    /// Apply the fixture's environment to a `std::process::Command`:
    /// strip inherited `GIT_*` routing vars, set `NB_DIR`, set a
    /// deterministic git author/committer identity, disable commit
    /// and tag signing, pin a safe child `PATH`, and set `current_dir`
    /// to [`working_dir`](Self::working_dir).
    pub fn configure_std(&self, cmd: &mut StdCommand) {
        scrub_git_env_std(cmd);
        cmd.env("PATH", fixture_child_path());
        cmd.env("NB_DIR", &self.nb_dir);
        cmd.env("HOME", &self.home_dir);
        cmd.env("GIT_AUTHOR_NAME", GIT_AUTHOR_NAME);
        cmd.env("GIT_AUTHOR_EMAIL", GIT_AUTHOR_EMAIL);
        cmd.env("GIT_COMMITTER_NAME", GIT_AUTHOR_NAME);
        cmd.env("GIT_COMMITTER_EMAIL", GIT_AUTHOR_EMAIL);
        apply_git_config_env(cmd);
        cmd.current_dir(&self.working_dir);
    }

    /// Async counterpart to [`configure_std`](Self::configure_std).
    /// Available only with the `testing-tokio` Cargo feature.
    #[cfg(feature = "testing-tokio")]
    pub fn configure_tokio(&self, cmd: &mut TokioCommand) {
        crate::git_env::scrub_git_env(cmd);
        cmd.env("PATH", fixture_child_path());
        cmd.env("NB_DIR", &self.nb_dir);
        cmd.env("HOME", &self.home_dir);
        cmd.env("GIT_AUTHOR_NAME", GIT_AUTHOR_NAME);
        cmd.env("GIT_AUTHOR_EMAIL", GIT_AUTHOR_EMAIL);
        cmd.env("GIT_COMMITTER_NAME", GIT_AUTHOR_NAME);
        cmd.env("GIT_COMMITTER_EMAIL", GIT_AUTHOR_EMAIL);
        apply_git_config_env(cmd);
        cmd.current_dir(&self.working_dir);
    }

    /// Convenience accessor: a fresh `std::process::Command` for `nb`
    /// with the fixture's environment applied. Uses the absolute
    /// [`nb_binary`] path so parent-process `PATH` poison cannot
    /// prevent executable lookup (`issues/api/7`).
    pub fn nb_command(&self) -> StdCommand {
        let mut cmd = StdCommand::new(nb_binary());
        self.configure_std(&mut cmd);
        cmd
    }

    /// Async counterpart to [`nb_command`](Self::nb_command). Available
    /// only with the `testing-tokio` Cargo feature.
    #[cfg(feature = "testing-tokio")]
    pub fn nb_command_async(&self) -> TokioCommand {
        let mut cmd = TokioCommand::new(nb_binary());
        self.configure_tokio(&mut cmd);
        cmd
    }

    fn initialize_notebook(&self) -> Result<(), NbTestError> {
        // Write `$HOME/.gitconfig` so `nb`'s `_git_required` global
        // check finds a deterministic `user.name`/`user.email` and
        // never falls into its interactive prompt. Without this, the
        // first `nb` invocation hangs on a `read` for Name/Email
        // when stdin is not a TTY.
        let gitconfig = self.home_dir.join(".gitconfig");
        let gitconfig_contents =
            format!("[user]\n\tname = {GIT_AUTHOR_NAME}\n\temail = {GIT_AUTHOR_EMAIL}\n",);
        std::fs::write(&gitconfig, gitconfig_contents).map_err(|e| NbTestError::Io {
            context: format!("write .gitconfig to {}", gitconfig.display()),
            source: e,
        })?;

        // Pre-create a hidden init stub. `nb`'s main loop short-
        // circuits its first-run `_init` (welcome screen and
        // interactive author prompt) when both `NB_DIR` and
        // `NB_NOTEBOOK_PATH` exist. The stub satisfies that check
        // without leaving a phantom `home` notebook: a leading dot
        // hides it from `ls -1` (which `nb notebooks` uses), and
        // `NB_NOTEBOOK_PATH` is pointed at the stub only for the
        // init command. Subsequent commands use `.current` and
        // never resolve through the stub.
        //
        // We use `.init_stub` rather than the conventional
        // `home` so that callers can build a fixture with
        // `notebook("home")` if they want — there is no conflict
        // because the stub is a different name.
        let init_stub = self.nb_dir.join(".init_stub");
        std::fs::create_dir_all(&init_stub).map_err(|e| NbTestError::Io {
            context: format!("create init stub at {}", init_stub.display()),
            source: e,
        })?;

        // Build the init command's env inline rather than via
        // `configure_std`. The init command needs `NB_NOTEBOOK_PATH`
        // pointing at the stub; `configure_std` deliberately does
        // not set `NB_NOTEBOOK_PATH` because subsequent operations
        // must resolve the current notebook through `.current`.
        // Absolute `nb` + safe child PATH: parent PATH may be
        // poisoned by a concurrent harness test (`issues/api/7`).
        let mut cmd = StdCommand::new(nb_binary());
        scrub_git_env_std(&mut cmd);
        cmd.env("PATH", fixture_child_path());
        cmd.env("NB_DIR", &self.nb_dir);
        cmd.env("NB_NOTEBOOK_PATH", &init_stub);
        cmd.env("HOME", &self.home_dir);
        cmd.env("GIT_AUTHOR_NAME", GIT_AUTHOR_NAME);
        cmd.env("GIT_AUTHOR_EMAIL", GIT_AUTHOR_EMAIL);
        cmd.env("GIT_COMMITTER_NAME", GIT_AUTHOR_NAME);
        cmd.env("GIT_COMMITTER_EMAIL", GIT_AUTHOR_EMAIL);
        apply_git_config_env(&mut cmd);
        cmd.current_dir(&self.working_dir);
        cmd.arg("notebooks").arg("add").arg(&self.notebook);
        let output = cmd.output().map_err(|e| NbTestError::Io {
            context: format!("spawn `nb notebooks add {}`", self.notebook),
            source: e,
        })?;
        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            return Err(NbTestError::Nb {
                context: format!("`nb notebooks add {}`", self.notebook),
                failure: NbFailure {
                    status: output.status,
                    stdout,
                    stderr,
                },
            });
        }

        // Point `nb` at our notebook so subsequent invocations
        // target it via the on-disk `.current` marker that nb
        // reads on every call. The init stub is left in place
        // (hidden, never listed) so the test process can verify
        // `_init` bypass behavior without rebuilding it.
        std::fs::write(self.nb_dir.join(".current"), &self.notebook).map_err(|e| {
            NbTestError::Io {
                context: format!(
                    "write .current to {}",
                    self.nb_dir.join(".current").display()
                ),
                source: e,
            }
        })?;

        // Ensure a clean committed baseline. nb's `_git checkpoint` runs
        // the init commit in a background subshell (`( ... ) &`). When nb
        // is spawned through a `.cmd` launcher (Windows), that orphaned
        // commit is torn down when the wrapper exits, leaving `.index`
        // untracked and every later `Transaction::commit` rejected with
        // `DirtyBaseline`. Run the commit synchronously ourselves with
        // the fixture's deterministic git env so the baseline is identical
        // on Unix and Windows. See `nb-api:todos/api/9`.
        //
        // `git` resolves through the inherited process PATH (as in
        // `crate::git::git_capture`); the fixture child PATH only carries
        // nb's dir + system dirs and would not locate git on Windows.
        // If git cannot spawn (poisoned process PATH, issues/api/7), the
        // baseline is skipped — the poisoned test never runs a
        // transaction, so a missing baseline is harmless there.
        let notebook_root = self.nb_dir.join(&self.notebook);
        let baseline_result: Result<(), NbTestError> = (|| {
            // Write the notebook repo's LOCAL config so every subsequent
            // git invocation (including production `git::git_capture`,
            // which scrubs GIT_CONFIG_* env) sees byte-identical LF
            // handling. The GIT_CONFIG_COUNT env override only reaches
            // fixture-spawned commands; repo-local config persists.
            // See `nb-api:todos/api/9`.
            let mut git_config = StdCommand::new("git");
            scrub_git_env_std(&mut git_config);
            git_config.env("HOME", &self.home_dir);
            apply_git_config_env(&mut git_config);
            git_config.current_dir(&notebook_root);
            git_config.args(["config", "core.autocrlf", "false"]);
            let cfg = git_config.output().map_err(|e| NbTestError::Io {
                context: format!(
                    "fixture baseline `git config core.autocrlf false` in {}",
                    notebook_root.display()
                ),
                source: e,
            })?;
            if !cfg.status.success() {
                let stdout = String::from_utf8_lossy(&cfg.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&cfg.stderr).into_owned();
                return Err(NbTestError::Nb {
                    context: format!(
                        "fixture baseline `git config core.autocrlf false` in {}",
                        notebook_root.display()
                    ),
                    failure: NbFailure {
                        status: cfg.status,
                        stdout,
                        stderr,
                    },
                });
            }
            let mut git = StdCommand::new("git");
            scrub_git_env_std(&mut git);
            git.env("HOME", &self.home_dir);
            git.env("GIT_AUTHOR_NAME", GIT_AUTHOR_NAME);
            git.env("GIT_AUTHOR_EMAIL", GIT_AUTHOR_EMAIL);
            git.env("GIT_COMMITTER_NAME", GIT_AUTHOR_NAME);
            git.env("GIT_COMMITTER_EMAIL", GIT_AUTHOR_EMAIL);
            apply_git_config_env(&mut git);
            git.current_dir(&notebook_root);
            git.arg("add").arg("-A");
            let add = git.output().map_err(|e| NbTestError::Io {
                context: format!(
                    "fixture baseline `git add -A` in {}",
                    notebook_root.display()
                ),
                source: e,
            })?;
            if !add.status.success() {
                let stdout = String::from_utf8_lossy(&add.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&add.stderr).into_owned();
                return Err(NbTestError::Nb {
                    context: format!(
                        "fixture baseline `git add -A` in {}",
                        notebook_root.display()
                    ),
                    failure: NbFailure {
                        status: add.status,
                        stdout,
                        stderr,
                    },
                });
            }
            let mut git_commit = StdCommand::new("git");
            scrub_git_env_std(&mut git_commit);
            git_commit.env("HOME", &self.home_dir);
            git_commit.env("GIT_AUTHOR_NAME", GIT_AUTHOR_NAME);
            git_commit.env("GIT_AUTHOR_EMAIL", GIT_AUTHOR_EMAIL);
            git_commit.env("GIT_COMMITTER_NAME", GIT_AUTHOR_NAME);
            git_commit.env("GIT_COMMITTER_EMAIL", GIT_AUTHOR_EMAIL);
            apply_git_config_env(&mut git_commit);
            git_commit.current_dir(&notebook_root);
            git_commit.args(["commit", "-m", "[nb] Initialize"]);
            let commit = git_commit.output().map_err(|e| NbTestError::Io {
                context: format!(
                    "fixture baseline `git commit` in {}",
                    notebook_root.display()
                ),
                source: e,
            })?;
            if !commit.status.success() {
                // nb may already have committed the baseline synchronously
                // (Unix); a `nothing to commit` exit is not an error.
                // Git may write the message to stdout or stderr.
                let stdout = String::from_utf8_lossy(&commit.stdout);
                let stderr = String::from_utf8_lossy(&commit.stderr);
                if !stdout.contains("nothing to commit") && !stderr.contains("nothing to commit") {
                    return Err(NbTestError::Nb {
                        context: format!(
                            "fixture baseline `git commit` in {}",
                            notebook_root.display()
                        ),
                        failure: NbFailure {
                            status: commit.status,
                            stdout: stdout.into_owned(),
                            stderr: stderr.into_owned(),
                        },
                    });
                }
            }
            Ok(())
        })();
        if let Err(NbTestError::Io { source, .. }) = &baseline_result
            && source.kind() == std::io::ErrorKind::NotFound
        {
            // Poisoned process PATH (issues/api/7): git unresolvable,
            // baseline intentionally skipped.
            tracing::debug!("fixture baseline skipped: git not on process PATH");
        } else {
            baseline_result?;
        }

        Ok(())
    }
}
