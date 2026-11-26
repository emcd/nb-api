use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::LazyLock;

use regex::Regex;
use tokio::process::Command;

use crate::error::NbError;
use crate::gate;
use crate::git::derive_git_notebook_name;
use crate::git_env::scrub_git_env;
use crate::git_signing::apply_git_signing_env;
use crate::nb_program::nb_program;
use crate::transaction::Transaction;
use crate::validate::{parse_qualified_selector, validate_notebook_name};
use crate::{Config, NbClient};

use super::lifecycle::{
    ensure_notebook_dir_at, notebook_dir_from_env_at, notebook_root_dir,
};

/// Regex to match ANSI/ISO 2022 escape sequences.
///
/// Covers:
/// - Fe sequences: `ESC [@-Z\-_]` (single byte after ESC)
/// - CSI sequences: `ESC [ ... m` (SGR colors, cursor control, etc.)
/// - nF sequences: `ESC [ -/]* [0-~]` (character set designation like `ESC ( B`)
static ANSI_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~]|[ -/]*[0-~])").unwrap()
});

/// Strip ANSI escape sequences from text.
fn strip_ansi(text: &str) -> String {
    ANSI_REGEX.replace_all(text, "").into_owned()
}

const FOLDER_REQUIRED_MESSAGE: &str = "This server is configured to require `folder` for new notes. Use the `nb.mkdir` tool to create new folders and the `nb.folders` tool to list existing folders.";

impl NbClient {
    /// Creates a new nb client.
    ///
    /// Uses the notebook from config if set, otherwise falls back to a
    /// Git-derived notebook name. Does NOT read `NB_MCP_NOTEBOOK` —
    /// that is an MCP-server-specific env var resolved by the server.
    pub fn new(config: &Config) -> anyhow::Result<Self> {
        let default_notebook = config
            .notebook
            .as_deref()
            .map(String::from)
            .or_else(derive_git_notebook_name);
        Ok(Self {
            default_notebook,
            create_notebook: config.create_notebook,
            disable_git_signing: config.disable_git_signing,
            nbrc_mode: config.nbrc_mode,
            allow_top_level_notes: config.allow_top_level_notes,
            gate_timeout: config.gate_timeout,
        })
    }

    /// Begin a collect-then-commit [`Transaction`] for `notebook`.
    ///
    /// Construction performs no durable I/O and does not acquire the gate.
    pub async fn transaction(
        &self,
        notebook: Option<&str>,
    ) -> Result<Transaction, NbError> {
        let notebook = self.resolve_notebook_name(notebook)?;
        // Native ensure converges on concurrent creators and refuses
        // cleanly; no subprocess runs on this path. The resolved
        // root is pinned in the transaction: commit must not
        // re-resolve.
        let root = self.ensure_notebook(&notebook).await?;
        Ok(Transaction::new(notebook, root, self.gate_timeout))
    }

    /// Begin a [`Transaction`] against an already-ensured notebook
    /// root: no discovery, no second resolution. The caller must
    /// have ensured this exact root (e.g. via
    /// [`Self::resolve_target_selector`]); the transaction pins it
    /// through commit.
    pub(super) fn transaction_in(
        &self,
        notebook: &str,
        notebook_path: PathBuf,
    ) -> Transaction {
        Transaction::new(
            notebook.to_string(),
            notebook_path,
            self.gate_timeout,
        )
    }

    /// Process-shared gate queue timeout configured on this client.
    pub fn gate_timeout(&self) -> std::time::Duration {
        self.gate_timeout
    }

    /// Gate hold over an already-resolved notebook root: no
    /// discovery, no ensure, no spawns. Native reads resolve the root
    /// with the no-create lookup before acquiring this gate, so a
    /// missing notebook never materializes as a side effect.
    pub(super) async fn with_notebook_gate_native<F, T>(
        &self,
        root: &Path,
        f: F,
    ) -> Result<T, NbError>
    where
        F: std::future::Future<Output = Result<T, NbError>>,
    {
        let key = gate::git_common_dir_realpath(root)?;
        let _hold =
            gate::acquire_notebook(key.clone(), self.gate_timeout, false)
                .await?;
        #[cfg(test)]
        {
            // Test-only rendezvous: park the holder so a
            // crate-internal regression test can mutate the mapping
            // before resolution runs. No production code installs a
            // probe; absent probe is a no-op. The wait is bounded
            // (fail-open) so an abandoned test never parks reads
            // forever.
            let notify = native_gate_probe::fired(&key);
            if let Some(proceed) = notify {
                let _ = tokio::time::timeout(
                    native_gate_probe::WAIT_TIMEOUT,
                    proceed.notified(),
                )
                .await;
            }
        }
        f.await
    }

    pub(super) fn require_folder_for_new_note(
        &self,
        folder: Option<&str>,
    ) -> Result<(), NbError> {
        if self.allow_top_level_notes
            || folder.is_some_and(|value| !value.trim().is_empty())
        {
            return Ok(());
        }
        Err(NbError::ValidationError {
            reason: FOLDER_REQUIRED_MESSAGE.to_string(),
            location: None,
        })
    }

    pub(super) async fn resolve_target_selector(
        &self,
        id: &str,
        notebook: Option<&str>,
    ) -> Result<(String, String, PathBuf), NbError> {
        if let Some((embedded_notebook, path)) = parse_qualified_selector(id)?
        {
            let notebook = match notebook {
                Some(value) => {
                    validate_notebook_name(value)?;
                    if value != embedded_notebook {
                        return Err(NbError::ValidationError {
                            reason: format!(
                                "ambiguous selector: id targets notebook `{embedded_notebook}`, but notebook field is `{value}`"
                            ),
                            location: None,
                        });
                    }
                    embedded_notebook.to_string()
                },
                _ => embedded_notebook.to_string(),
            };
            let root = self.ensure_existing_notebook(&notebook).await?;
            return Ok((
                notebook,
                format!("{}:{}", embedded_notebook, path),
                root,
            ));
        }
        let (notebook, root) = self.resolve_notebook(notebook).await?;
        Ok((notebook.clone(), format!("{}:{}", notebook, id), root))
    }

    /// Resolves the notebook to use for a command.
    pub(super) fn resolve_notebook_name(
        &self,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        if let Some(name) = notebook {
            validate_notebook_name(name)?;
            return Ok(name.to_string());
        }
        if let Some(name) = self.default_notebook.as_deref() {
            validate_notebook_name(name)?;
            return Ok(name.to_string());
        }
        Err(NbError::ValidationError {
            reason:
                "notebook not configured; set --notebook or NB_MCP_NOTEBOOK"
                    .to_string(),
            location: None,
        })
    }

    pub(super) async fn resolve_notebook(
        &self,
        notebook: Option<&str>,
    ) -> Result<(String, PathBuf), NbError> {
        let name = self.resolve_notebook_name(notebook)?;
        let root = self.ensure_notebook(&name).await?;
        Ok((name, root))
    }

    /// Ensures the notebook exists, creating it natively when allowed.
    /// The store root, resolved exactly once per call. Every
    /// ensure/create/existence check in an operation must derive
    /// from this root (via the `_at` variants), never re-resolve.
    pub(super) async fn resolve_store_root(&self) -> Result<PathBuf, NbError> {
        notebook_root_dir(self.nbrc_mode).await
    }

    pub(super) async fn ensure_notebook(
        &self,
        notebook: &str,
    ) -> Result<PathBuf, NbError> {
        let root = self.resolve_store_root().await?;
        ensure_notebook_dir_at(&root, notebook, self.create_notebook).await
    }

    /// Ensures the notebook exists without creating it; a missing
    /// notebook is terminal `NotFound`. Present-but-not-ready
    /// entries surface the genuine git failure instead of reading
    /// as absent.
    pub(super) async fn ensure_existing_notebook(
        &self,
        notebook: &str,
    ) -> Result<PathBuf, NbError> {
        let root = self.resolve_store_root().await?;
        notebook_dir_from_env_at(&root, notebook).await?.ok_or(
            NbError::NotFound {
                selector: format!("{notebook}:"),
            },
        )
    }

    /// Executes an nb command and returns stdout.
    pub(super) async fn exec(&self, args: &[&str]) -> Result<String, NbError> {
        tracing::debug!(?args, "executing nb command");
        // `nb_program` resolves `nb` to a spawnable absolute path on
        // Windows (PATHEXT-aware: Rust std only finds `.exe` there and
        // would miss a `nb.cmd` launcher). Unix keeps PATH resolution.
        let mut command = Command::new(nb_program());
        // Strip inherited `GIT_*` routing vars before chaining `.args` /
        // `.env`. Without this, any caller invoking us from inside a
        // git hook (pre-commit, pre-push, post-checkout) or CI runner
        // propagates GIT_DIR / GIT_INDEX_FILE / GIT_COMMON_DIR /
        // GIT_WORK_TREE / GIT_OBJECT_DIRECTORY /
        // GIT_ALTERNATE_OBJECT_DIRECTORIES into the spawned `nb`,
        // which is a bash script wrapping git — every git call inside
        // nb then redirects to the parent repo instead of the
        // notebook's repo. The blast-by-prefix also covers future
        // GIT_* redirect vars without requiring a code change.
        // See `nb-api:issues/3`. Do not remove.
        scrub_git_env(&mut command);
        // Canonicalize the child terminal: real `nb` 7.24.0 emits
        // SGR sequences plus an `ESC ( B` residue on some code
        // paths when `TERM` advertises color (e.g.
        // `TERM=xterm-256color`), despite trailing `--no-color`
        // flags, and it does not honor `NO_COLOR`. `TERM` unset
        // and `TERM=dumb` both yield plain output; `dumb` is set
        // (rather than removed) because it stays a valid terminfo
        // entry for children that require `TERM` to exist. The
        // `--no-color` flags and the ANSI stripper stay as defense
        // in depth. See `nb-mcp-server:todos/mcp/64`.
        command.env("TERM", "dumb");
        command
            .args(args)
            .stdin(Stdio::null()) // Prevent TTY hangs
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if self.disable_git_signing {
            apply_git_signing_env(&mut command);
        }
        let joined = format!("nb {}", args.join(" "));
        let output = command
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    NbError::ExecutableNotFound {
                        path: "nb".to_string(),
                    }
                } else {
                    NbError::Io {
                        path: PathBuf::from("nb"),
                        source: e.into(),
                    }
                }
            })?
            .wait_with_output()
            .await
            .map_err(|e| NbError::Io {
                path: PathBuf::from("nb"),
                source: e.into(),
            })?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            Ok(strip_ansi(&stdout))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            // nb sometimes writes errors to stdout
            let stderr_text = if stderr.is_empty() {
                strip_ansi(&stdout)
            } else {
                strip_ansi(&stderr)
            };
            Err(NbError::CommandFailed {
                command: joined,
                stderr: stderr_text,
                exit_code: output.status.code(),
            })
        }
    }

    /// Executes an nb command with dynamic arguments.
    pub(super) async fn exec_vec(
        &self,
        args: Vec<String>,
    ) -> Result<String, NbError> {
        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.exec(&args_ref).await
    }
}

/// Test-only rendezvous inside the native gate hold.
///
/// Fired after the hold is acquired, before the gated future runs,
/// so a crate-internal regression test can park a reader mid-hold,
/// mutate the mapping, then release it — deterministically proving
/// that resolution happens under the hold. No production code
/// installs a probe; an absent probe is a no-op. Waiting is bounded
/// (fail-open): an abandoned rendezvous never parks reads forever.
#[cfg(test)]
pub(super) mod native_gate_probe {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, MutexGuard};
    use std::time::Duration;

    use tokio::sync::{Notify, mpsc};

    static PROBE: Mutex<Option<Probe>> = Mutex::new(None);

    struct Probe {
        entered: mpsc::UnboundedSender<()>,
        proceed: Arc<Notify>,
        /// Registry key (canonical git common dir) this probe applies
        /// to; reads under other notebooks never rendezvous.
        scope: PathBuf,
    }

    /// RAII uninstall: dropping clears the probe before releasing
    /// the install lock, so later reads never rendezvous with a
    /// stale sender.
    pub(super) struct Guard {
        _lock: MutexGuard<'static, ()>,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            *PROBE.lock().unwrap() = None;
        }
    }

    static INSTALL_LOCK: Mutex<()> = Mutex::new(());

    /// Install the rendezvous scoped to one gate key; fails when
    /// another test holds it. Returns a guard that uninstalls on
    /// drop.
    pub(super) fn install(
        entered: mpsc::UnboundedSender<()>,
        proceed: Arc<Notify>,
        scope: PathBuf,
    ) -> Option<Guard> {
        let lock = INSTALL_LOCK.try_lock().ok()?;
        *PROBE.lock().unwrap() = Some(Probe {
            entered,
            proceed,
            scope,
        });
        Some(Guard { _lock: lock })
    }

    /// Whether a probe is currently installed (lifecycle assertion).
    pub(super) fn installed() -> bool {
        PROBE.lock().unwrap().is_some()
    }

    /// Signal entry and return the gate to wait on, when a probe is
    /// installed for this gate key. The wait itself is bounded by
    /// the caller (fail-open on abandonment).
    pub(super) fn fired(scope: &Path) -> Option<Arc<Notify>> {
        PROBE.lock().unwrap().as_ref().and_then(|probe| {
            if probe.scope != scope {
                return None;
            }
            // A failed send means the test is gone: still return
            // the gate so the caller can bound its own wait instead
            // of parking forever.
            let _ = probe.entered.send(());
            Some(probe.proceed.clone())
        })
    }

    /// Fail-open bound for a parked rendezvous wait.
    pub(super) const WAIT_TIMEOUT: Duration = Duration::from_secs(30);
}

// NOTE: this crate-internal test exists because the queued-reader
// property (a read parked inside the hold while the index mapping
// changes) is unreachable deterministically through any public
// interface: gate acquisition is crate-private, so integration
// tests can only race the hold statistically (see
// `native_reads_stay_consistent_under_contention`). Per convention
// this stays the single inline test exercising a public path;
// every other native-path case lives in `tests/integration`.
#[cfg(test)]
mod native_gate_tests {
    use std::ffi::OsString;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use tokio::sync::Notify;

    use super::native_gate_probe;
    use crate::{Config, NbClient};

    static FIXTURE_SEQ: AtomicU64 = AtomicU64::new(0);

    struct EnvRestore {
        key: &'static str,
        prev: Option<OsString>,
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[tokio::test]
    async fn native_held_gate_reader_sees_mapping_change() {
        // Bare git notebook, no `nb` needed: native reads never
        // spawn, and gate keys only need `git rev-parse`.
        let seq = FIXTURE_SEQ.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir()
            .join(format!("nb-api-gate-probe-{}-{seq}", std::process::id()));
        let root = base.join("nbdir").join("nb1");
        std::fs::create_dir_all(&root).expect("fixture dirs");
        let git_init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .expect("git init runs");
        assert!(git_init.success(), "git init must succeed");
        // Validity requires a resolvable HEAD: an initial commit
        // makes this hand-built repo a complete notebook without
        // involving `nb`.
        let git_commit = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=nb-api-test",
                "-c",
                "user.email=nb-api-test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "init",
            ])
            .current_dir(&root)
            .status()
            .expect("git commit runs");
        assert!(git_commit.success(), "git commit must succeed");
        std::fs::write(root.join(".index"), "victim.md\nother.md\n")
            .expect("index");
        std::fs::write(root.join("victim.md"), "# Victim\nv0\n")
            .expect("victim");
        std::fs::write(root.join("other.md"), "# Other\no\n").expect("other");

        let prev = std::env::var_os("NB_DIR");
        unsafe {
            std::env::set_var("NB_DIR", base.join("nbdir"));
        }
        let _env = EnvRestore {
            key: "NB_DIR",
            prev,
        };
        let _tmp = TmpRemove { path: base };

        let client = NbClient::new(&Config {
            notebook: Some("nb1".to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            disable_git_signing: true,
            ..Default::default()
        })
        .expect("client");

        // Park the reader inside the hold, then swap slot 1 while
        // it waits: resolution strictly follows the swap. A
        // pre-gate resolver (lookup before the hold) would resolve
        // first and read the stale victim file instead.
        let (entered_tx, mut entered_rx) =
            tokio::sync::mpsc::unbounded_channel();
        let proceed = Arc::new(Notify::new());
        let scope =
            crate::gate::git_common_dir_realpath(&root).expect("gate key");
        let probe =
            native_gate_probe::install(entered_tx, proceed.clone(), scope)
                .expect("probe installs once");
        assert!(
            native_gate_probe::installed(),
            "probe visible while installed"
        );
        let mut reader = tokio::spawn({
            let client = client.clone();
            async move { client.show_note("1", None).await }
        });
        // Bounded both ways: an early acquisition or discovery
        // failure must fail the test, never hang the suite. On
        // failure the reader is aborted and released.
        if tokio::time::timeout(
            native_gate_probe::WAIT_TIMEOUT,
            entered_rx.recv(),
        )
        .await
        .is_err()
        {
            reader.abort();
            panic!("reader never entered the hold");
        }
        std::fs::write(root.join(".index"), "other.md\nvictim.md\n")
            .expect("swap slot 1");
        proceed.notify_one();
        let shown = tokio::select! {
            joined = &mut reader => joined
                .expect("reader joins")
                .expect("queued read succeeds"),
            _ = tokio::time::sleep(native_gate_probe::WAIT_TIMEOUT) => {
                reader.abort();
                proceed.notify_one();
                panic!("queued reader never completed");
            }
        };
        assert_eq!(shown.path, "other.md".to_string(), "{shown:?}");
        assert_eq!(shown.body, "o\n", "{shown:?}");
        // Lifecycle: dropping the guard uninstalls, so later reads
        // never rendezvous with a stale sender.
        drop(probe);
        assert!(
            !native_gate_probe::installed(),
            "probe cleared after guard drop"
        );
    }

    struct TmpRemove {
        path: std::path::PathBuf,
    }

    impl Drop for TmpRemove {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
