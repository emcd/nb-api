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
