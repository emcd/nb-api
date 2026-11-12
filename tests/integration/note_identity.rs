//! 0.4.0 note identity: nb-faithful mangling, titleless local time,
//! collision suffixes, stable `.index` ids, and numeric selectors.

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient, NbError, NoteTarget};

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
    let mut lines: Vec<String> =
        text.split('\n').map(str::to_string).collect();
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
            .add_note(
                Some("Hello World Title"),
                "body\n",
                &[],
                Some("work"),
                None,
            )
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

/// Unix-only: numeric-id reads (`show 3` / `show 2`) go through
/// notebook-qualified selectors (`nb show <nb>:<id> --path`), which nb
/// 7.24.0 cannot run under Git Bash on Windows (path-in-regex bug,
/// `nb-api:todos/api/9`).
#[cfg(unix)]
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
        // Folder lines occupy ids alongside notes (nb parity): the moved
        // note is blanked in place, folders keep their lines.
        assert_eq!(
            index_lines(&root, ""),
            vec!["foo".to_string(), "bar".to_string(), String::new()]
        );
        assert_eq!(index_lines(&root, "foo"), vec!["movea.md"]);
    })
    .await;
}

#[tokio::test]
async fn delete_folder_empty_without_recursive() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("top.md", None, "x\n", &[]).unwrap();
        tx.add_folder("foo").unwrap();
        tx.commit().await.expect("seed");
        let outcome = client
            .delete_folder("foo", false, None)
            .await
            .expect("delete empty folder");
        assert_eq!(outcome.ops[0].path.as_deref(), Some("foo"));
        let root = client.show_notebook_path(None).await.expect("path");
        assert!(!root.join("foo").exists(), "folder dir removed");
        assert_eq!(
            index_lines(&root, ""),
            vec!["top.md".to_string(), String::new()]
        );
    })
    .await;
}

/// Non-empty folder without recursive refuses, listing contents.
#[tokio::test]
async fn delete_folder_non_empty_without_recursive_refuses() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("foo/a.md", None, "x\n", &[]).unwrap();
        tx.add_note("foo/sub/b.md", None, "y\n", &[]).unwrap();
        tx.commit().await.expect("seed");
        let err = client
            .delete_folder("foo", false, None)
            .await
            .expect_err("must refuse");
        let msg = format!("{err:?}");
        assert!(msg.contains("recursive"), "{msg:?}");
        assert!(msg.contains("a.md"), "{msg:?}");
        assert!(msg.contains("sub"), "{msg:?}");
        // Refusal leaves everything in place.
        let root = client.show_notebook_path(None).await.expect("path");
        assert!(root.join("foo/a.md").is_file());
    })
    .await;
}

/// Recursive delete removes the subtree and blanks the parent line.
#[tokio::test]
async fn delete_folder_recursive_removes_subtree() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("top.md", None, "x\n", &[]).unwrap();
        tx.add_note("foo/a.md", None, "x\n", &[]).unwrap();
        tx.add_note("foo/sub/b.md", None, "y\n", &[]).unwrap();
        tx.commit().await.expect("seed");
        let outcome = client
            .delete_folder("foo", true, None)
            .await
            .expect("recursive delete");
        assert_eq!(outcome.ops[0].path.as_deref(), Some("foo"));
        let root = client.show_notebook_path(None).await.expect("path");
        assert!(!root.join("foo").exists(), "subtree removed");
        assert!(root.join("top.md").is_file(), "sibling kept");
        // "foo" was created implicitly via note paths (not add_folder),
        // so it never occupied a parent .index line: nothing to blank.
        assert_eq!(index_lines(&root, ""), vec!["top.md".to_string()]);
    })
    .await;
}

/// Folder/file kind mismatches point at the right op.
#[tokio::test]
async fn delete_kind_mismatch_directs_to_other_op() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("n.md", None, "x\n", &[]).unwrap();
        tx.add_folder("foo").unwrap();
        tx.commit().await.expect("seed");
        let err = client
            .delete_folder("n.md", true, None)
            .await
            .expect_err("folder op on a note");
        match err {
            NbError::WrongKind {
                expected,
                actual,
                path,
                selector,
            } => {
                assert_eq!(expected, nb_api::KindSet::Folder, "{selector:?}");
                assert!(matches!(actual, nb_api::EntityKind::Note));
                assert_eq!(path, "n.md".to_string());
                assert_eq!(selector, format!("{}:1", env.notebook()));
            },
            other => panic!("expected WrongKind, got: {other:?}"),
        }
        let err = client
            .delete_note("foo", None)
            .await
            .expect_err("note op on a folder");
        match err {
            NbError::WrongKind {
                expected,
                actual,
                path,
                selector,
            } => {
                assert_eq!(expected, nb_api::KindSet::File);
                assert!(matches!(actual, nb_api::EntityKind::Folder));
                assert_eq!(path, "foo".to_string());
                // Canonical identity for the folder: folders take
                // index slots, so the numeric form wins here.
                assert_eq!(selector, format!("{}:2", env.notebook()));
            },
            other => panic!("expected WrongKind, got: {other:?}"),
        }
        let mut tx = client.transaction(None).await.expect("tx2");
        tx.move_note(NoteTarget::path("foo"), "bar/").unwrap();
        let err = tx.commit().await.expect_err("folder move");
        match err {
            NbError::WrongKind {
                expected,
                actual,
                path,
                selector,
            } => {
                assert_eq!(expected, nb_api::KindSet::File);
                assert!(matches!(actual, nb_api::EntityKind::Folder));
                assert_eq!(path, "foo".to_string());
                // Canonical identity for the folder: folders take
                // index slots, so the numeric form wins here.
                assert_eq!(selector, format!("{}:2", env.notebook()));
            },
            other => panic!("expected WrongKind, got: {other:?}"),
        }
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

/// `add_folder` records the folder basename in the parent `.index`,
/// mirroring `nb` (which lists folders it creates in the parent index).
/// Without the folder line, `nb list <nb>:<folder>/` fails at depth >= 2
/// (nbspec FYI 1472bebb / nb-api:issues/9).
#[tokio::test]
async fn add_folder_records_folder_line_in_parent_index() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("top.md", None, "x\n", &[]).unwrap();
        tx.add_folder("foo").unwrap();
        let outcome = tx.commit().await.expect("commit");
        let root = client.show_notebook_path(None).await.expect("path");
        assert_eq!(index_lines(&root, ""), vec!["top.md", "foo"]);
        // The folder create reports its own post-write placement.
        let op = &outcome.ops[1];
        assert_eq!(op.path.as_deref(), Some("foo"));
        assert_eq!(op.numeric_id, Some(2));
        assert_eq!(
            op.selector.as_deref(),
            Some(format!("{}:2", env.notebook()).as_str())
        );
    })
    .await;
}

/// Numeric ids count every `.index` line including folders, matching
/// `nb show <id>` numbering.
#[tokio::test]
async fn numeric_ids_count_folder_lines() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_folder("foo").unwrap();
        tx.add_note("foo/a.md", None, "x\n", &[]).unwrap();
        tx.add_note("top.md", None, "y\n", &[]).unwrap();
        let outcome = tx.commit().await.expect("commit");
        let root = client.show_notebook_path(None).await.expect("path");
        assert_eq!(index_lines(&root, ""), vec!["foo", "top.md"]);
        assert_eq!(index_lines(&root, "foo"), vec!["a.md"]);
        assert_eq!(outcome.ops[0].numeric_id, Some(1));
        assert_eq!(outcome.ops[2].numeric_id, Some(2));
        // Reads echo the folder-aware ids too.
        let shown = client.show_note("top.md", None).await.expect("show");
        assert_eq!(shown.numeric_id, Some(2));
    })
    .await;
}

/// Empty subfolders list successfully once the parent `.index` carries
/// the folder line (the nb-api:issues/9 repro shape).
///
/// Unix-only: `nb list <nb>:<nested>/` crashes under the Windows Git Bash
/// `.cmd` launcher (nb 7.24.0 lines 4839/5086 `[[` regex on backslash
/// paths) whenever a parent `.index` is in play (`nb-api:todos/api/9`).
#[cfg(unix)]
#[tokio::test]
async fn list_empty_subfolder_with_parent_index_succeeds() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("proposals/7/proposal.md", None, "p\n", &[])
            .unwrap();
        tx.add_folder("proposals/7/specifications").unwrap();
        tx.commit().await.expect("seed");
        let root = client.show_notebook_path(None).await.expect("path");
        assert_eq!(
            index_lines(&root, "proposals/7"),
            vec!["proposal.md", "specifications"]
        );
        let out = client
            .list_notes(Some("proposals/7/specifications"), &[], None, None)
            .await
            .expect("empty subfolder lists");
        assert!(out.contains("0 items"), "{out:?}");
    })
    .await;
}

/// Todo state ops on a non-todo are a typed kind mismatch.
#[tokio::test]
async fn wrong_kind_covers_state_ops() {
    use nb_api::NbError;

    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = env.nb_dir().join(env.notebook());
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("hello_title.md", Some("Hello Title"), "body\n", &[])
            .unwrap();
        tx.commit().await.expect("seed");

        // mark_task_done on a Note: Todo expected, Note observed.
        let err = client
            .mark_task_done("hello_title.md", None, None)
            .await
            .expect_err("todo state on a note");
        match err {
            NbError::WrongKind {
                expected,
                actual,
                path,
                selector,
            } => {
                assert_eq!(expected, nb_api::KindSet::Todo);
                assert!(matches!(actual, nb_api::EntityKind::Note));
                assert_eq!(path, "hello_title.md".to_string());
                // Canonical identity, not the raw input.
                assert_eq!(selector, format!("{}:1", env.notebook()));
            },
            other => panic!("expected WrongKind, got: {other:?}"),
        }

        // Binary content is `Other`, never a misclassified document:
        // textuality is checked before parsing.
        std::fs::write(root.join("blob.xyz"), [b'a', 1u8, b'b', b'\n'])
            .expect("binary");
        let committed = std::process::Command::new("git")
            .current_dir(&root)
            .args(["add", "blob.xyz"])
            .status()
            .expect("git add runs")
            .success()
            && std::process::Command::new("git")
                .current_dir(&root)
                .args(["commit", "-m", "binary fixture", "--quiet"])
                .status()
                .expect("git commit runs")
                .success();
        assert!(committed, "binary fixture must commit");
        let err = client
            .mark_task_done("blob.xyz", None, None)
            .await
            .expect_err("todo state on binary");
        match err {
            NbError::WrongKind {
                expected, actual, ..
            } => {
                assert_eq!(expected, nb_api::KindSet::Todo);
                assert!(
                    matches!(actual, nb_api::EntityKind::Other { .. }),
                    "{actual:?}"
                );
            },
            other => panic!("expected WrongKind, got: {other:?}"),
        }
    })
    .await;
}

/// Text edits on non-textual files are a typed kind mismatch: the
/// document set is required. The binary fixture is committed with
/// git directly (transactions only carry UTF-8 notes) so later
/// commits meet a clean tree.
#[tokio::test]
async fn wrong_kind_covers_text_edits() {
    use nb_api::{NbError, NoteTarget, Occurrence};

    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = env.nb_dir().join(env.notebook());
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("hello_title.md", Some("Hello Title"), "body\n", &[])
            .unwrap();
        tx.commit().await.expect("seed");
        std::fs::write(root.join("blob.xyz"), b"a\x01b\n").expect("binary");
        let committed = std::process::Command::new("git")
            .current_dir(&root)
            .args(["add", "blob.xyz"])
            .status()
            .expect("git add runs")
            .success()
            && std::process::Command::new("git")
                .current_dir(&root)
                .args(["commit", "-m", "binary fixture", "--quiet"])
                .status()
                .expect("git commit runs")
                .success();
        assert!(committed, "binary fixture must commit");

        let err = client
            .edit_note_substring(
                NoteTarget::path("blob.xyz"),
                "a",
                "b",
                Occurrence::First,
                1,
                None,
                None,
            )
            .await
            .expect_err("text edit on binary");
        match err {
            NbError::WrongKind {
                expected,
                actual,
                path,
                selector,
            } => {
                assert_eq!(expected, nb_api::KindSet::Document);
                assert!(
                    matches!(actual, nb_api::EntityKind::Other { .. }),
                    "{actual:?}"
                );
                assert_eq!(path, "blob.xyz".to_string());
                // Canonical identity re-resolves to the entity
                // (classify works on non-textual files).
                let back =
                    client.classify(&selector, None).await.unwrap_or_else(
                        |e| panic!("round-trip {selector:?}: {e:?}"),
                    );
                assert_eq!(back.path, "blob.xyz".to_string());
            },
            other => panic!("expected WrongKind, got: {other:?}"),
        }
    })
    .await;
}

/// Virtual-tree resolution follows the pinned table: exact path,
/// numeric id, then root title; dead slots fall through instead of
/// resolving to nothing.
#[tokio::test]
async fn virtual_tree_resolves_selector_table_natively() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = env.nb_dir().join(env.notebook());
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("hello_title.md", Some("Hello Title"), "body\n", &[])
            .unwrap();
        tx.add_note("notes.txt", Some("Notes"), "notes body\n", &[])
            .unwrap();
        tx.commit().await.expect("seed");

        // Numeric id through the virtual index.
        let mut tx = client.transaction(None).await.expect("tx");
        tx.delete_note(NoteTarget::selector("2")).unwrap();
        tx.commit().await.expect("delete by id");
        assert!(!root.join("notes.txt").exists(), "slot 2 deleted");

        // Root title through the virtual tree.
        let mut tx = client.transaction(None).await.expect("tx");
        tx.delete_note(NoteTarget::selector("Hello Title")).unwrap();
        tx.commit().await.expect("delete by title");
        assert!(!root.join("hello_title.md").exists(), "title deleted");

        // Missing stays NotFound.
        let mut tx = client.transaction(None).await.expect("tx");
        tx.delete_note(NoteTarget::selector("nope-missing"))
            .unwrap();
        let err = format!("{:?}", tx.commit().await.expect_err("missing"));
        assert!(err.contains("NotFound"), "{err:?}");
    })
    .await;
}

/// Backslash collision: a literal `a\b.md` file and a nested
/// `a/b.md` file coexist. Path, Selector, and one-shot deletes of
/// the literal name must remove only the literal file, and absolute
/// Path inputs must fail closed without touching anything.
/// Unix-only: on Windows the backslash is a native separator.
#[cfg(unix)]
#[tokio::test]
async fn path_resolution_never_collapses_backslash() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = env.nb_dir().join(env.notebook());
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("victim.md", Some("Victim"), "victim\n", &[])
            .unwrap();
        tx.add_note("a/b.md", Some("Nested"), "nested\n", &[])
            .unwrap();
        tx.commit().await.expect("seed");

        // Raw literal fixture: transactions refuse backslash creates,
        // so commit directly like the other binary fixtures.
        let commit_raw = |message: &str| {
            let added = std::process::Command::new("git")
                .current_dir(&root)
                .args(["add", "-A"])
                .status()
                .expect("git add runs")
                .success();
            assert!(added, "git add must succeed");
            let committed = std::process::Command::new("git")
                .current_dir(&root)
                .args(["commit", "-m", message, "--quiet"])
                .status()
                .expect("git commit runs")
                .success();
            assert!(committed, "raw fixture must commit");
        };
        std::fs::write(root.join("a\\b.md"), "# Literal\n\nliteral\n")
            .expect("literal backslash file");
        commit_raw("literal backslash fixture");

        // Exact Path targets only the literal file.
        let mut tx = client.transaction(None).await.expect("tx");
        tx.delete_note(NoteTarget::path("a\\b.md")).unwrap();
        tx.commit().await.expect("delete literal by path");
        assert!(!root.join("a\\b.md").exists(), "literal deleted");
        assert!(root.join("a/b.md").exists(), "nested untouched");
        assert!(root.join("victim.md").exists(), "victim untouched");

        // Selector form resolves the same literal, not the nesting.
        std::fs::write(root.join("a\\b.md"), "# Literal\n\nliteral\n")
            .expect("literal backslash file");
        commit_raw("literal backslash fixture");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.delete_note(NoteTarget::selector("a\\b.md")).unwrap();
        tx.commit().await.expect("delete literal by selector");
        assert!(!root.join("a\\b.md").exists(), "literal deleted");
        assert!(root.join("a/b.md").exists(), "nested untouched");

        // One-shot resolution mutates only the intended entity.
        std::fs::write(root.join("a\\b.md"), "# Literal\n\nliteral\n")
            .expect("literal backslash file");
        commit_raw("literal backslash fixture");
        client
            .delete_note("a\\b.md", None)
            .await
            .expect("one-shot deletes the literal");
        assert!(!root.join("a\\b.md").exists(), "literal deleted");
        assert!(root.join("a/b.md").exists(), "nested untouched");
        assert!(root.join("victim.md").exists(), "victim untouched");

        // Absolute Path never retargets: `/victim.md` is NotFound and
        // the relative file survives, on every write surface.
        let mut tx = client.transaction(None).await.expect("tx");
        tx.delete_note(NoteTarget::path("/victim.md")).unwrap();
        let err = format!("{:?}", tx.commit().await.expect_err("absolute"));
        assert!(err.contains("NotFound"), "{err:?}");
        assert!(root.join("victim.md").exists(), "victim untouched");
        let err = format!(
            "{:?}",
            client
                .delete_note("/victim.md", None)
                .await
                .expect_err("absolute one-shot")
        );
        assert!(err.contains("NotFound"), "{err:?}");
        assert!(root.join("victim.md").exists(), "victim untouched");
    })
    .await;
}

/// Todo state ops on textual-but-unsupported extensions (e.g.
/// `.tex`, rejected by the parser) are a typed kind mismatch —
/// Todo expected, Note observed — never a leaked parse failure.
#[tokio::test]
async fn wrong_kind_covers_unsupported_textual_note() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = env.nb_dir().join(env.notebook());
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("hello.md", Some("Hello"), "body\n", &[])
            .unwrap();
        tx.commit().await.expect("seed");
        // Transactions only carry supported notes, so the `.tex`
        // fixture is committed with git directly.
        std::fs::write(root.join("paper.tex"), "\\section{Paper}\nbody\n")
            .expect("tex fixture");
        let committed = std::process::Command::new("git")
            .current_dir(&root)
            .args(["add", "paper.tex"])
            .status()
            .expect("git add runs")
            .success()
            && std::process::Command::new("git")
                .current_dir(&root)
                .args(["commit", "-m", "tex fixture", "--quiet"])
                .status()
                .expect("git commit runs")
                .success();
        assert!(committed, "tex fixture must commit");
        let err = client
            .mark_task_done("paper.tex", None, None)
            .await
            .expect_err("todo state on a .tex note");
        match err {
            NbError::WrongKind {
                expected, actual, ..
            } => {
                assert_eq!(expected, nb_api::KindSet::Todo);
                assert!(
                    matches!(actual, nb_api::EntityKind::Note),
                    "{actual:?}"
                );
            },
            other => panic!("expected WrongKind, got: {other:?}"),
        }
    })
    .await;
}
