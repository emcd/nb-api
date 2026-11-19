use super::{NbrcSelection, ResolutionMode, resolve_with, select_nb_dir};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Parser contract: supported forms assign (last wins, empty
/// clears, comments/quotes/ordering honored, `$NB_DIR`
/// self-defaults track the table), undeterminable `NB_DIR`
/// syntax selects fallback (warn + default downstream), and
/// unrelated lines never matter.
#[test]
fn nb_dir_selection_matrix() {
    let home = std::path::Path::new("/home/test");
    // (input, expected): `Some(value)` is `Assign(value)`,
    // `None` is `Absent`, `Err(reason)` is `Fallback(reason)`.
    let cases: &[(&str, Result<Option<&str>, &str>)] = &[
        ("", Ok(None)),
        ("# nothing here\nFOO=bar\n", Ok(None)),
        ("NB_DIR=/data/nb\n", Ok(Some("/data/nb"))),
        ("export NB_DIR=/data/nb\n", Ok(Some("/data/nb"))),
        ("NB_DIR=/old\nNB_DIR=/new\n", Ok(Some("/new"))),
        ("NB_DIR=/new\nNB_DIR=\n", Ok(None)),
        ("NB_DIR=/new\nunset NB_DIR\n", Ok(None)),
        ("NB_DIR=/data # comment\n", Ok(Some("/data"))),
        ("NB_DIR=/data#kept\n", Ok(Some("/data#kept"))),
        ("NB_DIR='/a b'\n", Ok(Some("/a b"))),
        ("NB_DIR=\"/a $HOME\"\n", Ok(Some("/a /home/test"))),
        ("NB_DIR=$HOME/.nb\n", Ok(Some("/home/test/.nb"))),
        ("NB_DIR=${HOME}/x\n", Ok(Some("/home/test/x"))),
        ("NB_DIR=~/x\n", Ok(Some("/home/test/x"))),
        ("NB_DIR=\n", Ok(None)),
        ("NB_DIR=\"\"\n", Ok(None)),
        ("export NB_DIR\n", Ok(None)),
        ("MY_NB_DIR=/x\n", Ok(None)),
        ("echo hi\n", Ok(None)),
        ("NB_DIR=/x # trailing\n", Ok(Some("/x"))),
        ("NB_DIR=/win\r\n", Ok(Some("/win"))),
        ("NB_DIR=\"~/x\"\n", Ok(Some("~/x"))),
        ("NB_DIR='~/x'\n", Ok(Some("~/x"))),
        // `nb`-style self-defaults: process `NB_DIR` is unset
        // in this branch, so the default always applies first
        // and later assignments are tracked.
        // Operator machine shape: self-default plus trailing
        // comment left by `nb settings`.
        (
            "export NB_DIR=\"${NB_DIR:-/home/test/Dropbox/Notes}\" # Set by `nb`\n",
            Ok(Some("/home/test/Dropbox/Notes")),
        ),
        (
            "export NB_DIR=\"${NB_DIR:-$HOME/.nb}\"\n",
            Ok(Some("/home/test/.nb")),
        ),
        ("NB_DIR=/a\nNB_DIR=\"${NB_DIR:-/b}\"\n", Ok(Some("/a"))),
        ("NB_DIR=\nNB_DIR=\"${NB_DIR:-/b}\"\n", Ok(Some("/b"))),
        ("NB_DIR=/a\nNB_DIR=\"${NB_DIR:+/b}\"\n", Ok(Some("/b"))),
        ("NB_DIR=\nNB_DIR=\"${NB_DIR:+/b}\"\n", Ok(None)),
        ("NB_DIR=/a\nNB_DIR=\"$NB_DIR/x\"\n", Ok(Some("/a/x"))),
        ("NB_DIR=\"${NB_DIR:=/assigned}\"\n", Ok(Some("/assigned"))),
        ("NB_DIR=${NB_DIR:-/unquoted}\n", Ok(Some("/unquoted"))),
        // Reviewer-reproduced ordering and side-effect cases
        // (reviews/53): later assignments in one command
        // observe earlier ones; `:=` side effects persist
        // through other variables; subscripts fall back.
        ("NB_DIR=/a NB_DIR=\"${NB_DIR:-/b}\"\n", Ok(Some("/a"))),
        (
            "OTHER=\"${NB_DIR:=/a}\"\nNB_DIR=\"${NB_DIR:-/b}\"\n",
            Ok(Some("/a")),
        ),
        (
            "OTHER=\"${NB_DIR:=/a}\" true\nNB_DIR=\"${NB_DIR:-/b}\"\n",
            Ok(Some("/a")),
        ),
        ("NB_DIR=/a\nNB_DIR[0]=/b\n", Err("unsupported NB_DIR")),
        // Reviewer-reproduced reviews/54 variants: a bare
        // `${NB_DIR:=...}` through another variable persists
        // with no direct assignment, and `export` expands
        // before binding (verified against Bash).
        ("OTHER=\"${NB_DIR:=/a}\"\n", Ok(Some("/a"))),
        ("OTHER=\"${NB_DIR:=/a}\" true\n", Ok(Some("/a"))),
        (
            "export NB_DIR=/a NB_DIR=\"${NB_DIR:-/b}\"\n",
            Ok(Some("/b")),
        ),
        (
            "export NB_DIR=\"${NB_DIR:-/b}\" NB_DIR=/a\n",
            Ok(Some("/a")),
        ),
        (
            // Evaluation order is suffix expansion first (with
            // `${NB_DIR:=...}` taking immediate effect for later
            // suffix words), prefix evaluation second (side
            // effects and transient capture only), bindings last;
            // all verified against Bash, direct and sourced.
            "export OTHER=\"${NB_DIR:=/a}\" NB_DIR=\"${NB_DIR:-/b}\"\n",
            Ok(Some("/a")),
        ),
        (
            "export NB_DIR=\"${NB_DIR:=/a}\" NB_DIR=\"${NB_DIR:-/b}\"\n",
            Ok(Some("/a")),
        ),
        ("X=\"${NB_DIR:=/z}\" export NB_DIR=/q\n", Ok(Some("/q"))),
        ("X=\"${NB_DIR:=/z}\" export OTHER=w\n", Ok(Some("/z"))),
        ("NB_DIR=/p export NB_DIR=/q\n", Ok(Some("/q"))),
        // Reviewer-reproduced reviews/62: `readonly`
        // persistence is declined — a later assignment cannot be
        // honored (Bash rejects the reassignment and retains the
        // old value), so any `NB_DIR` involvement under
        // `readonly` falls back, including the bare-word form.
        (
            "X=\"${NB_DIR:=/z}\" readonly NB_DIR\n",
            Err("readonly NB_DIR"),
        ),
        ("NB_DIR=/a readonly NB_DIR\n", Err("readonly NB_DIR")),
        (
            "NB_DIR=/a readonly NB_DIR\nNB_DIR=/b\n",
            Err("readonly NB_DIR"),
        ),
        // Reviewer-reproduced reviews/63: quoted bare words
        // evaluate to NB_DIR too — raw spelling equality misses
        // quote removal. Bare words are evaluated, not
        // string-matched; only proven-unrelated words are
        // ignored.
        (
            "NB_DIR=/a readonly 'NB_DIR'\nNB_DIR=/b\n",
            Err("readonly NB_DIR"),
        ),
        (
            "NB_DIR=/a readonly \"NB_DIR\"\nNB_DIR=/b\n",
            Err("readonly NB_DIR"),
        ),
        ("NB_DIR=/a readonly FOO\nNB_DIR=/b\n", Ok(Some("/b"))),
        // Reviewer-reproduced reviews/65: brace expansion may
        // hide NB_DIR, so only demonstrably literal operands
        // (bare identifiers, simple flags) are ignored outright.
        (
            "NB_DIR=/a readonly {FOO,NB_DIR}\nNB_DIR=/b\n",
            Err("readonly NB_DIR"),
        ),
        (
            "NB_DIR=/a readonly \"FOO\"\nNB_DIR=/b\n",
            Err("readonly NB_DIR"),
        ),
        ("NB_DIR=/a readonly -p\nNB_DIR=/b\n", Ok(Some("/b"))),
        // Reviewer-reproduced reviews/66: unquoted expansions
        // word-split, so a `$NB_DIR` holding "/a NB_DIR" marks
        // NB_DIR readonly. The literal allowlist applies
        // directly — no word splitting or evaluate-and-ignore
        // for nonliterals. Bare FOO / -p stay ignored.
        (
            "NB_DIR=\"/a NB_DIR\"\nreadonly $NB_DIR\nNB_DIR=/b\n",
            Err("readonly NB_DIR"),
        ),
        // Reviewer-reproduced reviews/64: a quoted whole
        // assignment is a word, not an AssignmentWord — catch
        // it by evaluated shape, not spelling.
        (
            "NB_DIR=/a readonly 'NB_DIR=/x'\nNB_DIR=/b\n",
            Err("readonly NB_DIR"),
        ),
        (
            "NB_DIR=/a readonly \"NB_DIR=/x\"\nNB_DIR=/b\n",
            Err("readonly NB_DIR"),
        ),
        ("f() { exit 0; }\nf\nNB_DIR=/b\n", Err("control flow")),
        ("f() { return 0; }\nf\nNB_DIR=/b\n", Ok(Some("/b"))),
        // Reviewer-reproduced reviews/55 variant: an empty
        // direct assignment followed by a `:=` side effect in
        // the same command leaves the side-effect value, and a
        // later read observes it (single authoritative table,
        // each mutation applied once).
        ("NB_DIR= OTHER=\"${NB_DIR:=/a}\"\n", Ok(Some("/a"))),
        (
            "NB_DIR= OTHER=\"${NB_DIR:=/a}\"\nNB_DIR=\"${NB_DIR:-/b}\"\n",
            Ok(Some("/a")),
        ),
        // Reviewer-reproduced reviews/56 pair: defaults inherit
        // the expansion site's quoting context. Quoted `~`
        // stays literal (downstream rejects it as relative);
        // unquoted `~` expands against `home`.
        (
            "export NB_DIR=\"${NB_DIR:-~/Notes}\"\n",
            Ok(Some("~/Notes")),
        ),
        (
            "export NB_DIR=${NB_DIR:-~/Notes}\n",
            Ok(Some("/home/test/Notes")),
        ),
        // Reviewer-reproduced reviews/57 pair: nested quotes
        // in a double-quoted default are literal characters in
        // Bash (relative path), but a standalone reparse would
        // strip them and absolutize. Fall back instead; the
        // unquoted counterpart quotes normally.
        (
            "export NB_DIR=\"${NB_DIR:-'/a b'}\"\n",
            Err("nested quoting"),
        ),
        ("export NB_DIR=${NB_DIR:-'/a b'}\n", Ok(Some("/a b"))),
        // Reviewer-reproduced reviews/58: sourcing hidden
        // behind a guard or inside a compound command may set
        // `NB_DIR` from an unread file, so the whole file falls
        // back even with no `NB_DIR` token at the use site.
        // A compound without sourcing stays ignored.
        (
            "NB_DIR=/a\n[ ! -f \"$HOME/.nb-local\" ] || . \"$HOME/.nb-local\"\n",
            Err("sourced file"),
        ),
        (
            "NB_DIR=/a\nif [ x ]; then source other.rc; fi\n",
            Err("sourced file"),
        ),
        ("NB_DIR=/a\nif [ x ]; then FOO=1; fi\n", Ok(Some("/a"))),
        // Reviewer-reproduced reviews/60: `return`/`exit`
        // outside function bodies abort sourcing, so the whole
        // file falls back (a bare `return` after the last
        // assignment resolves nothing by itself); function-scoped
        // `return` is harmless. A transient `NB_DIR=` prefix
        // persists across `export`/`readonly` (bare word
        // captures it) but nothing else; subshell and background
        // sourcing cannot reach the parent store.
        ("NB_DIR=/a; return 0; NB_DIR=/b\n", Err("control flow")),
        ("NB_DIR=/a\nexit 0\nNB_DIR=/b\n", Err("control flow")),
        (
            "NB_DIR=/a\nif true; then return 0; fi\nNB_DIR=/b\n",
            Err("control flow"),
        ),
        ("f() { return 0; }\nNB_DIR=/a\n", Ok(Some("/a"))),
        ("NB_DIR=/a export NB_DIR\n", Ok(Some("/a"))),
        ("NB_DIR=/p export OTHER=x\n", Ok(None)),
        (
            "NB_DIR=/p export NB_DIR=\"${NB_DIR:-/d}\"\n",
            Ok(Some("/d")),
        ),
        ("NB_DIR=/a readonly FOO=1\n", Ok(None)),
        ("( source other.rc )\nNB_DIR=/a\n", Ok(Some("/a"))),
        ("source other.rc & \nNB_DIR=/a\n", Ok(Some("/a"))),
        // Reviewer-reproduced reviews/59: dispatch identity is
        // effective, not textual. Quoted `source`, `builtin`,
        // and `command`-wrapped sourcing all source in Bash;
        // quoted `export` still exports; `eval` and dynamic
        // names are opaque; plain externals stay ignored.
        ("NB_DIR=/a\n'source' other.rc\n", Err("sourced file")),
        (
            "NB_DIR=/a\nbuiltin source other.rc\n",
            Err("opaque command"),
        ),
        ("NB_DIR=/a\ncommand . other.rc\n", Err("opaque command")),
        ("NB_DIR=/a\n'export' NB_DIR=/b\n", Ok(Some("/b"))),
        ("NB_DIR=/a\neval \"echo hi\"\n", Err("opaque command")),
        ("NB_DIR=/a\ntouch other\n", Ok(Some("/a"))),
        // Undeterminable without execution: fallback with a
        // reason (resolution warns and uses `~/.nb`).
        ("NB_DIR=$(hostname)\n", Err("command substitution")),
        ("NB_DIR=`hostname`\n", Err("command substitution")),
        ("NB_DIR=\"$(hostname)\"\n", Err("command substitution")),
        ("if [ x ]; then NB_DIR=/x; fi\n", Err("compound command")),
        ("NB_DIR =/x\n", Err("unsupported NB_DIR use")),
        ("NB_DIR+=:/x\n", Err("append assignment")),
        ("NB_DIR=(/a /b)\n", Err("array assignment")),
        ("readonly NB_DIR=/x\n", Err("readonly NB_DIR")),
        ("source other.rc\n", Err("sourced file")),
        (". other.rc\n", Err("sourced file")),
        // `VAR=x cmd` is a transient environment for `cmd`,
        // not a persistent assignment (correct shell semantics).
        ("NB_DIR=/a b\n", Ok(None)),
        ("NB_DIR=~other/x\n", Err("tilde expansion")),
        ("NB_DIR=$OTHER/x\n", Err("non-HOME expansion")),
        ("NB_DIR=${OTHER}\n", Err("non-HOME expansion")),
        ("NB_DIR=${NB_DIR:?must be set}\n", Err("non-HOME expansion")),
        ("NB_DIR=$((1 + 1))\n", Err("command substitution")),
        ("NB_DIR=/x & \n", Err("backgrounded command")),
        ("false || NB_DIR=/x\n", Err("conditional command")),
        ("export NB_DIR=$OTHER\n", Err("non-HOME expansion")),
    ];
    for (input, expected) in cases {
        let result = select_nb_dir(input, Some(home), None);
        match (result, expected) {
            (NbrcSelection::Assign(got), Ok(Some(want))) => {
                assert_eq!(got, *want, "input {input:?}");
            },
            (NbrcSelection::Absent, Ok(None)) => {},
            (NbrcSelection::Fallback(got), Err(want)) => {
                assert!(
                    got.contains(want),
                    "input {input:?} reason {got:?} misses {want:?}"
                );
            },
            (got, want) => {
                panic!("input {input:?}: got {got:?}, want {want:?}");
            },
        }
    }
}

/// (rc, starting, expected): `Some` is `Assign`, `None` is `Absent`,
/// `Err` is `Fallback`.
type SelectCase<'a> =
    (&'a str, Option<&'a str>, Result<Option<&'a str>, &'a str>);

/// Always-parse: the starting value participates exactly as the
/// shell would see it — presets survive self-defaults,
/// unconditional assignments and `unset` override them, and empty
/// stays distinct from unset for `-` vs `:-`.
#[test]
fn selection_starts_from_process_value() {
    let home = std::path::Path::new("/home/test");
    let start = |v: Option<&str>| v.map(str::to_string);
    // (rc, starting, expected): `Some` is `Assign`, `None` is
    // `Absent`, `Err` is `Fallback`.
    let cases: &[SelectCase<'_>] = &[
        (
            "export NB_DIR=\"${NB_DIR:-/fb}\"\n",
            Some("/preset"),
            Ok(Some("/preset")),
        ),
        ("export NB_DIR=\"${NB_DIR:-/fb}\"\n", None, Ok(Some("/fb"))),
        (
            "export NB_DIR=\"${NB_DIR:-/fb}\"\n",
            Some(""),
            Ok(Some("/fb")),
        ),
        ("NB_DIR=${NB_DIR-/fb}\n", Some(""), Ok(None)),
        ("NB_DIR=${NB_DIR-/fb}\n", None, Ok(Some("/fb"))),
        ("NB_DIR=/override\n", Some("/preset"), Ok(Some("/override"))),
        ("unset NB_DIR\n", Some("/preset"), Ok(None)),
        (
            "export NB_DIR=$UNKNOWN_THING\n",
            Some("/preset"),
            Err("non-HOME"),
        ),
        ("# silent\nFOO=1\n", Some("/preset"), Ok(Some("/preset"))),
        ("# silent\nFOO=1\n", None, Ok(None)),
        ("# silent\nFOO=1\n", Some(""), Ok(None)),
        (
            "NB_DIR=$HOME/x\n",
            Some("/preset"),
            Ok(Some("/home/test/x")),
        ),
        ("NB_DIR=$HOME/x\n", None, Ok(Some("/home/test/x"))),
    ];
    for (input, starting, expected) in cases {
        let result = select_nb_dir(input, Some(home), start(*starting));
        match (result, expected) {
            (NbrcSelection::Assign(got), Ok(Some(want))) => {
                assert_eq!(&got, want, "input {input:?}");
            },
            (NbrcSelection::Absent, Ok(None)) => {},
            (NbrcSelection::Fallback(got), Err(want)) => {
                assert!(got.contains(want), "input {input:?} reason {got:?}");
            },
            (got, want) => {
                panic!("input {input:?}: got {got:?}, want {want:?}");
            },
        }
    }
}

/// Without `HOME`, home expansions are undeterminable instead of
/// expanding to garbage; home-free content still resolves.
#[test]
fn selection_without_home() {
    let cases: &[SelectCase<'_>] = &[
        ("NB_DIR=$HOME/x\n", None, Err("")),
        ("NB_DIR=~/x\n", None, Err("")),
        ("export NB_DIR=\"${NB_DIR:-/fb}\"\n", None, Ok(Some("/fb"))),
        ("NB_DIR=/abs\n", None, Ok(Some("/abs"))),
    ];
    for (input, starting, expected) in cases {
        let result = select_nb_dir(input, None, starting.map(str::to_string));
        match (result, expected) {
            (NbrcSelection::Assign(got), Ok(Some(want))) => {
                assert_eq!(&got, want, "input {input:?}");
            },
            (NbrcSelection::Absent, Ok(None)) => {},
            (NbrcSelection::Fallback(_), Err(_)) => {},
            (got, want) => {
                panic!("input {input:?}: got {got:?}, want {want:?}");
            },
        }
    }
}

static FIXTURE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Scratch dir unique to this test run (pid + sequence): no
/// shared state, safe under parallel execution.
fn scratch_home(name: &str) -> PathBuf {
    let seq = FIXTURE_SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir()
        .join(format!("nb-api-nbrc-{name}-{}-{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn write_rc(home: &std::path::Path, body: &str) -> PathBuf {
    let path = home.join(".nbrc");
    std::fs::write(&path, body).expect("rc fixture");
    path
}

/// Resolution precedence matrix (ParseNbrc default): strict
/// short-circuits, explicit paths are enforced, presets flow
/// through, undeterminable content warns + defaults.
#[tokio::test]
async fn resolve_precedence_matrix() {
    let home = scratch_home("precedence");
    let rc = write_rc(&home, "export NB_DIR=\"${NB_DIR:-/stores/rc}\"\n");
    let home_opt = Some(home.clone());
    let none: Option<std::ffi::OsString> = None;
    let some = |v: &str| Some(std::ffi::OsString::from(v));

    // Preset survives a self-default rc (always-parse proof).
    let got = resolve_with(
        ResolutionMode::ParseNbrc,
        false,
        some("/stores/env"),
        home_opt.clone(),
        none.clone(),
    )
    .await
    .expect("preset resolves");
    assert_eq!(got, PathBuf::from("/stores/env"));

    // Unconditional rc assignment overrides the preset.
    write_rc(&home, "NB_DIR=/stores/override\n");
    let got = resolve_with(
        ResolutionMode::ParseNbrc,
        false,
        some("/stores/env"),
        home_opt.clone(),
        none.clone(),
    )
    .await
    .expect("override resolves");
    assert_eq!(got, PathBuf::from("/stores/override"));

    // Unset in rc clears the preset back to the default.
    write_rc(&home, "unset NB_DIR\n");
    let got = resolve_with(
        ResolutionMode::ParseNbrc,
        false,
        some("/stores/env"),
        home_opt.clone(),
        none.clone(),
    )
    .await
    .expect("unset falls back");
    assert_eq!(got, home.join(".nb"));

    // No rc file at all: preset wins without needing anything else.
    std::fs::remove_file(&rc).expect("remove rc");
    let got = resolve_with(
        ResolutionMode::ParseNbrc,
        false,
        some("/stores/env"),
        home_opt.clone(),
        none.clone(),
    )
    .await
    .expect("preset without rc");
    assert_eq!(got, PathBuf::from("/stores/env"));

    // Strict ignores the rc entirely (even a present one).
    write_rc(&home, "NB_DIR=/stores/override\n");
    let got = resolve_with(
        ResolutionMode::ParseNbrc,
        true,
        some("/stores/env"),
        home_opt.clone(),
        none.clone(),
    )
    .await
    .expect("strict preset resolves");
    assert_eq!(got, PathBuf::from("/stores/env"));
    resolve_with(
        ResolutionMode::ParseNbrc,
        true,
        none.clone(),
        home_opt.clone(),
        none.clone(),
    )
    .await
    .expect_err("strict unset refuses");

    // IgnoreNbrc never reads the rc either.
    let got = resolve_with(
        ResolutionMode::IgnoreNbrc,
        false,
        some("/stores/env"),
        home_opt.clone(),
        none.clone(),
    )
    .await
    .expect("ignore preset resolves");
    assert_eq!(got, PathBuf::from("/stores/env"));
}

/// Explicit `NBRC_PATH` handling and pinned errors.
#[tokio::test]
async fn resolve_explicit_rc_path() {
    let home = scratch_home("explicit");
    let home_opt = Some(home.clone());
    let none: Option<std::ffi::OsString> = None;
    let some = |v: &str| Some(std::ffi::OsString::from(v));

    // Explicit missing file is a configuration error.
    let missing = home.join("no-such-rc");
    resolve_with(
        ResolutionMode::ParseNbrc,
        false,
        none.clone(),
        home_opt.clone(),
        some(missing.to_str().unwrap()),
    )
    .await
    .expect_err("explicit missing rc errors");

    // Explicit relative path is rejected.
    resolve_with(
        ResolutionMode::ParseNbrc,
        false,
        none.clone(),
        home_opt.clone(),
        some("relative/.nbrc"),
    )
    .await
    .expect_err("explicit relative rc errors");

    // Relative resolved store is rejected.
    let rc = write_rc(&home, "NB_DIR=relative/path\n");
    let got = resolve_with(
        ResolutionMode::ParseNbrc,
        false,
        none.clone(),
        home_opt.clone(),
        some(rc.to_str().unwrap()),
    )
    .await;
    assert!(got.is_err(), "relative store must fail: {got:?}");

    // Unreadable rc (a directory) warns and behaves as absent.
    let got = resolve_with(
        ResolutionMode::ParseNbrc,
        false,
        some("/stores/env"),
        home_opt.clone(),
        some(home.to_str().unwrap()),
    )
    .await
    .expect("unreadable rc falls back to preset");
    assert_eq!(got, PathBuf::from("/stores/env"));

    // Missing HOME with preset resolves without a default.
    let got = resolve_with(
        ResolutionMode::ParseNbrc,
        false,
        some("/stores/env"),
        None,
        none.clone(),
    )
    .await
    .expect("preset without HOME resolves");
    assert_eq!(got, PathBuf::from("/stores/env"));

    // Missing HOME without preset cannot compute a default.
    resolve_with(
        ResolutionMode::ParseNbrc,
        false,
        none.clone(),
        None,
        none.clone(),
    )
    .await
    .expect_err("no HOME and no preset errors");
}

/// `EvaluateNbrc` sources the rc for real (opted-in execution),
/// including constructs the static subset rejects.
#[tokio::test]
async fn resolve_evaluate_mode() {
    let home = scratch_home("evaluate");
    let home_opt = Some(home.clone());
    let none: Option<std::ffi::OsString> = None;
    let some = |v: &str| Some(std::ffi::OsString::from(v));

    // Command substitution works under Evaluate (fallback under Parse).
    let rc = write_rc(&home, "NB_DIR=$(echo /stores/eval)\n");
    let got = resolve_with(
        ResolutionMode::EvaluateNbrc,
        false,
        none.clone(),
        home_opt.clone(),
        some(rc.to_str().unwrap()),
    )
    .await
    .expect("evaluate resolves command substitution");
    assert_eq!(got, PathBuf::from("/stores/eval"));

    // The same rc falls back under Parse.
    let got = resolve_with(
        ResolutionMode::ParseNbrc,
        false,
        none.clone(),
        home_opt.clone(),
        some(rc.to_str().unwrap()),
    )
    .await
    .expect("parse falls back on command substitution");
    assert_eq!(got, home.join(".nb"));

    // `exit 1` is a hard error, never a fallback.
    let rc = write_rc(&home, "NB_DIR=/stores/nope\nexit 1\n");
    resolve_with(
        ResolutionMode::EvaluateNbrc,
        false,
        none.clone(),
        home_opt.clone(),
        some(rc.to_str().unwrap()),
    )
    .await
    .expect_err("failing rc is a hard error");

    // Seeded starting value flows into evaluation.
    let rc = write_rc(&home, "export NB_DIR=\"${NB_DIR:-/stores/fb}\"\n");
    let got = resolve_with(
        ResolutionMode::EvaluateNbrc,
        false,
        some("/stores/seed"),
        home_opt.clone(),
        some(rc.to_str().unwrap()),
    )
    .await
    .expect("evaluate sees the seed");
    assert_eq!(got, PathBuf::from("/stores/seed"));
}

/// `EvaluateNbrc` hardening: top-level `return` gets a real source
/// frame, `exec` is unregistered (process replacement refused),
/// and strict/Ignore presets face the same absolute-path gate.
#[tokio::test]
async fn evaluate_hardening_and_strict_absolute() {
    let home = scratch_home("hardening");
    let home_opt = Some(home.clone());
    let none: Option<std::ffi::OsString> = None;
    let some = |v: &str| Some(std::ffi::OsString::from(v));

    // Top-level return works: the rc is sourced, not run as a script.
    let rc = write_rc(
        &home,
        "NB_DIR=/stores/early\nreturn 0\nNB_DIR=/stores/late\n",
    );
    // NOTE: `return` ends sourcing; the late assignment never runs.
    let got = resolve_with(
        ResolutionMode::EvaluateNbrc,
        false,
        none.clone(),
        home_opt.clone(),
        some(rc.to_str().unwrap()),
    )
    .await
    .expect("top-level return is fine when sourcing");
    assert_eq!(got, PathBuf::from("/stores/early"));

    // `exec` is unregistered: process replacement is refused loudly.
    let rc = write_rc(&home, "NB_DIR=/stores/x\nexec echo hi\n");
    resolve_with(
        ResolutionMode::EvaluateNbrc,
        false,
        none.clone(),
        home_opt.clone(),
        some(rc.to_str().unwrap()),
    )
    .await
    .expect_err("exec builtin must not exist");

    // Top-level `return` ends sourcing with prior assignments intact
    // (proves a real source frame, not a run-and-discard string).
    let rc =
        write_rc(&home, "NB_DIR=/stores/early\nexit 0\nNB_DIR=/stores/late\n");
    let got = resolve_with(
        ResolutionMode::EvaluateNbrc,
        false,
        none.clone(),
        home_opt.clone(),
        some(rc.to_str().unwrap()),
    )
    .await
    .expect("top-level return works");
    assert_eq!(got, PathBuf::from("/stores/early"));

    // `umask`/`ulimit` touch host process state and are unregistered.
    for body in ["NB_DIR=/x\numask 022\n", "NB_DIR=/x\nulimit -n 1024\n"] {
        let rc = write_rc(&home, body);
        resolve_with(
            ResolutionMode::EvaluateNbrc,
            false,
            none.clone(),
            home_opt.clone(),
            some(rc.to_str().unwrap()),
        )
        .await
        .expect_err("host-state builtin must be absent");
    }

    // Strict and Ignore presets face the absolute-path gate too.
    // (Fresh home: the rc files written above must not interfere.)
    let clean = scratch_home("hardening-clean");
    let clean_opt = Some(clean);
    for mode in [ResolutionMode::IgnoreNbrc, ResolutionMode::ParseNbrc] {
        let strict = mode == ResolutionMode::IgnoreNbrc;
        resolve_with(
            mode,
            strict,
            some("relative/path"),
            clean_opt.clone(),
            none.clone(),
        )
        .await
        .expect_err("relative store must fail in every mode");
    }
    // ...but absolute presets still resolve.
    let got = resolve_with(
        ResolutionMode::IgnoreNbrc,
        false,
        some("/stores/abs"),
        clean_opt,
        none.clone(),
    )
    .await
    .expect("absolute preset resolves");
    assert_eq!(got, PathBuf::from("/stores/abs"));
}

/// `EvaluateNbrc` export preservation: the seeded `NB_DIR` is
/// exported by construction (all process-environment entries are),
/// so ordinary child processes spawned by the rc still see it.
/// Hermetic: the seed travels by argument, no host environment is
/// mutated. Unix-only: relies on `printenv(1)`.
#[cfg(unix)]
#[tokio::test]
async fn evaluate_seed_keeps_export_for_children() {
    let home = scratch_home("export-child");
    let rc = write_rc(&home, "NB_DIR=\"$(printenv NB_DIR)/child\"\n");
    let result = super::evaluate::evaluate_nbrc(
        &rc,
        &home,
        Some("/stores/seed".into()),
    )
    .await;
    assert_eq!(
        result.expect("evaluate succeeds"),
        Some("/stores/seed/child".to_string()),
        "child must see the exported seed"
    );
}

/// `EvaluateNbrc` export preservation, set-but-empty seed: an
/// empty `NB_DIR` is still present in the environment, and
/// `printenv` distinguishes present-but-empty (exit 0) from
/// absent (exit 1). Hermetic: no host environment is mutated.
/// Unix-only: relies on `printenv(1)`.
#[cfg(unix)]
#[tokio::test]
async fn evaluate_empty_seed_keeps_export_for_children() {
    let home = scratch_home("export-empty");
    let rc = write_rc(
        &home,
        "if printenv NB_DIR >/dev/null 2>&1; then NB_DIR=/stores/seen; else NB_DIR=/stores/unseen; fi\n",
    );
    let result =
        super::evaluate::evaluate_nbrc(&rc, &home, Some(String::new())).await;
    assert_eq!(
        result.expect("evaluate succeeds"),
        Some("/stores/seen".to_string()),
        "empty seed must stay exported (visible to printenv)"
    );
}
