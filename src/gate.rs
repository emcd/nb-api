//! Process-shared per-notebook operation gates.
//!
//! Serialization is **in-process only** and **process-shared** across all
//! [`NbClient`](crate::NbClient) values. The registry key is the realpath of
//! the notebook Git common directory (`git rev-parse --git-common-dir`).
//! Cross-process `index.lock` wait is deferred (`nb-api:todos/api/6`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{Mutex, MutexGuard, OwnedMutexGuard};

use crate::error::NbError;

/// Default maximum time to wait on a gate queue before returning
/// [`NbError::GateTimeout`].
pub const DEFAULT_GATE_TIMEOUT: Duration = Duration::from_secs(60);

static GLOBAL_GATE: LazyLock<Arc<Mutex<()>>> = LazyLock::new(|| Arc::new(Mutex::new(())));

static REGISTRY: LazyLock<StdMutex<HashMap<PathBuf, Arc<Mutex<()>>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

/// Held notebook (and optionally global) gate guards for a critical section.
pub struct GateHold {
    _notebook: OwnedMutexGuard<()>,
    _global: Option<OwnedMutexGuard<()>>,
}

/// Acquire only the process-shared global gate (e.g. `list_notebooks`).
pub async fn acquire_global(timeout: Duration) -> Result<OwnedMutexGuard<()>, NbError> {
    lock_with_timeout(Arc::clone(&GLOBAL_GATE), timeout, "global").await
}

/// Lookup-or-insert the notebook gate for `git_common_realpath` while holding
/// the global gate briefly, then acquire the notebook gate (global-then-notebook
/// order). The global gate is released before returning unless
/// `hold_global` is true.
pub async fn acquire_notebook(
    git_common_realpath: PathBuf,
    timeout: Duration,
    hold_global: bool,
) -> Result<GateHold, NbError> {
    let global_guard = lock_with_timeout(Arc::clone(&GLOBAL_GATE), timeout, "global").await?;
    let notebook_arc = {
        let mut map = REGISTRY.lock().unwrap_or_else(|p| p.into_inner());
        Arc::clone(
            map.entry(git_common_realpath)
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    };
    let notebook_guard = lock_with_timeout(notebook_arc, timeout, "notebook").await?;
    Ok(GateHold {
        _notebook: notebook_guard,
        _global: if hold_global {
            Some(global_guard)
        } else {
            drop(global_guard);
            None
        },
    })
}

/// Test/support: number of notebook gate entries currently registered.
#[cfg(feature = "testing")]
pub fn registry_len() -> usize {
    REGISTRY.lock().unwrap_or_else(|p| p.into_inner()).len()
}

/// Resolve `git rev-parse --git-common-dir` under `notebook_root` and
/// canonicalize to a realpath registry key.
pub fn git_common_dir_realpath(notebook_root: &Path) -> Result<PathBuf, NbError> {
    let raw =
        crate::git::git_rev_parse_in(notebook_root, &["--git-common-dir"]).ok_or_else(|| {
            NbError::ValidationError {
                reason: format!(
                    "notebook path {} is not an initialized git repository",
                    notebook_root.display()
                ),
                location: None,
            }
        })?;
    let absolute = if raw.is_relative() {
        notebook_root.join(raw)
    } else {
        raw
    };
    absolute.canonicalize().map_err(|e| NbError::Io {
        path: absolute.clone(),
        source: e.into(),
    })
}

async fn lock_with_timeout(
    lock: Arc<Mutex<()>>,
    timeout: Duration,
    label: &str,
) -> Result<OwnedMutexGuard<()>, NbError> {
    match tokio::time::timeout(timeout, lock.lock_owned()).await {
        Ok(guard) => Ok(guard),
        Err(_) => Err(NbError::GateTimeout {
            gate: label.to_string(),
            timeout_ms: timeout.as_millis() as u64,
        }),
    }
}

/// RAII helper used when a caller already holds nothing and only needs
/// a scoped async critical section name for docs/tests.
#[allow(dead_code)]
pub type GlobalGuard<'a> = MutexGuard<'a, ()>;
