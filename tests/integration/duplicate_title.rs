//! Regression tests for the `nb-api 0.2.0` duplicate-title H1
//! rejection in `NbClient::add`.
//!
//! `NbClient::add` inspects the first nonblank line of `content`
//! and rejects when it is a CommonMark ATX H1 (per the
//! public-api-surface specification grammar: 0-3 leading spaces,
//! exactly one opening `#`, required space/tab/EOL delimiter,
//! optional closing-hash sequence preceded by space/tab and
//! followed only by space/tab to EOL) whose trimmed heading text
//! equals the trimmed title. The validation runs in the caller
//! process before any subprocess invocation or notebook side
//! effect (including `resolve_notebook`); no `nb` side effect can
//! result from a rejection.
//!
//! See `nb-api:proposals/add-0-2-0-foundation/specifications/13`
//! (public-api-surface specification, "add_note SHALL reject
//! duplicate title H1 in note body" and "Validation runs before
//! any state-mutating call") and
//! `nb-api:proposals/add-0-2-0-foundation/designs/2` design note D5.

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient, NbError};

use crate::common::with_isolated_env;

/// Build a `Config` that allows top-level notes (the fixture's
/// notebook has no folders). `create_notebook: false` keeps tests
/// pinned to the fixture-owned notebook.
fn config_for(env: &NbTestEnv) -> Config {
    Config {
        notebook: Some(env.notebook().to_string()),
        create_notebook: false,
        allow_top_level_notes: true,
        ..Config::default()
    }
}

#[tokio::test]
async fn add_rejects_exact_duplicate_h1() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let result = client
            .add(
                Some("Title"),
                "# Title\n\nbody content here\n",
                &[],
                None,
                None,
            )
            .await;

        match result {
            Err(NbError::DuplicateTitleHeading { title, heading }) => {
                assert_eq!(title, "Title", "title should be the user-supplied value");
                assert_eq!(
                    heading, "# Title",
                    "heading should be the exact source line"
                );
            }
            other => panic!("expected DuplicateTitleHeading, got: {other:?}"),
        }
    })
    .await;
}

#[tokio::test]
async fn add_rejects_duplicate_h1_with_leading_blank_lines() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let result = client
            .add(Some("Title"), "\n\n\n# Title\n\nbody\n", &[], None, None)
            .await;

        match result {
            Err(NbError::DuplicateTitleHeading { title, heading }) => {
                assert_eq!(title, "Title");
                assert_eq!(heading, "# Title");
            }
            other => panic!("expected DuplicateTitleHeading, got: {other:?}"),
        }
    })
    .await;
}

#[tokio::test]
async fn add_allows_different_h1_text() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let result = client
            .add(
                Some("Title"),
                "# Different Title\n\nbody\n",
                &[],
                None,
                None,
            )
            .await;

        result.expect("different H1 text should not trigger rejection");
    })
    .await;
}

#[tokio::test]
async fn add_allows_lower_level_headings() {
    // Only ATX H1 triggers rejection. `## Title`, `### Title`,
    // etc. are H2/H3/... and are allowed even when the heading
    // text matches the title. Per CommonMark, H1 requires
    // exactly one opening `#` with a valid whitespace or EOL
    // delimiter.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        for level in 2..=6 {
            let prefix = "#".repeat(level) + " ";
            let content = format!("{prefix}Title\n\nbody\n");
            let result = client.add(Some("Title"), &content, &[], None, None).await;
            result.unwrap_or_else(|err| {
                panic!("H{level} with matching text should not trigger rejection: {err:?}")
            });
        }
    })
    .await;
}

#[tokio::test]
async fn add_allows_content_without_h1() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let result = client
            .add(
                Some("Title"),
                "regular body content, no markdown heading at all\n",
                &[],
                None,
                None,
            )
            .await;

        result.expect("content without H1 should not trigger rejection");
    })
    .await;
}

#[tokio::test]
async fn add_allows_no_title_with_h1_in_content() {
    // When title is None, the duplicate-title validation is
    // skipped entirely. A `# Title` first line in `content` is
    // allowed because the user explicitly opted out of title
    // metadata.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let result = client.add(None, "# Title\n\nbody\n", &[], None, None).await;

        result.expect("no title should skip duplicate-title validation");
    })
    .await;
}

#[tokio::test]
async fn add_validation_skips_on_empty_title() {
    // Empty title is treated the same as None — the validation
    // skips entirely. `nb` CLI itself rejects `--title ""` with
    // a CommandFailed error (the spec is about OUR validation,
    // not nb's), so we assert the result is NOT
    // `DuplicateTitleHeading`. The exact error is `nb`'s
    // responsibility.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let result = client
            .add(Some(""), "# Title\n\nbody\n", &[], None, None)
            .await;

        match result {
            Err(NbError::DuplicateTitleHeading { .. }) => {
                panic!("empty title should skip our validation, not trigger DuplicateTitleHeading");
            }
            _ => {
                // Anything else is fine: `nb` may reject the
                // empty title (CommandFailed), or the call may
                // succeed; what matters is that OUR validation
                // did not fire.
            }
        }
    })
    .await;
}

#[tokio::test]
async fn add_allows_whitespace_only_title_with_h1_in_content() {
    // A title of only whitespace is treated as empty (trimmed
    // empty). The validation is skipped entirely.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let result = client
            .add(Some("   \t  "), "# Title\n\nbody\n", &[], None, None)
            .await;

        result.expect("whitespace-only title should skip duplicate-title validation");
    })
    .await;
}

#[tokio::test]
async fn add_allows_case_different_h1() {
    // Spec is "trimmed heading text equals trimmed title"; the
    // comparison is byte-exact. `title="Title"` and content
    // `# title` (lowercase) are different by case, so allowed
    // (no fuzzy match per spec).
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let result = client
            .add(Some("Title"), "# title\n\nbody\n", &[], None, None)
            .await;
        result.expect("case-different H1 text should not trigger rejection");
    })
    .await;
}

#[tokio::test]
async fn add_carries_verbatim_title_in_duplicate_error() {
    // The spec is "trimmed heading text equals trimmed title":
    // the comparison is on the TRIMMED values, so a title of
    // `"  Title  "` (with surrounding whitespace) and content
    // `# Title` (no leading whitespace) ARE equal after trim and
    // trigger rejection. The `title` field on the error variant
    // carries the user-supplied value VERBATIM (with whitespace
    // preserved), so the consumer can match the rejected input
    // exactly. This is the spec contract for
    // `DuplicateTitleHeading { title, heading }`:
    // `title` is the user-supplied value, `heading` is the
    // exact source line.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let result = client
            .add(Some("  Title  "), "# Title\n\nbody\n", &[], None, None)
            .await;
        match result {
            Err(NbError::DuplicateTitleHeading { title, heading }) => {
                assert_eq!(
                    title, "  Title  ",
                    "title carries the user-supplied value verbatim"
                );
                assert_eq!(heading, "# Title");
            }
            other => panic!("expected DuplicateTitleHeading, got: {other:?}"),
        }
    })
    .await;
}

#[tokio::test]
async fn add_rejects_h1_with_closing_hashes() {
    // Per CommonMark, ATX H1 allows an optional closing hash
    // sequence: a run of `#`s preceded by a space/tab and
    // followed only by spaces/tabs to EOL. `# Title #`,
    // `# Title ###`, and similar all render as H1 text "Title".
    // The strict implementation strips the closing hashes before
    // comparison, so any of these forms trigger rejection.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        for (label, content, expected_heading) in [
            ("single closing hash", "# Title #\n\nbody\n", "# Title #"),
            (
                "multi-hash closing sequence",
                "# Title ###\n\nbody\n",
                "# Title ###",
            ),
            (
                "closing hash with trailing spaces",
                "# Title #  \n\nbody\n",
                "# Title #  ",
            ),
            (
                "tab delimiter with closing hash",
                "#\tTitle #\n\nbody\n",
                "#\tTitle #",
            ),
        ] {
            let result = client.add(Some("Title"), content, &[], None, None).await;
            match result {
                Err(NbError::DuplicateTitleHeading { title, heading }) => {
                    assert_eq!(title, "Title", "{label}: title field");
                    assert_eq!(
                        heading, expected_heading,
                        "{label}: heading carries exact source line"
                    );
                }
                other => panic!("{label}: expected DuplicateTitleHeading, got: {other:?}"),
            }
        }
    })
    .await;
}

#[tokio::test]
async fn add_rejects_h1_with_2_leading_spaces() {
    // Per CommonMark, ATX H1 allows 0-3 leading spaces of
    // indentation. `  # Title` (2 spaces) IS an H1 with text
    // "Title". The helper counts leading spaces (must be < 4) and
    // treats the rest as the ATX heading.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let result = client
            .add(Some("Title"), "  # Title\n\nbody\n", &[], None, None)
            .await;

        match result {
            Err(NbError::DuplicateTitleHeading { title, heading }) => {
                assert_eq!(title, "Title");
                assert_eq!(
                    heading, "  # Title",
                    "heading carries the exact source line including indentation"
                );
            }
            other => panic!("expected DuplicateTitleHeading, got: {other:?}"),
        }
    })
    .await;
}

#[tokio::test]
async fn add_rejects_h1_with_3_leading_spaces() {
    // 3 leading spaces is the maximum allowed for ATX H1
    // indentation. `   # Title` IS an H1 with text "Title".
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let result = client
            .add(Some("Title"), "   # Title\n\nbody\n", &[], None, None)
            .await;

        match result {
            Err(NbError::DuplicateTitleHeading { title, heading }) => {
                assert_eq!(title, "Title");
                assert_eq!(heading, "   # Title");
            }
            other => panic!("expected DuplicateTitleHeading, got: {other:?}"),
        }
    })
    .await;
}

#[tokio::test]
async fn add_rejects_h1_with_tab_delimiter() {
    // Per CommonMark, the required character after the opening
    // hash is a space, tab, or end of line. `#\tTitle` IS an H1
    // with text "Title". The helper accepts both space and tab
    // as the delimiter.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let result = client
            .add(Some("Title"), "#\tTitle\n\nbody\n", &[], None, None)
            .await;

        match result {
            Err(NbError::DuplicateTitleHeading { title, heading }) => {
                assert_eq!(title, "Title");
                assert_eq!(heading, "#\tTitle");
            }
            other => panic!("expected DuplicateTitleHeading, got: {other:?}"),
        }
    })
    .await;
}

#[tokio::test]
async fn add_allows_4_space_indented_line() {
    // Per CommonMark, 4+ leading spaces is an indented code
    // block, NOT an ATX heading. `    # Title` is a code line
    // with the literal text `# Title`, not an H1.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let result = client
            .add(Some("Title"), "    # Title\n\nbody\n", &[], None, None)
            .await;

        result.expect("4-space indented line is a code block, not an H1; not flagged");
    })
    .await;
}

#[tokio::test]
async fn add_rejects_literal_trailing_hash() {
    // Per CommonMark, a trailing `#` with no preceding space/tab
    // is part of the heading text, NOT a closing sequence. So
    // `# C#` has heading text "C#" — the trailing `#` is literal.
    // If the title is "C#", this is a duplicate and triggers
    // rejection. The `heading` field carries the exact source
    // line, NOT a stripped version.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let result = client
            .add(Some("C#"), "# C#\n\nbody\n", &[], None, None)
            .await;

        match result {
            Err(NbError::DuplicateTitleHeading { title, heading }) => {
                assert_eq!(title, "C#");
                assert_eq!(
                    heading, "# C#",
                    "heading carries the exact source line, not stripped"
                );
            }
            other => panic!("expected DuplicateTitleHeading, got: {other:?}"),
        }
    })
    .await;
}

#[tokio::test]
async fn add_allows_blank_h1_with_closing_hash() {
    // Per CommonMark, a closing-hash sequence can consume the
    // entire post-delimiter body. `# #` parses as an H1 with
    // empty heading text and ` #` as the closing sequence. The
    // empty heading text never matches a non-empty title, so
    // this is NOT flagged as a duplicate even when the title
    // is `"#"` (the title is the literal hash, the heading
    // text is empty).
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let result = client
            .add(Some("#"), "# #\n\nbody\n", &[], None, None)
            .await;

        result.expect(
            "blank H1 (closing sequence consumes entire body) does not match non-empty title; \
             not flagged as a duplicate",
        );
    })
    .await;
}

#[tokio::test]
async fn add_allows_blank_h1_with_multi_closing_hashes() {
    // Same as above but with a multi-hash closing sequence.
    // `# ###` parses as an H1 with empty heading text and ` ###`
    // as the closing sequence. Empty != non-empty, no match.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let result = client
            .add(Some("#"), "# ###\n\nbody\n", &[], None, None)
            .await;

        result.expect("blank H1 with multi-hash closing sequence does not match non-empty title");
    })
    .await;
}

#[tokio::test]
async fn add_rejects_h1_with_closing_hash_when_heading_text_matches_title() {
    // Combined case: title contains a literal hash AND the H1
    // has a separate closing-hash sequence. `# C# #` parses as
    // heading text "C#" with ` #` as the closing sequence.
    // Title "C#" matches → reject. The `heading` field carries
    // the exact source line, NOT a stripped version.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let result = client
            .add(Some("C#"), "# C# #\n\nbody\n", &[], None, None)
            .await;

        match result {
            Err(NbError::DuplicateTitleHeading { title, heading }) => {
                assert_eq!(title, "C#");
                assert_eq!(
                    heading, "# C# #",
                    "heading carries the exact source line with closing hash preserved"
                );
            }
            other => panic!("expected DuplicateTitleHeading, got: {other:?}"),
        }
    })
    .await;
}

#[tokio::test]
async fn add_allows_h1_without_space() {
    // `#Title` (no delimiter between hash and text) is not a
    // valid CommonMark ATX H1. The opening `#` must be followed
    // by a space, tab, or end of line.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client construction");

        let result = client
            .add(Some("Title"), "#Title\n\nbody\n", &[], None, None)
            .await;

        result.expect("#Title (no space) is not a valid ATX H1; not flagged");
    })
    .await;
}

#[tokio::test]
async fn add_validation_runs_before_resolve_notebook() {
    // The "Validation runs before any state-mutating call"
    // requirement: when `add` is called with title+content that
    // triggers duplicate-title rejection, no subprocess SHALL be
    // invoked and no notebook SHALL be created or modified —
    // including `resolve_notebook`. We test this by passing a
    // notebook that does NOT exist with `create_notebook: false`:
    // the only way the rejection can fire (instead of
    // `CommandFailed("notebook not found")`) is if the validation
    // runs first.
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&Config {
            notebook: Some("does-not-exist".to_string()),
            create_notebook: false,
            allow_top_level_notes: true,
            ..Config::default()
        })
        .expect("client construction");

        let result = client
            .add(
                Some("Title"),
                "# Title\n\nbody\n",
                &[],
                None,
                Some("does-not-exist"),
            )
            .await;

        match result {
            Err(NbError::DuplicateTitleHeading { .. }) => {
                // Validation fired before resolve_notebook; the
                // non-existent notebook never triggered the
                // CommandFailed path.
            }
            Err(NbError::CommandFailed(message)) => panic!(
                "expected DuplicateTitleHeading, but resolve_notebook fired first: {message:?}"
            ),
            other => panic!("expected DuplicateTitleHeading, got: {other:?}"),
        }
    })
    .await;
}
