//! Cross-process coexistence regressions for the 0.4.0 REVISE round.
//!
//! An external `nb add` racing our commit must lose neither its note file
//! (stale-snapshot materialization) nor its `.index` line (whole-file
//! rewrite). The external writer is simulated deterministically via
//! `NB_API_SIMULATE_EXTERNAL_WRITER`, which creates AND commits an external
//! note after our snapshot but before materialize.

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient, NoteTarget};

use crate::common::with_isolated_env;

fn config_for(env: &NbTestEnv) -> Config {
    Config {
        notebook: Some(env.notebook().to_string()),
        create_notebook: false,
        allow_top_level_notes: true,
        disable_git_signing: true,
        ..Config::default()
    }
}

fn index_lines(root: &std::path::Path, folder: &str) -> Vec<String> {
    let rel = if folder.is_empty() {
        ".index".to_string()
    } else {
        format!("{folder}/.index")
    };
    let bytes = std::fs::read(root.join(&rel)).expect("read .index");
    let text = String::from_utf8(bytes).expect("index utf8");
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// Unix-only: nb 7.24.0's background auto-checkpoint (fired during fixture
/// init) intermittently races commit dirty-baseline checks under the Git
/// Bash `.cmd` launcher on Windows (`nb-api:todos/api/9`); this commit-dense
/// test loses that race disproportionately often.
#[cfg(unix)]
#[tokio::test]
async fn external_note_and_index_line_survive_our_commit() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        unsafe {
            std::env::set_var("NB_API_SIMULATE_EXTERNAL_WRITER", "1");
        }
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("ours.md", None, "our body\n", &[]).unwrap();
        let outcome = tx.commit().await.expect("commit");
        unsafe {
            std::env::remove_var("NB_API_SIMULATE_EXTERNAL_WRITER");
        }

        let root = client.show_notebook_path(None).await.expect("path");
        let external = format!("external-{}.md", std::process::id());
        // External file preserved byte-for-byte (never deleted, never rewritten).
        assert_eq!(
            std::fs::read(root.join(&external)).expect("external file"),
            b"# External\n\nexternal body\n"
        );
        // Both index lines present: external (pre-existing at our read) and ours.
        let lines = index_lines(&root, "");
        assert!(lines.contains(&external), "{lines:?}");
        assert!(lines.contains(&"ours.md".to_string()), "{lines:?}");
        // Our outcome still reports its actual post-write position.
        let id = outcome.ops[0].numeric_id.expect("numeric id");
        assert_eq!(lines[(id - 1) as usize], "ours.md", "{lines:?}");
        // External note reads back through the public surface.
        let shown = client.show_note(&external, None).await.expect("show");
        assert!(shown.body.contains("external body"), "{:?}", shown.body);
    })
    .await;
}

#[tokio::test]
async fn delete_rewrite_preserves_preexisting_external_index_line() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        // Seed two notes, then splice in an external line as an external
        // `nb add` would (appended after our last read).
        for name in ["keep_a.md", "drop_b.md"] {
            let mut tx = client.transaction(None).await.expect("tx");
            tx.add_note(name, None, "x\n", &[]).unwrap();
            tx.commit().await.expect("commit");
        }
        let external = "external-late.md";
        std::fs::write(root.join(external), b"# E\n\n late\n").unwrap();
        {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(root.join(".index"))
                .unwrap();
            writeln!(f, "{external}").unwrap();
        }
        // An external `nb add` commits its files; do the same so the
        // baseline is clean and only the rewrite path is under test.
        git_add_commit(&root, "external add");
        // Delete goes through the whole-file rewrite path (blank, not append).
        let mut tx = client.transaction(None).await.expect("tx");
        tx.delete_note(NoteTarget::path("drop_b.md")).unwrap();
        tx.commit().await.expect("delete");
        // External line survived the rewrite; deleted id stayed blank.
        let lines = index_lines(&root, "");
        assert!(lines.contains(&external.to_string()), "{lines:?}");
        assert_eq!(lines[1], "", "{lines:?}");
        assert!(root.join(external).is_file());
    })
    .await;
}

#[tokio::test]
async fn splice_window_append_is_preserved() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        // Strict tail-window assertions apply where the amended spec's
        // no-loss guarantee holds (atomic range-collapse available);
        // elsewhere the spec covers only the narrowed truncate boundary,
        // so only the pre-splice batch is asserted there.
        let strict = supports_collapse(&root);
        for name in ["victim.md", "other.md"] {
            let mut tx = client.transaction(None).await.expect("tx");
            tx.add_note(name, None, "x\n", &[]).unwrap();
            tx.commit().await.expect("seed");
        }
        // Hold BOTH splice windows open (read-to-prefix and tail-splice);
        // the appender (no lock, like a real external `nb add`) lands one
        // batch inside each.
        unsafe {
            std::env::set_var("NB_API_INDEX_SPLICE_PAUSE_MS", "300");
        }
        let appender_root = root.clone();
        let appender = tokio::spawn(async move {
            use std::io::Write as _;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(appender_root.join(".index"))
                .expect("open .index");
            for name in ["ext-w1.md", "ext-w2.md", "ext-w3.md"] {
                writeln!(f, "{name}").expect("append");
            }
            drop(f);
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(appender_root.join(".index"))
                .expect("open .index");
            for name in ["ext-w4.md", "ext-w5.md", "ext-w6.md"] {
                writeln!(f, "{name}").expect("append");
            }
        });
        let mut tx = client.transaction(None).await.expect("tx");
        tx.delete_note(NoteTarget::path("victim.md")).unwrap();
        tx.commit().await.expect("delete");
        appender.await.expect("appender");
        unsafe {
            std::env::remove_var("NB_API_INDEX_SPLICE_PAUSE_MS");
        }

        // Nothing appended mid-splice was lost; the delete still blanked;
        // surviving ids stayed stable.
        let lines = index_lines(&root, "");
        assert_eq!(lines[0], "", "{lines:?}");
        assert_eq!(lines[1], "other.md", "{lines:?}");
        let mut tail = lines[2..].to_vec();
        tail.sort();
        if strict {
            assert_eq!(
                tail,
                vec![
                    "ext-w1.md",
                    "ext-w2.md",
                    "ext-w3.md",
                    "ext-w4.md",
                    "ext-w5.md",
                    "ext-w6.md"
                ],
                "{lines:?}"
            );
        } else {
            // Non-collapse platforms: the amended spec's guarantee covers
            // the narrowed truncate boundary (retry + refusal), so only
            // the pre-splice batch is asserted there.
            for name in ["ext-w1.md", "ext-w2.md", "ext-w3.md"] {
                assert!(tail.contains(&name.to_string()), "{lines:?}");
            }
        }
        assert!(!root.join("victim.md").exists());
        assert!(root.join("other.md").is_file());
    })
    .await;
}

/// Whether this kernel/filesystem can atomically collapse a byte range
/// (the lossless stale-range removal used by the index splice).
#[cfg(target_os = "linux")]
fn supports_collapse(dir: &std::path::Path) -> bool {
    let probe = dir.join(".collapse-probe");
    if std::fs::write(&probe, b"ab\n").is_err() {
        return false;
    }
    let ok = std::fs::File::options()
        .read(true)
        .write(true)
        .open(&probe)
        .map(|f| {
            use std::os::fd::AsRawFd;
            unsafe { libc::fallocate(f.as_raw_fd(), 0x08, 0, 1) == 0 }
        })
        .unwrap_or(false);
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Non-collapse platforms use the truncate fallback: strict tail-window
/// assertions do not apply there.
#[cfg(not(target_os = "linux"))]
fn supports_collapse(dir: &std::path::Path) -> bool {
    let _ = dir;
    false
}

#[tokio::test]
async fn growing_in_folder_rename_is_refused() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_folder("foo").unwrap();
        tx.commit().await.expect("folder");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("foo/a.md", None, "x\n", &[]).unwrap();
        tx.commit().await.expect("seed");

        // Same-folder rename to a LONGER name cannot splice losslessly:
        // refuse at plan validation (delete + recreate instead).
        let mut tx = client.transaction(None).await.expect("tx");
        tx.move_note(NoteTarget::path("foo/a.md"), "foo/a_much_longer_name.md")
            .unwrap();
        let err = tx.commit().await.expect_err("growing rename");
        assert!(
            matches!(err, nb_api::NbError::PlanValidation { .. })
                || matches!(err, nb_api::NbError::ValidationError { .. }),
            "{err:?}"
        );
        let root = client.show_notebook_path(None).await.expect("path");
        assert!(root.join("foo/a.md").is_file());
        assert!(!root.join("foo/a_much_longer_name.md").exists());
        assert_eq!(index_lines(&root, "foo"), vec!["a.md"]);

        // Same-folder rename to a SHORTER name still splices in place.
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("foo/longname.md", None, "x\n", &[]).unwrap();
        tx.commit().await.expect("seed2");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.move_note(NoteTarget::path("foo/longname.md"), "foo/s.md")
            .unwrap();
        let outcome = tx.commit().await.expect("shorter rename");
        assert_eq!(outcome.ops[0].numeric_id, Some(2));
        assert_eq!(index_lines(&root, "foo"), vec!["a.md", "s.md"]);
    })
    .await;
}

#[tokio::test]
async fn same_path_external_create_refuses_collision() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        let pre = git_head(&root);
        // External writer takes the exact path our plan is about to create.
        unsafe {
            std::env::set_var("NB_API_SIMULATE_EXTERNAL_WRITER", "1");
            std::env::set_var("NB_API_SIMULATE_EXTERNAL_SAME_PATH", "race.md");
        }
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("race.md", None, "our body\n", &[]).unwrap();
        let err = tx.commit().await.expect_err("same-path race");
        unsafe {
            std::env::remove_var("NB_API_SIMULATE_EXTERNAL_WRITER");
            std::env::remove_var("NB_API_SIMULATE_EXTERNAL_SAME_PATH");
        }
        // Refuse loudly: HEAD moved under us (external commit), so the
        // failure surfaces as IndeterminateCommit — never a silent
        // overwrite, never a checkpoint, never duplicate index entries.
        // Our bytes never landed; the external note is byte-intact.
        match err {
            nb_api::NbError::IndeterminateCommit {
                pre_revision,
                post_revision_observed,
                ..
            } => {
                assert_eq!(pre_revision, pre);
                assert!(
                    post_revision_observed.is_some_and(|h| h != pre),
                    "HEAD must show the external commit"
                );
            }
            other => panic!("expected IndeterminateCommit, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(root.join("race.md")).expect("external file"),
            b"# External\n\nexternal body\n"
        );
        assert_eq!(index_lines(&root, ""), vec!["race.md".to_string()]);
        let shown = client.show_note("race.md", None).await.expect("show");
        assert!(shown.body.contains("external body"), "{:?}", shown.body);
    })
    .await;
}

#[tokio::test]
async fn stale_foreign_lock_is_reaped_but_live_lock_is_not() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        let lock = root.join(".nb-api-index.lock");

        // Ancient foreign lock: reaped, commit proceeds, lock file gone.
        std::fs::write(&lock, "pid=1 nonce=1 ns=1").unwrap();
        let f = std::fs::File::options().write(true).open(&lock).unwrap();
        f.set_modified(std::time::SystemTime::UNIX_EPOCH).unwrap();
        drop(f);
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("reaped.md", None, "x\n", &[]).unwrap();
        tx.commit().await.expect("commit with stale lock");
        assert!(!lock.exists(), "reaped lock must be gone");
        assert!(root.join("reaped.md").is_file());

        // Fresh foreign lock: NOT reaped (left for its live owner); the
        // untracked lock file correctly fails the dirty baseline instead.
        std::fs::write(&lock, "pid=1 nonce=2 ns=2").unwrap();
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("blocked.md", None, "x\n", &[]).unwrap();
        let err = tx.commit().await.expect_err("live lock");
        assert!(
            matches!(err, nb_api::NbError::DirtyBaseline { .. }),
            "{err:?}"
        );
        assert!(lock.exists(), "live lock must not be reaped");
        assert_eq!(
            std::fs::read_to_string(&lock).unwrap(),
            "pid=1 nonce=2 ns=2"
        );
        std::fs::remove_file(&lock).unwrap();
    })
    .await;
}

fn git_head(root: &std::path::Path) -> String {
    git_capture(root, &["rev-parse", "HEAD"]).trim().to_string()
}

fn git_add_commit(root: &std::path::Path, message: &str) {
    git_capture(root, &["add", "-A"]);
    git_capture(root, &["commit", "-m", message, "--no-gpg-sign"]);
}

fn git_capture(root: &std::path::Path, args: &[&str]) -> String {
    let mut cmd = std::process::Command::new("git");
    nb_api::scrub_git_env_std(&mut cmd);
    cmd.current_dir(root).args(args);
    let out = cmd.output().expect("git");
    assert!(out.status.success(), "git {args:?} failed: {out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}
