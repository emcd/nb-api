//! Native read path: differential equivalence vs legacy `nb`
//! subprocess reads, plus a no-spawn proof under the flag.
//!
//! The differential battery covers paths, numeric ids (root and
//! folder-qualified), exact titles, basenames, colliding folders,
//! missing selectors, and the line-level/source-bytes surfaces.

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient, NoteTarget};

use crate::common::with_isolated_env;

fn config_for(env: &NbTestEnv, native_reads: bool) -> Config {
    Config {
        notebook: Some(env.notebook().to_string()),
        create_notebook: false,
        allow_top_level_notes: true,
        disable_git_signing: true,
        native_reads,
        ..Config::default()
    }
}

/// Seed the corpus; returns numeric ids in seed order:
/// hello, work note, todo, bookmark (folder `todos` unnumbered here).
async fn seed(client: &NbClient) -> Vec<Option<u32>> {
    let mut tx = client.transaction(None).await.expect("tx");
    tx.add_note("hello_title.md", Some("Hello Title"), "body\n", &[])
        .unwrap();
    tx.add_note("work/a.md", Some("Work Note"), "work body\n", &[])
        .unwrap();
    tx.add_todo(
        "work/do_the_thing.todo.md",
        "Do the thing",
        Some("desc\n"),
        &["task one".to_string(), "task two".to_string()],
        &[],
    )
    .unwrap();
    tx.add_bookmark(
        "work/example.bookmark.md",
        "https://example.org",
        Some("Example"),
        &[],
        None,
    )
    .unwrap();
    tx.add_folder("todos").unwrap();
    let outcome = tx.commit().await.expect("seed");
    assert!(outcome.commit_created, "{outcome:?}");
    outcome.ops.iter().map(|op| op.numeric_id).collect()
}

#[tokio::test]
async fn native_show_matches_legacy() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let legacy = NbClient::new(&config_for(&env, false)).expect("legacy");
        let native = NbClient::new(&config_for(&env, true)).expect("native");
        let ids = seed(&legacy).await;
        let nb = env.notebook();
        let hello_id = ids[0].expect("hello id");
        let work_id = ids[1].expect("work id");

        let cases = vec![
            "hello_title.md".to_string(),
            format!("{nb}:{hello_id}"),
            "Hello Title".to_string(),
            "work/a.md".to_string(),
            format!("{nb}:work/{work_id}"),
            "a.md".to_string(),
            "Work Note".to_string(),
            "work/do_the_thing.todo.md".to_string(),
            "Do the thing".to_string(),
            "work/example.bookmark.md".to_string(),
            "todos".to_string(),
            "work".to_string(),
            "nope-missing.md".to_string(),
        ];
        for id in &cases {
            let legacy_out = legacy.show_note(id, None).await;
            let native_out = native.show_note(id, None).await;
            assert_eq!(
                native_out, legacy_out,
                "native/legacy mismatch for {id:?}"
            );
        }

        // Line-level and raw-bytes surfaces ride the same path.
        let target = NoteTarget::path("work/a.md");
        assert_eq!(
            native
                .show_note_lines(target.clone(), None, None, None)
                .await,
            legacy
                .show_note_lines(target.clone(), None, None, None)
                .await,
            "show_note_lines mismatch"
        );
        assert_eq!(
            native.search_note_lines(target.clone(), "work", None).await,
            legacy.search_note_lines(target, "work", None).await,
            "search_note_lines mismatch"
        );
        assert_eq!(
            native
                .read_note_source_bytes(NoteTarget::path("work/a.md"), None)
                .await,
            legacy
                .read_note_source_bytes(NoteTarget::path("work/a.md"), None)
                .await,
            "source bytes mismatch"
        );
    })
    .await;
}

/// Every `nb` invocation fails: under the flag with `NB_DIR` set,
/// native reads must succeed, proving zero subprocess spawns.
///
/// Unix-only: relies on a `sh` dead-binary shim on `PATH`.
#[cfg(unix)]
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn native_show_spawns_no_nb() {
    use std::os::unix::fs::PermissionsExt;

    use crate::common::{EnvSnapshot, lock_env};

    let env = NbTestEnv::new().expect("fixture");
    // Seed with the real `nb` before poisoning `PATH`.
    with_isolated_env(&env, false, || async {
        let legacy = NbClient::new(&config_for(&env, false)).expect("legacy");
        seed(&legacy).await;
    })
    .await;

    let _guard = lock_env();
    let _snap = EnvSnapshot::capture(&["NB_DIR", "HOME", "PATH"]);
    let dir = env.nb_dir().join("deadbin");
    std::fs::create_dir_all(&dir).expect("deadbin");
    let shim = dir.join("nb");
    std::fs::write(&shim, "#!/bin/sh\nexit 1\n").expect("write shim");
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))
        .expect("chmod shim");
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let mut new_path = dir.into_os_string();
    new_path.push(":");
    new_path.push(path_var);
    // SAFETY: serialized by ENV_LOCK; restored by `_snap` on drop.
    unsafe { std::env::set_var("PATH", new_path) };
    unsafe { std::env::set_var("NB_DIR", env.nb_dir()) };
    unsafe { std::env::set_var("HOME", env.home_dir()) };

    let native = NbClient::new(&config_for(&env, true)).expect("native");
    let nb = env.notebook();
    let root_one = format!("{nb}:1");
    for id in [
        "hello_title.md",
        "Hello Title",
        "work/a.md",
        "work/do_the_thing.todo.md",
        root_one.as_str(),
    ] {
        let shown = native
            .show_note(id, None)
            .await
            .expect("native show must succeed with dead nb");
        assert!(!shown.path.is_empty(), "empty path for {id:?}");
    }
    // Folders and missing selectors stay typed without spawns.
    let err = format!(
        "{:?}",
        native
            .show_note("todos", None)
            .await
            .expect_err("folder show")
    );
    assert!(err.contains("folder"), "{err:?}");
    let err = format!(
        "{:?}",
        native
            .show_note("nope-missing.md", None)
            .await
            .expect_err("missing show")
    );
    assert!(err.contains("NotFound"), "{err:?}");
}
