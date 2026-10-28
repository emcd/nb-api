//! CLI argv builders and small path/tag normalizers for `nb` invocations.

use crate::error::NbError;
use crate::{SearchMode, TaskStatus};

pub(crate) fn normalize_tag(tag: &str) -> String {
    if tag.starts_with('#') {
        tag.to_string()
    } else {
        format!("#{tag}")
    }
}

pub(crate) fn normalize_folder(folder: &str) -> String {
    folder.trim_matches('/').to_string()
}

pub(crate) fn tasks_scope(notebook: &str, folder: Option<&str>) -> String {
    match folder {
        Some(path) if !path.is_empty() => format!("{}:{}/", notebook, path),
        _ => format!("{}:", notebook),
    }
}

/// `nb todos tasks` argv for one scope: checklist items within todos
/// (the `nb todos` split replaced legacy `nb tasks`; outputs are
/// identical but notebook/folder scope resolves on the new verb).
pub(crate) fn todos_tasks_command_args(
    scope: String,
    status: Option<TaskStatus>,
) -> Vec<String> {
    let mut args = vec!["todos".to_string(), "tasks".to_string(), scope];
    if let Some(filter) = status {
        let status = match filter {
            TaskStatus::Open => "open",
            TaskStatus::Closed => "closed",
        };
        args.push(status.to_string());
    }
    args.push("--no-color".to_string());
    args
}

pub(crate) fn search_command_args(
    scope: String,
    queries: &[String],
    mode: SearchMode,
    tags: &[String],
) -> Vec<String> {
    let mut args = vec!["search".to_string(), scope];
    let mut terms = queries.iter();
    if let Some(first) = terms.next() {
        args.push(first.to_string());
    }
    match mode {
        SearchMode::Any => {
            for query in terms {
                args.push("--or".to_string());
                args.push(query.to_string());
            }
        },
        SearchMode::All => {
            for query in terms {
                args.push(query.to_string());
            }
        },
    }
    for tag in tags {
        args.push("--tag".to_string());
        args.push(normalize_tag(tag));
    }
    args.push("--no-color".to_string());
    args
}

pub(crate) fn is_empty_tasks_error(message: &str) -> bool {
    message.trim_start().starts_with("! 0 ") && message.contains(" tasks.")
}

/// Normalize nb's empty-tasks stderr (`! 0 open tasks.`) into the
/// empty signal returned by `list_tasks` (`0 open tasks.`):
/// strip the `!` error marker, keep the signal and terminator.
pub(crate) fn empty_tasks_signal(message: &str) -> String {
    let signal = message
        .trim()
        .strip_prefix('!')
        .map(str::trim_start)
        .unwrap_or_else(|| message.trim());
    format!("{signal}\n")
}

pub(crate) fn child_folder_names(
    path: &std::path::Path,
) -> Result<Vec<String>, NbError> {
    let read_dir = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        },
        Err(err) => {
            return Err(NbError::Io {
                path: path.to_path_buf(),
                source: err.into(),
            });
        },
    };

    let mut names = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|e| NbError::Io {
            path: path.to_path_buf(),
            source: e.into(),
        })?;
        let Some(name) =
            entry.file_name().to_str().map(|value| value.to_string())
        else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(NbError::Io {
                    path: path.to_path_buf(),
                    source: err.into(),
                });
            },
        };
        if meta.is_dir() {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}
