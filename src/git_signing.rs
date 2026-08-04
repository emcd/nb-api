//! Spawn-time git signing env overrides for hermetic `nb` invocations.

use tokio::process::Command;

pub(crate) const GIT_SIGNING_OVERRIDES: [(&str, &str); 2] =
    [("commit.gpgsign", "false"), ("tag.gpgsign", "false")];

pub(crate) fn git_signing_env_vars(start_index: usize) -> Vec<(String, String)> {
    let total = start_index.saturating_add(GIT_SIGNING_OVERRIDES.len());
    let mut env_vars = Vec::with_capacity(1 + GIT_SIGNING_OVERRIDES.len() * 2);
    env_vars.push(("GIT_CONFIG_COUNT".to_string(), total.to_string()));
    for (offset, (key, value)) in GIT_SIGNING_OVERRIDES.iter().enumerate() {
        let index = start_index + offset;
        env_vars.push((format!("GIT_CONFIG_KEY_{index}"), (*key).to_string()));
        env_vars.push((format!("GIT_CONFIG_VALUE_{index}"), (*value).to_string()));
    }
    env_vars
}

pub(crate) fn apply_git_signing_env(command: &mut Command) {
    // Always start at index 0. `scrub_git_env` has already removed
    // every `GIT_CONFIG_*` from the spawn-time `Command` env (the
    // blast-by-prefix defense-in-depth pattern, mirrored from
    // `nbspec:71f369e`), so the spawn-time `GIT_CONFIG_COUNT` is
    // 0. The parent's pre-scrub `GIT_CONFIG_COUNT` does not flow
    // through to the child and must not influence the index.
    // (Pre-patch behavior read the parent count via
    // `std::env::var("GIT_CONFIG_COUNT")`, which produced a gap
    // in the emitted indices whenever the parent had set
    // `GIT_CONFIG_*` — see the regression test in
    // `tests/integration/git_signing_overrides.rs`.)
    for (name, value) in git_signing_env_vars(0) {
        command.env(name, value);
    }
}
