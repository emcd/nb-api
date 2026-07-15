//! Regression tests for the `nb-api 0.2.0` show probe: native
//! textual-classification via `nb show <selector> --type text`,
//! with a new typed error variant for non-textual targets.
//!
//! `NbClient::show` probes the selector's classification via
//! `nb show <selector> --type text` first; if `nb` reports the
//! type is not text, the method follows up with
//! `nb show <selector> --type` to recover the `actual_type` and
//! returns [`NbError::UnsupportedShowTarget`]. When both probes
//! fail (selector not found, internal error), the method falls
//! through to the original show path and returns its error or
//! output unchanged. The semantic check delegates "what is text"
//! to `nb` itself, so forward compatibility is automatic as `nb`
//! adds new textual types.
//!
//! See `nb-api:proposals/add-0-2-0-foundation/specifications/13`
//! (public-api-surface specification) and
//! `nb-api:proposals/add-0-2-0-foundation/designs/2` design note D4.

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient, NbError};

use crate::common::with_isolated_env;

/// Add a note with an explicit extension to the fixture's notebook.
///
/// The public `NbClient::add` API does not expose a `--type` flag,
/// but `nb add` accepts one. Tests that need an arbitrary extension
/// use the fixture's `nb_command()` to invoke `nb add` directly.
fn add_note_with_type(
    env: &NbTestEnv,
    notebook: &str,
    title: &str,
    content: &str,
    extension: &str,
) {
    let mut cmd = env.nb_command();
    cmd.arg("add")
        .arg(format!("{notebook}:"))
        .arg("--title")
        .arg(title)
        .arg("--content")
        .arg(content)
        .arg("--type")
        .arg(extension);
    let output = cmd.output().expect("spawn nb add --type");
    assert!(
        output.status.success(),
        "nb add --type {extension} failed: status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[tokio::test]
async fn show_accepts_md_extension() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        client
            .add(Some("alpha"), "hello md", &[], None, None)
            .await
            .expect("add .md note");

        let output = client.show("1", None).await.expect("show .md note");
        assert!(
            output.contains("hello md"),
            "show output missing content: {output:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn show_accepts_txt_extension() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        add_note_with_type(&env, env.notebook(), "beta", "hello txt", "txt");

        let output = client.show("1", None).await.expect("show .txt note");
        assert!(
            output.contains("hello txt"),
            "show output missing content: {output:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn show_accepts_org_extension() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        add_note_with_type(&env, env.notebook(), "gamma", "hello org", "org");

        let output = client.show("1", None).await.expect("show .org note");
        assert!(
            output.contains("hello org"),
            "show output missing content: {output:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn show_accepts_text_extension() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        add_note_with_type(&env, env.notebook(), "delta", "hello text", "text");

        let output = client.show("1", None).await.expect("show .text note");
        assert!(
            output.contains("hello text"),
            "show output missing content: {output:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn show_accepts_todo_via_md_extension() {
    // `*.todo.md` todo files are textual regardless of whether
    // `nb show --type` reports `md` (last segment of the
    // multi-dot extension) or `todo.md` (full multi-dot
    // extension). The semantic `--type text` check is the
    // canonical classification; this test verifies the API
    // accepts todos without depending on the specific
    // `--type` reporting, which is `nb` version-dependent.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        client
            .todo("task one", None, &[], &[], None, None)
            .await
            .expect("add todo");

        let output = client.show("1", None).await.expect("show todo");
        assert!(
            output.contains("task one"),
            "show output missing todo title: {output:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn show_accepts_source_data_markup_extensions() {
    // Per MCP Owner's Phase 2 review feedback: source, data, and
    // markup extensions (`json`, `py`, `rs`, `yaml`, `csv`, etc.)
    // are textual and must be accepted by `show`. The semantic
    // classification via `nb show --type text` delegates the
    // "what is text" decision to `nb` itself, so the API
    // automatically accepts whatever `nb` considers text.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        add_note_with_type(&env, env.notebook(), "jsonnote", "{\"k\":\"v\"}", "json");
        add_note_with_type(&env, env.notebook(), "pynote", "def f(): pass", "py");
        add_note_with_type(&env, env.notebook(), "rsnote", "fn main() {}", "rs");
        add_note_with_type(&env, env.notebook(), "yamlnote", "k: v", "yaml");
        add_note_with_type(&env, env.notebook(), "csvnote", "a,b,c", "csv");

        let json_out = client.show("1", None).await.expect("show .json");
        assert!(
            json_out.contains("\"k\":\"v\""),
            "json output: {json_out:?}"
        );

        let py_out = client.show("2", None).await.expect("show .py");
        assert!(py_out.contains("def f()"), "py output: {py_out:?}");

        let rs_out = client.show("3", None).await.expect("show .rs");
        assert!(rs_out.contains("fn main()"), "rs output: {rs_out:?}");

        let yaml_out = client.show("4", None).await.expect("show .yaml");
        assert!(yaml_out.contains("k: v"), "yaml output: {yaml_out:?}");

        let csv_out = client.show("5", None).await.expect("show .csv");
        assert!(csv_out.contains("a,b,c"), "csv output: {csv_out:?}");
    })
    .await;
}

#[tokio::test]
async fn show_accepts_uppercase_extension_via_native_classification() {
    // `nb` preserves the original case of the file extension. An
    // uppercase extension (e.g. `.MD`) is still classified as
    // text by `nb show --type text`. The semantic check accepts
    // it without the API needing to maintain a case-normalized
    // whitelist.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        add_note_with_type(&env, env.notebook(), "uppermd", "uppercase content", "MD");

        let output = client.show("1", None).await.expect("show .MD");
        assert!(
            output.contains("uppercase content"),
            "show .MD output: {output:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn show_accepts_extensionless_file() {
    // `nb` treats extensionless files as text by default
    // (`nb show --type text` returns 0 for extensionless
    // selectors). The semantic check accepts them.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        // Write an extensionless file directly into the notebook
        // dir. `nb add --type` requires an extension, so direct
        // write is the canonical way to create an extensionless
        // item.
        let note_path = env.nb_dir().join(env.notebook()).join("extless");
        std::fs::write(&note_path, b"extensionless content\n").expect("write extensionless file");

        let output = client
            .show("extless", None)
            .await
            .expect("show extensionless");
        assert!(
            output.contains("extensionless content"),
            "show extless output: {output:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn show_rejects_audio_extension() {
    // Audio files are non-textual per `nb`'s classification.
    // The semantic check rejects them with `actual_type` equal
    // to the `nb`-reported extension. This is the canonical
    // "non-textual extension rejected" path; `.zip` is exercised
    // in a separate test for the same reason.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        add_note_with_type(&env, env.notebook(), "mp3file", "x", "mp3");

        let result = client.show("1", None).await;
        match result {
            Err(NbError::UnsupportedShowTarget { actual_type, .. }) => {
                assert_eq!(
                    actual_type, "mp3",
                    "expected actual_type=mp3, got {actual_type:?}"
                );
            }
            other => {
                panic!("expected UnsupportedShowTarget {{ actual_type: \"mp3\" }}, got: {other:?}")
            }
        }
    })
    .await;
}

#[tokio::test]
async fn show_rejects_zip_extension() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        add_note_with_type(&env, env.notebook(), "epsilon", "x", "zip");

        let result = client.show("1", None).await;
        match result {
            Err(NbError::UnsupportedShowTarget { actual_type, .. }) => {
                assert_eq!(
                    actual_type, "zip",
                    "expected actual_type=zip, got {actual_type:?}"
                );
            }
            other => {
                panic!("expected UnsupportedShowTarget {{ actual_type: \"zip\" }}, got: {other:?}")
            }
        }
    })
    .await;
}

#[tokio::test]
async fn show_rejects_folder_selector() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        client
            .mkdir("subfolder", None)
            .await
            .expect("create folder");

        let result = client.show("subfolder", None).await;
        match result {
            Err(NbError::UnsupportedShowTarget { actual_type, .. }) => {
                assert_eq!(
                    actual_type, "folder",
                    "expected actual_type=folder, got {actual_type:?}"
                );
            }
            other => panic!(
                "expected UnsupportedShowTarget {{ actual_type: \"folder\" }}, got: {other:?}"
            ),
        }
    })
    .await;
}

#[tokio::test]
async fn show_probe_failure_falls_through_to_command_failed() {
    // When the `--type` probe itself fails (selector not found),
    // the show method MUST fall through to the original read so
    // the existing missing-selector diagnostic is preserved. The
    // probe error MUST NOT be substituted with UnsupportedShowTarget.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        let result = client.show("does-not-exist", None).await;
        match result {
            Err(NbError::CommandFailed(message)) => {
                assert!(
                    message.contains("Not found") || message.contains("not found"),
                    "expected missing-selector diagnostic, got: {message:?}"
                );
            }
            Err(NbError::UnsupportedShowTarget { actual_type, .. }) => {
                panic!(
                    "probe failure must not be substituted with UnsupportedShowTarget; got actual_type={actual_type:?}"
                );
            }
            other => panic!("expected CommandFailed, got: {other:?}"),
        }
    })
    .await;
}

/// Sanity: a single sweep with mixed whitelist + non-whitelist
/// members confirms ordering and that probes on different items
/// share the same probe mechanism. The per-extension focused tests
/// above cover each whitelist member in isolation; this one
/// confirms the probe is cheap enough that adding many items does
/// not regress.
#[tokio::test]
async fn show_probe_sweep_over_mixed_items() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        client
            .add(Some("alpha"), "alpha body", &[], None, None)
            .await
            .expect("add .md");
        add_note_with_type(&env, env.notebook(), "beta", "beta body", "txt");
        add_note_with_type(&env, env.notebook(), "gamma", "gamma body", "zip");

        let ok = client.show("1", None).await.expect("show .md");
        assert!(ok.contains("alpha body"));
        let ok = client.show("2", None).await.expect("show .txt");
        assert!(ok.contains("beta body"));

        let err = client
            .show("3", None)
            .await
            .expect_err("show .zip must reject");
        match err {
            NbError::UnsupportedShowTarget { actual_type, .. } => {
                assert_eq!(actual_type, "zip");
            }
            other => panic!("expected UnsupportedShowTarget, got: {other:?}"),
        }
    })
    .await;
}
