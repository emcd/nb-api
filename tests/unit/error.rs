//! Unit tests for `NbError` display rendering.
//!
//! Covers the `CommandFailed` exit-code formatting: a present code
//! renders as a bare number, an absent code as `unknown` — never
//! the `Option` `Debug` shape (`Some(1)`).

use nb_api::NbError;

#[test]
fn command_failed_renders_present_exit_code_bare() {
    let err = NbError::CommandFailed {
        command: "nb search".to_string(),
        stderr: "! Not found".to_string(),
        exit_code: Some(1),
    };
    let text = err.to_string();
    assert!(
        text.contains("(exit 1):"),
        "present code renders bare: {text:?}"
    );
    assert!(!text.contains("Some("), "no Option Debug leak: {text:?}");
}

#[test]
fn command_failed_renders_missing_exit_code_unknown() {
    let err = NbError::CommandFailed {
        command: "nb search".to_string(),
        stderr: "! Not found".to_string(),
        exit_code: None,
    };
    let text = err.to_string();
    assert!(
        text.contains("(exit unknown):"),
        "absent code renders unknown: {text:?}"
    );
    assert!(!text.contains("None"), "no Option Debug leak: {text:?}");
}
