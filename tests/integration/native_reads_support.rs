//! Shared helpers for the native read-path integration tests:
//! client config, frozen-oracle tree hashing, corpus seeding, and
//! a thin live-`nb` oracle for resolution parity.

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient};

pub(crate) fn config_for(env: &NbTestEnv) -> Config {
    Config {
        notebook: Some(env.notebook().to_string()),
        create_notebook: false,
        allow_top_level_notes: true,
        disable_git_signing: true,
        ..Config::default()
    }
}

/// Thin live-`nb` oracle: shells the real CLI for resolution target
/// and textual classification only. Native assertions compare path,
/// bytes, and error shape against this oracle; structured fields
/// carry explicit native expectations.
pub(crate) struct NbOracle {
    nb_dir: std::path::PathBuf,
    home: std::path::PathBuf,
    notebook: String,
}

impl NbOracle {
    pub(crate) fn new(env: &NbTestEnv) -> Self {
        Self {
            nb_dir: env.nb_dir().to_path_buf(),
            home: env.home_dir().to_path_buf(),
            notebook: env.notebook().to_string(),
        }
    }

    fn nb(&self, args: &[&str]) -> Result<String, String> {
        let mut cmd = std::process::Command::new("nb");
        cmd.env("NB_DIR", &self.nb_dir);
        cmd.env("HOME", &self.home);
        // Mirror `git_env` scrubbing: no `GIT_*` redirect may leak
        // into the oracle child.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                cmd.env_remove(key);
            }
        }
        let out = cmd.args(args).output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Notebook-relative resolution target, like the removed
    /// `nb show --path` fallback returned it, relativized. `None`
    /// when `nb` fails or prints nothing.
    pub(crate) fn resolve_path(&self, selector: &str) -> Option<String> {
        let qualified = if selector.contains(':') {
            selector.to_string()
        } else {
            format!("{}:{selector}", self.notebook)
        };
        let out = self
            .nb(&["show", &qualified, "--path", "--no-color"])
            .ok()?;
        let path = out.trim();
        if path.is_empty() {
            return None;
        }
        std::path::Path::new(path)
            .strip_prefix(self.nb_dir.join(&self.notebook))
            .ok()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
    }

    /// Whether plain `nb show` succeeds for the selector (used
    /// for paired failure evidence: the oracle fails where the
    /// native path fails).
    pub(crate) fn show_ok(&self, selector: &str) -> bool {
        let qualified = if selector.contains(':') {
            selector.to_string()
        } else {
            format!("{}:{selector}", self.notebook)
        };
        self.nb(&["show", &qualified, "--no-color"]).is_ok()
    }

    /// Whether `nb` classifies the selector textual.
    pub(crate) fn is_textual(&self, selector: &str) -> bool {
        let qualified = if selector.contains(':') {
            selector.to_string()
        } else {
            format!("{}:{selector}", self.notebook)
        };
        self.nb(&["show", &qualified, "--type", "text", "--no-color"])
            .is_ok()
    }
}

/// Content-hash per relative path, for frozen-oracle snapshots
/// and movement diagnostics.
pub(crate) fn tree_listing(
    root: &std::path::Path,
) -> Result<Vec<(String, u64)>, String> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    fn walk(
        root: &std::path::Path,
        dir: &std::path::Path,
        out: &mut Vec<(String, Vec<u8>)>,
    ) -> Result<(), String> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                return Err(format!("oracle read_dir {}: {e}", dir.display()));
            },
        };
        for entry in entries {
            let entry = entry.map_err(|e| {
                format!("oracle entry in {}: {e}", dir.display())
            })?;
            let path = entry.path();
            let Ok(rel) = path
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
            else {
                continue;
            };
            if rel == ".git"
                || rel.starts_with(".git/")
                || rel == "deadbin"
                || rel.starts_with("deadbin/")
            {
                continue;
            }
            match std::fs::symlink_metadata(&path) {
                Ok(m) if m.file_type().is_symlink() => {
                    let target = std::fs::read_link(&path).map_err(|e| {
                        format!("oracle read_link {}: {e}", path.display())
                    })?;
                    out.push((
                        format!("link:{rel}"),
                        target.as_os_str().as_encoded_bytes().to_vec(),
                    ));
                },
                Ok(m) if m.is_dir() => {
                    out.push((format!("dir:{rel}"), Vec::new()));
                    walk(root, &path, out)?;
                },
                Ok(m) => {
                    let bytes = std::fs::read(&path).map_err(|e| {
                        format!("oracle read {}: {e}", path.display())
                    })?;
                    out.push((format!("file:{rel}:{}", m.len()), bytes));
                },
                Err(e) => {
                    return Err(format!(
                        "oracle metadata {}: {e}",
                        path.display()
                    ));
                },
            }
        }
        Ok(())
    }

    fn git_blob(
        root: &std::path::Path,
        args: &[&str],
    ) -> Result<(String, Vec<u8>), String> {
        let out = std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .map_err(|e| format!("oracle git {}: {e}", args.join(" ")))?;
        if !out.status.success() {
            return Err(format!(
                "oracle git {} exited {}: {}",
                args.join(" "),
                out.status,
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok((format!("git:{}", args.join(" ")), out.stdout))
    }

    let mut items = Vec::new();
    walk(root, root, &mut items)?;
    // Track checkpoint state explicitly: background `nb`
    // checkpoints commit, so `.git` exclusion alone would miss them.
    let (k, status) = git_blob(root, &["status", "--porcelain", "-uall"])?;
    items.push((k, status));
    let (k, head) = git_blob(root, &["rev-parse", "HEAD"])?;
    items.push((k, head));
    items.sort();
    Ok(items
        .into_iter()
        .map(|(rel, bytes)| {
            let mut hasher = DefaultHasher::new();
            bytes.hash(&mut hasher);
            (rel, hasher.finish())
        })
        .collect())
}

/// Poll until the tree listing is stable twice in a row
/// (background `nb` checkpoints from fixture setup have settled),
/// then return the frozen listing. Fail-closed: an oracle error or
/// a never-settling tree panics instead of letting the caller
/// compare against a hole or a moving target.
pub(crate) fn quiesce(root: &std::path::Path) -> Vec<(String, u64)> {
    let mut frozen = tree_listing(root)
        .unwrap_or_else(|e| panic!("oracle failed before quiesce: {e}"));
    let mut last_err: Option<String> = None;
    for _ in 0..60 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        match tree_listing(root) {
            Err(e) => {
                last_err = Some(e);
            },
            Ok(now) => {
                if now == frozen && last_err.is_none() {
                    return now;
                }
                frozen = now;
                last_err = None;
            },
        }
    }
    panic!("quiesce: tree would not settle, last error: {last_err:?}");
}

/// Seed the corpus; returns numeric ids in seed order:
/// hello, work note, todo, bookmark (folder `todos` unnumbered here).
/// Raw files below are written straight to disk (no `.index`
/// entries, no checkpoints): they exercise classification and exact
/// paths without shifting numeric slots.
pub(crate) async fn seed(client: &NbClient, root: &std::path::Path) {
    let mut tx = client.transaction(None).await.expect("tx");
    tx.add_note("hello_title.md", Some("Hello Title"), "body\n", &[])
        .unwrap();
    tx.add_note("work/a.md", Some("Work Note"), "work body\n", &[])
        .unwrap();
    tx.add_note("notes.txt", Some("Notes"), "notes body\n", &[])
        .unwrap();
    tx.add_todo("rootodo.todo.md", "Root Todo", None, &[], &[])
        .unwrap();
    tx.add_note("setext.md", None, "Setext Title\n===========\nbody\n", &[])
        .unwrap();
    tx.add_note(
        "front.md",
        None,
        "---\ntitle: Front Matter\n---\nbody\n",
        &[],
    )
    .unwrap();
    tx.add_note("mark.markdown", Some("Mark Down"), "mark body\n", &[])
        .unwrap();
    tx.add_note("upper.MD", Some("Upper"), "upper body\n", &[])
        .unwrap();
    tx.add_todo(
        "work/do_the_thing.todo.md",
        "Do the thing",
        Some("desc\n"),
        &["task one".to_string(), "task two".to_string()],
        &[],
    )
    .unwrap();
    tx.add_bookmark(
        "work/example.bookmark.md",
        "https://example.org",
        Some("Example"),
        &[],
        None,
    )
    .unwrap();
    tx.add_folder("todos").unwrap();
    let outcome = tx.commit().await.expect("seed");
    assert!(outcome.commit_created, "{outcome:?}");
    assert_eq!(outcome.ops[0].numeric_id, Some(1));
    assert_eq!(outcome.ops[1].numeric_id, Some(1));

    // Unindexed raw files: extensionless text, unknown-extension
    // text, control-byte binary, magic-masked binaries, an empty
    // file, and a UTF-16 file with BOM.
    std::fs::write(root.join("plain"), "just text\n").expect("plain");
    std::fs::write(root.join("mystery.xyz"), "mystery body\n")
        .expect("xyz text");
    std::fs::write(root.join("blob.xyz"), b"a\x01b\x02c\n".as_slice())
        .expect("control binary");
    std::fs::write(root.join("fakepdf.xyz"), b"%PDF-1.4 fake\n")
        .expect("fake pdf");
    std::fs::write(
        root.join("fakepng.xyz"),
        [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'],
    )
    .expect("fake png");
    std::fs::write(root.join("empty.xyz"), b"").expect("empty");
    let mut utf16 = vec![0xff, 0xfe];
    utf16.extend_from_slice(b"H\x00i\x00\n\x00");
    std::fs::write(root.join("utf16.txt"), utf16).expect("utf16");
    // Magic-table boundary: text starting with what `file` still
    // calls binary (GIF, FLAC, BZh, RIFF/WAVE) reads as non-text on
    // both sides; text starting with what `file` calls text (ID3,
    // OggS, MZ, BM, FORM, ftyp, ustar) reads as text on both sides.
    std::fs::write(root.join("gif89a.xyz"), "GIF89a is a format\nmore\n")
        .expect("gif text");
    std::fs::write(root.join("flac.xyz"), "fLaC is lossless\nmore\n")
        .expect("flac text");
    std::fs::write(root.join("bzh.xyz"), "BZh9 spans blocks\nmore\n")
        .expect("bzh text");
    std::fs::write(root.join("riff.xyz"), "RIFF1234WAVE and more\nmore\n")
        .expect("riff text");
    std::fs::write(root.join("id3.xyz"), "ID3 tags explained\nmore text\n")
        .expect("id3 text");
    std::fs::write(root.join("mz.xyz"), "MZ is a project code\nmore\n")
        .expect("mz text");
    // Byte-level boundary of the strict rule: DEL without a newline
    // reads as text, DEL with one does not; single-byte files are
    // never text; Latin-1 high bytes read as text.
    std::fs::write(root.join("del_nonl.xyz"), b"a\x7fQ").expect("del no nl");
    std::fs::write(root.join("del_nl.xyz"), b"a\x7fQ\n").expect("del nl");
    std::fs::write(root.join("onebyte.xyz"), b"\x07").expect("one byte");
    std::fs::write(root.join("latin1.xyz"), b"caf\xe9\nmore\n")
        .expect("latin-1");
    // Untouched-by-nb shapes: trailing-space titles and
    // non-`.md` titles never match, exactly like nb.
    std::fs::write(root.join("trail.md"), "# Trail  \nbody\n")
        .expect("trailing-space title");
    std::fs::write(root.join("noorg.org"), "# No Org\nbody\n")
        .expect("org file");
}
