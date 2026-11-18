//! Notebook store resolution: explicit `NB_DIR`, else parsed
//! `~/.nbrc`, else `~/.nb`.
//!
//! These tests exercise the public client surface under caller
//! controlled `HOME`/`NB_DIR`/`NBRC_PATH`, proving the native
//! lookup matches `nb` startup behavior (source rc, then
//! `NB_DIR="${NB_DIR:-$HOME/.nb}"`) without spawning a shell.
//! `NB_DIR` set in the process always wins and skips every rc
//! read; `require_explicit_nb_dir` restores strict resolution for
//! embedders that assign the store themselves.

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient};

use crate::common::{EnvSnapshot, lock_env};

fn config_for(notebook: &str) -> Config {
    Config {
        notebook: Some(notebook.to_string()),
        create_notebook: false,
        allow_top_level_notes: true,
        disable_git_signing: true,
        ..Config::default()
    }
}

fn strict_config_for(notebook: &str) -> Config {
    Config {
        require_explicit_nb_dir: true,
        ..config_for(notebook)
    }
}

/// Hand-build a git notebook at `path` (no `nb` spawn): `git
/// init`, empty `.index`, one initial commit.
fn git_init_notebook(path: &Path) {
    std::fs::create_dir_all(path).expect("notebook dir");
    let mut init = StdCommand::new("git");
    nb_api::scrub_git_env_std(&mut init);
    init.current_dir(path).arg("init");
    assert!(init.output().expect("git init runs").status.success());
    std::fs::write(path.join(".index"), b"").expect("empty index");
    for args in [
        vec![
            "-c",
            "user.name=nb-api-test",
            "-c",
            "user.email=nb-api-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "add",
            "--",
            ".index",
        ],
        vec![
            "-c",
            "user.name=nb-api-test",
            "-c",
            "user.email=nb-api-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "[nb] Initialize",
            "--no-gpg-sign",
        ],
    ] {
        let mut git = StdCommand::new("git");
        nb_api::scrub_git_env_std(&mut git);
        git.current_dir(path).args(&args);
        assert!(
            git.output().expect("git runs").status.success(),
            "git {args:?} failed"
        );
    }
}

/// Run `f` with `ENV_LOCK` held and `NB_DIR`/`HOME`/`NBRC_PATH`
/// restored afterwards. Returns whatever `f` returns.
#[allow(clippy::await_holding_lock)]
async fn with_store_env<F, Fut, R>(f: F) -> R
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = R>,
{
    let _guard = lock_env();
    let _snap = EnvSnapshot::capture(&["NB_DIR", "HOME", "NBRC_PATH"]);
    f().await
}

/// An `rc` value in `~/.nbrc` resolves when `NB_DIR` is unset.
#[tokio::test]
async fn rc_nb_dir_resolves_without_explicit_nb_dir() {
    let env = NbTestEnv::new().expect("fixture");
    let home = tempfile::Builder::new()
        .prefix("nb-home-")
        .tempdir()
        .expect("home");
    std::fs::write(
        home.path().join(".nbrc"),
        format!("export NB_DIR={}\n", env.nb_dir().display()),
    )
    .expect("rc");
    with_store_env(|| async {
        unsafe {
            std::env::remove_var("NB_DIR");
            std::env::set_var("HOME", home.path());
        }
        let client =
            NbClient::new(&config_for(env.notebook())).expect("client");
        let root = client
            .show_notebook_path(Some(env.notebook()))
            .await
            .expect("rc-resolved notebook resolves");
        assert_eq!(root, env.nb_dir().join(env.notebook()));
    })
    .await;
}

/// No rc file at all: the `~/.nb` default resolves.
#[tokio::test]
async fn missing_rc_falls_back_to_home_dot_nb() {
    let home = tempfile::Builder::new()
        .prefix("nb-home-")
        .tempdir()
        .expect("home");
    git_init_notebook(&home.path().join(".nb/plain"));
    with_store_env(|| async {
        unsafe {
            std::env::remove_var("NB_DIR");
            std::env::set_var("HOME", home.path());
        }
        let client = NbClient::new(&config_for("plain")).expect("client");
        let root = client
            .show_notebook_path(Some("plain"))
            .await
            .expect("default store resolves");
        assert_eq!(root, home.path().join(".nb/plain"));
    })
    .await;
}

/// An empty `NB_DIR` counts as unset (`:-` semantics).
#[tokio::test]
async fn empty_nb_dir_falls_through_to_default() {
    let home = tempfile::Builder::new()
        .prefix("nb-home-")
        .tempdir()
        .expect("home");
    git_init_notebook(&home.path().join(".nb/plain"));
    with_store_env(|| async {
        unsafe {
            std::env::set_var("NB_DIR", "");
            std::env::set_var("HOME", home.path());
        }
        let client = NbClient::new(&config_for("plain")).expect("client");
        let root = client
            .show_notebook_path(Some("plain"))
            .await
            .expect("empty NB_DIR falls through");
        assert_eq!(root, home.path().join(".nb/plain"));
    })
    .await;
}

/// An explicit `NB_DIR` wins and performs no rc I/O: the rc path
/// is a directory (unreadable as a file) yet resolution succeeds.
#[tokio::test]
async fn explicit_nb_dir_skips_rc_io() {
    let env = NbTestEnv::new().expect("fixture");
    let home = tempfile::Builder::new()
        .prefix("nb-home-")
        .tempdir()
        .expect("home");
    std::fs::create_dir(home.path().join(".nbrc")).expect("rc as dir");
    with_store_env(|| async {
        unsafe {
            std::env::set_var("NB_DIR", env.nb_dir());
            std::env::set_var("HOME", home.path());
        }
        let client =
            NbClient::new(&config_for(env.notebook())).expect("client");
        let root = client
            .show_notebook_path(Some(env.notebook()))
            .await
            .expect("explicit NB_DIR resolves without rc");
        assert_eq!(root, env.nb_dir().join(env.notebook()));
    })
    .await;
}

/// The last supported assignment wins; earlier misses do not abort.
#[tokio::test]
async fn rc_last_assignment_wins() {
    let env = NbTestEnv::new().expect("fixture");
    let home = tempfile::Builder::new()
        .prefix("nb-home-")
        .tempdir()
        .expect("home");
    std::fs::write(
        home.path().join(".nbrc"),
        format!(
            "# comment\nNB_DIR=/nonexistent-store\nNB_DIR={}\n",
            env.nb_dir().display()
        ),
    )
    .expect("rc");
    with_store_env(|| async {
        unsafe {
            std::env::remove_var("NB_DIR");
            std::env::set_var("HOME", home.path());
        }
        let client =
            NbClient::new(&config_for(env.notebook())).expect("client");
        let root = client
            .show_notebook_path(Some(env.notebook()))
            .await
            .expect("last assignment resolves");
        assert_eq!(root, env.nb_dir().join(env.notebook()));
    })
    .await;
}

/// A write path creates inside the resolved store, not the cwd.
#[tokio::test]
async fn write_path_creates_inside_resolved_store() {
    let home = tempfile::Builder::new()
        .prefix("nb-home-")
        .tempdir()
        .expect("home");
    let store = home.path().join("picked");
    std::fs::create_dir_all(&store).expect("store");
    std::fs::write(
        home.path().join(".gitconfig"),
        "[user]\n\tname = nb-api tests\n\temail = nb-api@localhost\n",
    )
    .expect("git identity for nb");
    std::fs::write(
        home.path().join(".nbrc"),
        format!("NB_DIR={}\n", store.display()),
    )
    .expect("rc");
    with_store_env(|| async {
        unsafe {
            std::env::remove_var("NB_DIR");
            std::env::set_var("HOME", home.path());
        }
        let client = NbClient::new(&Config {
            notebook: Some("fresh".to_string()),
            create_notebook: true,
            ..Config::default()
        })
        .expect("client");
        client
            .transaction(Some("fresh"))
            .await
            .expect("ensure creates")
            .commit()
            .await
            .expect("commit");
        assert!(
            store.join("fresh/.git").is_dir(),
            "notebook created in rc store"
        );
    })
    .await;
}

/// An unreadable rc file fails instead of silently defaulting.
#[cfg(unix)]
#[tokio::test]
async fn unreadable_rc_fails_visibly() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::Builder::new()
        .prefix("nb-home-")
        .tempdir()
        .expect("home");
    let rc = home.path().join(".nbrc");
    std::fs::write(&rc, "NB_DIR=/whatever\n").expect("rc");
    std::fs::set_permissions(&rc, std::fs::Permissions::from_mode(0o000))
        .expect("chmod rc");
    if std::fs::read_to_string(&rc).is_ok() {
        std::fs::set_permissions(&rc, std::fs::Permissions::from_mode(0o644))
            .expect("restore rc");
        return;
    }
    with_store_env(|| async {
        unsafe {
            std::env::remove_var("NB_DIR");
            std::env::set_var("HOME", home.path());
        }
        let client = NbClient::new(&config_for("plain")).expect("client");
        let err = format!(
            "{:?}",
            client
                .show_notebook_path(Some("plain"))
                .await
                .expect_err("unreadable rc must fail")
        );
        assert!(err.contains("Io"), "expected Io, got: {err:?}");
    })
    .await;
    std::fs::set_permissions(&rc, std::fs::Permissions::from_mode(0o644))
        .expect("restore rc");
}

/// Unsupported `NB_DIR` syntax fails instead of falling through
/// to the default store.
#[tokio::test]
async fn unsupported_rc_syntax_fails_visibly() {
    for rc_text in [
        "export NB_DIR=$(hostname)\n",
        "NB_DIR=~/`hostname`\n",
        "if [ x ]; then NB_DIR=/x; fi\n",
        "NB_DIR =/x\n",
    ] {
        let home = tempfile::Builder::new()
            .prefix("nb-home-")
            .tempdir()
            .expect("home");
        git_init_notebook(&home.path().join(".nb/plain"));
        std::fs::write(home.path().join(".nbrc"), rc_text).expect("rc");
        with_store_env(|| async {
            unsafe {
                std::env::remove_var("NB_DIR");
                std::env::set_var("HOME", home.path());
            }
            let client = NbClient::new(&config_for("plain")).expect("client");
            let err = format!(
                "{:?}",
                client
                    .show_notebook_path(Some("plain"))
                    .await
                    .expect_err("unsupported rc must fail")
            );
            assert!(
                err.contains("ValidationError"),
                "rc {rc_text:?} must fail visibly, got: {err:?}"
            );
        })
        .await;
    }
}

/// A relative rc value is invalid, not a silent miss.
#[tokio::test]
async fn relative_rc_value_is_an_error() {
    let home = tempfile::Builder::new()
        .prefix("nb-home-")
        .tempdir()
        .expect("home");
    std::fs::write(home.path().join(".nbrc"), "NB_DIR=relative/path\n")
        .expect("rc");
    with_store_env(|| async {
        unsafe {
            std::env::remove_var("NB_DIR");
            std::env::set_var("HOME", home.path());
        }
        let client = NbClient::new(&config_for("plain")).expect("client");
        let err = format!(
            "{:?}",
            client
                .show_notebook_path(Some("plain"))
                .await
                .expect_err("relative rc must fail")
        );
        assert!(err.contains("ValidationError"), "got: {err:?}");
    })
    .await;
}

/// `NBRC_PATH` redirects the rc lookup; the default path is then
/// ignored even when it holds hostile syntax.
#[tokio::test]
async fn nbrc_path_redirects_rc_lookup() {
    let env = NbTestEnv::new().expect("fixture");
    let home = tempfile::Builder::new()
        .prefix("nb-home-")
        .tempdir()
        .expect("home");
    std::fs::write(home.path().join(".nbrc"), "export NB_DIR=$(hostname)\n")
        .expect("hostile default rc");
    let custom = home.path().join("custom.rc");
    std::fs::write(
        &custom,
        format!("export NB_DIR={}\n", env.nb_dir().display()),
    )
    .expect("custom rc");
    with_store_env(|| async {
        unsafe {
            std::env::remove_var("NB_DIR");
            std::env::set_var("HOME", home.path());
            std::env::set_var("NBRC_PATH", &custom);
        }
        let client =
            NbClient::new(&config_for(env.notebook())).expect("client");
        let root = client
            .show_notebook_path(Some(env.notebook()))
            .await
            .expect("NBRC_PATH rc resolves");
        assert_eq!(root, env.nb_dir().join(env.notebook()));
        unsafe {
            std::env::remove_var("NBRC_PATH");
        }
    })
    .await;
}

/// Missing `HOME` with no explicit `NB_DIR` is an error, not an
/// ambient-cwd fallback.
#[tokio::test]
async fn missing_home_is_an_error() {
    with_store_env(|| async {
        unsafe {
            std::env::remove_var("NB_DIR");
            std::env::remove_var("HOME");
        }
        let client = NbClient::new(&config_for("plain")).expect("client");
        let err = format!(
            "{:?}",
            client
                .show_notebook_path(Some("plain"))
                .await
                .expect_err("missing HOME must fail")
        );
        assert!(
            err.contains("ValidationError"),
            "expected configuration failure, got: {err:?}"
        );
    })
    .await;
}

/// Strict mode rejects an unset `NB_DIR` even with a valid rc.
#[tokio::test]
async fn strict_mode_rejects_unset_nb_dir() {
    let env = NbTestEnv::new().expect("fixture");
    let home = tempfile::Builder::new()
        .prefix("nb-home-")
        .tempdir()
        .expect("home");
    std::fs::write(
        home.path().join(".nbrc"),
        format!("export NB_DIR={}\n", env.nb_dir().display()),
    )
    .expect("rc");
    with_store_env(|| async {
        unsafe {
            std::env::remove_var("NB_DIR");
            std::env::set_var("HOME", home.path());
        }
        let client =
            NbClient::new(&strict_config_for(env.notebook())).expect("client");
        let err = format!(
            "{:?}",
            client
                .show_notebook_path(Some(env.notebook()))
                .await
                .expect_err("strict mode must refuse")
        );
        assert!(
            err.contains("strict"),
            "expected strict configuration failure, got: {err:?}"
        );
    })
    .await;
}

/// Strict mode with an explicit `NB_DIR` never reads the rc.
#[tokio::test]
async fn strict_mode_with_explicit_nb_dir_skips_rc() {
    let env = NbTestEnv::new().expect("fixture");
    let home = tempfile::Builder::new()
        .prefix("nb-home-")
        .tempdir()
        .expect("home");
    std::fs::create_dir(home.path().join(".nbrc")).expect("rc as dir");
    with_store_env(|| async {
        unsafe {
            std::env::set_var("NB_DIR", env.nb_dir());
            std::env::set_var("HOME", home.path());
        }
        let client =
            NbClient::new(&strict_config_for(env.notebook())).expect("client");
        let root = client
            .show_notebook_path(Some(env.notebook()))
            .await
            .expect("explicit NB_DIR resolves under strict mode");
        assert_eq!(root, env.nb_dir().join(env.notebook()));
    })
    .await;
}

/// Rc commands never execute: a `touch` trap plus a valid
/// assignment resolves while the marker stays absent.
#[tokio::test]
async fn rc_commands_never_execute() {
    let env = NbTestEnv::new().expect("fixture");
    let home = tempfile::Builder::new()
        .prefix("nb-home-")
        .tempdir()
        .expect("home");
    let marker = home.path().join("should-not-exist");
    std::fs::write(
        home.path().join(".nbrc"),
        format!(
            "touch {}\nexport NB_DIR={}\n",
            marker.display(),
            env.nb_dir().display()
        ),
    )
    .expect("rc");
    with_store_env(|| async {
        unsafe {
            std::env::remove_var("NB_DIR");
            std::env::set_var("HOME", home.path());
        }
        let client =
            NbClient::new(&config_for(env.notebook())).expect("client");
        client
            .show_notebook_path(Some(env.notebook()))
            .await
            .expect("rc with commands resolves");
        assert!(
            !marker.exists(),
            "rc command executed: {}",
            marker.display()
        );
    })
    .await;
}

/// Independent oracle: `bash` sourcing agrees with the parser on
/// the supported subset (quoting, comments, ordering, `$HOME`,
/// `~/`). Unix-only: needs a Bourne shell.
#[cfg(unix)]
#[tokio::test]
async fn bash_oracle_agrees_on_supported_subset() {
    let home = tempfile::Builder::new()
        .prefix("nb-home-")
        .tempdir()
        .expect("home");
    let rc = home.path().join("sample.rc");
    std::fs::write(
        &rc,
        "# leading comment\nNB_DIR=\"$HOME/a b\"\nNB_DIR=${HOME}/picked\n",
    )
    .expect("rc");
    let expected = home.path().join("picked");
    git_init_notebook(&expected.join("plain"));
    let mut oracle = StdCommand::new("bash");
    oracle.env("HOME", home.path());
    oracle.args([
        "-c".to_string(),
        "source \"$1\"; printf '%s' \"$NB_DIR\"".to_string(),
        "oracle".to_string(),
        rc.display().to_string(),
    ]);
    let out = oracle.output().expect("bash oracle runs");
    assert!(out.status.success(), "bash oracle failed: {out:?}");
    let oracle_dir = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(
        PathBuf::from(&oracle_dir),
        expected,
        "oracle disagrees with fixture layout"
    );
    with_store_env(|| async {
        unsafe {
            std::env::remove_var("NB_DIR");
            std::env::set_var("HOME", home.path());
            std::env::set_var("NBRC_PATH", &rc);
        }
        let client = NbClient::new(&config_for("plain")).expect("client");
        let root = client
            .show_notebook_path(Some("plain"))
            .await
            .expect("oracle rc resolves");
        assert_eq!(root, expected.join("plain"));
        unsafe {
            std::env::remove_var("NBRC_PATH");
        }
    })
    .await;
}
