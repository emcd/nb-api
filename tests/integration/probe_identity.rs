// Scratch probe: nb 7.24.0 identity model — filename generation,
// `.index` contents, `nb list` IDs, and numeric-id resolution.
// NOT part of the release suite; `#[ignore]` + standalone target.

#![cfg(feature = "testing")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use nb_api::testing::NbTestEnv;

fn run_nb<I, S>(env: &NbTestEnv, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = Command::new("timeout");
    env.configure_std(&mut command);
    command
        .args(["--signal=TERM", "--kill-after=1s", "5s", "nb"])
        .args(args)
        .stdin(Stdio::null());
    command.output().expect("run bounded nb subprocess")
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

fn print_out(label: &str, output: &Output) {
    println!("{label}_EXIT\t{}", output.status.code().unwrap_or(-1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("{label}_STDOUT\t{stdout:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("{label}_STDERR\t{stderr:?}");
}

fn probe_writer<I, S>(env: &NbTestEnv, label: &str, args: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let notebook = env.nb_dir().join(env.notebook());
    let before = regular_files(&notebook);
    println!("WRITER\t{label}");
    print_out("COMMAND", &run_nb(env, args));
    let after = regular_files(&notebook);
    for path in after.iter().filter(|path| !before.contains(path)) {
        let relative = path.strip_prefix(&notebook).expect("relative writer path");
        println!("CREATED\t{}", relative.display());
    }
    println!("END_WRITER");
}

fn dump_index_and_files(env: &NbTestEnv) {
    let notebook = env.nb_dir().join(env.notebook());
    let index = notebook.join(".index");
    if index.is_file() {
        let bytes = fs::read(&index).expect("read .index");
        let text = String::from_utf8_lossy(&bytes);
        println!("=== .index contents ===");
        for line in text.lines() {
            println!("INDEX_LINE\t{line:?}");
        }
    } else {
        println!("=== .index MISSING ===");
    }
    println!("=== all files under notebook ===");
    for path in regular_files(&notebook) {
        println!("FILE\t{}", path.strip_prefix(&notebook).unwrap().display());
    }
}

#[test]
#[ignore]
fn probe_identity_model() {
    let env = NbTestEnv::new().expect("fixture initialization");
    println!("NOTEBOOK\t{}", env.notebook());

    println!("--- add note with title ---");
    probe_writer(
        &env,
        "note-with-title",
        [
            "add",
            "--title",
            "Hello World Title",
            "--content",
            "Body text here",
        ],
    );
    println!("--- add note without title ---");
    probe_writer(
        &env,
        "note-titleless",
        ["add", "--content", "Just content here"],
    );
    println!("--- add todo ---");
    probe_writer(&env, "todo", ["add", "todo", "--title", "Do the thing"]);
    println!("--- add note with long title / special chars ---");
    probe_writer(
        &env,
        "note-special-title",
        [
            "add",
            "--title",
            "Hello: World / What? [x] *y* _z_",
            "--content",
            "Body",
        ],
    );

    println!("=== nb list ===");
    print_out("LIST", &run_nb(&env, ["list", "--no-color"]));

    println!("=== nb ls -n ===");
    print_out("LS", &run_nb(&env, ["ls", "--no-color"]));

    dump_index_and_files(&env);

    println!("--- resolve numeric id 1 ---");
    print_out("SHOW_1_PATH", &run_nb(&env, ["show", "1", "--path"]));
    print_out(
        "SHOW_1",
        &run_nb(&env, ["show", "1", "--print", "--no-color"]),
    );
    println!("--- resolve numeric id 2 ---");
    print_out("SHOW_2_PATH", &run_nb(&env, ["show", "2", "--path"]));
    println!("--- show by title ---");
    print_out(
        "SHOW_TITLE",
        &run_nb(&env, ["show", "Hello World", "--path"]),
    );

    println!("--- re-add same title: filename collision behavior ---");
    probe_writer(
        &env,
        "note-title-again",
        [
            "add",
            "--title",
            "Hello World Title",
            "--content",
            "Second body",
        ],
    );
    dump_index_and_files(&env);

    println!("--- file created DIRECTLY (nb-api style, no .index entry) ---");
    let notebook = env.nb_dir().join(env.notebook());
    let direct = notebook.join("1787096169-0.md");
    fs::write(&direct, "# Direct File\n\nWritten without nb add\n").expect("write direct file");
    println!("=== nb list after direct create ===");
    print_out("LIST", &run_nb(&env, ["list", "--no-color"]));
    println!("=== nb show <filename> after direct create ===");
    print_out(
        "SHOW_DIRECT",
        &run_nb(&env, ["show", "1787096169-0.md", "--print", "--no-color"]),
    );
    println!("=== nb show <numeric id that would map to it> ===");
    print_out("SHOW_NEXT_ID", &run_nb(&env, ["show", "5", "--path"]));
    print_out(
        "SHOW_BY_TITLE",
        &run_nb(&env, ["show", "Direct File", "--path"]),
    );
    println!("=== .index after direct create (still unchanged?) ===");
    let index = notebook.join(".index");
    if index.is_file() {
        let bytes = fs::read(&index).expect("read .index");
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines() {
            println!("INDEX_LINE\t{line:?}");
        }
    } else {
        println!("=== .index MISSING ===");
    }
}
