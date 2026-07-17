//! Regression tests for the `nb-api 0.2.0` empty-result hint
//! block sanitization.
//!
//! `NbClient::list_notes` and `NbClient::list_folders` strip the
//! trailing usage/help hint block when an empty result is
//! detected, returning only the empty-result signal (e.g.,
//! `0 items.` or `0 folders.`). Non-empty results are
//! returned unchanged. The `tasks` method's existing
//! empty-todos handling is preserved (single-line `! 0
//! tasks.` format).
//!
//! See `nb-api:proposals/add-0-2-0-foundation/specifications/14`
//! (output-behavior specification) and
//! `nb-api:proposals/add-0-2-0-foundation/designs/2` design
//! note D2.

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient};

use crate::common::with_isolated_env;

#[cfg(unix)]
use crate::common::with_shim_nb_env;

fn config_for(env: &NbTestEnv) -> Config {
    Config {
        notebook: Some(env.notebook().to_string()),
        create_notebook: false,
        allow_top_level_notes: true,
        ..Config::default()
    }
}

#[tokio::test]
async fn list_strips_hint_block_for_empty_notebook() {
    // Empty notebook (no notes). `nb ls` returns
    // `0 items.\n\nAdd a note:\n  ...` plus more hint lines.
    // The sanitization must return only the signal.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let output = client
            .list_notes(None, &[], None, None)
            .await
            .expect("list");
        assert_eq!(
            output, "0 items.\n",
            "empty list should return only the empty-result signal; got:\n{output:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn list_returns_full_output_for_non_empty_notebook() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        client
            .add_note(Some("alpha"), "alpha body", &[], None, None)
            .await
            .expect("add alpha");
        client
            .add_note(Some("beta"), "beta body", &[], None, None)
            .await
            .expect("add beta");

        let output = client
            .list_notes(None, &[], None, None)
            .await
            .expect("list");
        assert!(
            output.contains("alpha") && output.contains("beta"),
            "non-empty list should contain item titles; got:\n{output:?}"
        );
        assert!(
            !output.contains("Add a note:"),
            "non-empty list should not have the hint block; got:\n{output:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn folders_strips_hint_block_for_empty_folders() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let output = client.list_folders(None, None).await.expect("folders");
        assert_eq!(
            output, "0 folders.\n",
            "empty folders should return only the empty-result signal; got:\n{output:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn folders_returns_full_output_for_non_empty_folders() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        client.add_folder("subfolder", None).await.expect("mkdir");
        client.add_folder("other", None).await.expect("mkdir");

        let output = client.list_folders(None, None).await.expect("folders");
        assert!(
            output.contains("subfolder") && output.contains("other"),
            "non-empty folders should contain folder names; got:\n{output:?}"
        );
        assert!(
            !output.contains("Import a file:"),
            "non-empty folders should not have the hint block; got:\n{output:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn list_and_folders_sweep_sanity() {
    // Sanity check that the sanitization is consistent across
    // both methods: empty notebook has both empty list and
    // empty folders; non-empty notebook has both populated.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        // Initially empty: both methods should return their
        // empty-result signals only.
        let list_empty = client
            .list_notes(None, &[], None, None)
            .await
            .expect("list");
        let folders_empty = client.list_folders(None, None).await.expect("folders");
        assert_eq!(list_empty, "0 items.\n");
        assert_eq!(folders_empty, "0 folders.\n");

        // Add a note and a folder.
        client
            .add_note(Some("one"), "body", &[], None, None)
            .await
            .expect("add");
        client.add_folder("dir", None).await.expect("mkdir");

        let list_pop = client
            .list_notes(None, &[], None, None)
            .await
            .expect("list");
        let folders_pop = client.list_folders(None, None).await.expect("folders");
        assert!(list_pop.contains("one"));
        assert!(folders_pop.contains("dir"));
        assert!(!list_pop.contains("Add a note:"));
        assert!(!folders_pop.contains("Import a file:"));
    })
    .await;
}

#[tokio::test]
async fn list_after_deleting_all_items_returns_clean_signal() {
    // Sanity: delete all items then re-list; should be clean.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        client
            .add_note(Some("only"), "body", &[], None, None)
            .await
            .expect("add");
        client.delete_note("1", None).await.expect("delete");

        let output = client
            .list_notes(None, &[], None, None)
            .await
            .expect("list");
        assert_eq!(output, "0 items.\n");
    })
    .await;
}

#[tokio::test]
async fn show_does_not_apply_hint_block_sanitization() {
    // The sanitization helper is scoped to list-style methods.
    // User-content methods (show, add, edit, etc.) MUST NOT
    // apply the helper, because a note whose first line is
    // `0 items.` would be wrongly truncated. This test
    // verifies the boundary: a note with body `0 items.`
    // followed by hint-marker-looking content is returned
    // verbatim by show.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        // Body contains `0 items.\n\nAdd a note:` which is the
        // exact pattern the helper would truncate if applied
        // to show. Verify show returns the body verbatim.
        let body = "0 items.\n\nAdd a note:\n  malicious-looking hint\n";
        client
            .add_note(Some("decoy"), body, &[], None, None)
            .await
            .expect("add");

        let output = client.show_note("1", None).await.expect("show");
        assert!(
            output.contains("malicious-looking hint"),
            "show must NOT apply the sanitization helper; user content should be returned verbatim. \
             got output: {output:?}"
        );
        assert!(
            output.contains("Add a note:"),
            "show must return the entire user body including the `Add a note:` line; \
             got output: {output:?}"
        );
    })
    .await;
}

// The remaining tests in this module use a deterministic `nb`
// shim (`with_shim_nb_env`) to drive the public list/folders
// API with crafted output that real `nb 7.24.0` does not
// produce: CRLF terminators, missing terminators, and `0
// <kind>.` signal lines without a trailing hint block. These
// cases cannot be triggered through real `nb` invocations but
// are part of the helper's documented contract.
//
// All shim-based tests are Unix-only: they write a Bash
// script, chmod the executable bit, and use `:` as the PATH
// separator. On non-Unix platforms the shim helper panics
// at fixture setup, so these tests are gated accordingly.

#[cfg(unix)]
#[tokio::test]
async fn list_preserves_lf_terminator_via_shim() {
    let env = NbTestEnv::new().expect("fixture initialization");
    let crafted = "0 items.\n\nAdd a note:\n  nb add scratch:\n";
    with_shim_nb_env(&env, crafted, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");
        let output = client
            .list_notes(None, &[], None, None)
            .await
            .expect("list");
        assert_eq!(
            output, "0 items.\n",
            "LF terminator must be preserved exactly"
        );
    })
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn list_preserves_crlf_terminator_via_shim() {
    let env = NbTestEnv::new().expect("fixture initialization");
    // Full CRLF hint block: signal\r\n + blank\r\n + markers\r\n.
    let crafted = "0 items.\r\n\r\nAdd a note:\r\n  nb add scratch:\r\nAdd a bookmark:\r\n  nb scratch: <url>\r\nHelp information:\r\n  nb help\r\n";
    with_shim_nb_env(&env, crafted, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");
        let output = client
            .list_notes(None, &[], None, None)
            .await
            .expect("list");
        assert_eq!(
            output, "0 items.\r\n",
            "CRLF terminator must be preserved exactly (Windows-style \\r\\n, not LF)"
        );
    })
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn list_preserves_signal_only_no_terminator_no_hint_via_shim() {
    // True no-terminator case: SHIM_OUTPUT is the bare signal
    // with no trailing `\n`/`\r\n` and no hint block. The byte-
    // level parser finds no `\n`, so `line_end_with_terminator`
    // equals `output.len()` and `rest` is empty. The helper has
    // no terminator to preserve AND no content to strip, so it
    // returns input unchanged. The byte-for-byte preservation
    // of the bare signal IS the contract.
    let env = NbTestEnv::new().expect("fixture initialization");
    let crafted = "0 items.";
    with_shim_nb_env(&env, crafted, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");
        let output = client
            .list_notes(None, &[], None, None)
            .await
            .expect("list");
        assert_eq!(
            output, "0 items.",
            "Bare signal with no terminator and no hint block must round-trip unchanged"
        );
    })
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn list_preserves_trailing_hint_without_final_newline_via_shim() {
    // Partial no-terminator case: signal IS terminated, but
    // the final hint line has no trailing newline. The helper
    // strips the hint block and preserves the signal terminator
    // (a single `\n`). No extra LF is appended.
    let env = NbTestEnv::new().expect("fixture initialization");
    let crafted = "0 items.\n\nAdd a note:\n  nb add scratch:";
    with_shim_nb_env(&env, crafted, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");
        let output = client
            .list_notes(None, &[], None, None)
            .await
            .expect("list");
        assert_eq!(
            output, "0 items.\n",
            "Signal terminator preserved; no extra LF appended beyond input"
        );
    })
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn list_no_recognized_marker_returns_input_unchanged_via_shim() {
    // Signal + blank separator, but NO recognized hint marker.
    // The helper must return input unchanged (false-positive
    // guard for user content that happens to match the
    // signal pattern).
    let env = NbTestEnv::new().expect("fixture initialization");
    let crafted = "0 items.\n\nJust some content\n";
    with_shim_nb_env(&env, crafted, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");
        let output = client
            .list_notes(None, &[], None, None)
            .await
            .expect("list");
        assert_eq!(
            output, crafted,
            "Missing recognized markers must result in input being returned unchanged"
        );
    })
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn list_no_blank_separator_returns_input_unchanged_via_shim() {
    // Signal with NO blank separator before content. The
    // helper must return input unchanged.
    let env = NbTestEnv::new().expect("fixture initialization");
    let crafted = "0 items.\nJust some content\n";
    with_shim_nb_env(&env, crafted, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");
        let output = client
            .list_notes(None, &[], None, None)
            .await
            .expect("list");
        assert_eq!(
            output, crafted,
            "Missing blank separator must result in input being returned unchanged"
        );
    })
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn folders_crlf_terminator_via_shim() {
    let env = NbTestEnv::new().expect("fixture initialization");
    let crafted = "0 folders.\r\n\r\nImport a file:\r\n  nb import scratch:\r\nHelp information:\r\n  nb help import\r\n";
    with_shim_nb_env(&env, crafted, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");
        let output = client.list_folders(None, None).await.expect("folders");
        assert_eq!(
            output, "0 folders.\r\n",
            "folders with CRLF terminator: signal\\r\\n preserved exactly"
        );
    })
    .await;
}

// Panic-restoration regression for the RAII `EnvSnapshot`
// lives in `tests/integration/common/mod.rs::env_snapshot_restores_path_and_shim_output_on_panic`.
// It exercises `EnvSnapshot::Drop` directly inside one locked
// critical section, avoiding the race that a previous variant
// (which read env vars before and after `with_shim_nb_env`
// outside `ENV_LOCK`) was subject to.
