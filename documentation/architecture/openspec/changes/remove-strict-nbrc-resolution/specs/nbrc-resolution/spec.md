## ADDED Requirements

### Requirement: Unset store location resolves to the default store

Store resolution SHALL NOT offer a refusal mode. When `NB_DIR` is unset (or empty), every resolution mode SHALL resolve to the default store under the home directory instead of raising an error.

#### Scenario: Unset location with default mode

- **WHEN** `NB_DIR` is unset and the mode is `ParseNbrc`
- **THEN** resolution returns the default store path

#### Scenario: Unset location with evaluation mode

- **WHEN** `NB_DIR` is unset and the mode is `EvaluateNbrc`
- **THEN** resolution returns the default store path

#### Scenario: Unset location with ignore mode

- **WHEN** `NB_DIR` is unset and the mode is `IgnoreNbrc`
- **THEN** resolution returns the default store path
