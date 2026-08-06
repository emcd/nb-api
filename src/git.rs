//! Git repository detection and notebook-repo helpers.
//!
//! Consumed by `nb-api` directly and by downstream tools (e.g.,
//! `nb-mcp-server`'s `paths.rs`) via the published crate.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::NbError;

/// Derive the notebook name from the current Git repository.
///
/// Returns the directory name of the master repository (not the worktree).
/// Used as a fallback when no explicit notebook name is configured.
pub fn derive_git_notebook_name() -> Option<String> {
    let current_root = git_rev_parse(&["--show-toplevel"])?;
    let git_common_dir = git_rev_parse(&["--git-common-dir"])?;
    let git_common_dir = if git_common_dir.is_relative() {
        current_root.join(&git_common_dir)
    } else {
        git_common_dir
    };
    let git_common_dir = git_common_dir.canonicalize().ok()?;
    let master_root = if git_common_dir.file_name().is_some_and(|n| n == ".git") {
        git_common_dir.parent()?.to_path_buf()
    } else {
        return None;
    };
    master_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
}

/// Run `git rev-parse` with the given arguments and return the output as a path.
///
/// Returns `None` if git is not available, the command fails, or the output is empty.
///
/// Strips inherited `GIT_*` routing vars before spawning so the git
/// invocation resolves the repository from its own cwd/args, not from
/// the parent hook or CI environment. See `nb-api:issues/3`.
pub fn git_rev_parse(args: &[&str]) -> Option<PathBuf> {
    git_rev_parse_in(Path::new("."), args)
}

/// `git rev-parse` with an explicit working directory.
pub fn git_rev_parse_in(cwd: &Path, args: &[&str]) -> Option<PathBuf> {
    let output = git_capture(cwd, &{
        let mut v = vec!["rev-parse".to_string()];
        v.extend(args.iter().map(|s| (*s).to_string()));
        v
    })
    .ok()?;
    let value = output.trim();
    if value.is_empty() {
        return None;
    }
    Some(PathBuf::from(value))
}

/// Run git in `cwd` with scrubbed `GIT_*` routing vars. Returns stdout on success.
pub fn git_capture(cwd: &Path, args: &[String]) -> Result<String, NbError> {
    let mut command = Command::new("git");
    crate::git_env::scrub_git_env_std(&mut command);
    // Preserve fixture git identity / signing overrides after scrub.
    if let Ok(v) = std::env::var("GIT_AUTHOR_NAME") {
        command.env("GIT_AUTHOR_NAME", v);
    }
    if let Ok(v) = std::env::var("GIT_AUTHOR_EMAIL") {
        command.env("GIT_AUTHOR_EMAIL", v);
    }
    if let Ok(v) = std::env::var("GIT_COMMITTER_NAME") {
        command.env("GIT_COMMITTER_NAME", v);
    }
    if let Ok(v) = std::env::var("GIT_COMMITTER_EMAIL") {
        command.env("GIT_COMMITTER_EMAIL", v);
    }
    command.current_dir(cwd);
    command.args(args);
    let joined = format!("git {}", args.join(" "));
    let output = command.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            NbError::ExecutableNotFound {
                path: "git".to_string(),
            }
        } else {
            NbError::Io {
                path: cwd.to_path_buf(),
                source: e.into(),
            }
        }
    })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr_text = if stderr.is_empty() {
            stdout.into_owned()
        } else {
            stderr.into_owned()
        };
        Err(NbError::CommandFailed {
            command: joined,
            stderr: stderr_text,
            exit_code: output.status.code(),
        })
    }
}

/// HEAD revision (`git rev-parse HEAD`) for a notebook worktree.
pub fn notebook_head(notebook_root: &Path) -> Result<String, NbError> {
    let out = git_capture(notebook_root, &["rev-parse".into(), "HEAD".into()])?;
    let rev = out.trim().to_string();
    if rev.is_empty() {
        return Err(NbError::CommandFailed {
            command: "git rev-parse HEAD".into(),
            stderr: "empty HEAD".into(),
            exit_code: None,
        });
    }
    Ok(rev)
}

/// True when `git status --porcelain` is non-empty (any dirty state).
pub fn notebook_is_dirty(notebook_root: &Path) -> Result<bool, NbError> {
    let out = git_capture(
        notebook_root,
        &[
            "status".into(),
            "--porcelain".into(),
            "-uall".into(),
            "--ignored=no".into(),
        ],
    )?;
    if !out.trim().is_empty() {
        return Ok(true);
    }
    // Belt-and-suspenders: unstaged or staged diffs against HEAD.
    let mut command = Command::new("git");
    crate::git_env::scrub_git_env_std(&mut command);
    command.current_dir(notebook_root);
    command.args(["diff-index", "--quiet", "HEAD", "--"]);
    let status = command.status().map_err(|e| NbError::Io {
        path: notebook_root.to_path_buf(),
        source: e.into(),
    })?;
    match status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(NbError::CommandFailed {
            command: "git diff-index --quiet HEAD --".into(),
            stderr: format!("unexpected exit {status}"),
            exit_code: status.code(),
        }),
    }
}

/// Concise porcelain status for recovery errors.
pub fn notebook_status_porcelain(notebook_root: &Path) -> Result<String, NbError> {
    git_capture(
        notebook_root,
        &["status".into(), "--porcelain".into(), "-uall".into()],
    )
    .map(|s| s.trim().to_string())
}

/// Stage paths and create one commit. Returns `Ok(true)` if a commit was
/// created, `Ok(false)` if there was nothing to commit (pure no-op tree).
///
/// `force_paths` are always `git add -f`'d so transaction-owned outputs that
/// match ignore rules (e.g. `.gitkeep`, new ignored filenames) enter the
/// checkpoint. Pre-existing ignored mutators must be rejected before calling.
///
/// When the `testing` feature is enabled, `NB_API_FAIL_AFTER_STAGE=1` makes
/// this return an error after staging (for rollback isolation regressions).
pub fn notebook_commit_all(
    notebook_root: &Path,
    message: &str,
    force_paths: &[String],
) -> Result<bool, NbError> {
    git_capture(notebook_root, &["add".into(), "-A".into()])?;
    for rel in force_paths {
        // `-f` stages ignored paths; missing paths are skipped (delete cases).
        let abs = notebook_root.join(rel);
        if abs.is_file() {
            git_capture(
                notebook_root,
                &["add".into(), "-f".into(), "--".into(), rel.clone()],
            )?;
        }
    }
    #[cfg(feature = "testing")]
    if std::env::var_os("NB_API_FAIL_AFTER_STAGE").is_some() {
        return Err(NbError::CommandFailed {
            command: "nb-api://fail-after-stage".into(),
            stderr: "injected staging failure for rollback tests".into(),
            exit_code: Some(1),
        });
    }
    // Detect empty commit: diff --cached --quiet exits 1 when there are staged changes.
    let mut command = Command::new("git");
    crate::git_env::scrub_git_env_std(&mut command);
    command.current_dir(notebook_root);
    command.args(["diff", "--cached", "--quiet"]);
    let status = command.status().map_err(|e| NbError::Io {
        path: notebook_root.to_path_buf(),
        source: e.into(),
    })?;
    // exit 0 = no staged diff; exit 1 = has staged changes; other = error
    match status.code() {
        Some(0) => Ok(false),
        Some(1) => {
            git_capture(
                notebook_root,
                &[
                    "commit".into(),
                    "-m".into(),
                    message.to_string(),
                    "--no-gpg-sign".into(),
                ],
            )?;
            Ok(true)
        }
        _ => Err(NbError::CommandFailed {
            command: "git diff --cached --quiet".into(),
            stderr: format!("unexpected exit {status}"),
            exit_code: status.code(),
        }),
    }
}

/// Hard-reset worktree/index to `revision` and remove untracked files.
pub fn notebook_reset_clean(notebook_root: &Path, revision: &str) -> Result<(), NbError> {
    git_capture(
        notebook_root,
        &["reset".into(), "--hard".into(), revision.to_string()],
    )?;
    git_capture(notebook_root, &["clean".into(), "-fd".into()])?;
    Ok(())
}

/// List tracked + untracked (non-ignored) notebook-relative paths, excluding `.git`.
pub fn list_notebook_paths(notebook_root: &Path) -> Result<Vec<String>, NbError> {
    let mut paths = Vec::new();
    let tracked = git_capture(notebook_root, &["ls-files".into(), "-z".into()])?;
    for p in tracked.split('\0') {
        if !p.is_empty() {
            paths.push(p.to_string());
        }
    }
    let untracked = git_capture(
        notebook_root,
        &[
            "ls-files".into(),
            "--others".into(),
            "--exclude-standard".into(),
            "-z".into(),
        ],
    )?;
    for p in untracked.split('\0') {
        if !p.is_empty() && !paths.iter().any(|e| e == p) {
            paths.push(p.to_string());
        }
    }
    // Also include empty directories that exist on disk (folders).
    fn walk_dirs(base: &Path, rel: &Path, out: &mut Vec<String>) -> Result<(), NbError> {
        let entries = std::fs::read_dir(base).map_err(|e| NbError::Io {
            path: base.to_path_buf(),
            source: e.into(),
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| NbError::Io {
                path: base.to_path_buf(),
                source: e.into(),
            })?;
            let name = entry.file_name();
            if name == ".git" {
                continue;
            }
            let file_type = entry.file_type().map_err(|e| NbError::Io {
                path: entry.path(),
                source: e.into(),
            })?;
            if file_type.is_dir() {
                let child_rel = rel.join(&name);
                let rel_str = child_rel.to_string_lossy().replace('\\', "/");
                if !out
                    .iter()
                    .any(|p| p == &rel_str || p.starts_with(&format!("{rel_str}/")))
                {
                    out.push(rel_str);
                }
                walk_dirs(&entry.path(), &child_rel, out)?;
            }
        }
        Ok(())
    }
    walk_dirs(notebook_root, Path::new(""), &mut paths)?;
    Ok(paths)
}

/// Existing ignored paths on disk (not part of the editable virtual tree).
pub fn list_ignored_paths(notebook_root: &Path) -> Result<Vec<String>, NbError> {
    let ignored = git_capture(
        notebook_root,
        &[
            "ls-files".into(),
            "--others".into(),
            "--ignored".into(),
            "--exclude-standard".into(),
            "-z".into(),
        ],
    )?;
    let mut paths = Vec::new();
    for p in ignored.split('\0') {
        if !p.is_empty() {
            paths.push(p.to_string());
        }
    }
    Ok(paths)
}
