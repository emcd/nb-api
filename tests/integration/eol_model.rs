//! 0.4.0 document-level EOL model: exact-byte scenarios from the
//! `line-eol-model` specification, exercised through the public client.

use nb_api::testing::NbTestEnv;
use nb_api::{BoundaryAt, Config, LineEdit, LineEol, LinePosition, LineRef, NbClient, NoteTarget};

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

async fn add_body(client: &NbClient, name: &str, body: &str) {
    let mut tx = client.transaction(None).await.expect("tx");
    tx.add_note(name, None, body, &[]).unwrap();
    tx.commit().await.expect("commit");
}

fn line_ref(line: &nb_api::NoteLine) -> LineRef {
    LineRef {
        number: line.number,
        anchor: line.anchor.clone(),
    }
}

#[tokio::test]
async fn lf_body_reports_document_eol() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        add_body(&client, "eol.md", "a\nb\n").await;
        let lines = client
            .show_note_lines(NoteTarget::path("eol.md"), Some(1), Some(10), None)
            .await
            .expect("lines");
        assert_eq!(lines.eol, Some(LineEol::Lf));
        assert!(lines.has_final_eol);
        assert_eq!(lines.total_lines, 2);
        assert_eq!(lines.lines[0].text, "a");
        assert_eq!(lines.lines[1].text, "b");
    })
    .await;
}

#[tokio::test]
async fn cr_only_body_is_single_line_with_preserved_cr() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        add_body(&client, "cr.md", "a\rb\rc").await;
        let lines = client
            .show_note_lines(NoteTarget::path("cr.md"), Some(1), Some(10), None)
            .await
            .expect("lines");
        assert_eq!(lines.eol, None);
        assert!(!lines.has_final_eol);
        assert_eq!(lines.total_lines, 1);
        assert_eq!(lines.lines[0].text, "a\rb\rc");
        assert_eq!(
            lines.lines[0].anchor,
            nb_api::LineAnchor::from_line_bytes(b"a\rb\rc\x00")
        );
        // Read-only round-trip stays faithful.
        let raw = client
            .read_note_source_bytes(NoteTarget::path("cr.md"), None)
            .await
            .expect("hatch");
        assert!(raw.windows(5).any(|w| w == b"a\rb\rc"), "{raw:?}");
    })
    .await;
}

#[tokio::test]
async fn stray_cr_before_first_supported_eol_preserved() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        add_body(&client, "stray.md", "a\rb\nc\n").await;
        let lines = client
            .show_note_lines(NoteTarget::path("stray.md"), Some(1), Some(10), None)
            .await
            .expect("lines");
        assert_eq!(lines.eol, Some(LineEol::Lf));
        assert!(lines.has_final_eol);
        assert_eq!(lines.total_lines, 2);
        assert_eq!(lines.lines[0].text, "a\rb");
        assert_eq!(lines.lines[1].text, "c");
    })
    .await;
}

#[tokio::test]
async fn mixed_body_uses_first_supported_eol() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        add_body(&client, "mixed.md", "a\nb\r\nc\n").await;
        let lines = client
            .show_note_lines(NoteTarget::path("mixed.md"), Some(1), Some(10), None)
            .await
            .expect("lines");
        assert_eq!(lines.eol, Some(LineEol::Lf));
        assert!(
            lines.lines[1].text.contains('\r'),
            "{:?}",
            lines.lines[1].text
        );
    })
    .await;
}

#[tokio::test]
async fn final_line_without_eol_uses_null_marker() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        add_body(&client, "noeol.md", "a\nb").await;
        let lines = client
            .show_note_lines(NoteTarget::path("noeol.md"), Some(1), Some(10), None)
            .await
            .expect("lines");
        assert_eq!(lines.eol, Some(LineEol::Lf));
        assert!(!lines.has_final_eol);
        assert_eq!(
            lines.lines[1].anchor,
            nb_api::LineAnchor::from_line_bytes(b"b\x00")
        );
    })
    .await;
}

#[tokio::test]
async fn insert_after_on_cr_only_adopts_lf() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        add_body(&client, "adopt.md", "a\rb\rc").await;
        let lines = client
            .show_note_lines(NoteTarget::path("adopt.md"), Some(1), Some(10), None)
            .await
            .expect("lines");
        assert_eq!(lines.eol, None);
        client
            .edit_note_lines(
                NoteTarget::path("adopt.md"),
                vec![LineEdit::Insert {
                    at: LinePosition::After {
                        line: line_ref(&lines.lines[0]),
                    },
                    content: "x".to_string(),
                }],
                None,
            )
            .await
            .expect("insert");
        let shown = client.show_note("adopt.md", None).await.expect("show");
        assert!(shown.body.contains("a\rb\rc\nx\n"), "{:?}", shown.body);
        let lines = client
            .show_note_lines(NoteTarget::path("adopt.md"), Some(1), Some(10), None)
            .await
            .expect("lines2");
        assert_eq!(lines.eol, Some(LineEol::Lf));
        assert!(lines.has_final_eol);
    })
    .await;
}

#[tokio::test]
async fn insert_at_dollar_without_final_eol_normalizes() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        add_body(&client, "dollar.md", "a\nb").await;
        let before = client
            .show_note_lines(NoteTarget::path("dollar.md"), Some(1), Some(10), None)
            .await
            .expect("lines");
        let old_anchor = before.lines[1].anchor.clone();
        client
            .edit_note_lines(
                NoteTarget::path("dollar.md"),
                vec![LineEdit::Insert {
                    at: LinePosition::Boundary {
                        at: BoundaryAt::Dollar,
                    },
                    content: "c".to_string(),
                }],
                None,
            )
            .await
            .expect("insert");
        let shown = client.show_note("dollar.md", None).await.expect("show");
        assert_eq!(shown.body, "a\nb\nc\n");
        let after = client
            .show_note_lines(NoteTarget::path("dollar.md"), Some(1), Some(10), None)
            .await
            .expect("lines2");
        assert!(after.has_final_eol);
        // Previous final line gained a terminator: anchor changed b\0 -> b\n.
        assert_ne!(after.lines[1].anchor, old_anchor);
        assert_eq!(
            after.lines[1].anchor,
            nb_api::LineAnchor::from_line_bytes(b"b\n")
        );
        assert_eq!(
            after.lines[2].anchor,
            nb_api::LineAnchor::from_line_bytes(b"c\n")
        );
    })
    .await;
}

#[tokio::test]
async fn replace_final_line_without_eol_normalizes() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        add_body(&client, "repl.md", "a\nb").await;
        let lines = client
            .show_note_lines(NoteTarget::path("repl.md"), Some(1), Some(10), None)
            .await
            .expect("lines");
        client
            .edit_note_lines(
                NoteTarget::path("repl.md"),
                vec![LineEdit::Replace {
                    start: line_ref(&lines.lines[1]),
                    end: line_ref(&lines.lines[1]),
                    content: "c".to_string(),
                }],
                None,
            )
            .await
            .expect("replace");
        let shown = client.show_note("repl.md", None).await.expect("show");
        assert_eq!(shown.body, "a\nc\n");
    })
    .await;
}

#[tokio::test]
async fn insert_appends_document_crlf() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        add_body(&client, "crlf.md", "a\r\nb\r\n").await;
        let lines = client
            .show_note_lines(NoteTarget::path("crlf.md"), Some(1), Some(10), None)
            .await
            .expect("lines");
        assert_eq!(lines.eol, Some(LineEol::CrLf));
        client
            .edit_note_lines(
                NoteTarget::path("crlf.md"),
                vec![LineEdit::Insert {
                    at: LinePosition::After {
                        line: line_ref(&lines.lines[0]),
                    },
                    content: "x".to_string(),
                }],
                None,
            )
            .await
            .expect("insert");
        let shown = client.show_note("crlf.md", None).await.expect("show");
        assert_eq!(shown.body, "a\r\nx\r\nb\r\n");
    })
    .await;
}

#[tokio::test]
async fn insert_at_caret_into_empty_body() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        add_body(&client, "empty.md", "").await;
        client
            .edit_note_lines(
                NoteTarget::path("empty.md"),
                vec![LineEdit::Insert {
                    at: LinePosition::Boundary {
                        at: BoundaryAt::Caret,
                    },
                    content: "x".to_string(),
                }],
                None,
            )
            .await
            .expect("insert");
        let shown = client.show_note("empty.md", None).await.expect("show");
        assert_eq!(shown.body, "x\n");
        // Dollar into an empty body is rejected: Caret is the only valid boundary.
        add_body(&client, "empty2.md", "").await;
        let err = client
            .edit_note_lines(
                NoteTarget::path("empty2.md"),
                vec![LineEdit::Insert {
                    at: LinePosition::Boundary {
                        at: BoundaryAt::Dollar,
                    },
                    content: "x".to_string(),
                }],
                None,
            )
            .await
            .expect_err("dollar on empty");
        assert!(
            matches!(err, nb_api::NbError::ValidationError { .. })
                || matches!(err, nb_api::NbError::PlanValidation { .. }),
            "{err:?}"
        );
    })
    .await;
}
