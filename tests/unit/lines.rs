use nb_api::{BoundaryAt, LineEdit, LineEol, LinePosition, LineRef, Occurrence};

// Direct unit coverage of line split/anchor via show path is in integration tests.
// Here we only lock wire type serde for line edits.

#[test]
fn line_edit_delete_round_trip() {
    let edit = LineEdit::Delete {
        start: LineRef {
            number: 1,
            anchor: nb_api::LineAnchor::parse("b3l1:0123456789abcdef0123456789abcdef").unwrap(),
        },
        end: LineRef {
            number: 2,
            anchor: nb_api::LineAnchor::parse("b3l1:0123456789abcdef0123456789abcdef").unwrap(),
        },
    };
    let json = serde_json::to_string(&edit).unwrap();
    let back: LineEdit = serde_json::from_str(&json).unwrap();
    assert_eq!(edit, back);
}

#[test]
fn line_position_boundary_wire() {
    let pos = LinePosition::Boundary {
        at: BoundaryAt::Caret,
    };
    let json = serde_json::to_string(&pos).unwrap();
    assert!(json.contains("\"type\":\"boundary\""));
    assert!(json.contains("\"at\":\"caret\""));
}

#[test]
fn occurrence_wire() {
    let o = Occurrence::Nth { n: 2 };
    let json = serde_json::to_string(&o).unwrap();
    let back: Occurrence = serde_json::from_str(&json).unwrap();
    assert_eq!(o, back);
}

#[test]
fn line_edit_content_is_bare_string() {
    let edit = LineEdit::Insert {
        at: LinePosition::Boundary {
            at: BoundaryAt::Caret,
        },
        content: "x".to_string(),
    };
    let json = serde_json::to_string(&edit).unwrap();
    assert!(json.contains(r#""content":"x""#));
    let back: LineEdit = serde_json::from_str(&json).unwrap();
    assert_eq!(edit, back);
}

#[test]
fn line_eol_serde() {
    assert_eq!(serde_json::to_string(&LineEol::CrLf).unwrap(), "\"crlf\"");
    assert_eq!(serde_json::to_string(&LineEol::Lf).unwrap(), "\"lf\"");
}
