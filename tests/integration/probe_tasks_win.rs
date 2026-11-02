// TEMPORARY Windows diagnostic for tasks_tags failure. Dumps raw nb
// outputs per verb/scope; always passes. REMOVE AFTER DIAGNOSIS.
#![cfg(feature = "testing")]

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient};

use crate::common::with_isolated_env;

fn config_for(env: &NbTestEnv) -> Config {
    Config {
        notebook: Some(env.notebook().to_string()),
        create_notebook: false,
        allow_top_level_notes: true,
        disable_git_signing: true,
        ..Config::default()
    }
}

fn dump(env: &NbTestEnv, label: &str, args: &[&str]) {
    let mut cmd = env.nb_command();
    cmd.args(args);
    match cmd.output() {
        Ok(o) => println!(
            "DUMP {label} exit={} stdout={:?} stderr={:?}",
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        ),
        Err(e) => println!("DUMP {label} SPAWN-ERR {e:?}"),
    }
}

#[tokio::test]
async fn dump_todos_verb_shapes() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        client
            .add_todo("Root", None, &["r1".to_string()], &[], None, None)
            .await
            .expect("root todo");
        client
            .add_todo(
                "Nested",
                None,
                &["n1".to_string()],
                &[],
                Some("proj"),
                None,
            )
            .await
            .expect("nested todo");
        let nb = env.notebook().to_string();
        println!("DUMP nb-binary={:?}", nb_api::testing::nb_binary());
        for scope in [format!("{nb}:"), format!("{nb}:proj/")] {
            for verb in [
                vec!["tasks", scope.as_str(), "open", "--no-color"],
                vec!["todos", "tasks", scope.as_str(), "open", "--no-color"],
                vec!["todos", scope.as_str(), "open", "--no-color"],
            ] {
                dump(&env, &verb.join("_"), &verb);
            }
        }
        // Tree state for correlation.
        let root = client.show_notebook_path(None).await.expect("path");
        let mut files: Vec<String> = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).expect("read_dir") {
                let p = e.expect("entry").path();
                if p.file_name().is_some_and(|n| n == ".git") {
                    continue;
                }
                if p.is_dir() {
                    stack.push(p);
                } else {
                    files.push(
                        p.strip_prefix(&root)
                            .expect("prefix")
                            .display()
                            .to_string(),
                    );
                }
            }
        }
        files.sort();
        println!("DUMP files={files:?}");
        for idx in [".index", "proj/.index"] {
            match std::fs::read(root.join(idx)) {
                Ok(b) => println!("DUMP index[{idx}]={b:?}"),
                Err(e) => println!("DUMP index[{idx}] MISSING {e:?}"),
            }
        }
    })
    .await;
}
