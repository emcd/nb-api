<!-- nbspec: change=add-0-2-0-foundation notebook=nb-api note=proposals/add-0-2-0-foundation/specifications/test-infrastructure_specification.md hash=sha256:7fa6f033dd6902b4c8cd7595b4fa0ba553acd1e5b8503900cc7e19d0b61865fa -->
# test-infrastructure specification

#nbspec #specification

#nbspec #specification

## Purpose

Define the integration test harness for `nb-api` so that future defensive hardening (concurrency control, additional sanitization, etc.) can ship with regression coverage. Covers the `tests/integration/` tree, the `nb` CI dependency, and the optional public `nb_api::testing` helper for downstream consumer integration tests.

## ADDED Requirements

### Requirement: nb-api SHALL have an integration test directory

The `nb-api` repository SHALL contain a `tests/integration/` directory with shell-out regression tests that exercise the public API against a real `nb` CLI. The CI `qa` workflow SHALL install `nb` before running tests.

#### Scenario: tests/integration/ directory exists

- **WHEN** a consumer or contributor inspects the repository
- **THEN** a `tests/integration/` directory SHALL be present, with at least one `.rs` test file exercising `NbClient` end-to-end.

#### Scenario: nb is installed in CI before tests run

- **WHEN** the `qa` workflow runs on push or pull_request
- **THEN** the workflow SHALL install `nb` (per the official installation guide) before invoking `cargo test`. The install SHALL be hermetic (no project-specific PATH leakage).

#### Scenario: Integration tests run on every push and PR

- **WHEN** a commit is pushed to `master` or a PR is opened
- **THEN** the `qa` workflow SHALL run integration tests in addition to unit tests. Failures SHALL block the merge.

### Requirement: Integration tests SHALL enumerate every spawn point

The integration test suite SHALL include at least one regression test per subprocess spawn site in the crate. Currently two: `NbClient::exec` (every public method funnels through this) and `git_rev_parse` (the only direct `git` spawn).

#### Scenario: regression test for NbClient::exec

- **WHEN** any `NbClient` method runs under the leaked `GIT_DIR` env
- **THEN** the integration test SHALL verify the spawned `nb` subprocess does NOT inherit `GIT_DIR` (or any `GIT_*` routing var). The verification may use env inspection or behavior assertion (e.g., the notebook resolves correctly, not the parent repo).

#### Scenario: regression test for git_rev_parse

- **WHEN** `git_rev_parse` runs under the leaked `GIT_DIR` env
- **THEN** the integration test SHALL verify the spawned `git` subprocess does NOT inherit `GIT_DIR`. The resolved path SHALL come from the local repo, not the inherited env.

#### Scenario: a future third spawn site requires a new regression test

- **WHEN** a new spawn site is added to the crate
- **THEN** the existing enumeration SHALL be updated to include the new site. The integration test list SHALL grow accordingly.

### Requirement: Integration test hermeticity SHALL be guaranteed

Integration tests SHALL use isolated scratch notebooks under a temp directory. They SHALL NOT touch any project repository config.

#### Scenario: Isolated scratch notebook per test

- **WHEN** an integration test runs
- **THEN** the test SHALL create a scratch `nb` notebook under a temp directory. The notebook SHALL NOT be the project's actual notebook.

#### Scenario: Hermetic docstring stating no project config is used

- **WHEN** an integration test is written
- **THEN** the test SHALL carry a docstring stating "no project repository config used by this reproduction" (or equivalent), explicitly noting the isolation guarantee.

### Requirement: Public testing helper SHALL be a builder-backed owned fixture with a universal configure primitive

The crate SHALL expose a public `nb_api::testing` module gated behind a `testing` Cargo feature so test-only dependencies (e.g., `tempfile`) do not enter normal consumers. The helper SHALL be a builder-backed owned fixture (with a `new()` constructor for the common case), NOT a free function returning a pre-spawned `Child`, and NOT a trait abstraction. The universal primitive is `configure_std(&mut Command)` / `configure_tokio(&mut Command)` — apply the fixture's environment to any `Command` the caller owns. Convenience accessors (`nb_command()`, `nb_command_async()`) are degenerate forms of `configure_std(Command::new("nb"))` and exist for ergonomics, not as the primary API.

The fixture exposes two distinct paths by design: `nb_dir()` (the data store where `nb` stores notebooks; target of the `NB_DIR` env var) and `working_dir()` (the execution cwd; where spawned `nb` processes run). Both are hermetic by default — neither inherits the caller's project-root CWD. The `current_dir` of spawned processes SHALL auto-apply via `nb_command()`/`nb_command_async()`/`configure_std()`/`configure_tokio()`; callers can override post-construction if needed.

#### Scenario: `new()` covers the common case

- **WHEN** a downstream consumer wants one isolated notebook, hermetic, with default location, and no extra knobs
- **THEN** the consumer calls `NbTestEnv::new()` and obtains the fixture. No builder ceremony required. The fixture owns the isolated root and cleans up on `Drop`.

#### Scenario: `builder()` for configuration cases

- **WHEN** a downstream consumer needs a deterministic notebook name or a custom working directory
- **THEN** the consumer calls `NbTestEnv::builder()`, sets knobs (`notebook(name)`, `working_directory(path)`), and calls `.build()` to obtain the fixture. The builder returns `Result` so fixture initialization can fail loudly (e.g., temp dir creation).

#### Scenario: `configure_std` and `configure_tokio` are the universal primitives

- **WHEN** a downstream consumer has an arbitrary `std::process::Command` (or `tokio::process::Command`) they want to spawn under the fixture
- **THEN** the consumer calls `env.configure_std(&mut cmd)` (or `env.configure_tokio(&mut cmd)`) which sets the right env vars (`NB_DIR` from `env.nb_dir()`, scrubbed `GIT_*` routing vars, etc.) and the `current_dir` to `env.working_dir()` on the command. Spawning, stdio, args, timeout, and child lifecycle remain with the caller. This is the API both downstream consumers (`nb-mcp-server` integration tests, `nbspec` integration tests) use to apply the fixture's environment to the binary they actually want to spawn.

#### Scenario: `nb_command` and `nb_command_async` are convenience accessors

- **WHEN** a downstream consumer wants a pre-configured `nb` command (rather than constructing one from scratch)
- **THEN** `env.nb_command()` (or `env.nb_command_async()`) returns a `Command` pre-loaded with the fixture's env (`NB_DIR=env.nb_dir()`, scrubbed `GIT_*`) and `current_dir=env.working_dir()`. The caller adds args and spawns. This is the degenerate form of `configure_std(Command::new("nb"))` and exists for ergonomics; both call sites are valid.

#### Scenario: hermetic defaults

- **WHEN** `NbTestEnv::new()` is called with no extra configuration
- **THEN** the fixture:
- Owns an isolated root (typically under a temp dir) with cleanup on `Drop`.
- Sets `nb_dir()` to a fixture-owned subdir (data store; isolated git repo for the notebook).
- Sets `working_dir()` to a different fixture-owned subdir (execution cwd; distinct from `nb_dir()`).
- Strips inherited `GIT_*` routing vars (`GIT_DIR`, `GIT_INDEX_FILE`, `GIT_COMMON_DIR`, `GIT_WORK_TREE`, `GIT_OBJECT_DIRECTORY`, `GIT_ALTERNATE_OBJECT_DIRECTORIES`) before applying any intentional fixture overrides. Uses the same blast-by-prefix policy as `nb_api::git_env::scrub_git_env`.
- Initializes one notebook with deterministic Git identity and signing behavior.
- Returns `Result` errors with command/status/output context during fixture initialization failures.

#### Scenario: builder knobs cover real configuration needs

- **WHEN** a downstream consumer needs non-default behavior
- **THEN** the builder accepts:
- `notebook(name)` — deterministic notebook name for reproducible tests.
- `working_directory(path)` — override the execution cwd (e.g., to a real project root for in-repo tests). The fixture SHALL NOT delete the path on `Drop` when this is set to a caller-owned path.
- The fixture SHALL NOT delete a caller-owned root on `Drop` when the builder's caller-owned root option is used.

#### Scenario: feature-gated so test-only deps don't enter normal consumers

- **WHEN** a downstream consumer depends on `nb-api` without the `testing` feature
- **THEN** the consumer's `cargo build` SHALL succeed and the `nb_api::testing` module SHALL NOT be reachable. Test-only dependencies (e.g., `tempfile`, possibly `assert_fs`) are gated behind `testing`. The `tokio` feature flag separately gates `configure_tokio` and `nb_command_async`.

#### Scenario: shape is intentionally narrow

The helper SHALL expose:
- Constructors: `NbTestEnv::new()`, `NbTestEnv::builder()`.
- Knobs: notebook name, working directory.
- Inspection: `nb_dir()` (data store path), `working_dir()` (execution cwd), `notebook()`.
- Configuration methods: `configure_std(&mut Command)`, `configure_tokio(&mut Command)`.
- Convenience accessors: `nb_command()`, `nb_command_async()`.

The helper SHALL NOT expose:
- A trait abstraction (no polymorphic consumer need; enlarges a pre-1.0 contract unnecessarily).
- A pre-spawned `Child` as the primary API (callers own spawning and lifecycle).
- Built-in test assertions, MCP-specific knowledge, fixed timeout/retry policy, or output parsing.
- Global environment mutation (`std::env::set_var`), current-directory mutation (caller's CWD specifically), or process-wide locks.
- Automatic cleanup of caller-owned paths.
- An `nb_executable` override knob (`nb` is PATH-resolved; shim-binary patterns are deferred to a future change).
- A giant arbitrary `nb`-command DSL (return/configure standard `Command` types instead).

### Requirement: testing helper SHALL be feature-gated and versioned conservatively

The `nb_api::testing` module is pre-1.0 API surface. Breaking changes between pre-1.0 versions are permitted, but the module's narrow scope (per the shape above) is intended to minimize consumer impact.

#### Scenario: feature-gating is wired correctly

- **WHEN** the `testing` feature is enabled in `Cargo.toml`
- **THEN** `nb_api::testing` modules and types are reachable. When disabled, they are not. The `tokio` feature flag separately gates the tokio-side methods.

### Requirement: Integration tests SHALL be the canonical regression coverage for future hygiene findings

When a future hygiene finding surfaces (similar to `nb-api:issues/2`, `/3`, `/api/4`, `/api/5`, `/api/6`), the fix SHALL land with at least one integration test that exercises the affected code path under the relevant adversarial condition.

#### Scenario: future finding lands with regression test

- **WHEN** a hygiene finding is filed and the fix is scoped
- **THEN** the implementation SHALL add at least one `tests/integration/<finding>.rs` test that fails without the fix and passes with it. Comment-only defense is acceptable only for findings where the integration test would require infrastructure beyond the harness's scope.

