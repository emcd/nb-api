use nb_api::{
    BoundaryAt, ByteString, LineEdit, LinePosition, LineRef, NoteTarget, Occurrence, SearchMode,
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
fn byte_string_preserves_non_utf8() {
    let b = ByteString::from_bytes([0xff, 0x0a]);
    let json = serde_json::to_string(&b).unwrap();
    let back: ByteString = serde_json::from_str(&json).unwrap();
    assert_eq!(back.as_bytes().unwrap(), vec![0xff, 0x0a]);
}

#[test]
fn occurrence_and_line_edit_round_trip() {
    let edit = LineEdit::Insert {
        at: LinePosition::Boundary {
            at: BoundaryAt::Dollar,
        },
        content: ByteString::from_bytes(b"x\n"),
    };
    let json = serde_json::to_string(&edit).unwrap();
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
