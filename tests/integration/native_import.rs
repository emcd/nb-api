//! Native file import: admission, naming, collision, and purity
//! through the public client on the hermetic fixture (no `nb`).

use std::path::{Path, PathBuf};

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient, NbError, NoteTarget};

use crate::common::with_isolated_env;

fn config_for(
    env: &NbTestEnv,
    notebook: Option<&str>,
    create_notebook: bool,
) -> Config {
    Config {
        notebook: Some(notebook.unwrap_or(env.notebook()).to_string()),
        create_notebook,
        allow_top_level_notes: true,
        disable_git_signing: true,
        ..Config::default()
    }
}

fn head_of(env: &NbTestEnv, notebook: &str) -> String {
    let root = env.nb_dir().join(notebook);
    let mut cmd = std::process::Command::new("git");
    nb_api::scrub_git_env_std(&mut cmd);
    cmd.current_dir(&root).args(["rev-parse", "HEAD"]);
    let out = cmd.output().expect("git rev-parse");
    assert!(out.status.success(), "git rev-parse failed: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn status_of(env: &NbTestEnv, notebook: &str) -> String {
    let root = env.nb_dir().join(notebook);
    let mut cmd = std::process::Command::new("git");
    nb_api::scrub_git_env_std(&mut cmd);
    cmd.current_dir(&root).args(["status", "--porcelain"]);
    let out = cmd.output().expect("git status");
    assert!(out.status.success(), "git status failed: {out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn write_source(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write source");
    path
}

#[tokio::test]
async fn import_stores_text_byte_identical() {
    let env = NbTestEnv::hermetic().expect("native fixture");
    let body = "body line 1\nbody line 2\n";
    let src = write_source(env.working_dir(), "greeting.md", body.as_bytes());
    with_isolated_env(&env, false, || async {
        let client =
            NbClient::new(&config_for(&env, None, false)).expect("client");
        let outcome = client
            .import_note(&src, None, None, None)
            .await
            .expect("import");
        assert!(outcome.commit_created);
        let shown = client.show_note("greeting.md", None).await.expect("show");
        assert_eq!(shown.body, body);
        let raw = client
            .read_note_source_bytes(NoteTarget::path("greeting.md"), None)
            .await
            .expect("raw");
        assert_eq!(raw, body.as_bytes());
    })
    .await;
}

#[tokio::test]
async fn import_filename_override_wins_and_mangles() {
    let env = NbTestEnv::hermetic().expect("native fixture");
    let src =
        write_source(env.working_dir(), "My Report.TXT", b"override body\n");
    with_isolated_env(&env, false, || async {
        let client =
            NbClient::new(&config_for(&env, None, false)).expect("client");
        let outcome = client
            .import_note(&src, Some("docs"), Some("Custom Name.md"), None)
            .await
            .expect("import");
        assert!(outcome.commit_created);
        let shown = client
            .show_note("docs/custom_name.md", None)
            .await
            .expect("show");
        assert_eq!(shown.body, "override body\n");
    })
    .await;
}

#[tokio::test]
async fn import_default_name_mangles_stem_and_keeps_extension() {
    let env = NbTestEnv::hermetic().expect("native fixture");
    let src = write_source(env.working_dir(), "My Note.TXT", b"named body\n");
    with_isolated_env(&env, false, || async {
        let client =
            NbClient::new(&config_for(&env, None, false)).expect("client");
        client
            .import_note(&src, None, None, None)
            .await
            .expect("import");
        let shown = client.show_note("my_note.TXT", None).await.expect("show");
        assert_eq!(shown.body, "named body\n");
    })
    .await;
}

#[tokio::test]
async fn import_missing_file_is_io_and_creates_nothing() {
    let env = NbTestEnv::hermetic().expect("native fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env, Some("nope"), false))
            .expect("client");
        let missing = env.working_dir().join("absent.md");
        let err = client
            .import_note(&missing, None, None, Some("nope"))
            .await
            .expect_err("missing source must fail");
        assert!(
            matches!(err, NbError::Io { .. }),
            "expected Io, got: {err:?}"
        );
        assert!(
            !env.nb_dir().join("nope").exists(),
            "refused import must not create the notebook"
        );
    })
    .await;
}

#[tokio::test]
async fn import_punctuation_only_filename_refused_before_creation() {
    let env = NbTestEnv::hermetic().expect("native fixture");
    let src = write_source(env.working_dir(), "ok.md", b"punct body\n");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env, Some("nope"), true))
            .expect("client");
        // `---` mangles to empty, which must refuse before the
        // folder can silently become the basename — and before
        // any notebook creation, even with creation enabled.
        let err = client
            .import_note(&src, Some("docs"), Some("---"), Some("nope"))
            .await
            .expect_err("punctuation-only filename must fail");
        assert!(
            matches!(
                err,
                NbError::ValidationError { ref reason, .. }
                if reason.contains("mangled to empty")
            ),
            "expected mangled-empty ValidationError, got: {err:?}"
        );
        assert!(
            !env.nb_dir().join("nope").exists(),
            "refused import must not create the notebook"
        );
    })
    .await;
}

#[tokio::test]
async fn import_windows_reserved_name_refused_before_creation() {
    let env = NbTestEnv::hermetic().expect("native fixture");
    let src = write_source(env.working_dir(), "ok.md", b"reserved body\n");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env, Some("nope"), true))
            .expect("client");
        // `con.md` survives mangling but breaks Windows
        // checkouts: refuse pre-create with creation enabled.
        let err = client
            .import_note(&src, None, Some("con.md"), Some("nope"))
            .await
            .expect_err("reserved name must fail");
        assert!(
            matches!(
                err,
                NbError::ValidationError { ref reason, .. }
                if reason.contains("Windows-reserved")
            ),
            "expected reserved-name ValidationError, got: {err:?}"
        );
        assert!(
            !env.nb_dir().join("nope").exists(),
            "refused import must not create the notebook"
        );
    })
    .await;
}

#[tokio::test]
async fn import_windows_superscript_reserved_name_refused() {
    let env = NbTestEnv::hermetic().expect("native fixture");
    let src = write_source(env.working_dir(), "ok.md", b"ok body\n");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env, Some("nope"), true))
            .expect("client");
        // Win32 reserves COM¹/²/³ and LPT¹/²/³ (superscripts
        // U+00B9/U+00B2/U+00B3), including with an extension.
        // Creation is enabled on a missing notebook: refusal
        // must still precede creation and leave no directory.
        for name in ["com¹.md", "LPT².txt"] {
            let err = client
                .import_note(&src, None, Some(name), Some("nope"))
                .await
                .expect_err("superscript reserved name must fail");
            assert!(
                matches!(
                    err,
                    NbError::ValidationError { ref reason, .. }
                    if reason.contains("Windows-reserved")
                ),
                "expected reserved-name ValidationError, got: {err:?}"
            );
        }
        assert!(
            !env.nb_dir().join("nope").exists(),
            "refused import must not create the notebook"
        );
    })
    .await;
}

#[tokio::test]
async fn import_windows_invalid_chars_refused_before_creation() {
    let env = NbTestEnv::hermetic().expect("native fixture");
    // Preserved extensions keep their exact bytes, so the
    // invalid name arrives via override: creating `note.TX<T`
    // on disk would fail on Windows before import runs. `TX<T`
    // stays invalid past mangling and refuses with the
    // portability reason. Folders are never mangled, but `a:b`
    // refuses even earlier at the folder-field check — also
    // pre-create. Both leave no notebook behind.
    let src = write_source(env.working_dir(), "note.txt", b"charset body\n");
    write_source(env.working_dir(), "ok.md", b"ok body\n");
    let ok = env.working_dir().join("ok.md");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env, Some("nope"), true))
            .expect("client");
        let err = client
            .import_note(&src, None, Some("note.TX<T"), Some("nope"))
            .await
            .expect_err("Windows-invalid extension must fail");
        assert!(
            matches!(
                err,
                NbError::ValidationError { ref reason, .. }
                if reason.contains("Windows-invalid")
            ),
            "expected Windows-invalid ValidationError, got: {err:?}"
        );
        let err = client
            .import_note(&ok, Some("a:b"), Some("ok.md"), Some("nope"))
            .await
            .expect_err("colon folder must fail");
        assert!(
            matches!(err, NbError::ValidationError { .. }),
            "expected ValidationError, got: {err:?}"
        );
        // `a<b` passes the folder-field check (only `:` is
        // field-refused) and reaches the portability backstop.
        let err = client
            .import_note(&ok, Some("a<b"), Some("ok.md"), Some("nope"))
            .await
            .expect_err("bracket folder must fail");
        assert!(
            matches!(
                err,
                NbError::ValidationError { ref reason, .. }
                if reason.contains("Windows-invalid")
            ),
            "expected Windows-invalid ValidationError, got: {err:?}"
        );
        assert!(
            !env.nb_dir().join("nope").exists(),
            "refused import must not create the notebook"
        );
    })
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn import_fifo_refused_without_hanging() {
    let env = NbTestEnv::hermetic().expect("native fixture");
    let fifo = env.working_dir().join("pipe.md");
    let mk = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo");
    assert!(mk.success(), "mkfifo failed");
    with_isolated_env(&env, false, || async {
        let before = head_of(&env, env.notebook());
        let client =
            NbClient::new(&config_for(&env, None, false)).expect("client");
        // Nonblocking proof with a bound: a backstop thread
        // opens the FIFO read-write after 5s (`O_RDWR` never
        // blocks, and counts as the peer that releases a
        // blocking reader). A correct nonblocking open refuses
        // in well under the backstop delay; a regression to a
        // blocking open fails in bounded time instead of
        // hanging CI.
        let unblock = fifo.clone();
        let backstop = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let _ = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&unblock);
        });
        let start = std::time::Instant::now();
        let err = client
            .import_note(&fifo, None, None, None)
            .await
            .expect_err("FIFO source must fail");
        let elapsed = start.elapsed();
        assert!(
            matches!(err, NbError::ValidationError { .. }),
            "expected typed ValidationError, got: {err:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "open blocked on FIFO ({elapsed:?})"
        );
        backstop.join().expect("backstop thread");
        assert_eq!(head_of(&env, env.notebook()), before);
        assert!(status_of(&env, env.notebook()).trim().is_empty());
    })
    .await;
}

#[tokio::test]
async fn import_directory_refused_without_effects() {
    let env = NbTestEnv::hermetic().expect("native fixture");
    let dir = env.working_dir().join("adir");
    std::fs::create_dir_all(&dir).expect("mkdir source dir");
    with_isolated_env(&env, false, || async {
        let before = head_of(&env, env.notebook());
        let client =
            NbClient::new(&config_for(&env, None, false)).expect("client");
        let err = client
            .import_note(&dir, None, None, None)
            .await
            .expect_err("directory source must fail");
        assert!(
            matches!(err, NbError::ValidationError { .. }),
            "expected ValidationError, got: {err:?}"
        );
        assert_eq!(head_of(&env, env.notebook()), before);
        assert!(status_of(&env, env.notebook()).trim().is_empty());
    })
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn import_symlink_refused_without_following() {
    use std::os::unix::fs::symlink;
    let env = NbTestEnv::hermetic().expect("native fixture");
    let real = write_source(env.working_dir(), "real.md", b"symlink target\n");
    let link = env.working_dir().join("link.md");
    symlink(&real, &link).expect("symlink source");
    with_isolated_env(&env, false, || async {
        let before = head_of(&env, env.notebook());
        let client =
            NbClient::new(&config_for(&env, None, false)).expect("client");
        let err = client
            .import_note(&link, None, None, None)
            .await
            .expect_err("symlink source must fail");
        assert!(
            matches!(err, NbError::ValidationError { .. }),
            "expected ValidationError, got: {err:?}"
        );
        assert_eq!(head_of(&env, env.notebook()), before);
        assert!(status_of(&env, env.notebook()).trim().is_empty());
    })
    .await;
}

#[tokio::test]
async fn import_binary_refused_without_effects() {
    let env = NbTestEnv::hermetic().expect("native fixture");
    let src = write_source(
        env.working_dir(),
        "blob.bin",
        &[0x89, 0x50, 0x4e, 0x47, 0xff, 0x00],
    );
    with_isolated_env(&env, false, || async {
        let before = head_of(&env, env.notebook());
        let client =
            NbClient::new(&config_for(&env, None, false)).expect("client");
        let err = client
            .import_note(&src, None, None, None)
            .await
            .expect_err("binary source must fail");
        assert!(
            matches!(err, NbError::ValidationError { .. }),
            "expected ValidationError, got: {err:?}"
        );
        assert_eq!(head_of(&env, env.notebook()), before);
        assert!(status_of(&env, env.notebook()).trim().is_empty());
    })
    .await;
}

#[tokio::test]
async fn import_url_shape_refused_without_network() {
    let env = NbTestEnv::hermetic().expect("native fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env, Some("nope"), false))
            .expect("client");
        let err = client
            .import_note(
                Path::new("https://example.com/note.md"),
                None,
                None,
                Some("nope"),
            )
            .await
            .expect_err("URL source must fail");
        assert!(
            matches!(err, NbError::ValidationError { .. }),
            "expected ValidationError, got: {err:?}"
        );
        assert!(
            !env.nb_dir().join("nope").exists(),
            "URL refusal must precede notebook creation"
        );
    })
    .await;
}

#[tokio::test]
async fn import_taken_destination_collides_and_preserves() {
    let env = NbTestEnv::hermetic().expect("native fixture");
    let src = write_source(env.working_dir(), "dup.md", b"first body\n");
    with_isolated_env(&env, false, || async {
        let client =
            NbClient::new(&config_for(&env, None, false)).expect("client");
        client
            .import_note(&src, None, None, None)
            .await
            .expect("first");
        let before = head_of(&env, env.notebook());
        let err = client
            .import_note(&src, None, None, None)
            .await
            .expect_err("second import must collide");
        assert!(
            matches!(err, NbError::PathCollision { .. }),
            "expected PathCollision, got: {err:?}"
        );
        assert_eq!(head_of(&env, env.notebook()), before);
        let shown = client.show_note("dup.md", None).await.expect("show");
        assert_eq!(shown.body, "first body\n");
    })
    .await;
}

#[tokio::test]
async fn import_destination_cannot_escape() {
    let env = NbTestEnv::hermetic().expect("native fixture");
    let src = write_source(env.working_dir(), "ok.md", b"escape body\n");
    with_isolated_env(&env, false, || async {
        let before = head_of(&env, env.notebook());
        let client =
            NbClient::new(&config_for(&env, None, false)).expect("client");
        for (folder, filename) in [
            (Some("../escape"), None),
            (None, Some("../../evil.md")),
            (Some("/abs"), None),
        ] {
            let err = client
                .import_note(&src, folder, filename, None)
                .await
                .expect_err("escaping destination must fail");
            assert!(
                matches!(err, NbError::ValidationError { .. }),
                "expected ValidationError, got: {err:?}"
            );
        }
        assert_eq!(head_of(&env, env.notebook()), before);
        assert!(status_of(&env, env.notebook()).trim().is_empty());
    })
    .await;
}

#[tokio::test]
async fn import_post_create_collision_persists_notebook() {
    let env = NbTestEnv::hermetic().expect("native fixture");
    let src = write_source(env.working_dir(), "fresh.md", b"fresh body\n");
    with_isolated_env(&env, false, || async {
        // Racing writer takes the import destination during the
        // commit: the import refuses, the new notebook persists
        // with the legitimate external change.
        unsafe {
            std::env::set_var("NB_API_SIMULATE_EXTERNAL_WRITER", "1");
            std::env::set_var(
                "NB_API_SIMULATE_EXTERNAL_SAME_PATH",
                "fresh.md",
            );
        }
        let client = NbClient::new(&config_for(&env, Some("freshnb"), true))
            .expect("client");
        let err = client
            .import_note(&src, None, None, Some("freshnb"))
            .await
            .expect_err("raced import must fail");
        unsafe {
            std::env::remove_var("NB_API_SIMULATE_EXTERNAL_WRITER");
            std::env::remove_var("NB_API_SIMULATE_EXTERNAL_SAME_PATH");
        }
        // The gate-bypassing writer committed mid-commit, so the
        // pipeline honestly reports IndeterminateCommit (its own
        // checkpoint never ran: materialize refused first).
        // Targets present at snapshot refuse PathCollision
        // instead (see the taken-destination test above).
        assert!(
            matches!(
                err,
                NbError::IndeterminateCommit {
                    post_revision_observed: Some(_),
                    ..
                }
            ),
            "expected IndeterminateCommit with observed revision, got: {err:?}"
        );
        let root = env.nb_dir().join("freshnb");
        assert!(root.is_dir(), "created notebook persists");
        assert!(
            !head_of(&env, "freshnb").trim().is_empty(),
            "external commit survives"
        );
        let external =
            std::fs::read(root.join("fresh.md")).expect("external file");
        assert!(
            external.starts_with(b"# External"),
            "no silent overwrite of the racing writer"
        );
    })
    .await;
}
