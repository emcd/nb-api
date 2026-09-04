//! 0.4.0 note identity: nb-faithful mangling, titleless local time,
//! collision suffixes, stable `.index` ids, and numeric selectors.

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

#[tokio::test]
async fn mangled_title_filename_and_numeric_outcome() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let outcome = client
            .add_note(Some("Hello World Title"), "body\n", &[], Some("work"), None)
            .await
            .expect("add");
        let op = &outcome.ops[0];
        assert_eq!(op.path.as_deref(), Some("work/hello_world_title.md"));
        assert_eq!(
            op.selector.as_deref(),
            Some(format!("{}:work/1", env.notebook()).as_str())
        );
        assert_eq!(op.numeric_id, Some(1));
        let root = client.show_notebook_path(None).await.expect("path");
        assert_eq!(index_lines(&root, "work"), vec!["hello_world_title.md"]);
        // Reads echo the numeric selector.
        let shown = client
            .show_note("work/hello_world_title.md", None)
            .await
            .expect("show");
        assert_eq!(shown.selector, format!("{}:work/1", env.notebook()));
        assert_eq!(shown.numeric_id, Some(1));
    })
    .await;
}

#[tokio::test]
async fn non_ascii_title_preserved_verbatim() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let outcome = client
            .add_note(Some("Café Über"), "body\n", &[], None, None)
            .await
            .expect("add");
        assert_eq!(outcome.ops[0].path.as_deref(), Some("café_Über.md"));
    })
    .await;
}

#[tokio::test]
async fn collision_gains_numeric_suffix() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        for _ in 0..2 {
            client
                .add_note(Some("Hello World Title"), "body\n", &[], None, None)
                .await
                .expect("add");
        }
        let root = client.show_notebook_path(None).await.expect("path");
        assert!(root.join("hello_world_title.md").is_file());
        assert!(root.join("hello_world_title-1.md").is_file());
        assert_eq!(
            index_lines(&root, ""),
            vec!["hello_world_title.md", "hello_world_title-1.md"]
        );
    })
    .await;
}

#[tokio::test]
async fn titleless_create_uses_local_timestamp() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let outcome = client
            .add_note(None, "body\n", &[], None, None)
            .await
            .expect("add");
        let path = outcome.ops[0].path.clone().expect("path");
        let stem = path.strip_suffix(".md").expect("md suffix");
        assert_eq!(stem.len(), 14, "expected %Y%m%d%H%M%S stem, got {path:?}");
        assert!(stem.bytes().all(|b| b.is_ascii_digit()), "{path:?}");
        assert!(
            !path.contains('-'),
            "opaque epoch-seq scheme is gone: {path:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn delete_blanks_index_line_and_ids_stay_stable() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        for name in ["note_a", "note_b", "note_c"] {
            let mut tx = client.transaction(None).await.expect("tx");
            tx.add_note(&format!("{name}.md"), None, "x\n", &[])
                .unwrap();
            tx.commit().await.expect("commit");
        }
        // Delete id 2 via numeric selector.
        let mut tx = client.transaction(None).await.expect("tx");
        tx.delete_note(NoteTarget::selector("2")).unwrap();
        tx.commit().await.expect("delete");
        let root = client.show_notebook_path(None).await.expect("path");
        assert_eq!(
            index_lines(&root, ""),
            vec![
                "note_a.md".to_string(),
                String::new(),
                "note_c.md".to_string()
            ]
        );
        // Id 3 still resolves to note_c; id 2 is NotFound.
        let shown = client.show_note("3", None).await.expect("show 3");
        assert_eq!(shown.path, "note_c.md");
        let err = client.show_note("2", None).await.expect_err("show 2");
        assert!(matches!(err, nb_api::NbError::NotFound { .. }), "{err:?}");
        // Create after delete does not reuse the id.
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("note_d.md", None, "x\n", &[]).unwrap();
        let outcome = tx.commit().await.expect("commit");
        assert_eq!(outcome.ops[0].numeric_id, Some(4));
        assert_eq!(index_lines(&root, "").len(), 4);
    })
    .await;
}

#[tokio::test]
async fn move_within_folder_updates_in_place() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_folder("foo").unwrap();
        tx.commit().await.expect("folder");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("foo/foonote.md", None, "x\n", &[]).unwrap();
        tx.commit().await.expect("add");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.move_note(NoteTarget::path("foo/foonote.md"), "foo/renamed.md")
            .unwrap();
        let outcome = tx.commit().await.expect("move");
        assert_eq!(outcome.ops[0].path.as_deref(), Some("foo/renamed.md"));
        assert_eq!(outcome.ops[0].numeric_id, Some(1));
        let root = client.show_notebook_path(None).await.expect("path");
        assert_eq!(index_lines(&root, "foo"), vec!["renamed.md"]);
    })
    .await;
}

#[tokio::test]
async fn move_across_folders_blanks_source_and_appends_dest() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_folder("foo").unwrap();
        tx.add_folder("bar").unwrap();
        tx.commit().await.expect("folders");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("movea.md", None, "x\n", &[]).unwrap();
        tx.commit().await.expect("add");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.move_note(NoteTarget::path("movea.md"), "foo/").unwrap();
        let outcome = tx.commit().await.expect("move");
        assert_eq!(outcome.ops[0].numeric_id, Some(1));
        let root = client.show_notebook_path(None).await.expect("path");
        assert_eq!(index_lines(&root, ""), vec![String::new()]);
        assert_eq!(index_lines(&root, "foo"), vec!["movea.md"]);
    })
    .await;
}

#[tokio::test]
async fn concurrent_commits_get_distinct_ids() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let c2 = client.clone();
        let (r1, r2) = tokio::join!(
            async {
                let mut tx = client.transaction(None).await.expect("tx1");
                tx.add_note("con_a.md", None, "a\n", &[]).unwrap();
                tx.commit().await.expect("commit1")
            },
            async {
                let mut tx = c2.transaction(None).await.expect("tx2");
                tx.add_note("con_b.md", None, "b\n", &[]).unwrap();
                tx.commit().await.expect("commit2")
            }
        );
        let id1 = r1.ops[0].numeric_id.expect("id1");
        let id2 = r2.ops[0].numeric_id.expect("id2");
        assert_ne!(id1, id2, "concurrent appends must not share an id");
        let root = client.show_notebook_path(None).await.expect("path");
        let lines = index_lines(&root, "");
        assert!(lines.contains(&"con_a.md".to_string()), "{lines:?}");
        assert!(lines.contains(&"con_b.md".to_string()), "{lines:?}");
        // Each outcome reports its actual post-write position.
        for (name, id) in [("con_a.md", id1), ("con_b.md", id2)] {
            assert_eq!(lines[(id - 1) as usize], name, "{lines:?}");
        }
    })
    .await;
}
