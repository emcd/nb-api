use super::{NbrcSelection, select_nb_dir};

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
        let result = select_nb_dir(input, home);
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
