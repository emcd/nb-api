## ADDED Requirements

### Requirement: No refusal for an unselected store

Store resolution SHALL NOT offer a refusal mode. The absence of a configured store is never itself a failure: after successful mode-specific resolution, a still-unselected store resolves to the default store. Mode semantics are otherwise unchanged — an rc assignment in `ParseNbrc`/`EvaluateNbrc` wins, a failing rc still errors, an uncomputable fallback still errors, and the selected path is still validated before use.

#### Scenario: Rc supplies the store under Parse or Evaluate

- **WHEN** the mode is `ParseNbrc` or `EvaluateNbrc`, `NB_DIR` is unset, and the rc assigns a store
- **THEN** resolution returns the rc-supplied store

#### Scenario: Final unset or empty with computable HOME falls back

- **WHEN** the final value after mode-specific resolution is unset or empty and the home directory is computable
- **THEN** resolution returns the default store instead of failing

#### Scenario: Failing rc still errors

- **WHEN** evaluation is opted in and the rc exits unsuccessfully
- **THEN** resolution fails instead of falling back

#### Scenario: Uncomputable fallback still errors

- **WHEN** no store is selected and the home directory is undiscoverable
- **THEN** resolution fails instead of inventing a default

#### Scenario: Selected path is still validated

- **WHEN** the selected store path points at a non-directory
- **THEN** resolution fails with a typed configuration failure
