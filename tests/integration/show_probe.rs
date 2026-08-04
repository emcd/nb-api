//! Regression tests for the `nb-api 0.2.0` show probe: native
//! textual-classification via `nb show <selector> --type text`,
//! with a new typed error variant for non-textual targets.
//!
//! `NbClient::show_note` probes the selector's classification via
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
/// The public `NbClient::add_note` API does not expose a `--type` flag,
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
            .add_note(Some("alpha"), "hello md", &[], None, None)
            .await
            .expect("add .md note");

        let output = client.show_note("1", None).await.expect("show .md note");
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

        let output = client.show_note("1", None).await.expect("show .txt note");
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

        let output = client.show_note("1", None).await.expect("show .org note");
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

        let output = client.show_note("1", None).await.expect("show .text note");
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
            .add_todo("task one", None, &[], &[], None, None)
            .await
            .expect("add todo");

        let output = client.show_note("1", None).await.expect("show todo");
        assert!(
            output.contains("task one"),
            "show output missing todo title: {output:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn show_accepts_source_data_markup_extensions() {
    // Source, data, and markup extensions (`json`, `py`, `rs`,
    // `yaml`, `csv`, etc.) are textual and must be accepted by
    // `show`. Classification via `nb show --type text` delegates
    // the "what is text" decision to `nb` itself, so the API
    // accepts whatever `nb` considers text.
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

        let json_out = client.show_note("1", None).await.expect("show .json");
        assert!(
            json_out.contains("\"k\":\"v\""),
            "json output: {json_out:?}"
        );

        let py_out = client.show_note("2", None).await.expect("show .py");
        assert!(py_out.contains("def f()"), "py output: {py_out:?}");

        let rs_out = client.show_note("3", None).await.expect("show .rs");
        assert!(rs_out.contains("fn main()"), "rs output: {rs_out:?}");

        let yaml_out = client.show_note("4", None).await.expect("show .yaml");
        assert!(yaml_out.contains("k: v"), "yaml output: {yaml_out:?}");

        let csv_out = client.show_note("5", None).await.expect("show .csv");
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

        let output = client.show_note("1", None).await.expect("show .MD");
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
            .show_note("extless", None)
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

        let result = client.show_note("1", None).await;
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

        let result = client.show_note("1", None).await;
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
            .add_folder("subfolder", None)
            .await
            .expect("create folder");

        let result = client.show_note("subfolder", None).await;
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

        let result = client.show_note("does-not-exist", None).await;
        match result {
            // Genuine `nb` selector
            // absence is now surfaced as typed `NbError::NotFound`,
            // not `CommandFailed`. The selector field carries the
            // original `<notebook>:id` selector string.
            Err(NbError::NotFound { selector }) => {
                assert!(
                    selector.contains("does-not-exist"),
                    "expected missing-selector diagnostic in NotFound, got: {selector:?}"
                );
            }
            Err(NbError::UnsupportedShowTarget { actual_type, .. }) => {
                panic!(
                    "probe failure must not be substituted with UnsupportedShowTarget; got actual_type={actual_type:?}"
                );
            }
            other => panic!("expected NotFound for missing selector, got: {other:?}"),
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
            .add_note(Some("alpha"), "alpha body", &[], None, None)
            .await
            .expect("add .md");
        add_note_with_type(&env, env.notebook(), "beta", "beta body", "txt");
        add_note_with_type(&env, env.notebook(), "gamma", "gamma body", "zip");

        let ok = client.show_note("1", None).await.expect("show .md");
        assert!(ok.contains("alpha body"));
        let ok = client.show_note("2", None).await.expect("show .txt");
        assert!(ok.contains("beta body"));

        let err = client
            .show_note("3", None)
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

// ---------- narrow error mapping for show_note / check_notebook ----------

/// `show_note` with a missing id surfaces typed `NotFound`. The
/// `selector` field carries the original resolved selector
/// (e.g., `home:does-not-exist`) verbatim — without any
/// decorative verb suffix added by the client. Under the
/// previous implementation the selector was rewritten
/// as `format!("{} show", selector)` (e.g.,
/// `home:does-not-exist show`), which leaked the subcommand
/// into the diagnostic.
#[tokio::test]
async fn show_note_missing_selector_notfound_carries_qualified_id_verbatim() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        let result = client.show_note("does-not-exist", None).await;
        match result {
            Err(NbError::NotFound { selector }) => {
                let expected = format!("{}:does-not-exist", env.notebook());
                assert_eq!(
                    selector, expected,
                    "NotFound.selector must carry the original \
                     `<notebook>:<id>` string verbatim; got: {selector:?}"
                );
                // The previous bug suffix " show" must NOT be
                // present under any conditions.
                assert!(
                    !selector.ends_with(" show"),
                    "NotFound.selector must not contain a decorative \
                     verb suffix; got: {selector:?}"
                );
            }
            other => panic!("expected NotFound for missing selector, got: {other:?}"),
        }
    })
    .await;
}

/// When `show_note` is called with an explicit notebook that
/// does not exist, the inner `check_notebook` must surface the
/// genuine notebook-absence diagnostic as a typed error rather
/// than swallow it via a broad case-insensitive substring match
/// or skip past it. With `create_notebook: false`, `ensure_notebook`
/// propagates the absence as `ValidationError` rather than
/// attempting creation; the test pins that surfacing.
#[tokio::test]
async fn show_note_nonexistent_notebook_returns_validation_error_when_create_disabled() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        // Pass an explicit notebook name that is not registered
        // with the test fixture; nb emits its pinned
        // "Notebook not found: <name>" diagnostic.
        let result = client
            .show_note("anything", Some("nonexistent-notebook-for-absence-mapping"))
            .await;
        match result {
            Err(NbError::ValidationError { reason, .. }) => {
                assert!(
                    reason.contains("not found"),
                    "expected validation error to mention absence; got: {reason:?}"
                );
                assert!(
                    reason.contains("nonexistent-notebook-for-absence-mapping"),
                    "expected validation error to mention the missing \
                     notebook name; got: {reason:?}"
                );
            }
            // Critical anti-regression assertions: under the
            // previous broad-substring classifier the path
            // could reach NotFound when the failure was a real
            // permission-denied; pin that we surface the typed
            // validation error instead.
            Err(NbError::NotFound { selector }) => panic!(
                "expected ValidationError for missing notebook, \
                 got NotFound with selector {selector:?}"
            ),
            Err(other) => panic!("expected ValidationError for missing notebook, got: {other:?}"),
            Ok(s) => panic!(
                "expected ValidationError for missing notebook, \
                 got Ok with content: {s:?}"
            ),
        }
    })
    .await;
}

/// Qualified-selector path (`<notebook>:<item>`) calls
/// `ensure_existing_notebook`, which propagates typed
/// `NbError::NotFound` verbatim rather than converting to
/// `ValidationError`. Notebook-field validation:
/// the typed `NotFound` was erased to `ValidationError` at the
/// qualified-selector boundary under the previous
/// implementation, losing the variant distinction.
#[tokio::test]
async fn show_note_qualified_selector_nonexistent_notebook_propagates_typed_not_found() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        // `<nonexistent-notebook>:any-item` exercises the
        // qualified-selector path: `parse_qualified_selector`
        // returns Some(("nonexistent-notebook", "any-item")),
        // and `ensure_existing_notebook("nonexistent-notebook")`
        // is called. Under the corrected rework, the typed
        // `NotFound` propagates to the caller as typed
        // NotFound, not as ValidationError or CommandFailed.
        let result = client
            .show_note("nonexistent-notebook-for-absence-mapping:item", None)
            .await;
        match result {
            Err(NbError::NotFound { selector }) => {
                // The selector carries the notebook name with a
                // trailing colon (the shape produced by
                // `check_notebook`'s notebook probe). It
                // identifies the missing notebook without
                // conflating with the path component of the
                // user's qualified selector.
                assert_eq!(
                    selector, "nonexistent-notebook-for-absence-mapping:",
                    "NotFound.selector must carry the missing notebook \
                     identifier with trailing colon (check_notebook's \
                     pinned shape)"
                );
            }
            Err(NbError::ValidationError { reason, .. }) => panic!(
                "qualified-selector path must propagate typed NotFound, \
                 not erase to ValidationError: got reason={reason:?}"
            ),
            Err(NbError::CommandFailed { stderr, .. }) => panic!(
                "qualified-selector path must map to typed NotFound, \
                 not surface raw CommandFailed: stderr={stderr:?}"
            ),
            Err(other) => panic!("expected typed NotFound for qualified selector, got: {other:?}"),
            Ok(_) => panic!("qualified-selector notebook absence must not return Ok"),
        }
    })
    .await;
}

// ---------- show_notebook_path successful-empty-output regression ----------
//
// Regression: when `show_notebook_path` succeeds with empty
// stdout, the synthesized `CommandFailed.command` must carry
// the actual argv (`nb notebooks show {notebook} --path`), not
// a rewritten display string such as
// `nb {notebook}:notebooks show --path`. Drives the public API
// through `with_shim_nb_env` returning empty stdout.
//
// Unix-only because `with_shim_nb_env` requires a Bash script
// + executable-bit chmod + `:`-separated PATH.

#[cfg(unix)]
use crate::common::with_shim_nb_env;

#[cfg(unix)]
#[tokio::test]
async fn show_notebook_path_successful_empty_output_command_field_is_actual_argv() {
    let env = NbTestEnv::new().expect("fixture initialization");
    // The shim emits zero bytes for `nb notebooks show
    // {notebook} --path`. `show_notebook_path` interprets this
    // as a successful invocation returning no path and synthesizes
    // a `CommandFailed` for the empty-output contract failure
    // path with the actual argv as the `command` field.
    let crafted = "";
    with_shim_nb_env(&env, crafted, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        let result = client.show_notebook_path(Some(env.notebook())).await;
        match result {
            Ok(_) => panic!(
                "show_notebook_path must return Err for empty stdout, not Ok; \
                 this is the empty-output contract failure path"
            ),
            Err(NbError::CommandFailed {
                command,
                stderr,
                exit_code,
                ..
            }) => {
                assert_eq!(
                    command,
                    format!("nb notebooks show {} --path", env.notebook()),
                    "successful-empty-output command field must match the actual \
                     argv `nb notebooks show {{notebook}} --path`, not the legacy \
                     display string `nb {{notebook}}:notebooks show --path`. \
                     successful-empty-output command-field regression."
                );
                assert_eq!(
                    stderr, "nb notebooks path output was empty",
                    "synthesized stderr must identify the empty-output cause"
                );
                assert_eq!(
                    exit_code, None,
                    "synthesized exit_code must be None (subprocess succeeded with empty output)"
                );
            }
            Err(other) => panic!(
                "expected NbError::CommandFailed for empty-output contract failure, got: {other:?}"
            ),
        }
    })
    .await;
}
