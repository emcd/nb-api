## ADDED Requirements

### Requirement: EOL scenarios run on the hermetic fixture

Each document-EOL scenario SHALL construct its notebook via `NbTestEnv::hermetic()` (synchronous git setup, no `nb` spawn) and SHALL exercise the public client against that notebook.

#### Scenario: Hermetic setup for every EOL test

- **WHEN** any `eol_model` test initializes its environment
- **THEN** no `nb` process is spawned during setup and the notebook root is a git worktree with a resolvable `HEAD`.

#### Scenario: Existing EOL assertions preserved

- **WHEN** the converted tests run
- **THEN** every pre-existing EOL assertion (document EOL value, `has_final_eol`, line texts, anchors, body bytes) holds unchanged from the nb-backed version.

### Requirement: Settled-repo postconditions accompany EOL assertions

Each converted test SHALL assert the notebook repo is settled (HEAD peels to a commit, worktree clean) at the pilot's points: after the initial body write and after each mutation.

#### Scenario: Postconditions fail loudly on dirt

- **WHEN** `HEAD` is unresolvable or `git status --porcelain` is non-empty at a postcondition point
- **THEN** the test fails with the HEAD/tree output attached, without retry loops or sleeps.

#### Scenario: Postcondition checks spawn no `nb`

- **WHEN** the settled-repo probe runs
- **THEN** it invokes only plain `git` with scrubbed env (`scrub_git_env_std`) and never the `nb` CLI or the client under test.
