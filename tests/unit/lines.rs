use nb_api::{BoundaryAt, ByteString, LineEdit, LinePosition, LineRef, LineTerminator, Occurrence};

// Re-export internal helpers via integration-style pure logic tests using public types only
// by exercising through public APIs would need a notebook. These unit tests live beside
// the lines module by including the same algorithms — prefer public surface.

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
fn byte_string_round_trip() {
    let b = ByteString::from_bytes([0xff, 0x0a]);
    let json = serde_json::to_string(&b).unwrap();
    let back: ByteString = serde_json::from_str(&json).unwrap();
    assert_eq!(back.as_bytes().unwrap(), vec![0xff, 0x0a]);
}

#[test]
fn line_terminator_serde() {
    assert_eq!(
        serde_json::to_string(&LineTerminator::Crlf).unwrap(),
        "\"crlf\""
    );
}
