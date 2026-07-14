//! Hermetic regression test for the `nb show --print` fix carried in
//! from `nb-api 0.1.2`. Without `--print`, `nb show` pipes its output
//! through the renderer/pager which word-wraps at ~80 columns,
//! silently corrupting any stored line longer than that. With
//! `--print`, the file is returned verbatim.
//!
//! See `nb-api:issues/2`.
//!
//! No project repository config used by this reproduction; the
//! `NbTestEnv` fixture provides an isolated `NB_DIR`.

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient};

use crate::common::with_isolated_env;

/// A long unbroken line that, if word-wrapped, would not appear
/// verbatim in the show output. 500 'x' chars exceeds any reasonable
/// wrap width and contains no whitespace to wrap at.
const LONG_LINE: &str = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";

#[tokio::test]
async fn show_preserves_long_unbroken_line_verbatim() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            // The fixture's notebook has no folders; permit a root
            // note for this test rather than threading a folder
            // through `add(...)`.
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        client
            .add(Some("long-line"), LONG_LINE, &[], None, None)
            .await
            .expect("add note");

        let output = client.show("1", None).await.expect("show note");
        assert!(
            output.contains(LONG_LINE),
            "show output did not contain the long line verbatim; \
             --print regression. expected len={} got output len={}",
            LONG_LINE.len(),
            output.len()
        );
    })
    .await;
}
