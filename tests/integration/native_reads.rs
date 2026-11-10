//! Native read path: differential equivalence vs legacy `nb`
//! subprocess reads, plus a no-spawn proof under the flag.
//!
//! The differential battery covers paths, numeric ids (root and
//! folder-qualified), exact titles, basenames, colliding folders,
//! missing selectors, and the line-level/source-bytes surfaces.

use nb_api::testing::NbTestEnv;
use nb_api::{NbClient, NoteTarget};

use crate::common::with_isolated_env;
use crate::native_reads_support::{config_for, quiesce, seed, tree_listing};

#[tokio::test]
async fn native_show_matches_legacy() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let legacy = NbClient::new(&config_for(&env, false)).expect("legacy");
        let native = NbClient::new(&config_for(&env, true)).expect("native");
        let root = env.nb_dir().join(env.notebook());
        seed(&legacy, &root).await;
        let nb = env.notebook();

        // Frozen oracle: per-attempt quiesce plus snapshot, with
        // verification after each battery. A moving tree invalidates
        // that attempt instead of silently comparing across a
        // checkpoint; persistent movement fails loudly.
        let cases = vec![
            "hello_title.md".to_string(),
            format!("{nb}:1"),
            "Hello Title".to_string(),
            "Notes".to_string(),
            "Root Todo".to_string(),
            "[ ] Root Todo".to_string(),
            "Setext Title".to_string(),
            "Front Matter".to_string(),
            "Mark Down".to_string(),
            "Upper".to_string(),
            "Trail".to_string(),
            format!("Trail{}", "  "),
            "No Org".to_string(),
            "noorg.org".to_string(),
            "work/a.md".to_string(),
            format!("{nb}:work/1"),
            "a.md".to_string(),
            "Work Note".to_string(),
            "work/do_the_thing.todo.md".to_string(),
            "Do the thing".to_string(),
            "work/example.bookmark.md".to_string(),
            "todos".to_string(),
            "work".to_string(),
            "nope-missing.md".to_string(),
            "../evil.md".to_string(),
            "C:/evil.md".to_string(),
            "plain".to_string(),
            "notes.txt".to_string(),
            "mystery.xyz".to_string(),
            "blob.xyz".to_string(),
            "fakepdf.xyz".to_string(),
            "fakepng.xyz".to_string(),
            "empty.xyz".to_string(),
            "utf16.txt".to_string(),
            "gif89a.xyz".to_string(),
            "flac.xyz".to_string(),
            "bzh.xyz".to_string(),
            "riff.xyz".to_string(),
            "id3.xyz".to_string(),
            "mz.xyz".to_string(),
            "del_nonl.xyz".to_string(),
            "del_nl.xyz".to_string(),
            "onebyte.xyz".to_string(),
            "latin1.xyz".to_string(),
        ];
        // Attempts: quiesce, then run both batteries on a frozen
        // tree. A background `nb` checkpoint moving the tree
        // mid-battery invalidates the run (fail loudly after
        // retries), so a passing run always compared identical
        // frozen states.
        for _ in 0..3u32 {
            // One legacy read as warmup: it triggers any pending
            // one-time `nb` reconciliation before the window opens.
            let _ = legacy.show_note("hello_title.md", None).await;
            let frozen = quiesce(&root);
            let mut legacy_outs = Vec::with_capacity(cases.len());
            for id in &cases {
                legacy_outs.push(legacy.show_note(id, None).await);
            }
            if tree_listing(&root).expect("oracle must read") != frozen {
                continue;
            }
            let mut native_outs = Vec::with_capacity(cases.len());
            for id in &cases {
                native_outs.push(native.show_note(id, None).await);
            }
            if tree_listing(&root).expect("oracle must read") != frozen {
                continue;
            }
            for (id, (legacy_out, native_out)) in
                cases.iter().zip(legacy_outs.into_iter().zip(native_outs))
            {
                assert_eq!(
                    native_out, legacy_out,
                    "native/legacy mismatch for {id:?}"
                );
            }

            // Line-level and raw-bytes surfaces ride the same path.
            let target = NoteTarget::path("work/a.md");
            assert_eq!(
                native
                    .show_note_lines(target.clone(), None, None, None)
                    .await,
                legacy
                    .show_note_lines(target.clone(), None, None, None)
                    .await,
                "show_note_lines mismatch"
            );
            assert_eq!(
                native.search_note_lines(target.clone(), "work", None).await,
                legacy.search_note_lines(target, "work", None).await,
                "search_note_lines mismatch"
            );
            assert_eq!(
                native
                    .read_note_source_bytes(
                        NoteTarget::path("work/a.md"),
                        None
                    )
                    .await,
                legacy
                    .read_note_source_bytes(
                        NoteTarget::path("work/a.md"),
                        None
                    )
                    .await,
                "source bytes mismatch"
            );
            assert_eq!(
                tree_listing(&root).expect("oracle must read"),
                frozen,
                "tree moved during native battery"
            );
            return;
        }
        panic!("notebook tree would not settle across retries");
    })
    .await;
}

/// No `nb` subprocess may spawn on the native path: the shim logs
/// every invocation to a file and fails, so an empty log plus
/// successful reads proves zero spawns. A frozen tree hash before
/// and after proves no background checkpoint moved the comparison.
///
/// Unix-only: relies on a `sh` counting shim on `PATH`.
#[cfg(unix)]
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn native_show_spawns_no_nb() {
    use std::os::unix::fs::PermissionsExt;

    use crate::common::{EnvSnapshot, lock_env};

    let env = NbTestEnv::new().expect("fixture");
    // Seed with the real `nb` before poisoning `PATH`.
    with_isolated_env(&env, false, || async {
        let legacy = NbClient::new(&config_for(&env, false)).expect("legacy");
        let root = env.nb_dir().join(env.notebook());
        seed(&legacy, &root).await;
    })
    .await;

    let root = env.nb_dir().join(env.notebook());
    // Frozen oracle: quiesce background checkpoints from seeding,
    // then require the content hash unchanged after the run.
    let frozen = quiesce(&root);

    let _guard = lock_env();
    let _snap = EnvSnapshot::capture(&["NB_DIR", "HOME", "PATH"]);
    let dir = env.nb_dir().join("deadbin");
    std::fs::create_dir_all(&dir).expect("deadbin");
    let log = dir.join("calls.log");
    let shim = dir.join("nb");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexit 1\n",
            log.display()
        ),
    )
    .expect("write shim");
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))
        .expect("chmod shim");
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let mut new_path = dir.into_os_string();
    new_path.push(":");
    new_path.push(path_var);
    // SAFETY: serialized by ENV_LOCK; restored by `_snap` on drop.
    unsafe { std::env::set_var("PATH", new_path) };
    unsafe { std::env::set_var("NB_DIR", env.nb_dir()) };
    unsafe { std::env::set_var("HOME", env.home_dir()) };

    let native = NbClient::new(&config_for(&env, true)).expect("native");
    let nb = env.notebook();
    let root_one = format!("{nb}:1");
    // All four read surfaces, Selector and Path targets: every
    // success below proves its path spawned zero `nb` processes,
    // since any spawn is recorded in the counter log and fails.
    for id in [
        "hello_title.md",
        "Hello Title",
        "work/a.md",
        "work/do_the_thing.todo.md",
        root_one.as_str(),
        "plain",
    ] {
        let shown = native
            .show_note(id, None)
            .await
            .expect("native show must succeed with dead nb");
        assert!(!shown.path.is_empty(), "empty path for {id:?}");
        for target in [
            NoteTarget::selector(id),
            NoteTarget::path(shown.path.as_str()),
        ] {
            native
                .show_note_lines(target.clone(), Some(1), Some(10), None)
                .await
                .expect("native lines must succeed with dead nb");
            native
                .search_note_lines(target.clone(), "body", None)
                .await
                .expect("native search must succeed with dead nb");
            native
                .read_note_source_bytes(target, None)
                .await
                .expect("native bytes must succeed with dead nb");
        }
    }
    // Folders and missing selectors stay typed without spawns.
    let err = format!(
        "{:?}",
        native
            .show_note("todos", None)
            .await
            .expect_err("folder show")
    );
    assert!(err.contains("folder"), "{err:?}");
    let err = format!(
        "{:?}",
        native
            .show_note("nope-missing.md", None)
            .await
            .expect_err("missing show")
    );
    assert!(err.contains("NotFound"), "{err:?}");
    // The counter log must not exist: zero spawn attempts, not
    // merely zero successful spawns.
    assert!(
        !log.exists(),
        "native path spawned nb: {:?}",
        std::fs::read_to_string(&log).unwrap_or_default()
    );
    // The tree must not have moved under the comparison.
    assert_eq!(
        tree_listing(&root).expect("oracle must read"),
        frozen,
        "notebook tree moved during native reads"
    );
}

/// Intended native divergences from legacy `nb` behavior (each is a
/// specified migration, not a differential case): Path targets are
/// exact-only, lookups never escape the notebook, and crafted
/// `.index` lines are dead slots.
#[tokio::test]
async fn native_path_exactness_and_confinement() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let legacy = NbClient::new(&config_for(&env, false)).expect("legacy");
        let native = NbClient::new(&config_for(&env, true)).expect("native");
        let root = env.nb_dir().join(env.notebook());
        seed(&legacy, &root).await;

        // A literal file named `7` beats numeric slot 7 (step 0).
        std::fs::write(root.join("7"), "lucky\n").expect("literal 7");
        let shown = native
            .show_note("7", None)
            .await
            .expect("literal 7 resolves");
        assert_eq!(shown.path, "7".to_string(), "{shown:?}");

        // `Path` never falls back to the numeric table: no file
        // named `1` exists, even though slot 1 is occupied. Legacy
        // resolves the slot (paired divergence, documented).
        let err = format!(
            "{:?}",
            native
                .read_note_source_bytes(NoteTarget::path("1"), None)
                .await
                .expect_err("path must be exact")
        );
        assert!(err.contains("NotFound"), "{err:?}");
        legacy
            .read_note_source_bytes(NoteTarget::path("1"), None)
            .await
            .expect("legacy resolves numeric-looking paths");

        // `..` escapes are terminal `NotFound`, never resolved.
        // Absolute, drive-letter, and UNC forms are rejected the
        // same way on every platform.
        for evil in [
            "../evil.md",
            "work/../../evil.md",
            "..",
            "/evil.md",
            "C:/evil.md",
            "C:evil.md",
            "\\\\server\\share",
            "\\evil",
        ] {
            let err = format!(
                "{:?}",
                native
                    .show_note(evil, None)
                    .await
                    .expect_err("escape must not resolve")
            );
            assert!(err.contains("NotFound"), "{evil}: {err:?}");
            let err = format!(
                "{:?}",
                native
                    .read_note_source_bytes(NoteTarget::path(evil), None)
                    .await
                    .expect_err("escape must not read")
            );
            assert!(err.contains("NotFound"), "{evil}: {err:?}");
        }

        // A crafted `.index` line pointing outside the notebook is
        // a dead slot, not a traversal. Appended last, so its slot
        // is lines-count (slots before it stay valid).
        let index = root.join(".index");
        let mut text = std::fs::read_to_string(&index).expect("index");
        text.push_str("../evil.md\n");
        std::fs::write(&index, &text).expect("craft index");
        let slot = text.lines().count();
        let err = format!(
            "{:?}",
            native
                .show_note(&format!("{}:{slot}", env.notebook()), None)
                .await
                .expect_err("crafted slot is dead")
        );
        assert!(err.contains("NotFound"), "{err:?}");
        assert!(
            legacy
                .show_note(&format!("{}:{slot}", env.notebook()), None)
                .await
                .is_err(),
            "legacy must not resolve the escaping slot cleanly"
        );

        // A missing notebook is `NotFound` without creating it.
        let err = format!(
            "{:?}",
            native
                .show_note("whatever", Some("no-such-notebook"))
                .await
                .expect_err("missing notebook")
        );
        assert!(err.contains("NotFound"), "{err:?}");
        assert!(
            !env.nb_dir().join("no-such-notebook").exists(),
            "native reads must not create notebooks"
        );
        assert!(
            legacy
                .show_note("whatever", Some("no-such-notebook"))
                .await
                .is_err(),
            "legacy must also refuse the missing notebook"
        );
    })
    .await;
}

/// Accepted native divergences with explicit oracle/native pairs:
/// duplicate titles are ambiguous, symlinks are never followed.
#[cfg(unix)]
#[tokio::test]
async fn native_divergence_pairs_are_explicit() {
    use std::os::unix::fs::symlink;

    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let legacy = NbClient::new(&config_for(&env, false)).expect("legacy");
        let native = NbClient::new(&config_for(&env, true)).expect("native");
        let root = env.nb_dir().join(env.notebook());
        seed(&legacy, &root).await;

        // Duplicate root titles: native is ambiguous `NotFound`
        // while legacy resolves mtime-first (paired divergence).
        std::fs::write(root.join("dup1.md"), "# Dup\none\n").expect("dup1");
        std::fs::write(root.join("dup2.md"), "# Dup\ntwo\n").expect("dup2");
        let err = format!(
            "{:?}",
            native.show_note("Dup", None).await.expect_err("ambiguous")
        );
        assert!(err.contains("NotFound"), "{err:?}");
        legacy
            .show_note("Dup", None)
            .await
            .expect("legacy resolves duplicates mtime-first");

        // Symlinks are never followed by native resolution (legacy
        // follows them: paired divergence).
        symlink("hello_title.md", root.join("link.md")).expect("link file");
        std::fs::create_dir(root.join("realdir")).expect("realdir");
        symlink("realdir", root.join("linkdir")).expect("link dir");
        for id in ["link.md", "linkdir"] {
            let err = format!(
                "{:?}",
                native.show_note(id, None).await.expect_err("symlink")
            );
            assert!(err.contains("NotFound"), "{id}: {err:?}");
        }
        // Paired oracle: legacy follows the file symlink into the
        // note, and classifies the directory symlink as a folder.
        legacy
            .show_note("link.md", None)
            .await
            .expect("legacy follows file symlinks");
        assert!(
            legacy.show_note("linkdir", None).await.is_err(),
            "legacy must not resolve the dir symlink as a note"
        );

        // Ancestor symlinks never resolve: `linkdir/note.md`
        // through the symlinked `linkdir` is `NotFound` without
        // following the link, and the folder-qualified numeric form
        // is a dead slot for the same reason.
        std::fs::write(root.join("realdir/note.md"), "# Real\nbody\n")
            .expect("real note");
        for id in ["linkdir/note.md", "linkdir/1"] {
            let err = format!(
                "{:?}",
                native.show_note(id, None).await.expect_err("ancestor link")
            );
            assert!(err.contains("NotFound"), "{id}: {err:?}");
        }
        // Paired oracle: legacy follows the ancestor link.
        legacy
            .show_note("linkdir/note.md", None)
            .await
            .expect("legacy follows ancestor symlinks");

        // A symlinked `.index` blinds numeric resolution only:
        // exact paths still resolve, and legacy (which reads through
        // the link) is the paired divergence.
        let index = root.join(".index");
        let staged = root.join(".index.real");
        std::fs::rename(&index, &staged).expect("stage index");
        symlink(".index.real", &index).expect("link index");
        let err = format!(
            "{:?}",
            native.show_note("1", None).await.expect_err("linked index")
        );
        assert!(err.contains("NotFound"), "{err:?}");
        native
            .show_note("hello_title.md", None)
            .await
            .expect("exact paths do not need the index");
        legacy
            .show_note("1", None)
            .await
            .expect("legacy reads through the linked index");
        std::fs::remove_file(&index).expect("unlink index");
        std::fs::rename(&staged, &index).expect("restore index");

        // Dead numeric slots fall through to the title table:
        // append a missing file as the last slot, then title a
        // note with that slot number.
        let mut text =
            std::fs::read_to_string(root.join(".index")).expect("read index");
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("missing.md\n");
        std::fs::write(root.join(".index"), &text).expect("craft slot");
        let slot = text.lines().count().to_string();
        std::fs::write(root.join("seven.md"), format!("# {slot}\nseven\n"))
            .expect("seven");
        let shown = native
            .show_note(&slot, None)
            .await
            .expect("dead slot falls through to title");
        assert_eq!(shown.path, "seven.md".to_string(), "{shown:?}");
        assert!(
            legacy.show_note(&slot, None).await.is_err(),
            "legacy must not resolve the dead slot cleanly"
        );

        // A literal backslash is a normal filename character, never
        // a separator: `a\b` resolves as one component on Unix.
        std::fs::write(root.join("a\\b"), "backslash\n")
            .expect("backslash file");
        let shown = native
            .show_note("a\\b", None)
            .await
            .expect("literal backslash resolves");
        assert_eq!(shown.path, "a\\b".to_string(), "{shown:?}");
    })
    .await;
}
