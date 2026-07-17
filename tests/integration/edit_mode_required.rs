//! Regression tests for Phase 5 of `add-0-2-0-foundation`:
//! the vocabulary-trap fix on `NbClient::edit_note`.
//!
//! The destructive-incident root cause tracked at
//! `nb-api:issues/api/6` / `nb-mcp-server:issues/mcp/6` was
//! callers reading `mode: "replace"` and reasonably expecting a
//! substring-style replacement (analogous to
//! [`str::replace`]). The `EditMode` variant
//! previously named `Replace` was renamed to `Overwrite` so the
//! destructive intent is unambiguous at the call site. The
//! legacy string `"replace"` is accepted as a serde alias for
//! backward compatibility with payloads produced before the
//! rename.
//!
//! Phase 5 is the **API-layer** defense. The MCP-layer defense
//! (dropping `#[serde(default)] mode` from the consumer's
//! `EditArgs` struct so missing `mode` is a schema error) is the
//! downstream consumer's responsibility, tracked separately.
//!
//! See `nb-api:proposals/add-0-2-0-foundation/specifications/13`
//! (public API surface) and design note D5.

use nb_api::testing::NbTestEnv;
use nb_api::{Config, EditMode, NbClient};

use crate::common::with_isolated_env;

fn config_for(env: &NbTestEnv) -> Config {
    Config {
        notebook: Some(env.notebook().to_string()),
        create_notebook: false,
        allow_top_level_notes: true,
        ..Config::default()
    }
}

/// Compile-time check: the `edit` signature accepts each
/// `EditMode` variant explicitly. If `mode` were ever made
/// optional in the future, these would still compile (with
/// `mode` defaulted), which would be a regression of the
/// API-layer vocabulary contract — but the structural defense
/// is the variant name itself: `Overwrite` makes the
/// destructivity unambiguous. Both the variant name and the
/// canonical serialization (`"overwrite"`) are what catch the
/// vocabulary-trap regression.
#[allow(dead_code)]
fn _compile_time_check_edit_signature_compiles(client: &NbClient) {
    core::mem::drop(client.edit_note("id", "content", EditMode::Overwrite, None));
    core::mem::drop(client.edit_note("id", "content", EditMode::Append, None));
    core::mem::drop(client.edit_note("id", "content", EditMode::Prepend, None));
}

// --------------------------------------------------------------------
// Behavioral tests: edit with each mode produces distinct output.
// All tests use `add(None, ...)` so no `# title\n\n` header is added.
// --------------------------------------------------------------------

#[tokio::test]
async fn edit_overwrite_destroys_existing_body() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        client
            .add_note(
                None,
                "original content that should be gone\n",
                &[],
                None,
                None,
            )
            .await
            .expect("add");

        client
            .edit_note("1", "fresh body\n", EditMode::Overwrite, None)
            .await
            .expect("edit overwrite");

        let output = client.show_note("1", None).await.expect("show");
        assert!(
            !output.contains("original content"),
            "Overwrite must destroy original body; got:\n{output:?}"
        );
        assert!(
            output.contains("fresh body"),
            "Overwrite must contain new body; got:\n{output:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn edit_append_preserves_and_extends_body() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        client
            .add_note(None, "first chunk\n", &[], None, None)
            .await
            .expect("add");

        client
            .edit_note("1", "second chunk\n", EditMode::Append, None)
            .await
            .expect("edit append");

        let output = client.show_note("1", None).await.expect("show");
        assert!(
            output.contains("first chunk"),
            "Append must preserve original body; got:\n{output:?}"
        );
        assert!(
            output.contains("second chunk"),
            "Append must include new content; got:\n{output:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn edit_prepend_preserves_and_prepends_body() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        client
            .add_note(None, "second chunk\n", &[], None, None)
            .await
            .expect("add");

        client
            .edit_note("1", "first chunk\n", EditMode::Prepend, None)
            .await
            .expect("edit prepend");

        let output = client.show_note("1", None).await.expect("show");
        assert!(
            output.contains("first chunk"),
            "Prepend must include new content; got:\n{output:?}"
        );
        assert!(
            output.contains("second chunk"),
            "Prepend must preserve original body; got:\n{output:?}"
        );
        let first_idx = output.find("first chunk").expect("find first");
        let second_idx = output.find("second chunk").expect("find second");
        assert!(
            first_idx < second_idx,
            "Prepend must place new content before original body; got:\n{output:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn edit_overwrite_with_single_byte_body() {
    // Boundary: overwrite a multi-line body with a single-byte body.
    // Verifies Overwrite truly replaces every byte (not just the
    // first/last line).
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        client
            .add_note(None, "alpha\nbeta\ngamma\ndelta\n", &[], None, None)
            .await
            .expect("add");

        client
            .edit_note("1", "x", EditMode::Overwrite, None)
            .await
            .expect("edit overwrite");

        let output = client.show_note("1", None).await.expect("show");
        assert!(
            !output.contains("alpha")
                && !output.contains("beta")
                && !output.contains("gamma")
                && !output.contains("delta"),
            "Overwrite with single byte must destroy all original lines; got:\n{output:?}"
        );
        assert!(
            output.contains("x"),
            "Overwrite must contain new single byte; got:\n{output:?}"
        );
    })
    .await;
}
