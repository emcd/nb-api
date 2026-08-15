//! `edit_note` / `EditMode` removal (0.3.0). Body edits use replace/lines/substring.
//!
//! # Windows
//!
//! Unix-only (`#![cfg(unix)]`): the replace path runs two
//! collect-then-commit transactions in quick succession, and nb 7.24.0's
//! background auto-checkpoint (fired during fixture init) intermittently
//! races the second commit's dirty baseline check under the Git Bash
//! `.cmd` launcher. This is the nb-under-Git-Bash limitation documented
//! in `nb-api:todos/api/9`, not a nb-api defect.

#![cfg(unix)]

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

#[tokio::test]
async fn replace_note_body_destroys_existing_body() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("t.md", None, "original content that should be gone\n", &[])
            .unwrap();
        tx.commit().await.expect("add");

        let shown = client.show_note("t.md", None).await.expect("show");
        client
            .replace_note_body(
                NoteTarget::path("t.md"),
                b"fresh body\n",
                shown.fingerprint,
                None,
            )
            .await
            .expect("replace");

        let shown = client.show_note("t.md", None).await.expect("show2");
        let body_bytes = shown.body.as_bytes().unwrap();
        let body = String::from_utf8_lossy(&body_bytes);
        assert!(!body.contains("original content"), "{body}");
        assert!(body.contains("fresh body"), "{body}");
    })
    .await;
}
