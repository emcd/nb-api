// Advisor probes: .index delete/move semantics + mangling + titleless timezone
// Use standard NbTestEnv harness
#![cfg(feature = "testing")]

use nb_api::testing::NbTestEnv;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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
    command.output().expect("run bounded nb")
}
fn run_date(env: &NbTestEnv, utc: bool) -> String {
    let mut c = Command::new("timeout");
    env.configure_std(&mut c);
    if utc {
        c.args([
            "--signal=TERM",
            "--kill-after=1s",
            "5s",
            "date",
            "-u",
            "+%Y%m%d%H%M%S",
        ]);
    } else {
        c.args([
            "--signal=TERM",
            "--kill-after=1s",
            "5s",
            "date",
            "+%Y%m%d%H%M%S",
        ]);
    }
    c.stdin(Stdio::null());
    let out = c.output().expect("date");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
fn print_out(label: &str, output: &Output) {
    println!("{label}_EXIT\t{}", output.status.code().unwrap_or(-1));
    println!(
        "{label}_STDOUT\t{:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    println!(
        "{label}_STDERR\t{:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn dump_index(env: &NbTestEnv, notebook: &str) {
    let p = env.nb_dir().join(notebook).join(".index");
    if p.is_file() {
        let bytes = fs::read(&p).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        println!("=== .index {notebook} ===");
        for (i, line) in text.lines().enumerate() {
            println!("INDEX_LINE {}: {:?}", i + 1, line);
        }
        println!("INDEX_RAW {:?}", text);
    } else {
        println!("=== .index {notebook} MISSING ===");
    }
}
fn list_files(env: &NbTestEnv, notebook: &str) {
    let root = env.nb_dir().join(notebook);
    println!("=== files under {notebook} ===");
    for e in walk(&root) {
        println!("FILE {}", e.strip_prefix(&root).unwrap().display());
    }
}
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    fn rec(p: &Path, v: &mut Vec<PathBuf>) {
        for e in fs::read_dir(p).unwrap() {
            let pp = e.unwrap().path();
            if pp.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            if pp.is_dir() {
                rec(&pp, v);
            } else if pp.is_file() {
                v.push(pp);
            }
        }
    }
    rec(root, &mut v);
    v.sort();
    v
}

#[test]
#[ignore]
fn probe_advisor_index_and_mangling() {
    let env = NbTestEnv::new().expect("fixture init");
    let nb = env.notebook().to_string();
    println!("NOTEBOOK {}", nb);

    println!("=== CREATE A,B,C ===");
    for title in ["Note A", "Note B", "Note C"] {
        let out = run_nb(
            &env,
            [
                "add",
                "--title",
                title,
                "--content",
                &format!("body {title}"),
            ],
        );
        print_out(&format!("ADD_{}", title.replace(' ', "_")), &out);
    }
    dump_index(&env, &nb);
    print_out("LIST_after_ABC", &run_nb(&env, ["list", "--no-color"]));
    print_out("SHOW_1_PATH", &run_nb(&env, ["show", "1", "--path"]));
    print_out("SHOW_2_PATH", &run_nb(&env, ["show", "2", "--path"]));
    print_out("SHOW_3_PATH", &run_nb(&env, ["show", "3", "--path"]));

    println!("=== DELETE B (id 2) ===");
    print_out("DELETE_2", &run_nb(&env, ["delete", "2", "--force"]));
    dump_index(&env, &nb);
    print_out("LIST_after_delete", &run_nb(&env, ["list", "--no-color"]));
    print_out("SHOW_1_after", &run_nb(&env, ["show", "1", "--path"]));
    print_out("SHOW_2_after", &run_nb(&env, ["show", "2", "--path"]));
    print_out("SHOW_3_after", &run_nb(&env, ["show", "3", "--path"]));
    list_files(&env, &nb);

    println!("=== ADD D after delete ===");
    print_out(
        "ADD_D",
        &run_nb(&env, ["add", "--title", "Note D", "--content", "body D"]),
    );
    dump_index(&env, &nb);
    print_out("LIST_after_D", &run_nb(&env, ["list", "--no-color"]));
    print_out("SHOW_D_PATH", &run_nb(&env, ["show", "Note D", "--path"]));

    println!("=== ADD FOLDERS foo ===");
    print_out("ADD_FOLDER_foo", &run_nb(&env, ["add", "folder", "foo"]));
    // create note in foo
    print_out(
        "ADD_foo_note",
        &run_nb(
            &env,
            [
                "add",
                "--title",
                "Foo Note",
                "--content",
                "foo body",
                "--folder",
                "foo",
            ],
        ),
    );
    dump_index(&env, &nb);
    // dump foo folder index if exists (foo has its own .index? check)
    let foo_index = env.nb_dir().join(&nb).join("foo").join(".index");
    if foo_index.is_file() {
        println!(
            "FOO_INDEX {:?}",
            String::from_utf8_lossy(&fs::read(&foo_index).unwrap())
        );
    } else {
        println!("FOO .index missing (expected at notebook root only)");
    }
    list_files(&env, &nb);

    println!("=== MANGLE non-ASCII Café Über ===");
    print_out(
        "ADD_cafe",
        &run_nb(
            &env,
            ["add", "--title", "Café Über", "--content", "body cafe"],
        ),
    );
    dump_index(&env, &nb);
    print_out("LIST_cafe", &run_nb(&env, ["list", "--no-color"]));
    // try to show created cafe file
    let list = String::from_utf8_lossy(&run_nb(&env, ["list", "--no-color"]).stdout).to_string();
    println!("LIST_RAW {:?}", list);

    println!("=== TITLELESS twice ===");
    let out1 = run_nb(&env, ["add", "--content", "titleless one"]);
    print_out("ADD_titleless1", &out1);
    println!(
        "DATE_LOCAL1 {} DATE_UTC1 {}",
        run_date(&env, false),
        run_date(&env, true)
    );
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let out2 = run_nb(&env, ["add", "--content", "titleless two"]);
    print_out("ADD_titleless2", &out2);
    println!(
        "DATE_LOCAL2 {} DATE_UTC2 {}",
        run_date(&env, false),
        run_date(&env, true)
    );
    dump_index(&env, &nb);
    list_files(&env, &nb);
}
