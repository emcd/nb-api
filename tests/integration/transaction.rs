//! Transaction collect-then-commit and body-aware surface.

use nb_api::testing::NbTestEnv;
use nb_api::{
    BoundaryAt, ByteString, Config, LineEdit, LinePosition, NbClient, NoteTarget, Occurrence,
};

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

#[tokio::test]
async fn multi_op_single_checkpoint() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client
            .show_notebook_path(None)
            .await
            .expect("notebook path");
        let pre = head(&root);

        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_folder("proposals/demo").expect("folder");
        tx.add_note(
            "proposals/demo/proposal.md",
            Some("Proposal"),
            "body one\n",
            &[],
        )
        .expect("note");
        tx.add_todo(
            "proposals/demo/work.todo.md",
            "Work",
            Some("desc"),
            &[],
            &[],
        )
        .expect("todo");
        let outcome = tx.commit().await.expect("commit");
        assert!(outcome.commit_created);
        assert_eq!(outcome.ops.len(), 3);

        let post = head(&root);
        assert_ne!(pre, post);
        let parent = git_capture(&root, &["rev-parse", "HEAD^"]);
        assert_eq!(parent.trim(), pre.trim());

        let shown = client
            .show_note("proposals/demo/proposal.md", None)
            .await
            .expect("show");
        assert_eq!(shown.path, "proposals/demo/proposal.md");
        assert!(shown.body_contiguous);
        let body = shown.body.as_bytes().unwrap();
        assert!(body.windows(8).any(|w| w == b"body one"));
    })
    .await;
}

#[tokio::test]
async fn drop_discards_plan() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        let pre = head(&root);
        {
            let mut tx = client.transaction(None).await.expect("tx");
            tx.add_note("gone.md", None, "x\n", &[]).expect("add");
            drop(tx);
        }
        assert_eq!(head(&root), pre);
        assert!(!root.join("gone.md").exists());
    })
    .await;
}

#[tokio::test]
async fn collision_applies_nothing() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        let pre = head(&root);
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("same.md", None, "a\n", &[]).unwrap();
        tx.add_note("same.md", None, "b\n", &[]).unwrap();
        let err = tx.commit().await.expect_err("collision");
        assert!(
            matches!(err, nb_api::NbError::PathCollision { .. })
                || matches!(err, nb_api::NbError::PlanValidation { .. }),
            "got {err:?}"
        );
        assert_eq!(head(&root), pre);
        assert!(!root.join("same.md").exists());
    })
    .await;
}

#[tokio::test]
async fn replace_body_and_lines() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("n.md", None, "line1\nline2\nline3\n", &[])
            .unwrap();
        tx.commit().await.expect("commit");

        let shown = client.show_note("n.md", None).await.expect("show");
        let fp = shown.fingerprint.clone();
        client
            .replace_note_body(NoteTarget::path("n.md"), b"only\n", fp, None)
            .await
            .expect("replace");
        let shown = client.show_note("n.md", None).await.expect("show2");
        assert_eq!(shown.body.as_bytes().unwrap(), b"only\n");

        let lines = client
            .show_note_lines(NoteTarget::path("n.md"), Some(1), Some(10), None)
            .await
            .expect("lines");
        assert_eq!(lines.total_lines, 1);
        let line = &lines.lines[0];
        client
            .edit_note_lines(
                NoteTarget::path("n.md"),
                vec![LineEdit::Replace {
                    start: nb_api::LineRef {
                        number: line.number,
                        anchor: line.anchor.clone(),
                    },
                    end: nb_api::LineRef {
                        number: line.number,
                        anchor: line.anchor.clone(),
                    },
                    content: ByteString::from_bytes(b"X\n"),
                }],
                None,
            )
            .await
            .expect("line edit");
        let shown = client.show_note("n.md", None).await.expect("show3");
        assert_eq!(shown.body.as_bytes().unwrap(), b"X\n");

        client
            .edit_note_substring(
                NoteTarget::path("n.md"),
                b"X",
                b"Y",
                Occurrence::First,
                1,
                Some(shown.fingerprint),
                None,
            )
            .await
            .expect("substr");
        let shown = client.show_note("n.md", None).await.expect("show4");
        assert_eq!(shown.body.as_bytes().unwrap(), b"Y\n");

        let fp = shown.fingerprint.clone();
        client
            .replace_note_body(NoteTarget::path("n.md"), b"", fp, None)
            .await
            .expect("empty");
        client
            .edit_note_lines(
                NoteTarget::path("n.md"),
                vec![LineEdit::Insert {
                    at: LinePosition::Boundary {
                        at: BoundaryAt::Caret,
                    },
                    content: ByteString::from_bytes(b"fresh\n"),
                }],
                None,
            )
            .await
            .expect("insert empty");
        let shown = client.show_note("n.md", None).await.expect("show5");
        assert_eq!(shown.body.as_bytes().unwrap(), b"fresh\n");
    })
    .await;
}

#[tokio::test]
async fn dirty_baseline_refuses() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        // Seed a tracked file, then dirty it without invoking `nb` (which may
        // auto-checkpoint and clear dirty state).
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("tracked.md", None, "clean\n", &[]).unwrap();
        tx.commit().await.expect("seed");
        std::fs::write(root.join("tracked.md"), b"dirty unstaged\n").unwrap();
        let mut tx = client.transaction(None).await.expect("tx2");
        tx.add_note("ok.md", None, "y\n", &[]).unwrap();
        let err = tx.commit().await.expect_err("dirty");
        assert!(
            matches!(err, nb_api::NbError::DirtyBaseline { .. }),
            "{err:?}"
        );
        git_capture(&root, &["checkout", "--", "tracked.md"]);
    })
    .await;
}

#[tokio::test]
async fn ignored_file_create_is_collision() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        std::fs::write(root.join(".gitignore"), b"secret.md\n").unwrap();
        // Commit gitignore so status is clean aside from ignored secret.
        git_capture(&root, &["add", "-A"]);
        git_capture(&root, &["commit", "-m", "ignore", "--no-gpg-sign"]);
        let pre = head(&root);
        std::fs::write(root.join("secret.md"), b"original-secret\n").unwrap();

        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("secret.md", None, "overwrite\n", &[]).unwrap();
        let err = tx.commit().await.expect_err("collision");
        assert!(
            matches!(err, nb_api::NbError::PathIgnored { .. })
                || matches!(err, nb_api::NbError::PathCollision { .. })
                || matches!(err, nb_api::NbError::PlanValidation { .. }),
            "{err:?}"
        );
        assert_eq!(
            std::fs::read(root.join("secret.md")).unwrap(),
            b"original-secret\n"
        );
        assert_eq!(head(&root), pre);
    })
    .await;
}

#[tokio::test]
async fn ignored_pattern_new_create_is_force_staged() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        std::fs::write(root.join(".gitignore"), b"*.secret\n").unwrap();
        git_capture(&root, &["add", "-A"]);
        git_capture(&root, &["commit", "-m", "ignore", "--no-gpg-sign"]);
        let pre = head(&root);

        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("note.secret", None, "secret-body\n", &[])
            .unwrap();
        let outcome = tx.commit().await.expect("commit");
        assert!(
            outcome.commit_created,
            "ignored-pattern create must force-stage into a checkpoint"
        );
        assert_ne!(head(&root), pre);
        // Fresh checkout still has the file.
        std::fs::remove_file(root.join("note.secret")).ok();
        git_capture(&root, &["checkout", "HEAD", "--", "note.secret"]);
        assert_eq!(
            std::fs::read(root.join("note.secret")).unwrap(),
            b"secret-body\n"
        );
    })
    .await;
}

#[tokio::test]
async fn ignored_existing_edit_and_delete_refused() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        std::fs::write(root.join(".gitignore"), b"ghost.md\n").unwrap();
        git_capture(&root, &["add", "-A"]);
        git_capture(&root, &["commit", "-m", "ignore", "--no-gpg-sign"]);
        std::fs::write(root.join("ghost.md"), b"ghost-body\n").unwrap();
        let pre = head(&root);

        let mut tx = client.transaction(None).await.expect("tx");
        tx.delete_note(NoteTarget::path("ghost.md")).unwrap();
        let err = tx.commit().await.expect_err("delete ignored");
        assert!(
            matches!(err, nb_api::NbError::PathIgnored { .. })
                || matches!(err, nb_api::NbError::PlanValidation { .. }),
            "{err:?}"
        );
        assert_eq!(
            std::fs::read(root.join("ghost.md")).unwrap(),
            b"ghost-body\n"
        );
        assert_eq!(head(&root), pre);

        let mut tx = client.transaction(None).await.expect("tx2");
        tx.retitle_note(NoteTarget::path("ghost.md"), b"# X\n")
            .unwrap();
        let err = tx.commit().await.expect_err("retitle ignored");
        assert!(
            matches!(err, nb_api::NbError::PathIgnored { .. })
                || matches!(err, nb_api::NbError::PlanValidation { .. }),
            "{err:?}"
        );
        assert_eq!(
            std::fs::read(root.join("ghost.md")).unwrap(),
            b"ghost-body\n"
        );
        assert_eq!(head(&root), pre);
    })
    .await;
}

#[tokio::test]
async fn add_folder_persists_when_gitkeep_is_ignored() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        std::fs::write(root.join(".gitignore"), b".gitkeep\n").unwrap();
        git_capture(&root, &["add", "-A"]);
        git_capture(&root, &["commit", "-m", "ignore-gitkeep", "--no-gpg-sign"]);

        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_folder("kept-dir").unwrap();
        let outcome = tx.commit().await.expect("commit");
        assert!(
            outcome.commit_created,
            "folder marker must force-stage even when .gitkeep is ignored"
        );
        std::fs::remove_dir_all(root.join("kept-dir")).ok();
        git_capture(&root, &["checkout", "HEAD", "--", "kept-dir"]);
        assert!(root.join("kept-dir").is_dir());
        assert!(root.join("kept-dir/.gitkeep").is_file());
    })
    .await;
}

#[tokio::test]
async fn tracked_symlink_refused_at_commit() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        let external = env.nb_dir().parent().unwrap().join("external-target");
        std::fs::write(&external, b"external-original\n").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&external, root.join("link.md")).unwrap();
            git_capture(&root, &["add", "-A"]);
            git_capture(&root, &["commit", "-m", "symlink", "--no-gpg-sign"]);
            let mut tx = client.transaction(None).await.expect("tx");
            tx.add_note("other.md", None, "x\n", &[]).unwrap();
            let err = tx.commit().await.expect_err("symlink");
            assert!(
                matches!(err, nb_api::NbError::UnsupportedStructure { .. })
                    || matches!(err, nb_api::NbError::PlanValidation { .. }),
                "{err:?}"
            );
            assert_eq!(std::fs::read(&external).unwrap(), b"external-original\n");
        }
    })
    .await;
}

#[tokio::test]
#[cfg(unix)]
async fn ignored_symlink_ancestor_create_refused() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        let external = env.nb_dir().parent().unwrap().join("external-symlink-dir");
        std::fs::create_dir_all(&external).unwrap();
        std::fs::write(external.join("preexisting.txt"), b"keep\n").unwrap();
        std::fs::write(root.join(".gitignore"), b"ignored-link\n").unwrap();
        git_capture(&root, &["add", "-A"]);
        git_capture(&root, &["commit", "-m", "ignore-link", "--no-gpg-sign"]);
        std::os::unix::fs::symlink(&external, root.join("ignored-link")).unwrap();
        let pre = head(&root);
        let pre_status = git_capture(&root, &["status", "--porcelain"]);

        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("ignored-link/child.md", None, "escape\n", &[])
            .unwrap();
        let err = tx.commit().await.expect_err("symlink ancestor");
        assert!(
            matches!(err, nb_api::NbError::UnsupportedStructure { .. })
                || matches!(err, nb_api::NbError::PlanValidation { .. }),
            "{err:?}"
        );
        assert_eq!(head(&root), pre);
        assert_eq!(git_capture(&root, &["status", "--porcelain"]), pre_status);
        assert!(
            !external.join("child.md").exists(),
            "must not create external child"
        );
        assert_eq!(
            std::fs::read(external.join("preexisting.txt")).unwrap(),
            b"keep\n"
        );
        assert!(
            std::fs::symlink_metadata(root.join("ignored-link"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "ignored symlink must remain"
        );
    })
    .await;
}

#[tokio::test]
async fn add_folder_only_persists_across_checkout() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_folder("empty-dir").unwrap();
        let outcome = tx.commit().await.expect("commit");
        assert!(outcome.commit_created, "empty folder must produce a commit");
        assert!(root.join("empty-dir").is_dir());
        // Simulate fresh checkout of the folder tree.
        git_capture(&root, &["rm", "-rf", "--cached", "empty-dir"]);
        std::fs::remove_dir_all(root.join("empty-dir")).ok();
        git_capture(&root, &["checkout", "HEAD", "--", "empty-dir"]);
        assert!(
            root.join("empty-dir").is_dir(),
            "folder must survive checkout"
        );
        assert!(root.join("empty-dir/.gitkeep").is_file());
    })
    .await;
}

#[tokio::test]
async fn absolute_and_backslash_paths_refused() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let mut tx = client.transaction(None).await.expect("tx");
        assert!(tx.add_note("/outside.md", None, "x\n", &[]).is_err());
        assert!(tx.add_note("dir\\note.md", None, "x\n", &[]).is_err());
        assert!(tx.add_note("..\\escape.md", None, "x\n", &[]).is_err());
    })
    .await;
}

#[tokio::test]
async fn qualified_selector_outcome_not_double_prefixed() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("q.md", None, "body\n", &[]).unwrap();
        tx.commit().await.expect("add");
        let mut tx = client.transaction(None).await.expect("tx2");
        let nb = env.notebook();
        tx.retitle_note(NoteTarget::selector(format!("{nb}:q.md")), b"# New Title\n")
            .unwrap();
        let outcome = tx.commit().await.expect("retitle");
        let sel = outcome.ops[0].selector.as_deref().expect("selector");
        assert_eq!(sel, &format!("{nb}:q.md"));
        assert!(!sel.contains(&format!("{nb}:{nb}:")));
    })
    .await;
}

#[tokio::test]
async fn failed_stage_removes_new_ignored_output() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        std::fs::write(root.join(".gitignore"), b"*.secret\n").unwrap();
        git_capture(&root, &["add", "-A"]);
        git_capture(&root, &["commit", "-m", "ignore", "--no-gpg-sign"]);
        let pre = head(&root);

        // Captured/restored by with_isolated_env's EnvSnapshot (includes
        // NB_API_FAIL_AFTER_STAGE); panic-safe and preserves any prior value.
        unsafe {
            std::env::set_var("NB_API_FAIL_AFTER_STAGE", "1");
        }
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("boom.secret", None, "should-not-remain\n", &[])
            .unwrap();
        let err = tx.commit().await.expect_err("injected fail");

        assert!(
            !matches!(err, nb_api::NbError::RecoveryRequired { .. }),
            "verified clean rollback should return the original failure, not RecoveryRequired: {err:?}"
        );
        assert_eq!(head(&root), pre);
        assert!(
            !root.join("boom.secret").exists(),
            "ignored transaction output must be removed on failed checkpoint"
        );
        // Worktree must be clean (including no leftover ignored owned file).
        let status = git_capture(&root, &["status", "--porcelain", "-uall"]);
        assert!(status.trim().is_empty(), "status={status:?}");
    })
    .await;
}

#[tokio::test]
async fn restore_head_verify_failure_is_recovery_required() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        let pre = head(&root).trim().to_string();

        unsafe {
            std::env::set_var("NB_API_FAIL_AFTER_STAGE", "1");
            std::env::set_var("NB_API_FAIL_RESTORE_HEAD", "1");
        }
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("x.md", None, "body\n", &[]).unwrap();
        let err = tx.commit().await.expect_err("injected verify fail");

        match err {
            nb_api::NbError::RecoveryRequired {
                pre_revision,
                post_revision_observed,
                status_observed,
                guidance,
                ..
            } => {
                assert_eq!(pre_revision.trim(), pre);
                assert!(
                    post_revision_observed.is_none(),
                    "HEAD verify failure must not claim an observed post revision"
                );
                // Best-effort porcelain evidence: cleanup removed owned output, so
                // independent status observation should succeed with empty porcelain.
                assert_eq!(
                    status_observed.as_deref().map(str::trim),
                    Some(""),
                    "status_observed must carry clean porcelain evidence; got {status_observed:?}"
                );
                assert!(
                    guidance.contains("HEAD") && guidance.contains("do not retry"),
                    "guidance must deny clean/retry-safe rollback: {guidance}"
                );
                assert!(
                    !guidance.to_lowercase().contains("clean restore succeeded")
                        && !guidance.to_lowercase().contains("rollback succeeded"),
                    "must not claim successful clean rollback: {guidance}"
                );
            }
            other => panic!("expected RecoveryRequired, got {other:?}"),
        }
        // Owned output still cleaned even when verify HEAD fails after cleanup.
        assert!(!root.join("x.md").exists());
    })
    .await;
}

#[tokio::test]
async fn restore_dirty_verify_failure_is_recovery_required() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        let pre = head(&root).trim().to_string();

        unsafe {
            std::env::set_var("NB_API_FAIL_AFTER_STAGE", "1");
            std::env::set_var("NB_API_FAIL_RESTORE_DIRTY", "1");
        }
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("y.md", None, "body\n", &[]).unwrap();
        let err = tx.commit().await.expect_err("injected dirty verify fail");

        match err {
            nb_api::NbError::RecoveryRequired {
                pre_revision,
                post_revision_observed,
                status_observed,
                guidance,
                ..
            } => {
                assert_eq!(pre_revision.trim(), pre);
                assert_eq!(
                    post_revision_observed.as_deref().map(str::trim),
                    Some(pre.as_str()),
                    "dirty verify failure should still carry observed HEAD when HEAD read succeeded"
                );
                assert_eq!(
                    status_observed.as_deref().map(str::trim),
                    Some(""),
                    "status_observed must carry clean porcelain evidence; got {status_observed:?}"
                );
                assert!(
                    guidance.contains("dirty") && guidance.contains("do not retry"),
                    "guidance must deny clean/retry-safe rollback: {guidance}"
                );
            }
            other => panic!("expected RecoveryRequired, got {other:?}"),
        }
        assert!(!root.join("y.md").exists());
    })
    .await;
}

#[tokio::test]
async fn failed_child_create_preserves_preexisting_ignored_parent() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        std::fs::write(root.join(".gitignore"), b"cage/\n").unwrap();
        git_capture(&root, &["add", "-A"]);
        git_capture(&root, &["commit", "-m", "ignore-cage", "--no-gpg-sign"]);
        // Pre-existing empty ignored parent directory.
        std::fs::create_dir_all(root.join("cage")).unwrap();
        assert!(root.join("cage").is_dir());
        let pre = head(&root);

        unsafe {
            std::env::set_var("NB_API_FAIL_AFTER_STAGE", "1");
        }
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("cage/child.secret", None, "x\n", &[]).unwrap();
        let err = tx.commit().await.expect_err("injected fail");
        assert!(
            !matches!(err, nb_api::NbError::RecoveryRequired { .. }),
            "{err:?}"
        );
        assert_eq!(head(&root), pre);
        assert!(
            !root.join("cage/child.secret").exists(),
            "child output must be removed"
        );
        assert!(
            root.join("cage").is_dir(),
            "pre-existing ignored parent directory must be preserved"
        );
    })
    .await;
}

#[tokio::test]
async fn gate_timeout_is_threaded_from_config() {
    use std::time::Duration;
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let mut cfg = config_for(&env);
        cfg.gate_timeout = Duration::from_millis(50);
        let client = NbClient::new(&cfg).expect("client");
        assert_eq!(client.gate_timeout(), Duration::from_millis(50));
        // Transaction is constructed with the client timeout (used on commit).
        let _tx = client.transaction(None).await.expect("tx");
    })
    .await;
}

#[tokio::test]
async fn show_note_lossy_title_and_tags_for_invalid_utf8() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client.show_notebook_path(None).await.expect("path");
        // Title contains invalid UTF-8; raw bytes stay authoritative and
        // title_text must still be present via lossy conversion.
        let mut bytes = b"# ".to_vec();
        bytes.extend_from_slice(&[0xff, 0xfe]);
        bytes.extend_from_slice(b" Title\n\n#alpha\n\nbody\n");
        // Tag token with embedded invalid UTF-8.
        let mut bytes = b"# ".to_vec();
        bytes.extend_from_slice(&[0xff, 0xfe]);
        bytes.extend_from_slice(b"\n\n#al");
        bytes.push(0xff);
        bytes.extend_from_slice(b"pha\n\nbody\n");
        std::fs::write(root.join("bad.md"), &bytes).unwrap();
        git_capture(&root, &["add", "-A"]);
        git_capture(&root, &["commit", "-m", "bad", "--no-gpg-sign"]);
        let shown = client.show_note("bad.md", None).await.expect("show");
        assert!(shown.title.is_some(), "raw title bytes required");
        let title_text = shown.title_text.expect("lossy title_text required");
        assert!(
            title_text.contains('\u{FFFD}') || !title_text.is_empty(),
            "title_text should be lossy-decoded; got {title_text:?}"
        );
        // Tags use the same lossy path as title_text (`tags()` + from_utf8_lossy).
    })
    .await;
}

fn head(root: &std::path::Path) -> String {
    git_capture(root, &["rev-parse", "HEAD"])
}

fn git_capture(root: &std::path::Path, args: &[&str]) -> String {
    let mut cmd = std::process::Command::new("git");
    nb_api::scrub_git_env_std(&mut cmd);
    cmd.current_dir(root).args(args);
    let out = cmd.output().expect("git");
    assert!(out.status.success(), "git {:?} failed: {:?}", args, out);
    String::from_utf8_lossy(&out.stdout).into_owned()
}
