//! Subcommand-colliding folder regression (nb-mcp-server:todos/mcp/62).
//!
//! Selectors whose trailing component collides with `nb` subcommand
//! vocabulary (`todos`, `notebooks`, `list`, ...) misdispatch when
//! resolved via `nb show <sel> --path`. Existing filesystem entries
//! must resolve directly without spawning `nb`.

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient};

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
async fn subcommand_colliding_folders_resolve_directly() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_folder("todos").unwrap();
        tx.add_folder("notebooks").unwrap();
        tx.add_folder("list").unwrap();
        tx.commit().await.expect("seed colliding folders");

        // delete_folder on colliding names must succeed via direct
        // filesystem resolution, not via `nb show --path` (which
        // misdispatches: todos -> tasks listing, notebooks -> notebook
        // root, list -> folder listing).
        for folder in ["todos", "notebooks", "list"] {
            let outcome = client
                .delete_folder(folder, false, None)
                .await
                .expect("colliding folder delete must succeed");
            assert_eq!(
                outcome.ops[0].path.as_deref(),
                Some(folder),
                "wrong path for {folder}"
            );
        }

        // Re-seed one colliding folder for show/delete_note routing.
        let mut tx = client.transaction(None).await.expect("tx2");
        tx.add_folder("todos").unwrap();
        tx.commit().await.expect("reseed");

        // show on a folder must be a typed folder refusal, not a
        // generic CommandFailed from a misdispatched `nb show`.
        let err =
            client.show_note("todos", None).await.expect_err("show folder");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("folder") || msg.contains("UnsupportedShowTarget"),
            "show on colliding folder must be typed folder refusal, got: {msg:?}"
        );

        // delete_note on a folder must be a typed kind mismatch
        // (not a generic CommandFailed from a misdispatched `nb show`).
        let err = client
            .delete_note("todos", None)
            .await
            .expect_err("note op on colliding folder");
        let msg = format!("{err:?}");
        assert!(msg.contains("wrong kind"), "{err:?}");
        assert!(msg.contains("expected File"), "{err:?}");
        assert!(msg.contains("observed Folder"), "{err:?}");
    })
    .await;
}
