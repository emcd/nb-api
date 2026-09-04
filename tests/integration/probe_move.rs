#![cfg(feature = "testing")]
use nb_api::testing::NbTestEnv;
use std::fs;
use std::process::{Command, Output, Stdio};
fn run_nb<I, S>(env: &NbTestEnv, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut c = Command::new("timeout");
    env.configure_std(&mut c);
    c.args(["--signal=TERM", "--kill-after=1s", "5s", "nb"])
        .args(args)
        .stdin(Stdio::null());
    c.output().unwrap()
}
fn dump_index(env: &NbTestEnv, notebook: &str, folder: &str) {
    let p = if folder.is_empty() {
        env.nb_dir().join(notebook).join(".index")
    } else {
        env.nb_dir().join(notebook).join(folder).join(".index")
    };
    if p.is_file() {
        let t = String::from_utf8_lossy(&fs::read(&p).unwrap()).to_string();
        println!(
            "INDEX {}/{}:\n{:?}",
            notebook,
            if folder.is_empty() { "<root>" } else { folder },
            t
        );
        for (i, l) in t.lines().enumerate() {
            println!("  {}: {:?}", i + 1, l);
        }
    } else {
        println!("INDEX {}/{} MISSING", notebook, folder);
    }
}
fn print_out(l: &str, o: &Output) {
    println!(
        "{} EXIT {} STDOUT {:?} STDERR {:?}",
        l,
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}
#[test]
#[ignore]
fn probe_move_semantics() {
    let env = NbTestEnv::new().unwrap();
    let nb = env.notebook().to_string();
    println!("NB {}", nb);
    // create A,B in root
    for t in ["MoveA", "MoveB"] {
        let o = run_nb(
            &env,
            ["add", "--title", t, "--content", &format!("body {t}")],
        );
        print_out(&format!("ADD_{t}"), &o);
    }
    dump_index(&env, &nb, "");
    // create folder foo and bar
    print_out("ADD_folder_foo", &run_nb(&env, ["add", "folder", "foo"]));
    print_out("ADD_folder_bar", &run_nb(&env, ["add", "folder", "bar"]));
    dump_index(&env, &nb, "");
    // add note in foo
    let o = run_nb(
        &env,
        [
            "add",
            "--title",
            "FooNote",
            "--content",
            "foo body",
            "--folder",
            "foo",
        ],
    );
    print_out("ADD_FooNote", &o);
    dump_index(&env, &nb, "");
    dump_index(&env, &nb, "foo");
    dump_index(&env, &nb, "bar");
    // list
    print_out("LIST_root", &run_nb(&env, ["list", "--no-color"]));
    print_out("LIST_foo", &run_nb(&env, ["list", "foo/", "--no-color"]));
    // move within foo: rename FooNote -> foo/renamed.md
    // need id for FooNote: it's foo/1 per earlier, try move  foo/1 -> foo/renamed.md
    println!("=== MOVE within foo ===");
    let o = run_nb(&env, ["move", "foo/1", "foo/renamed.md", "--force"]);
    print_out("MOVE_within", &o);
    if !o.status.success() {
        // try alternate syntax: nb move foo/1 foo/renamed.md
        let o2 = run_nb(&env, ["move", "foo/1", "foo/renamed.md"]);
        print_out("MOVE_within2", &o2);
    }
    dump_index(&env, &nb, "foo");
    print_out(
        "LIST_foo_after",
        &run_nb(&env, ["list", "foo/", "--no-color"]),
    );
    // move across: move MoveA (id 1) to foo/
    println!("=== MOVE across root->foo ===");
    let o = run_nb(&env, ["move", "1", "foo/", "--force"]);
    print_out("MOVE_across", &o);
    dump_index(&env, &nb, "");
    dump_index(&env, &nb, "foo");
    print_out("LIST_root_after", &run_nb(&env, ["list", "--no-color"]));
    print_out(
        "LIST_foo_after2",
        &run_nb(&env, ["list", "foo/", "--no-color"]),
    );
    // move across foo->bar
    println!("=== MOVE across foo->bar ===");
    // find foo note id after moves
    dump_index(&env, &nb, "foo");
    dump_index(&env, &nb, "bar");
    let o = run_nb(&env, ["move", "foo/1", "bar/", "--force"]);
    print_out("MOVE_foo_to_bar", &o);
    dump_index(&env, &nb, "foo");
    dump_index(&env, &nb, "bar");
    print_out(
        "LIST_foo_final",
        &run_nb(&env, ["list", "foo/", "--no-color"]),
    );
    print_out(
        "LIST_bar_final",
        &run_nb(&env, ["list", "bar/", "--no-color"]),
    );
}
