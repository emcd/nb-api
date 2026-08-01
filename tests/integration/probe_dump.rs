// Empirical probe part 2: inspect the actual file contents that `nb`
// produced in probe_empirical test 1. Reads the files via the
// harness's `nb show` command and prints raw bytes for analysis.
//
// The probe function is `#[ignore]` so it does NOT run under
// `cargo test --all-features` (which would otherwise execute
// it since the `testing` feature is enabled). The probe
// requires a system-installed `nb` binary; running it under
// the normal all-features test matrix makes the suite
// environment-dependent.
//
// Run after `cargo test --test probe_empirical --features testing`:
//   cargo test --test probe_dump --features testing -- --ignored --nocapture

#![cfg(feature = "testing")]

use std::fs;
use std::path::PathBuf;

use nb_api::testing::NbTestEnv;

// Empirical probe part 2: dump actual file contents that `nb` produced.
// Every subprocess is noninteractive and time-bounded.
//
// This test is `#[ignore]` so it does NOT run under normal
// `cargo test --all-features` (which would otherwise execute
// it since the `testing` feature is enabled). The probe
// requires a system-installed `nb` binary; running it under
// the normal all-features test matrix makes the suite
// environment-dependent.
//
// Invoke intentionally:
//   cargo test --test probe_dump --features testing -- --ignored --nocapture
#[test]
#[ignore]
fn probe_dump() {
    let env = NbTestEnv::new().expect("fixture initialization");

    // Discover the NB_DIR layout
    let nb_dir: PathBuf = env.nb_dir().to_path_buf();
    println!("=== NB_DIR: {} ===", nb_dir.display());

    // Recursively list all files
    fn walk(dir: &PathBuf, depth: usize) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with('.') {
                        println!("{:indent$}DIR  {}/", "", name, indent = depth);
                        walk(&path, depth + 2);
                    }
                } else {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    println!(
                        "{:indent$}FILE {}/{}",
                        "",
                        path.parent()
                            .and_then(|p| p.file_name())
                            .and_then(|n| n.to_str())
                            .unwrap_or(""),
                        name,
                        indent = depth
                    );
                }
            }
        }
    }
    walk(&nb_dir, 0);
    println!();

    // For each .md / .todo / .bookmark file, dump raw bytes
    fn dump_all(dir: &PathBuf) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dump_all(&path);
                } else if let Some(name) = path.file_name().and_then(|n| n.to_str())
                    && (name.ends_with(".md")
                        || name.ends_with(".todo")
                        || name.ends_with(".bookmark"))
                {
                    println!("=== FILE: {} ===", path.display());
                    match fs::read(&path) {
                        Ok(bytes) => {
                            println!("SIZE: {} bytes", bytes.len());
                            println!("RAW (with escapes for non-printable):");
                            for (i, b) in bytes.iter().enumerate() {
                                if (i % 16) == 0 {
                                    print!("  {:04x}: ", i);
                                }
                                print!("{:02x} ", b);
                                if (i % 16) == 15 || i == bytes.len() - 1 {
                                    println!();
                                }
                            }
                            println!();
                            println!("TEXT (UTF-8 lossy):");
                            let text = String::from_utf8_lossy(&bytes);
                            for line in text.lines() {
                                println!("  | {}", line);
                            }
                        }
                        Err(e) => println!("READ ERROR: {}", e),
                    }
                    println!();
                }
            }
        }
    }
    dump_all(&nb_dir);

    // Now also probe a few more specific edge cases to round out
    // the empirical evidence.

    // Probe: explicit todo via add with a properly-formatted title
    let content = "# [ ] Buy milk\n\ndescription\n";
    let result = run_nb(
        &env,
        &["add", "Real Todo", "--type", "todo", "--content", "-"],
        Some(content),
    );
    print_probe(
        "Real todo with [ ] prefix",
        "nb add 'Real Todo' --type todo --content -",
        Some(content),
        result,
    );

    // Probe: todo with tags section
    let content =
        "# [ ] Buy milk\n\n## Description\n\nGet 2L skim\n\n## Tags\n\n#shopping #errands\n";
    let result = run_nb(
        &env,
        &["add", "Todo With Tags", "--type", "todo", "--content", "-"],
        Some(content),
    );
    print_probe(
        "Todo with [ ] title, Description, and Tags sections",
        "nb add 'Todo With Tags' --type todo --content -",
        Some(content),
        result,
    );

    // Probe: bookmark via direct content
    let content = "# My Bookmark\n\n<https://example.com>\n\n## Description\n\nA description\n\n## Tags\n\n#tag1 #tag2\n";
    let result = run_nb(
        &env,
        &[
            "bookmark",
            "add",
            "https://example.com",
            "--title",
            "My Bookmark",
            "--content",
            "-",
        ],
        Some(content),
    );
    print_probe(
        "Bookmark with title, URL, Description, Tags",
        "nb bookmark add https://example.com --title 'My Bookmark' --content -",
        Some(content),
        result,
    );

    // Probe: bookmark with non-terminal Tags (followed by Content)
    let content = "# My Bookmark\n\n<https://example.com>\n\n## Description\n\ndesc\n\n## Tags\n\n#t1\n\n## Content\n\nprocessed content here\n";
    let result = run_nb(
        &env,
        &[
            "bookmark",
            "add",
            "https://example.com",
            "--title",
            "Non-Terminal Tags",
            "--content",
            "-",
        ],
        Some(content),
    );
    print_probe(
        "Bookmark with non-terminal Tags (followed by Content)",
        "nb bookmark add https://example.com --title 'Non-Terminal Tags' --content -",
        Some(content),
        result,
    );

    // Probe: bookmark with no Title
    let content = "<https://example.com>\n\nbody content\n";
    let result = run_nb(
        &env,
        &[
            "bookmark",
            "add",
            "https://example.com",
            "--title",
            "No Body Bookmark",
            "--content",
            "-",
        ],
        Some(content),
    );
    print_probe(
        "Bookmark with URL but no body sections",
        "nb bookmark add https://example.com --title 'No Body Bookmark' --content -",
        Some(content),
        result,
    );

    // Final dump of all files
    println!("=== FINAL DUMP ===");
    dump_all(&env.nb_dir().to_path_buf());
}

fn run_nb(env: &NbTestEnv, args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
    let mut command = env.nb_command();
    command.args(args);
    if stdin.is_some() {
        command.stdin(std::process::Stdio::piped());
    }
    let output = command.output().expect("nb subprocess failed to spawn");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (output.status.code().unwrap_or(-1), stdout, stderr)
}

fn print_probe(label: &str, cmd: &str, stdin: Option<&str>, result: (i32, String, String)) {
    let (exit, stdout, stderr) = result;
    println!("=== PROBE: {} ===", label);
    println!("CMD: {}", cmd);
    if let Some(s) = stdin {
        println!("STDIN (escaped):");
        for line in s.lines() {
            println!("  | {}", line);
        }
    }
    println!("EXIT: {}", exit);
    if !stdout.is_empty() {
        println!("STDOUT:");
        for line in stdout.lines() {
            println!("  {}", line);
        }
    }
    if !stderr.is_empty() {
        println!("STDERR:");
        for line in stderr.lines() {
            println!("  {}", line);
        }
    }
    println!();
}
