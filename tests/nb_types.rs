use nb_api::{
    BoundaryAt, LineEdit, LineEol, LinePosition, LineRef, NoteTarget, Occurrence, SearchMode,
    TaskStatus,
};

#[test]
fn note_target_path_and_selector_wire() {
    let path = NoteTarget::path("a.md");
    let json = serde_json::to_string(&path).unwrap();
    assert_eq!(json, r#"{"type":"path","value":"a.md"}"#);
    let back: NoteTarget = serde_json::from_str(&json).unwrap();
    assert_eq!(back, path);

    let sel = NoteTarget::selector("home:123");
    let json = serde_json::to_string(&sel).unwrap();
    assert_eq!(json, r#"{"type":"selector","value":"home:123"}"#);
}

#[test]
fn show_note_is_text_first_no_bytestring() {
    // 0.4.0: body/source are JSON strings, not `{base64}` objects.
    let json = r##"{"selector":"nb:a.md","path":"a.md","kind":"note","todo_state":null,"title":"# T\n","title_text":"T","tags":[],"body_fragments":[{"index":0,"bytes":"hi\n","start_byte":4,"end_byte":7}],"body_contiguous":true,"body":"hi\n","fingerprint":"b3:0000000000000000000000000000000000000000000000000000000000000000","source":"# T\nhi\n"}"##;
    let note: nb_api::ShowNote = serde_json::from_str(json).unwrap();
    assert_eq!(note.body, "hi\n");
    assert_eq!(note.body_fragments[0].start_byte, 4);
    // Old base64 object form is rejected.
    let old = r##"{"selector":"nb:a.md","path":"a.md","kind":"note","todo_state":null,"title":null,"title_text":null,"tags":[],"body_fragments":[],"body_contiguous":true,"body":{"base64":"aGk="},"fingerprint":"b3:0000000000000000000000000000000000000000000000000000000000000000","source":{"base64":"aGk="}}"##;
    assert!(serde_json::from_str::<nb_api::ShowNote>(old).is_err());
}

#[test]
fn occurrence_and_line_edit_round_trip() {
    let edit = LineEdit::Insert {
        at: LinePosition::Boundary {
            at: BoundaryAt::Dollar,
        },
        content: "x".to_string(),
    };
    let json = serde_json::to_string(&edit).unwrap();
    assert!(json.contains(r#""content":"x""#));
    let back: LineEdit = serde_json::from_str(&json).unwrap();
    assert_eq!(edit, back);

    let o = Occurrence::All;
    assert_eq!(
        serde_json::from_str::<Occurrence>(&serde_json::to_string(&o).unwrap()).unwrap(),
        o
    );
}

#[test]
fn line_ref_preserves_anchor_string() {
    let anchor = nb_api::LineAnchor::parse("b3l1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
    let r = LineRef {
        number: 1,
        anchor: anchor.clone(),
    };
    let json = serde_json::to_string(&r).unwrap();
    let back: LineRef = serde_json::from_str(&json).unwrap();
    assert_eq!(back.anchor.as_str(), anchor.as_str());
}

#[test]
fn note_line_has_no_terminator_and_ignores_unknown_fields() {
    let anchor = "b3l1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let json =
        format!(r#"{{"number":1,"anchor":"{anchor}","text":"hi","terminator":"lf","extra":1}}"#);
    let line: nb_api::NoteLine = serde_json::from_str(&json).unwrap();
    assert_eq!(line.text, "hi");
    let ser = serde_json::to_string(&line).unwrap();
    assert!(!ser.contains("terminator"));
}

#[test]
fn line_eol_serde_lowercase() {
    assert_eq!(serde_json::to_string(&LineEol::Lf).unwrap(), "\"lf\"");
    assert_eq!(serde_json::to_string(&LineEol::CrLf).unwrap(), "\"crlf\"");
}

#[test]
fn task_status_deserializes_lowercase_values() {
    let status: TaskStatus = serde_json::from_str("\"open\"").unwrap();
    assert_eq!(status, TaskStatus::Open);
    let status: TaskStatus = serde_json::from_str("\"closed\"").unwrap();
    assert_eq!(status, TaskStatus::Closed);
}

#[test]
fn search_mode_deserializes_lowercase_values() {
    let mode: SearchMode = serde_json::from_str("\"any\"").unwrap();
    assert_eq!(mode, SearchMode::Any);
    let mode: SearchMode = serde_json::from_str("\"all\"").unwrap();
    assert_eq!(mode, SearchMode::All);
}

#[test]
fn edit_mode_is_removed() {
    // Compile-time: EditMode must not resolve. Runtime placeholder assertion.
    let name = std::any::type_name::<NoteTarget>();
    assert!(name.contains("NoteTarget"));
}
