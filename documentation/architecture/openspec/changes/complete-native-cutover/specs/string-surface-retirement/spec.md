# string-surface-retirement Specification

## Purpose
Define the removal of the `nb`-backed verbatim String rendering surfaces and the migration path to typed APIs.

## REMOVED Requirements

### Requirement: Retired String surfaces
The system SHALL NOT provide `nb`-backed verbatim String rendering methods. Typed APIs are the supported read surface.

**Reason**: Byte-faithful reproduction of another program's display text is not a library contract; every String surface spawns `nb` and inherits its races.
**Migration**: Use the typed counterpart for each retired method —
`list_notebooks` → new typed `list_notebook_names` (sorted names only; the verbatim `nb notebooks` text rendering has no replacement by design);
`list_notes` → `list_notes_typed`; `list_folders` → `list_folders_typed`;
`list_tasks` → `list_tasks_typed`; `search_notes` → `search_notes_typed`.

#### Scenario: Retired methods are absent
- **WHEN** a consumer builds against the new version
- **THEN** the retired String methods SHALL NOT exist on `NbClient`

#### Scenario: Typed counterparts cover the reads
- **WHEN** a consumer migrates each retired call per the mapping above
- **THEN** the typed API SHALL return the same entities the String surface rendered, except notebook enumeration which SHALL return names only (no String rendering equivalent is provided), and modulo the documented deltas below

## ADDED Requirements

### Requirement: Spawn machinery removal (production paths)

The system SHALL contain no `nb` subprocess machinery in production code paths once the last production caller is gone (`exec`/`exec_vec`, `nb_program` resolution). The `nb` integration harness and its fixtures are retained for true-oracle checks behind the dedicated Nextest profile.

#### Scenario: No nb spawn in production code
- **WHEN** any production API is exercised with `nb` spawning prohibited (`git` subprocesses explicitly permitted)
- **THEN** zero `nb` processes SHALL be spawned

#### Scenario: Spawn-failure error retained for the harness
- **WHEN** the retained integration harness fails to spawn `nb`
- **THEN** the system SHALL report the typed `NbError::ExecutableNotFound` failure

### Requirement: Oracle test classification
True independent `nb` comparisons SHALL be classified into an explicit oracle set, separate from pure-native safety tests (wire shapes, never-create, path rules, no-spawn writes), which SHALL remain unignored in the default suite. Broad ignore-gating whole modules is forbidden where it would strip default safety coverage.

#### Scenario: Safety tests stay in default
- **WHEN** a test asserts native behavior without invoking `nb`
- **THEN** it SHALL run unignored in the default suite, even if it lives alongside oracle comparisons

#### Scenario: Oracle set is explicit and profile-bound
- **WHEN** the extended command runs
- **THEN** it SHALL invoke the dedicated Nextest profile (`--profile oracle` defined in `.config/nextest.toml`) with `--run-ignored` and a filter selecting exactly the classified oracle set — never blanket historical ignored probes — and oracle-nonempty SHALL mean that set is nonempty

### Requirement: True-nb oracle profile
The system SHALL provide a dedicated Nextest profile running true-`nb` oracle checks as the complete/extended suite, separate from the purely-native default suite.

#### Scenario: Default suite is hermetic
- **WHEN** the default test suite runs without `nb` installed
- **THEN** all tests SHALL pass with zero `nb` spawns

#### Scenario: Extended profile checks the oracle
- **WHEN** the oracle Nextest profile runs with `nb` available
- **THEN** native behavior SHALL be checked against the true-`nb` oracle

#### Scenario: Default entry points are named exactly
- **WHEN** the native default suite runs
- **THEN** it SHALL be exactly these commands (all with `--all-features`, because the integration binary requires the `testing` feature and the crate default is empty): `cargo test --locked --all-features`, `cargo nextest run --locked --all-features --no-tests=warn`, doctests via `cargo test --doc --locked --all-features`, release via `cargo test --release --locked --all-features`, hooks per `.auxiliary/configuration/pre-commit.yaml`, CI per `.github/workflows/tester.yaml` — all with oracle tests excluded

#### Scenario: Oracle tests are ignore-gated with explicit inclusion
- **WHEN** a true-`nb` oracle test exists
- **THEN** it SHALL carry `#[ignore]` so default `cargo test` and default nextest skip it, and the extended command SHALL include it explicitly via `--run-ignored` (a profile name alone is not inclusion)

#### Scenario: Extended selection is known oracles, nonempty oracles, nb-required
- **WHEN** the extended command runs
- **THEN** it SHALL be `cargo nextest run --locked --all-features --profile oracle --run-ignored all -E '<oracle-set filter>' --no-tests=fail`, where the filter selects exactly the classified oracle set (not every historical ignored probe); it SHALL fail when that oracle selection is empty, which is distinct from total-suite nonempty; and it SHALL fail visibly (never skip-success) when `nb` is missing

#### Scenario: Complete is a documented union
- **WHEN** complete verification is claimed
- **THEN** it SHALL mean the default native command plus the extended oracle command above — two invocations, documented as a union, never one ambiguous run

#### Scenario: Default proves zero nb attempts with git alive
- **WHEN** the default suite runs with a logging dead-`nb` tripwire stub on `PATH`
- **THEN** the stub SHALL record zero invocations while git-backed tests still pass, proving no attempt rather than mere lookup absence

### Requirement: Documented behavioral deltas
String surfaces render `nb` commands while typed APIs follow their own scope, query, filter, and index rules. The system SHALL document each deliberate difference; parity is claimed only for entity identity within the same scope, never for display text, ordering, or `nb` selection quirks.

#### Scenario: Leading slash handling differs deliberately
- **WHEN** a leading-slash scope is passed
- **THEN** the typed API SHALL fail closed as `NotFound` while the retired String surface tolerated `nb`'s trimming — documented, not a defect

#### Scenario: Empty results differ by kind
- **WHEN** a scope is empty
- **THEN** the retired String surface returned `nb`'s signal text while the typed API SHALL return an empty list

#### Scenario: Ordering and scope rules are the typed ones
- **WHEN** results are enumerated
- **THEN** ordering, recursion, tag filtering, and unindexed-content handling SHALL follow the typed list/search rules, not `nb`'s display behavior; the downstream audit SHALL record these per call site
