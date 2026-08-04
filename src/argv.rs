//! CLI argv builders and small path/tag normalizers for `nb` invocations.

use crate::error::NbError;
use crate::{EditMode, SearchMode, TaskStatus};

pub(crate) fn edit_args(selector: String, content: &str, mode: EditMode) -> Vec<String> {
    let mut args = vec!["edit".to_string(), selector];
    match mode {
        EditMode::Overwrite => args.push("--overwrite".to_string()),
        EditMode::Append => {}
        EditMode::Prepend => args.push("--prepend".to_string()),
    }
    args.push("--content".to_string());
    args.push(content.to_string());
    args
}

pub(crate) fn task_command_args(
    action: &str,
    selector: String,
    task_number: Option<u32>,
) -> Vec<String> {
    let mut args = vec![action.to_string(), selector];
    if let Some(number) = task_number {
        args.push(number.to_string());
    }
    args
}

pub(crate) fn todo_command_args(
    notebook: &str,
    title: &str,
    description: Option<&str>,
    tasks: &[String],
    tags: &[String],
    folder: Option<&str>,
) -> Vec<String> {
    let mut args = vec![format!("{notebook}:todo"), "add".to_string()];

    // Folder path comes as a positional argument before the title.
    if let Some(folder) = folder {
        args.push(folder_scope(folder));
    }

    args.push(title.to_string());

    if let Some(description) = description {
        args.push("--description".to_string());
        args.push(description.to_string());
    }

    for task in tasks {
        args.push("--task".to_string());
        args.push(task.to_string());
    }

    for tag in tags {
        args.push("--tags".to_string());
        args.push(normalize_tag(tag));
    }

    args
}

pub(crate) fn folder_scope(folder: &str) -> String {
    if folder.ends_with('/') {
        folder.to_string()
    } else {
        format!("{folder}/")
    }
}

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

pub(crate) fn mkdir_selector(notebook: &str, path: &str) -> String {
    let normalized = normalize_folder(path);
    format!("{}:{}", notebook, normalized)
}

pub(crate) fn tasks_scope(notebook: &str, folder: Option<&str>) -> String {
    match folder {
        Some(path) if !path.is_empty() => format!("{}:{}/", notebook, path),
        _ => format!("{}:", notebook),
    }
}

pub(crate) fn tasks_command_args(scope: String, status: Option<TaskStatus>) -> Vec<String> {
    let mut args = vec!["tasks".to_string(), scope];
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
        }
        SearchMode::All => {
            for query in terms {
                args.push(query.to_string());
            }
        }
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

pub(crate) fn empty_tasks_message(status: Option<TaskStatus>) -> String {
    match status {
        Some(TaskStatus::Open) => "! 0 open tasks.".to_string(),
        Some(TaskStatus::Closed) => "! 0 closed tasks.".to_string(),
        None => "! 0 tasks.".to_string(),
    }
}

pub(crate) fn child_folder_names(path: &std::path::Path) -> Result<Vec<String>, NbError> {
    let read_dir = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(NbError::Io {
                path: path.to_path_buf(),
                source: err.into(),
            });
        }
    };

    let mut names = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|e| NbError::Io {
            path: path.to_path_buf(),
            source: e.into(),
        })?;
        let Some(name) = entry.file_name().to_str().map(|value| value.to_string()) else {
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
            }
        };
        if meta.is_dir() {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}
