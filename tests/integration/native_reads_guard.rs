//! Native read-path guardrails: shadow-checked canonical selectors,
//! the no-create notebook policy, and gate contention consistency.

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient, NoteTarget};

use crate::common::with_isolated_env;
use crate::native_reads_support::{NbOracle, config_for, quiesce, seed};

/// Shadowed numeric ids fall back to the exact-path selector: when
/// a literal file occupies the numeric form, emitting `nb:<id>`
/// would point at the wrong entity. Root and folder cases, each
/// with an explicit `nb` pair (`nb` resolves the shadowed file;
/// native emits the exact path).
#[tokio::test]
async fn native_shadowed_numeric_ids_fall_back_to_exact_path() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let native = NbClient::new(&config_for(&env)).expect("native");
        let oracle = NbOracle::new(&env);
        let root = env.nb_dir().join(env.notebook());
        seed(&native, &root).await;
        let nb = env.notebook();

        // Root: slot 1 holds the seed's first note; a literal `1`
        // shadows the numeric form.
        let first = std::fs::read_to_string(root.join(".index"))
            .expect("read index")
            .lines()
            .next()
            .expect("seeded index is non-empty")
            .to_string();
        std::fs::write(root.join("1"), "literal one\n").expect("literal 1");
        let shown = native
            .show_note(&first, None)
            .await
            .expect("shadowed root note resolves");
        assert_eq!(shown.path, first, "{shown:?}");
        assert_eq!(
            shown.selector,
            format!("{nb}:{first}"),
            "shadowed numeric must fall back to the exact path"
        );
        // The literal still wins step 0 from both sides.
        assert_eq!(
            native.show_note("1", None).await.expect("literal").path,
            "1".to_string()
        );
        // Paired oracle: bare `1` is numeric in `nb` syntax, so
        // `nb` resolves the slot while native resolves the literal
        // file (step 0 wins).
        assert_eq!(
            oracle.resolve_path("1"),
            Some(first.clone()),
            "nb resolves the numeric slot"
        );
        assert_eq!(
            oracle.resolve_path(&first),
            Some(first.clone()),
            "nb resolves the shadowed note by path"
        );

        // Folder: slot `work/1` holds the seed's first work note; a
        // literal `work/1` shadows the numeric form.
        let work_first = std::fs::read_to_string(root.join("work/.index"))
            .expect("read work index")
            .lines()
            .next()
            .expect("seeded work index is non-empty")
            .to_string();
        std::fs::write(root.join("work/1"), "literal work one\n")
            .expect("literal work/1");
        let target = format!("work/{work_first}");
        let shown = native
            .show_note(&target, None)
            .await
            .expect("shadowed folder note resolves");
        assert_eq!(shown.path, target, "{shown:?}");
        assert_eq!(
            shown.selector,
            format!("{nb}:{target}"),
            "shadowed folder numeric must fall back to the exact path"
        );
        // `nb` addresses the folder literal too (no path-level
        // divergence here); the specified divergence is the native
        // exact-path selector above.
        assert_eq!(
            oracle.resolve_path("work/1"),
            Some("work/1".to_string()),
            "nb addresses the folder literal"
        );
    })
    .await;
}

/// Native reads never create notebooks, with or without
/// `create_notebook`: a missing notebook is terminal `NotFound`
/// and no directory materializes.
#[tokio::test]
async fn native_reads_never_create_notebooks() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        for create_notebook in [false, true] {
            let config = Config {
                notebook: Some(env.notebook().to_string()),
                create_notebook,
                allow_top_level_notes: true,
                disable_git_signing: true,
                    ..Config::default()
            };
            let native = NbClient::new(&config).expect("native");
            let err = format!(
                "{:?}",
                native
                    .show_note("whatever", Some("no-such-notebook"))
                    .await
                    .expect_err("missing notebook")
            );
            assert!(err.contains("NotFound"), "{err:?}");
            let err = format!(
                "{:?}",
                native
                    .read_note_source_bytes(
                        NoteTarget::path("whatever"),
                        Some("no-such-notebook"),
                    )
                    .await
                    .expect_err("missing notebook bytes")
            );
            assert!(err.contains("NotFound"), "{err:?}");
            assert!(
                !env.nb_dir().join("no-such-notebook").exists(),
                "create_notebook={create_notebook}: native reads must not create notebooks"
            );
        }
    })
    .await;
}

/// Genuine I/O failures stay `Io` under the native flag: unreadable
/// `.index` files and unreadable notes must not degrade into
/// `NotFound` or skips.
///
/// Unix-only: relies on permission bits (root would still read
/// through them, so the test self-skips when the files stay
/// readable).
#[cfg(unix)]
#[tokio::test]
async fn native_io_failures_are_not_not_found() {
    use std::os::unix::fs::PermissionsExt;

    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let native = NbClient::new(&config_for(&env)).expect("native");
        let oracle = NbOracle::new(&env);
        let root = env.nb_dir().join(env.notebook());
        seed(&native, &root).await;

        // A numeric lookup must surface an unreadable `.index`
        // as `Io`, not fall through to title matching or
        // `NotFound`.
        let index = root.join(".index");
        std::fs::set_permissions(
            &index,
            std::fs::Permissions::from_mode(0o000),
        )
        .expect("chmod index");
        if std::fs::read(&index).is_ok() {
            // Running with privilege (e.g. root): permissions do
            // not block reads, so the refusal cannot be simulated.
            return;
        }
        let err = format!(
            "{:?}",
            native
                .show_note(&format!("{}:1", env.notebook()), None)
                .await
                .expect_err("unreadable index must fail")
        );
        assert!(err.contains("Io"), "unreadable .index degraded: {err:?}");
        // Paired oracle: `nb` also fails without a readable index.
        assert!(
            !oracle.show_ok(&format!("{}:1", env.notebook())),
            "nb must not resolve through an unreadable .index"
        );
        // The exact existing note itself stays readable, but
        // selector/numeric metadata requires the index: native
        // resolution-critical index reads propagate `Io` even when
        // the note file is fine. `nb` also fails (it needs the
        // index even for exact paths): pinned divergence.
        let err = format!(
            "{:?}",
            native
                .show_note("hello_title.md", None)
                .await
                .expect_err("exact show needs index metadata")
        );
        assert!(err.contains("Io"), "degraded: {err:?}");
        assert!(
            !oracle.show_ok("hello_title.md"),
            "nb needs the index even for exact paths"
        );
        std::fs::set_permissions(
            &index,
            std::fs::Permissions::from_mode(0o644),
        )
        .expect("restore index");

        // Same for an unreadable note file: `Io`, not `NotFound`.
        let note = root.join("hello_title.md");
        std::fs::set_permissions(
            &note,
            std::fs::Permissions::from_mode(0o000),
        )
        .expect("chmod note");
        if std::fs::read(&note).is_ok() {
            return;
        }
        let err = format!(
            "{:?}",
            native
                .read_note_source_bytes(
                    NoteTarget::path("hello_title.md"),
                    None
                )
                .await
                .expect_err("unreadable note must fail")
        );
        assert!(err.contains("Io"), "unreadable note degraded: {err:?}");
        // Paired oracle: `nb` also fails on the unreadable note.
        assert!(
            !oracle.show_ok("hello_title.md"),
            "nb must not resolve an unreadable note"
        );
    })
    .await;
}

/// Gate contention stays consistent: concurrent native reads of
/// one note race a writer that repeatedly deletes and recreates
/// that same note. Resolution and content read happen inside one
/// gate hold, so every read observes a committed state — never a
/// `NotFound` torn between a stale lookup and a landed commit, and
/// never a gate-induced failure. This is statistical stress: the
/// deterministic held-gate ordering proof lives crate-internally
/// (`client::core::native_gate_tests`).
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_reads_stay_consistent_under_contention() {
    let env = NbTestEnv::new().expect("fixture");
    // Seed via transactions only (no untracked files): the
    // concurrent commits below require a committable tree.
    with_isolated_env(&env, false, || async {
        let native = NbClient::new(&config_for(&env)).expect("native");
        let mut tx = native.transaction(None).await.expect("tx");
        tx.add_note("victim.md", Some("Victim"), "v0\n", &[])
            .unwrap();
        tx.commit().await.expect("seed");
    })
    .await;

    let _guard = crate::common::lock_env();
    let _snap = crate::common::EnvSnapshot::capture(&[
        "NB_DIR",
        "HOME",
        "GIT_DIR",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_WORK_TREE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ]);
    // Quiesce seed-time background checkpoints so the concurrent
    // commits meet a clean tree (the DirtyBaseline guard would
    // otherwise trip on checkpoint droppings).
    let root = env.nb_dir().join(env.notebook());
    let _frozen = quiesce(&root);
    // SAFETY: serialized by ENV_LOCK; restored by `_snap` on drop.
    unsafe { std::env::set_var("NB_DIR", env.nb_dir()) };
    unsafe { std::env::set_var("HOME", env.home_dir()) };

    const VERSIONS: u32 = 12;
    const READERS: usize = 3;
    const READS: u32 = 40;
    let native = NbClient::new(&config_for(&env)).expect("native");
    // Barrier: readers and writer start together so reads genuinely
    // race the mapping churn. Numeric ids are excluded: delete plus
    // recreate legitimately retires slot numbers, so only path and
    // title targets (stable across the churn) must never fail.
    // Full determinism is not achievable through the public API
    // (the gate is not directly acquirable), so iteration volume
    // plus the barrier makes interleaving near-certain; the
    // single-hold structure is pinned by the pre-resolved-root plus
    // native-gate call shape.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(READERS + 1));
    let spawn_reader = |id: &'static str| {
        let client = native.clone();
        let barrier = barrier.clone();
        async move {
            barrier.wait().await;
            let mut bodies = Vec::with_capacity(READS as usize);
            for _ in 0..READS {
                let shown = client
                    .show_note(id, None)
                    .await
                    .expect("concurrent read must succeed");
                assert_eq!(shown.path, "victim.md".to_string());
                bodies.push(shown.body);
            }
            bodies
        }
    };
    let writer = native.clone();
    let writer_barrier = barrier.clone();
    let (reads, ()) = tokio::join!(
        async {
            let (a, b, c) = tokio::join!(
                spawn_reader("victim.md"),
                spawn_reader("Victim"),
                spawn_reader("victim.md"),
            );
            [a, b, c]
        },
        async move {
            writer_barrier.wait().await;
            for v in 1..=VERSIONS {
                let mut tx = writer.transaction(None).await.expect("tx");
                tx.delete_note(NoteTarget::path("victim.md")).unwrap();
                tx.add_note(
                    "victim.md",
                    Some("Victim"),
                    &format!("v{v}\n"),
                    &[],
                )
                .unwrap();
                tx.commit().await.expect("churn commit");
            }
        },
    );
    // Every observed body is a committed version: no torn reads.
    let valid: Vec<String> =
        (0..=VERSIONS).map(|v| format!("v{v}\n")).collect();
    for (i, bodies) in reads.into_iter().enumerate() {
        assert_eq!(bodies.len(), READS as usize, "reader {i} short");
        for body in bodies {
            assert!(
                valid.contains(&body),
                "reader {i} saw uncommitted body {body:?}"
            );
        }
    }
    // A follow-up read observes the final committed state.
    let shown = native
        .show_note("victim.md", None)
        .await
        .expect("follow-up read sees the commit");
    assert_eq!(shown.body, format!("v{VERSIONS}\n"), "{shown:?}");
}

/// Indexed invalid-UTF8 notes fail as `NonUtf8` carrying the
/// requested selector: only successful payloads are canonically
/// renumbered. Covers path and numeric-selector targets.
#[tokio::test]
async fn native_non_utf8_keeps_requested_selector() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let native = NbClient::new(&config_for(&env)).expect("native");
        let root = env.nb_dir().join(env.notebook());
        let mut tx = native.transaction(None).await.expect("tx");
        tx.add_note("bad.md", Some("Bad"), "ok\n", &[]).unwrap();
        tx.commit().await.expect("seed");
        std::fs::write(root.join("bad.md"), b"# Bad\n\xff\xfe\n")
            .expect("poison utf8");
        let nb = env.notebook();
        for (target, requested) in [
            (NoteTarget::path("bad.md"), format!("{nb}:bad.md")),
            (NoteTarget::selector("1"), format!("{nb}:1")),
        ] {
            let err = format!(
                "{:?}",
                native
                    .show_note_lines(target.clone(), None, None, None)
                    .await
                    .expect_err("invalid utf8 must fail")
            );
            assert!(
                err.contains("NonUtf8"),
                "{requested}: wrong variant: {err:?}"
            );
            assert!(
                err.contains(&requested),
                "{requested}: diagnostic lost the requested selector: {err:?}"
            );
        }
    })
    .await;
}

/// Backslash traversal is platform-scoped: on Unix the input names
/// one literal file, on Windows it is a separator and the escape
/// fails closed.
#[tokio::test]
async fn native_backslash_traversal_is_platform_scoped() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let native = NbClient::new(&config_for(&env)).expect("native");
        let root = env.nb_dir().join(env.notebook());
        let mut tx = native.transaction(None).await.expect("tx");
        tx.add_note("sub/nested.md", Some("Nested"), "body\n", &[])
            .unwrap();
        tx.commit().await.expect("seed");
        #[cfg(unix)]
        {
            std::fs::write(root.join("sub\\..\\..\\foreign"), "literal\n")
                .expect("literal backslash file");
            let shown = native
                .show_note("sub\\..\\..\\foreign", None)
                .await
                .expect("Unix resolves the literal name");
            assert_eq!(shown.path, "sub\\..\\..\\foreign".to_string());
        }
        #[cfg(windows)]
        {
            let err = format!(
                "{:?}",
                native
                    .show_note("sub\\..\\..\\foreign", None)
                    .await
                    .expect_err("Windows must not escape")
            );
            assert!(err.contains("NotFound"), "{err:?}");
            // Native separators still resolve through subfolders.
            let shown = native
                .show_note("sub\\nested.md", None)
                .await
                .expect("Windows resolves native separators");
            assert_eq!(shown.path, "sub/nested.md".to_string());
        }
    })
    .await;
}

/// With `NB_DIR` absent, native reads fail closed as `NotFound`
/// without spawning or creating anything.
#[tokio::test]
async fn native_reads_fail_closed_without_nb_dir() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let native = NbClient::new(&config_for(&env)).expect("native");
        // `with_isolated_env` already serializes process env and
        // restores it on exit; removal needs no extra guard.
        // SAFETY: restored by `with_isolated_env` on exit.
        unsafe { std::env::remove_var("NB_DIR") };
        let err = format!(
            "{:?}",
            native
                .show_note("whatever", None)
                .await
                .expect_err("no NB_DIR must fail")
        );
        assert!(err.contains("NotFound"), "{err:?}");
        assert!(
            !env.nb_dir().join("no-such-notebook").exists(),
            "no notebook may materialize"
        );
    })
    .await;
}

/// Classify one id without content reads: kind, path,
/// shadow-aware selector, and numeric id.
#[tokio::test]
async fn native_classify_reports_entity_kinds() {
    use nb_api::EntityKind;

    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let native = NbClient::new(&config_for(&env)).expect("native");
        let root = env.nb_dir().join(env.notebook());
        seed(&native, &root).await;
        let nb = env.notebook();

        for (id, kind, path, selector, numeric) in [
            (
                "hello_title.md",
                EntityKind::Note,
                "hello_title.md",
                format!("{nb}:1"),
                Some(1),
            ),
            (
                "Hello Title",
                EntityKind::Note,
                "hello_title.md",
                format!("{nb}:1"),
                Some(1),
            ),
            (
                "work/do_the_thing.todo.md",
                EntityKind::Todo,
                "work/do_the_thing.todo.md",
                format!("{nb}:work/2"),
                Some(2),
            ),
            (
                "work/example.bookmark.md",
                EntityKind::Bookmark,
                "work/example.bookmark.md",
                format!("{nb}:work/3"),
                Some(3),
            ),
            (
                "work",
                EntityKind::Folder,
                "work",
                format!("{nb}:work"),
                None,
            ),
        ] {
            let got = native
                .classify(id, None)
                .await
                .unwrap_or_else(|e| panic!("{id}: {e:?}"));
            assert_eq!(got.kind, kind, "{id}");
            assert_eq!(got.path, path, "{id}");
            assert_eq!(got.selector, selector, "{id}");
            assert_eq!(got.numeric_id, numeric, "{id}");
        }
        // Raw files gain background-`nb` index slots, so the
        // selector is validated by round-trip instead of a literal
        // expectation: the emitted form always re-resolves.
        let got = native
            .classify("mystery.xyz", None)
            .await
            .expect("raw file classifies");
        assert_eq!(got.kind, EntityKind::Note);
        assert_eq!(got.path, "mystery.xyz".to_string());
        let back = native
            .show_note(&got.selector, None)
            .await
            .expect("round-trip resolves");
        assert_eq!(back.path, "mystery.xyz".to_string());
        // Other carries the non-textual diagnostic.
        let got = native
            .classify("blob.xyz", None)
            .await
            .expect("binary classifies");
        assert_eq!(
            got.kind,
            EntityKind::Other {
                reason: "non-textual:xyz".to_string()
            },
            "{got:?}"
        );
        // Missing and escaping ids stay NotFound.
        for id in ["nope-missing.md", "../evil.md", "C:/evil.md"] {
            let err = format!(
                "{:?}",
                native.classify(id, None).await.expect_err("must miss")
            );
            assert!(err.contains("NotFound"), "{id}: {err:?}");
        }
    })
    .await;
}

/// EntityKind wire shapes: exact JSON per variant plus round-trip.
#[tokio::test]
async fn entity_kind_wire_shapes_round_trip() {
    use nb_api::EntityKind;
    use serde_json::{from_value, to_value};

    for (kind, json) in [
        (EntityKind::Note, r#"{"type":"note"}"#),
        (EntityKind::Todo, r#"{"type":"todo"}"#),
        (EntityKind::Bookmark, r#"{"type":"bookmark"}"#),
        (EntityKind::Folder, r#"{"type":"folder"}"#),
        (
            EntityKind::Other {
                reason: "non-textual:pdf".to_string(),
            },
            r#"{"type":"other","reason":"non-textual:pdf"}"#,
        ),
    ] {
        let value = to_value(&kind).expect("serialize");
        assert_eq!(
            value,
            serde_json::from_str::<serde_json::Value>(json).expect("literal"),
            "{kind:?}"
        );
        let back: EntityKind = from_value(value).expect("round-trip");
        assert_eq!(back, kind);
    }
}
