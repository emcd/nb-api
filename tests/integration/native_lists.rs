//! Typed native listings: notes, folders, search, tasks, todos,
//! and notebook status over the seeded corpus.

use nb_api::NbClient;
use nb_api::testing::NbTestEnv;

use crate::common::with_isolated_env;
use crate::native_reads_support::{config_for, seed};

/// Typed note listing: every in-scope file once, lexical order,
/// shadow-aware selectors, tag/limit filters, no spawns.
#[tokio::test]
async fn native_list_notes_typed_covers_scope() {
    use nb_api::EntityKind;

    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let native = NbClient::new(&config_for(&env)).expect("native");
        let root = env.nb_dir().join(env.notebook());
        seed(&native, &root).await;
        let nb = env.notebook();

        let listed = native
            .list_notes_typed(None, &[], None, None)
            .await
            .expect("root listing");
        let mut expected: Vec<String> = [
            "blob.xyz",
            "bzh.xyz",
            "del_nl.xyz",
            "del_nonl.xyz",
            "empty.xyz",
            "fakepdf.xyz",
            "fakepng.xyz",
            "flac.xyz",
            "front.md",
            "gif89a.xyz",
            "hello_title.md",
            "id3.xyz",
            "latin1.xyz",
            "mark.markdown",
            "mystery.xyz",
            "mz.xyz",
            "noorg.org",
            "notes.txt",
            "onebyte.xyz",
            "plain",
            "riff.xyz",
            "rootodo.todo.md",
            "setext.md",
            "trail.md",
            "upper.MD",
            "utf16.txt",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        expected.sort();
        let paths: Vec<String> =
            listed.iter().map(|e| e.path.clone()).collect();
        assert_eq!(paths, expected, "root listing mismatch");
        // Spot-check kinds and canonical selectors.
        let by_path =
            |p: &str| listed.iter().find(|e| e.path == p).unwrap().clone();
        assert_eq!(by_path("hello_title.md").kind, EntityKind::Note);
        assert_eq!(by_path("hello_title.md").selector, format!("{nb}:1"));
        assert_eq!(by_path("rootodo.todo.md").kind, EntityKind::Todo);
        assert_eq!(
            by_path("blob.xyz").kind,
            EntityKind::Other {
                reason: "non-textual:xyz".to_string()
            }
        );

        // Folder scope is non-recursive: only direct children.
        let work = native
            .list_notes_typed(Some("work"), &[], None, None)
            .await
            .expect("work listing");
        let work_paths: Vec<String> =
            work.iter().map(|e| e.path.clone()).collect();
        assert_eq!(
            work_paths,
            vec![
                "work/a.md".to_string(),
                "work/do_the_thing.todo.md".to_string(),
                "work/example.bookmark.md".to_string(),
            ],
            "{work_paths:?}"
        );
        assert_eq!(work[0].selector, format!("{nb}:work/1"));
        assert_eq!(work[0].kind, EntityKind::Note);
        assert_eq!(work[1].kind, EntityKind::Todo);
        assert_eq!(work[2].kind, EntityKind::Bookmark);

        // Missing scope and file-as-scope are terminal NotFound.
        for scope in ["nope", "notes.txt", "../evil"] {
            let err = format!(
                "{:?}",
                native
                    .list_notes_typed(Some(scope), &[], None, None)
                    .await
                    .expect_err("bad scope must fail")
            );
            assert!(err.contains("NotFound"), "{scope}: {err:?}");
        }

        // Tags filter on parsed document tags; limit takes the head.
        // Written raw (no commit): committing after seeding always
        // trips DirtyBaseline on the untracked seed files.
        std::fs::write(
            root.join("tagged.md"),
            "# Tagged\n\n#alpha #beta\n\nbody\n",
        )
        .expect("tagged file");
        let tagged = native
            .list_notes_typed(None, &["alpha".to_string()], None, None)
            .await
            .expect("tag filter");
        assert_eq!(tagged.len(), 1, "{tagged:?}");
        assert_eq!(tagged[0].path, "tagged.md".to_string());
        let hashed = native
            .list_notes_typed(None, &["#alpha".to_string()], None, None)
            .await
            .expect("hash tag filter");
        assert_eq!(hashed.len(), 1, "{hashed:?}");
        let none = native
            .list_notes_typed(None, &["nope".to_string()], None, None)
            .await
            .expect("empty tag filter");
        assert!(none.is_empty(), "{none:?}");
        let limited = native
            .list_notes_typed(None, &[], Some(2), None)
            .await
            .expect("limit");
        assert_eq!(limited.len(), 2, "{limited:?}");
        assert_eq!(limited[0].path, "blob.xyz".to_string());
    })
    .await;
}

/// Typed folder listing: direct child directories only.
#[tokio::test]
async fn native_list_folders_typed_covers_scope() {
    use nb_api::EntityKind;

    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let native = NbClient::new(&config_for(&env)).expect("native");
        let root = env.nb_dir().join(env.notebook());
        seed(&native, &root).await;
        let nb = env.notebook();

        let listed = native
            .list_folders_typed(None, None)
            .await
            .expect("root folders");
        let paths: Vec<String> =
            listed.iter().map(|e| e.path.clone()).collect();
        assert_eq!(paths, vec!["todos".to_string(), "work".to_string()]);
        for entry in &listed {
            assert_eq!(entry.kind, EntityKind::Folder, "{entry:?}");
        }
        // `todos` is tracked in the root index (slot 8); `work` is
        // not, so only `todos` carries the numeric form.
        assert_eq!(listed[0].selector, format!("{nb}:8"), "{listed:?}");
        assert_eq!(listed[0].numeric_id, Some(8), "{listed:?}");
        assert_eq!(listed[1].selector, format!("{nb}:work"), "{listed:?}");
        assert_eq!(listed[1].numeric_id, None, "{listed:?}");
        let empty = native
            .list_folders_typed(Some("work"), None)
            .await
            .expect("leaf folders");
        assert!(empty.is_empty(), "{empty:?}");
        let err = format!(
            "{:?}",
            native
                .list_folders_typed(Some("nope"), None)
                .await
                .expect_err("missing scope must fail")
        );
        assert!(err.contains("NotFound"), "{err:?}");
    })
    .await;
}

/// Notebook status reads git state without spawning `nb`.
#[tokio::test]
async fn native_show_notebook_status_reports_git_state() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let native = NbClient::new(&config_for(&env)).expect("native");
        let root = env.nb_dir().join(env.notebook());
        seed(&native, &root).await;

        // Seeded raw files are untracked: porcelain shows them.
        let status = native
            .show_notebook_status(None)
            .await
            .expect("seeded status");
        assert!(status.contains("?? plain"), "{status:?}");

        // Untracked droppings show up as porcelain lines.
        std::fs::write(root.join("scratch.txt"), "scratch\n")
            .expect("scratch file");
        let status = native
            .show_notebook_status(None)
            .await
            .expect("dirty status");
        assert!(status.contains("scratch.txt"), "{status:?}");

        // A missing notebook is terminal NotFound, never created.
        let err = format!(
            "{:?}",
            native
                .show_notebook_status(Some("no-such-notebook"))
                .await
                .expect_err("missing notebook")
        );
        assert!(err.contains("NotFound"), "{err:?}");
        assert!(
            !env.nb_dir().join("no-such-notebook").exists(),
            "status must not create notebooks"
        );
    })
    .await;
}

/// Typed search: recursive case-insensitive title/body match with
/// tag filtering, lexical entries, zero spawns.
#[tokio::test]
async fn native_search_notes_typed_matches_text() {
    use nb_api::{EntityKind, SearchMode};

    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let native = NbClient::new(&config_for(&env)).expect("native");
        let root = env.nb_dir().join(env.notebook());
        seed(&native, &root).await;

        // Case-insensitive title/body matching across the tree.
        let hits = native
            .search_notes_typed(
                &["work body".to_string()],
                SearchMode::Any,
                &[],
                None,
                None,
            )
            .await
            .expect("search body");
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].path, "work/a.md".to_string());
        assert_eq!(hits[0].kind, EntityKind::Note);

        // `All` requires every query; `Any` requires one.
        let all = native
            .search_notes_typed(
                &["work".to_string(), "body".to_string()],
                SearchMode::All,
                &[],
                None,
                None,
            )
            .await
            .expect("search all");
        assert_eq!(all.len(), 1, "{all:?}");
        let any = native
            .search_notes_typed(
                &["work".to_string(), "absent-xyz".to_string()],
                SearchMode::Any,
                &[],
                None,
                None,
            )
            .await
            .expect("search any");
        assert_eq!(any.len(), 1, "{any:?}");
        let all_miss = native
            .search_notes_typed(
                &["work".to_string(), "absent-xyz".to_string()],
                SearchMode::All,
                &[],
                None,
                None,
            )
            .await
            .expect("search all miss");
        assert!(all_miss.is_empty(), "{all_miss:?}");

        // Folder scope restricts the walk; missing scope is NotFound.
        let scoped = native
            .search_notes_typed(
                &["body".to_string()],
                SearchMode::Any,
                &[],
                Some("work"),
                None,
            )
            .await
            .expect("scoped search");
        assert!(
            scoped.iter().all(|e| e.path.starts_with("work/")),
            "{scoped:?}"
        );
        let err = format!(
            "{:?}",
            native
                .search_notes_typed(
                    &["body".to_string()],
                    SearchMode::Any,
                    &[],
                    Some("nope"),
                    None
                )
                .await
                .expect_err("missing scope must fail")
        );
        assert!(err.contains("NotFound"), "{err:?}");

        // Empty queries are ValidationError, never a match-all.
        let err = format!(
            "{:?}",
            native
                .search_notes_typed(&[], SearchMode::Any, &[], None, None)
                .await
                .expect_err("empty queries must fail")
        );
        assert!(err.contains("ValidationError"), "{err:?}");
    })
    .await;
}

/// Typed task and todo listings over the seed corpus.
#[tokio::test]
async fn native_task_surfaces_enumerate_todos() {
    use nb_api::{EntityKind, TaskStatus, TodoState};

    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let native = NbClient::new(&config_for(&env)).expect("native");
        let root = env.nb_dir().join(env.notebook());
        seed(&native, &root).await;
        let nb = env.notebook();

        // Per-item entries with fragment selectors, file order.
        // The root todo carries no items; the work todo has two
        // (covered recursively below).
        let tasks = native
            .list_tasks_typed(None, None, false, None)
            .await
            .expect("root tasks");
        assert!(tasks.is_empty(), "{tasks:?}");

        // Item-level status filter: the seed todo has no items, so
        // state filters select by file openness below.
        let open = native
            .list_tasks_typed(None, Some(TaskStatus::Open), true, None)
            .await
            .expect("open tasks");
        assert!(open.iter().all(|e| e.kind == EntityKind::Todo), "{open:?}");

        // Per-file summaries with counts and derived state.
        let todos = native
            .list_todos(None, None, false, None)
            .await
            .expect("root todos");
        assert_eq!(todos.len(), 1, "{todos:?}");
        let summary = &todos[0];
        assert_eq!(summary.path, "rootodo.todo.md".to_string());
        assert_eq!(summary.title, "Root Todo".to_string());
        assert_eq!(summary.id, format!("{nb}:3"));
        assert_eq!(summary.task_count, 0);
        assert_eq!(summary.completed_count, 0);
        assert_eq!(summary.state, TodoState::Open);

        // Recursive walk reaches work todos with real items.
        let rec = native
            .list_todos(None, None, true, None)
            .await
            .expect("recursive todos");
        let work = rec
            .iter()
            .find(|t| t.path == "work/do_the_thing.todo.md")
            .expect("work todo present");
        assert_eq!(work.task_count, 2, "{work:?}");
        assert_eq!(work.completed_count, 0, "{work:?}");
        assert_eq!(work.state, TodoState::Open);
        assert_eq!(work.title, "Do the thing".to_string());

        let rec_tasks = native
            .list_tasks_typed(None, None, true, None)
            .await
            .expect("recursive tasks");
        assert_eq!(rec_tasks.len(), 2, "{rec_tasks:?}");
        assert_eq!(
            rec_tasks[0].selector,
            format!("{nb}:work/2#task-1"),
            "{rec_tasks:?}"
        );
        assert_eq!(
            rec_tasks[1].selector,
            format!("{nb}:work/2#task-2"),
            "{rec_tasks:?}"
        );

        // State filters: nothing is Done in the seed corpus.
        let done = native
            .list_todos(None, Some(TaskStatus::Closed), true, None)
            .await
            .expect("done filter");
        assert!(done.is_empty(), "{done:?}");
        let closed_tasks = native
            .list_tasks_typed(None, Some(TaskStatus::Closed), true, None)
            .await
            .expect("closed filter");
        assert!(closed_tasks.is_empty(), "{closed_tasks:?}");
    })
    .await;
}
