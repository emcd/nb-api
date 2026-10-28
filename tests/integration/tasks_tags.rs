//! 0.4.2 regressions: notebook-scope task listing (57) and tag
//! shrink orphans (58).

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient, NoteTarget, TaskStatus};

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

/// Notebook-scope recursive listing reaches nested-folder todos.
#[tokio::test]
async fn list_tasks_notebook_scope_covers_nested_folders() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        client
            .add_todo("Root", None, &["r1".to_string()], &[], None, None)
            .await
            .expect("root todo");
        client
            .add_todo(
                "Nested",
                None,
                &["n1".to_string()],
                &[],
                Some("proj"),
                None,
            )
            .await
            .expect("nested todo");

        let all = client
            .list_tasks(None, Some(TaskStatus::Open), true, None)
            .await
            .expect("recursive list");
        assert!(all.contains("r1"), "{all:?}");
        assert!(all.contains("n1"), "{all:?}");

        let shallow = client
            .list_tasks(None, Some(TaskStatus::Open), false, None)
            .await
            .expect("shallow list");
        assert!(shallow.contains("r1"), "{shallow:?}");
        assert!(!shallow.contains("n1"), "{shallow:?}");
    })
    .await;
}

/// Empty scope returns the empty signal, not an error.
#[tokio::test]
async fn list_tasks_empty_scope_returns_signal() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let out = client
            .list_tasks(None, Some(TaskStatus::Open), true, None)
            .await
            .expect("empty list is Ok");
        assert!(out.contains("0 open tasks"), "{out:?}");
    })
    .await;
}

/// Removing one tag keeps the canonical grouped section: show tags
/// and file bytes agree, no orphan tag line in the body.
#[tokio::test]
async fn edit_tags_shrink_keeps_grouped_section() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let outcome = client
            .add_todo(
                "Tagged",
                None,
                &[],
                &["alpha".to_string(), "beta".to_string()],
                None,
                None,
            )
            .await
            .expect("add");
        let path = outcome.ops[0].path.clone().expect("path");

        client
            .edit_note_tags(
                NoteTarget::path(path.clone()),
                &[],
                &["beta".to_string()],
                None,
            )
            .await
            .expect("remove beta");

        let shown = client.show_note(&path, None).await.expect("show");
        assert_eq!(shown.tags, vec!["alpha".to_string()]);
        assert!(
            shown.source.contains("## Tags\n\n#alpha\n"),
            "canonical grouped section, got:\n{}",
            shown.source
        );
        // No orphan `#alpha` body line outside the Tags section:
        // exactly one occurrence, inside the canonical section.
        assert_eq!(
            shown.source.matches("#alpha").count(),
            1,
            "orphan tag line in body:\n{}",
            shown.source
        );
    })
    .await;
}

/// Note-kind shrink to a single tag: the survivor stays a tag
/// (nb parity: nb writes single-tag notes and matches them via
/// `list --tags`), so show and bytes must agree.
#[tokio::test]
async fn edit_note_tags_shrink_to_single_keeps_tag() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let outcome = client
            .add_note(
                Some("Titled"),
                "body\n",
                &["alpha".to_string(), "beta".to_string()],
                None,
                None,
            )
            .await
            .expect("add");
        let path = outcome.ops[0].path.clone().expect("path");

        client
            .edit_note_tags(
                NoteTarget::path(path.clone()),
                &[],
                &["beta".to_string()],
                None,
            )
            .await
            .expect("remove beta");

        let shown = client.show_note(&path, None).await.expect("show");
        assert_eq!(shown.tags, vec!["alpha".to_string()]);
        assert!(
            !shown.body.contains("#alpha"),
            "survivor leaked into body:\n{}",
            shown.body
        );
    })
    .await;
}

/// Removing the last tag drops the section entirely.
#[tokio::test]
async fn edit_tags_remove_all_drops_section() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let outcome = client
            .add_todo("Solo", None, &[], &["solo".to_string()], None, None)
            .await
            .expect("add");
        let path = outcome.ops[0].path.clone().expect("path");

        client
            .edit_note_tags(
                NoteTarget::path(path.clone()),
                &[],
                &["solo".to_string()],
                None,
            )
            .await
            .expect("remove solo");

        let shown = client.show_note(&path, None).await.expect("show");
        assert!(shown.tags.is_empty(), "{:?}", shown.tags);
        assert!(
            !shown.source.contains("## Tags"),
            "stale section header:\n{}",
            shown.source
        );
        assert!(
            !shown.source.lines().any(|l| l.trim() == "#solo"),
            "orphan tag line:\n{}",
            shown.source
        );
    })
    .await;
}
