//! Bounded empirical matrix for `nb 7.24.0` document behavior.
//!
//! This is an evidence-gathering probe, not part of the release test suite.
//! Every subprocess has closed stdin and a five-second timeout.

#![cfg(feature = "testing")]

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use nb_api::testing::NbTestEnv;

fn run_nb<I, S>(env: &NbTestEnv, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("timeout");
    env.configure_std(&mut command);
    command
        .args(["--signal=TERM", "--kill-after=1s", "5s", "nb"])
        .args(args)
        .stdin(Stdio::null());
    command.output().expect("run bounded nb subprocess")
}

fn notebook_path(env: &NbTestEnv) -> PathBuf {
    env.nb_dir().join(env.notebook())
}

fn regular_files(root: &Path) -> Vec<PathBuf> {
    fn walk(path: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(path).expect("read probe directory") {
            let path = entry.expect("read probe entry").path();
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            if path.is_dir() {
                walk(&path, files);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(root, &mut files);
    files.sort();
    files
}

fn print_output(label: &str, output: &Output) {
    println!("{label}_EXIT\t{}", output.status.code().unwrap_or(-1));
    println!("{label}_STDOUT_HEX\t{}", hex(&output.stdout));
    println!("{label}_STDOUT_ESC\t{}", escaped(&output.stdout));
    println!("{label}_STDERR_HEX\t{}", hex(&output.stderr));
    println!("{label}_STDERR_ESC\t{}", escaped(&output.stderr));
}

fn probe_writer<I, S>(env: &NbTestEnv, label: &str, args: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let notebook = notebook_path(env);
    let before = regular_files(&notebook);
    println!("WRITER\t{label}");
    print_output("COMMAND", &run_nb(env, args));
    let after = regular_files(&notebook);
    for path in after.iter().filter(|path| !before.contains(path)) {
        let bytes = fs::read(path).expect("read writer output");
        let relative = path.strip_prefix(&notebook).expect("relative writer path");
        println!(
            "CREATED\t{}\t{}\t{}",
            relative.display(),
            hex(&bytes),
            escaped(&bytes)
        );
    }
    println!("END_WRITER");
}

fn probe_file(env: &NbTestEnv, label: &str, name: &str, bytes: &[u8], task_ops: bool) {
    let path = notebook_path(env).join(name);
    fs::write(&path, bytes).expect("write exact probe bytes");
    println!("CASE\t{label}");
    println!("FILE\t{name}");
    println!("INPUT_HEX\t{}", hex(bytes));
    println!("INPUT_ESC\t{}", escaped(bytes));
    print_output(
        "SHOW",
        &run_nb(env, ["show", name, "--print", "--no-color"]),
    );
    print_output("TYPE", &run_nb(env, ["show", name, "--type", "--no-color"]));
    if task_ops {
        print_output("DO", &run_nb(env, ["do", name]));
        let after_do = fs::read(&path).expect("read bytes after do");
        println!("AFTER_DO_HEX\t{}", hex(&after_do));
        println!("AFTER_DO_ESC\t{}", escaped(&after_do));
        print_output("UNDO", &run_nb(env, ["undo", name]));
        let after_undo = fs::read(&path).expect("read bytes after undo");
        println!("AFTER_UNDO_HEX\t{}", hex(&after_undo));
        println!("AFTER_UNDO_ESC\t{}", escaped(&after_undo));
    }
    println!("END_CASE");
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

fn escaped(bytes: &[u8]) -> String {
    let mut output = String::new();
    for byte in bytes {
        match *byte {
            b'\\' => output.push_str("\\\\"),
            b'\n' => output.push_str("\\n"),
            b'\r' => output.push_str("\\r"),
            b'\t' => output.push_str("\\t"),
            0x20..=0x7e => output.push(*byte as char),
            value => output.push_str(&format!("\\x{value:02x}")),
        }
    }
    output
}

// Empirical probe of `nb 7.24.0` behavior on P1 edge cases.
// Every subprocess is noninteractive and time-bounded.
//
// This test is `#[ignore]` so it does NOT run under normal
// `cargo test --all-features` (which would otherwise execute
// it since the `testing` feature is enabled). The probe
// requires a system-installed `nb` binary AND GNU `timeout` in
// PATH; running it under the normal all-features test matrix
// makes the suite environment- and platform-dependent.
//
// Invoke intentionally:
//   cargo test --test probe_empirical --features testing -- --ignored --nocapture
#[test]
#[ignore]
fn probe_empirical() {
    let env = NbTestEnv::new().expect("initialize established NbTestEnv");
    let version = run_nb(&env, ["--version"]);
    print_output("NB_VERSION", &version);

    probe_writer(
        &env,
        "note",
        [
            "scratch:add",
            "--title",
            "Writer Note",
            "--content",
            "Writer body\nsecond line\n",
            "--tags",
            "#alpha",
            "--tags",
            "#beta",
        ],
    );
    probe_writer(
        &env,
        "note-titleless",
        [
            "scratch:add",
            "--content",
            "Writer titleless body\n",
            "--tags",
            "#alpha",
            "--tags",
            "#beta",
        ],
    );
    probe_writer(
        &env,
        "todo",
        [
            "scratch:todo",
            "add",
            "Writer Todo",
            "--description",
            "Writer description",
            "--tags",
            "#alpha",
            "--tags",
            "#beta",
        ],
    );
    probe_writer(
        &env,
        "bookmark-title",
        [
            "scratch:bookmark",
            "https://example.com",
            "--title",
            "Writer Bookmark",
            "--tags",
            "#alpha",
            "--tags",
            "#beta",
            "--offline",
            "--no-content",
        ],
    );
    probe_writer(
        &env,
        "bookmark-titleless",
        [
            "scratch:bookmark",
            "https://example.org/no-title",
            "--offline",
            "--no-content",
        ],
    );

    probe_file(&env, "note-atx", "note-atx.md", b"# Title\n\nBody\n", false);
    probe_file(
        &env,
        "note-titleless",
        "note-titleless.md",
        b"Just content.\n",
        false,
    );
    probe_file(
        &env,
        "note-hash-no-delimiter",
        "note-hash.md",
        b"#Title\n\nBody\n",
        false,
    );
    probe_file(
        &env,
        "note-indented",
        "note-indented.md",
        b"    # Title\n\nBody\n",
        false,
    );
    probe_file(
        &env,
        "note-setext",
        "note-setext.md",
        b"Title\n=====\n\nBody\n",
        false,
    );
    probe_file(
        &env,
        "note-leading-blank",
        "note-leading.md",
        b"\n\n# Title\n\nBody\n",
        false,
    );
    probe_file(
        &env,
        "note-tags-heading",
        "note-tags.md",
        b"Text\n\n## Tags\n\n#alpha #beta\n",
        false,
    );
    probe_file(
        &env,
        "note-bom",
        "note-bom.md",
        b"\xef\xbb\xbf# Title\n\nBody\n",
        false,
    );
    probe_file(
        &env,
        "note-cr-only",
        "note-cr.md",
        b"# Title\r\rBody\rSecond\r",
        false,
    );
    probe_file(
        &env,
        "note-invalid-utf8",
        "note-invalid.md",
        b"# Title\n\nBody \xff\xfe\n",
        false,
    );

    probe_file(
        &env,
        "todo-dot-todo",
        "canonical.todo",
        b"# [ ] Task\n\n## Description\n\nBody\n\n## Tags\n\n#alpha #beta\n",
        true,
    );
    probe_file(
        &env,
        "todo-dot-todo-md",
        "canonical.todo.md",
        b"# [ ] Task\n\nBody\n",
        true,
    );
    probe_file(
        &env,
        "todo-no-checkbox",
        "no-checkbox.todo.md",
        b"# Task\n\nBody\n",
        true,
    );
    probe_file(
        &env,
        "todo-nonterminal-tags",
        "nonterminal.todo.md",
        b"# [ ] Task\n\n## Tags\n\n#alpha\n\n## Description\n\nBody\n",
        true,
    );
    probe_file(
        &env,
        "todo-duplicate-tags",
        "duplicate.todo.md",
        b"# [ ] Task\n\n## Tags\n\n#first\n\n## Description\n\nBody\n\n## Tags\n\n#last\n",
        true,
    );

    probe_file(
        &env,
        "bookmark-minimal",
        "minimal.bookmark.md",
        b"# Bookmark\n\n<https://example.com>\n",
        false,
    );
    probe_file(
        &env,
        "bookmark-titleless",
        "titleless.bookmark.md",
        b"<https://example.com>\n",
        false,
    );
    probe_file(
        &env,
        "bookmark-missing-url",
        "missing-url.bookmark.md",
        b"# Bookmark\n\nBody\n",
        false,
    );
    probe_file(
        &env,
        "bookmark-nonterminal-tags",
        "nonterminal.bookmark.md",
        b"# Bookmark\n\n<https://example.com>\n\n## Description\n\nDesc\n\n## Tags\n\n#alpha\n\n## Content\n\nContent body\n",
        false,
    );
    probe_file(
        &env,
        "bookmark-tags-in-content",
        "content-tags.bookmark.md",
        b"# Bookmark\n\n<https://example.com>\n\n## Tags\n\n#official\n\n## Content\n\nParagraph\n\n## Tags\n\nUser heading\n",
        false,
    );
    probe_file(
        &env,
        "bookmark-tags-in-source",
        "source-tags.bookmark.md",
        b"# Bookmark\n\n<https://example.com>\n\n## Tags\n\n#official\n\n## Source\n\n```html\n## Tags\n<p>raw</p>\n```\n",
        false,
    );
}
