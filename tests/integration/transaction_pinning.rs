//! Pinned-store regression tests: the transaction resolves the
//! notebook store exactly once at build and commits pinned output
//! without re-resolving. A concurrent rc/env edit or a swapped
//! notebook must not move, leak, or traverse the commit.

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient};

use crate::common::with_isolated_env;

fn config_for(env: &NbTestEnv) -> Config {
    Config {
        notebook: Some(env.notebook().to_string()),
        create_notebook: true,
        allow_top_level_notes: true,
        disable_git_signing: true,
        ..Config::default()
    }
}

fn git_capture(dir: &std::path::Path, args: &[&str]) -> String {
    let mut cmd = std::process::Command::new("git");
    nb_api::scrub_git_env_std(&mut cmd);
    cmd.current_dir(dir).args(args);
    let out = cmd.output().expect("git");
    assert!(out.status.success(), "git {args:?} failed: {out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Pinned-store regression: a notebook replaced by a symlink
/// between planning and commit refuses instead of writing through
/// it (Unix-only: symlink fixtures need platform privileges).
#[cfg(unix)]
#[tokio::test]
async fn commit_rejects_post_plan_symlink_replacement() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = env.nb_dir().join(env.notebook());
        let mut tx = client.transaction(None).await.expect("tx");
        tx.add_note("swap.md", None, "v0\n", &[]).expect("plan");
        // Swap the notebook directory for a symlink before commit.
        let elsewhere = env.nb_dir().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("decoy dir");
        let target = root.join("swap.md");
        assert!(!target.exists(), "nothing written before commit");
        let nb_path = root.clone();
        std::fs::remove_dir_all(&nb_path).expect("remove notebook");
        std::os::unix::fs::symlink(&elsewhere, &nb_path)
            .expect("symlink swap");
        let err = tx.commit().await.expect_err("swapped root must refuse");
        assert!(
            format!("{err:?}").contains("symlink")
                || format!("{err:?}").contains("changed during transaction"),
            "unexpected error: {err:?}"
        );
        assert!(
            !elsewhere.join("swap.md").exists(),
            "no write through the swapped symlink"
        );
    })
    .await;
}

/// Pinned-store regression: an rc edit between planning and commit
/// does not move the commit — the transaction lands in the store
/// resolved when it was built.
#[tokio::test]
async fn commit_lands_in_pinned_store_despite_rc_edit() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        // SAFETY: serialized by with_isolated_env; restored on exit.
        unsafe { std::env::remove_var("NB_DIR") };
        let home = env.home_dir();
        let store_a = home.join("store-a");
        let store_b = home.join("store-b");
        std::fs::create_dir_all(&store_a).expect("store a");
        std::fs::create_dir_all(&store_b).expect("store b");
        let rc = home.join(".nbrc");
        std::fs::write(&rc, "NB_DIR=\"$HOME/store-a\"\n").expect("rc a");
        // SAFETY: serialized by with_isolated_env; restored on exit.
        unsafe {
            std::env::set_var("NBRC_PATH", rc.to_str().expect("rc path"));
        }
        let config = config_for(&env);
        let client = NbClient::new(&config).expect("client");
        let mut tx = client.transaction(Some("pin-nb")).await.expect("tx");
        tx.add_note("pinned.md", None, "v0\n", &[]).expect("plan");
        // Swap the rc out from under the open transaction.
        std::fs::write(&rc, "NB_DIR=\"$HOME/store-b\"\n").expect("rc b");
        tx.commit().await.expect("commit lands in pinned store");
        assert!(
            store_a.join("pin-nb").join("pinned.md").exists(),
            "note lands in the store pinned at build time"
        );
        assert!(
            !store_b.join("pin-nb").join("pinned.md").exists(),
            "nothing lands in the post-edit store"
        );
    })
    .await;
}

/// Pinned-store regression: same guarantee when the flip is the
/// `NB_DIR` variable itself rather than rc content.
#[tokio::test]
async fn commit_lands_in_pinned_store_despite_nb_dir_flip() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let store_a = env.nb_dir().join("store-a");
        let store_b = env.nb_dir().join("store-b");
        std::fs::create_dir_all(&store_a).expect("store a");
        std::fs::create_dir_all(&store_b).expect("store b");
        // SAFETY: serialized by with_isolated_env; restored on exit.
        unsafe { std::env::set_var("NB_DIR", &store_a) };
        let config = config_for(&env);
        let client = NbClient::new(&config).expect("client");
        let mut tx = client.transaction(Some("pin-nb")).await.expect("tx");
        tx.add_note("pinned.md", None, "v0\n", &[]).expect("plan");
        // SAFETY: serialized by with_isolated_env; restored on exit.
        unsafe { std::env::set_var("NB_DIR", &store_b) };
        tx.commit().await.expect("commit lands in pinned store");
        assert!(
            store_a.join("pin-nb").join("pinned.md").exists(),
            "note lands in the store pinned at build time"
        );
        assert!(
            !store_b.join("pin-nb").join("pinned.md").exists(),
            "nothing lands in the post-flip store"
        );
    })
    .await;
}

/// Pinned-notebook revalidation: a `.git/HEAD` corrupted after
/// planning refuses instead of letting git rediscover an ancestor
/// repository, and the committed ancestor is left untouched.
#[tokio::test]
async fn commit_refuses_malformed_child_under_committed_ancestor() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        // Committed ancestor: walk-up *would* find a repo here, so
        // a refusal proves the bound check, not a lucky miss.
        let ancestor = env.nb_dir().join("ancestor-store");
        std::fs::create_dir_all(&ancestor).expect("ancestor dir");
        git_capture(&ancestor, &["init", "-q"]);
        git_capture(&ancestor, &["commit", "--allow-empty", "-m", "init"]);
        let ancestor_head = git_capture(&ancestor, &["rev-parse", "HEAD"]);
        // SAFETY: serialized by with_isolated_env; restored on exit.
        unsafe { std::env::set_var("NB_DIR", &ancestor) };
        let config = config_for(&env);
        let client = NbClient::new(&config).expect("client");
        let mut tx = client.transaction(Some("victim")).await.expect("tx");
        tx.add_note("pinned.md", None, "v0\n", &[]).expect("plan");
        // Corrupt the child's HEAD after planning (Tier-1's repro
        // shape: marker remains, content is garbage).
        std::fs::write(
            ancestor.join("victim").join(".git").join("HEAD"),
            "abc123\n",
        )
        .expect("corrupt child HEAD");
        let err = tx.commit().await.expect_err("malformed child refuses");
        assert!(
            format!("{err:?}").contains("no resolvable HEAD")
                || format!("{err:?}").contains("changed during transaction"),
            "unexpected error: {err:?}"
        );
        // Ancestor untouched: same HEAD, clean tree, nothing staged.
        assert_eq!(
            git_capture(&ancestor, &["rev-parse", "HEAD"]),
            ancestor_head,
            "ancestor HEAD unchanged"
        );
        assert!(
            git_capture(
                &ancestor,
                &["status", "--porcelain", "--untracked-files=no"]
            )
            .trim()
            .is_empty(),
            "ancestor tracked tree clean"
        );
        assert!(
            !ancestor.join("victim").join("pinned.md").exists(),
            "nothing written to the malformed child"
        );
    })
    .await;
}

/// Pinned-notebook revalidation: swapping an ancestor directory
/// for a symlink drifts the canonical path, which refuses even
/// though every component still resolves.
#[tokio::test]
#[cfg(unix)]
async fn commit_rejects_canonical_drift_from_symlinked_parent() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let store = env.nb_dir().join("drift-store");
        std::fs::create_dir_all(&store).expect("store dir");
        // SAFETY: serialized by with_isolated_env; restored on exit.
        unsafe { std::env::set_var("NB_DIR", &store) };
        let config = config_for(&env);
        let client = NbClient::new(&config).expect("client");
        let mut tx = client.transaction(Some("drift-nb")).await.expect("tx");
        tx.add_note("pinned.md", None, "v0\n", &[]).expect("plan");
        // Move the store aside and leave a symlink at its path:
        // same content, drifted canonical form.
        let moved = env.nb_dir().join("drift-store-real");
        std::fs::rename(&store, &moved).expect("move store aside");
        std::os::unix::fs::symlink(&moved, &store).expect("symlink parent");
        let err = tx.commit().await.expect_err("drifted root refuses");
        assert!(
            format!("{err:?}").contains("canonicalized away")
                || format!("{err:?}").contains("changed during transaction"),
            "unexpected error: {err:?}"
        );
        assert!(
            !moved.join("drift-nb").join("pinned.md").exists(),
            "nothing written through the drifted path"
        );
    })
    .await;
}

/// Qualified one-shot regression: a `notebook:selector` id resolves
/// against the probed notebook path, not the store root. Pre-fix,
/// `ensure_existing_notebook` returned the store root, so the
/// stripped selector missed (or hit the wrong notebook).
#[tokio::test]
async fn qualified_oneshot_delete_targets_embedded_notebook() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let config = config_for(&env);
        let client = NbClient::new(&config).expect("client");
        client
            .create_notebook(Some("other"))
            .await
            .expect("other nb");
        for nb in [env.notebook(), "other"] {
            let mut tx = client.transaction(Some(nb)).await.expect("tx");
            tx.add_note("shared.md", None, "body\n", &[]).expect("plan");
            tx.commit().await.expect("commit");
        }
        // No notebook field: the embedded notebook must win, and the
        // delete must land in its directory — not the store root or
        // the sibling notebook.
        client
            .delete_note("other:shared.md", None)
            .await
            .expect("qualified delete");
        assert!(
            !env.nb_dir().join("other").join("shared.md").exists(),
            "embedded notebook file is deleted"
        );
        assert!(
            env.nb_dir().join(env.notebook()).join("shared.md").exists(),
            "sibling notebook is untouched"
        );
        assert!(
            !env.nb_dir().join("shared.md").exists(),
            "nothing resolves at the store root"
        );
    })
    .await;
}
