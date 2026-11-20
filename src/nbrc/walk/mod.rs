//! AST walk for notebook rc evaluation (see parent module docs).
//!
//! Pure helpers over brush-parser types: nothing here reads process
//! state or the filesystem.
//!
//! Declared evaluation subset — everything else selects warned
//! fallback to `~/.nb` instead of resolving elsewhere:
//! - Evaluated: bare `NAME=literal` assignments (`NB_DIR`
//!   persists last-wins; other names only for `${NB_DIR:=...}`
//!   side effects, which apply immediately); `export` (suffix
//!   words expand first against the live table with `:=`
//!   immediate, prefix values evaluated second for side effects
//!   and a transient binding, last suffix binding or bare-word
//!   transient capture commits; verified against Bash);
//!   `readonly` with any `NB_DIR` involvement in the suffix
//!   falls back (persistence would freeze later reassignment);
//!   `unset NB_DIR` (plus `-v`/`--` spellings); `~`/`$HOME` and `$NB_DIR`
//!   self-defaults (`:-`, `:=`, `:+` and friends) with quoting
//!   context preserved.
//! - Ignored (proven irrelevant to the store): redirections,
//!   comments, blank lines, unrelated assignments and variables,
//!   no-op words (`export NB_DIR` bare, `unset` flags), transient
//!   `NB_DIR=...` prefixes before regular commands (only
//!   `export`/`readonly` persist them, verified), subshell and
//!   backgrounded commands (confinement provably blocks parent
//!   effects), function definitions and calls without opaque
//!   content, and plain external invocations with no assignments
//!   and no `NB_DIR`-mentioning words.
//! - Fallback: anything dynamic or opaque (command/arithmetic/
//!   process substitution, non-`$HOME` expansions, arrays,
//!   `+=`, dynamic command names, `source`/`.`/`builtin`/
//!   `command`/`eval`/`exec` in any effective spelling and any
//!   position, `return` outside function bodies, `exit`
//!   everywhere (it exits the shell even from a function),
//!   unparseable input). Sourced files are never read.
//!
//! Best-effort boundary (documented, trusted system): bodies of
//! uncalled functions are still scanned conservatively; aliases
//! are not expanded; exotic builtins that assign variables by
//! name argument (`read NB_DIR`, `printf -v NB_DIR`, ...) are not
//! modeled and resolve literally. Prefer an explicit rc when the
//! store matters.

use std::path::Path;

mod scan;

pub(crate) use scan::program_opaque_reason;
use scan::{effective_command_name, opaque_dispatch_reason};

/// Outcome of walking one top-level list item.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ItemOutcome {
    Assign(String),
    Clear,
    Undetermined(&'static str),
    Ignore,
}

/// Walk one top-level `;`/newline-separated item. Only a lone
/// simple command in the foreground counts; anything conditional,
/// backgrounded, piped, timed, negated, or compound falls back
/// when it mentions `NB_DIR`, and is ignored otherwise.
pub(crate) fn walk_item(
    item: &brush_parser::ast::CompoundListItem,
    home: Option<&Path>,
    nb_dir: &mut Option<String>,
) -> ItemOutcome {
    use brush_parser::ast::SeparatorOperator;
    let brush_parser::ast::CompoundListItem(andor, sep) = item;
    if matches!(sep, SeparatorOperator::Async) {
        return if mentions_nb_dir(&format!("{andor:?}")) {
            ItemOutcome::Undetermined("backgrounded command")
        } else {
            ItemOutcome::Ignore
        };
    }
    if !andor.additional.is_empty() {
        return if mentions_nb_dir(&format!("{andor:?}")) {
            ItemOutcome::Undetermined("conditional command")
        } else {
            ItemOutcome::Ignore
        };
    }
    let pipe = &andor.first;
    if pipe.timed.is_some() || pipe.bang || pipe.seq.len() != 1 {
        return if mentions_nb_dir(&format!("{pipe:?}")) {
            ItemOutcome::Undetermined("conditional command")
        } else {
            ItemOutcome::Ignore
        };
    }
    match &pipe.seq[0] {
        brush_parser::ast::Command::Simple(command) => {
            walk_simple(command, home, nb_dir)
        },
        other => {
            if mentions_nb_dir(&format!("{other:?}")) {
                ItemOutcome::Undetermined("compound command")
            } else {
                ItemOutcome::Ignore
            }
        },
    }
}

/// Scan every assignment in a non-persisting command position
/// for `${NB_DIR:=...}` side effects (which persist no matter
/// which variable is assigned) and undeterminable values.
/// Returns the first fallback reason, if any.
fn scan_side_effects(
    command: &brush_parser::ast::SimpleCommand,
    home: Option<&Path>,
    nb_dir: &mut Option<String>,
) -> Option<&'static str> {
    use brush_parser::ast::CommandPrefixOrSuffixItem as Item;
    let parts = command
        .prefix
        .as_ref()
        .map(|prefix| prefix.0.as_slice())
        .unwrap_or(&[])
        .iter()
        .chain(
            command
                .suffix
                .as_ref()
                .map(|suffix| suffix.0.as_slice())
                .unwrap_or(&[]),
        );
    for item in parts {
        if let Item::AssignmentWord(assignment, _) = item
            && let AssignOutcome::Undetermined(reason) =
                classify_assignment(assignment, home, nb_dir)
        {
            return Some(reason);
        }
    }
    None
}

/// Walk one unconditional simple command. A bare `NB_DIR=...`
/// (with only redirections beside it) persists; `export NB_DIR=...`
/// persists; `unset NB_DIR` clears; anything else mentioning
/// `NB_DIR` falls back, and anything else is ignored. Prefix
/// assignments with a command name are transient and ignored.
fn walk_simple(
    command: &brush_parser::ast::SimpleCommand,
    home: Option<&Path>,
    nb_dir: &mut Option<String>,
) -> ItemOutcome {
    use brush_parser::ast::CommandPrefixOrSuffixItem as Item;
    // Opaque dispatch gate with the live table: the program-wide
    // scan uses an empty table, so a name resolving through a
    // prior assignment (or failing only against live state) is
    // judged here with what the shell would actually dispatch.
    if let Some(reason) = opaque_dispatch_reason(command, home, nb_dir, false)
    {
        return ItemOutcome::Undetermined(reason);
    }
    // Effective identity (quotes/concatenation resolved) routes
    // builtins: a quoted `'export'` still exports in Bash.
    let effective =
        effective_command_name(command, home, nb_dir).ok().flatten();
    // A `NB_DIR=...` prefix is transient, except before the
    // declaration builtins, which get their own branches
    // (`export` persists; `readonly` declines on involvement);
    // anything else with a command name runs the body with a
    // transient environment.
    let transient = command.word_or_name.is_some()
        && !effective.as_deref().is_some_and(is_declaration_builtin)
        && command.prefix.as_ref().is_some_and(|prefix| {
            prefix.0.iter().any(|item| {
                matches!(item,
                    Item::AssignmentWord(assignment, _)
                        if is_nb_dir(&assignment.name))
            })
        });
    if transient {
        // Transient `NB_DIR=... cmd`: the `NB_DIR` assignment
        // itself does not persist, but `${NB_DIR:=...}` side
        // effects anywhere in the command do. Scan every
        // assignment for those (and for anything undeterminable);
        // anything else is ignored.
        if let Some(reason) = scan_side_effects(command, home, nb_dir) {
            return ItemOutcome::Undetermined(reason);
        }
        return ItemOutcome::Ignore;
    }
    // Effective identity (quotes/concatenation resolved) routes
    // builtins: a quoted `'export'` still exports in Bash.
    // (Computed above; reused by the branches below.)
    match &command.word_or_name {
        None => {
            let mut outcome = ItemOutcome::Ignore;
            if let Some(prefix) = &command.prefix {
                for item in &prefix.0 {
                    match item {
                        Item::IoRedirect(_) => {},
                        Item::AssignmentWord(assignment, _) => {
                            match classify_assignment(assignment, home, nb_dir)
                            {
                                AssignOutcome::Value(value) => {
                                    // Eager table update: later
                                    // assignments in this same command
                                    // observe this one, matching shell
                                    // left-to-right semantics.
                                    *nb_dir = Some(value.clone());
                                    outcome = ItemOutcome::Assign(value);
                                },
                                AssignOutcome::Undetermined(reason) => {
                                    return ItemOutcome::Undetermined(reason);
                                },
                                AssignOutcome::Unrelated => {},
                            }
                        },
                        _ => {
                            return ItemOutcome::Undetermined(
                                "unsupported assignment form",
                            );
                        },
                    }
                }
            }
            if let Some(suffix) = &command.suffix {
                for item in &suffix.0 {
                    match item {
                        Item::IoRedirect(_) => {},
                        Item::AssignmentWord(assignment, _) => {
                            match classify_assignment(assignment, home, nb_dir)
                            {
                                AssignOutcome::Value(value) => {
                                    // Eager table update: later
                                    // assignments in this same command
                                    // observe this one, matching shell
                                    // left-to-right semantics.
                                    *nb_dir = Some(value.clone());
                                    outcome = ItemOutcome::Assign(value);
                                },
                                AssignOutcome::Undetermined(reason) => {
                                    return ItemOutcome::Undetermined(reason);
                                },
                                AssignOutcome::Unrelated => {},
                            }
                        },
                        _ => {
                            return ItemOutcome::Undetermined(
                                "unsupported assignment form",
                            );
                        },
                    }
                }
            }
            outcome
        },
        Some(_) if effective.as_deref() == Some("export") => {
            // Evaluation order is suffix expansion first, prefix
            // evaluation second, bindings last (all verified
            // against Bash): suffix `NB_DIR` words expand against
            // the pre-command table with `${NB_DIR:=...}` taking
            // immediate effect for later suffix words, while
            // prefix values only contribute side effects and a
            // transient binding. The last suffix binding commits;
            // otherwise a bare `NB_DIR` word captures the transient
            // prefix value; otherwise the prefix is dropped.
            let mut outcome = ItemOutcome::Ignore;
            let mut pending: Option<String> = None;
            let mut bare_word = false;
            if let Some(suffix) = &command.suffix {
                for item in &suffix.0 {
                    match item {
                        Item::IoRedirect(_) => {},
                        Item::AssignmentWord(assignment, _) => {
                            if is_nb_dir(&assignment.name) {
                                match classify_assignment(
                                    assignment, home, nb_dir,
                                ) {
                                    AssignOutcome::Value(value) => {
                                        pending = Some(value.clone());
                                        outcome = ItemOutcome::Assign(value);
                                    },
                                    AssignOutcome::Undetermined(reason) => {
                                        return ItemOutcome::Undetermined(
                                            reason,
                                        );
                                    },
                                    AssignOutcome::Unrelated => {},
                                }
                            } else {
                                match classify_assignment(
                                    assignment, home, nb_dir,
                                ) {
                                    AssignOutcome::Value(_) => {},
                                    AssignOutcome::Undetermined(reason) => {
                                        return ItemOutcome::Undetermined(
                                            reason,
                                        );
                                    },
                                    AssignOutcome::Unrelated => {},
                                }
                            }
                        },
                        Item::Word(word) if word.value == "NB_DIR" => {
                            bare_word = true;
                        },
                        Item::Word(word) if mentions_nb_dir(&word.value) => {
                            return ItemOutcome::Undetermined(
                                "unsupported export form",
                            );
                        },
                        Item::Word(_) => {},
                        _ => {
                            return ItemOutcome::Undetermined(
                                "unsupported export form",
                            );
                        },
                    }
                }
            }
            let transient = match collect_transient_prefix(
                command,
                home,
                nb_dir,
                "unsupported export form",
            ) {
                Ok(transient) => transient,
                Err(reason) => return ItemOutcome::Undetermined(reason),
            };
            if let Some(value) = pending {
                *nb_dir = Some(value);
            } else if bare_word && let Some(value) = transient {
                *nb_dir = Some(value.clone());
                outcome = ItemOutcome::Assign(value);
            }
            outcome
        },
        Some(_) if effective.as_deref() == Some("readonly") => {
            // `readonly` persistence is declined rather than
            // modeled: a bare `NB_DIR` word or assignment would
            // freeze the store against later reassignment (Bash
            // rejects the reassignment and retains the old
            // value), so any `NB_DIR` involvement in the suffix
            // selects fallback. Bare words are evaluated, not
            // string-matched, so quoted spellings (`'NB_DIR'`,
            // `"NB_DIR"`) decline the same way; only words proven
            // unrelated are ignored. Anything else is scanned
            // for side effects; a bare prefix stays transient
            // like any other command.
            if let Some(suffix) = &command.suffix {
                for item in &suffix.0 {
                    match item {
                        Item::AssignmentWord(assignment, _)
                            if is_nb_dir(&assignment.name) =>
                        {
                            return ItemOutcome::Undetermined(
                                "readonly NB_DIR is not modeled",
                            );
                        },
                        // Ignore only demonstrably literal
                        // unrelated operands: bare identifiers
                        // other than NB_DIR, and simple flags.
                        // Everything else falls back without
                        // evaluation — unquoted expansions
                        // word-split (a `$NB_DIR` holding
                        // "/a NB_DIR" marks NB_DIR readonly),
                        // and quote removal or brace expansion
                        // may reveal it.
                        Item::Word(word)
                            if word.value == "NB_DIR"
                                || !is_demonstrably_literal(&word.value) =>
                        {
                            return ItemOutcome::Undetermined(
                                "readonly NB_DIR is not modeled",
                            );
                        },
                        _ => {},
                    }
                }
            }
            if let Some(reason) = scan_side_effects(command, home, nb_dir) {
                return ItemOutcome::Undetermined(reason);
            }
            ItemOutcome::Ignore
        },
        Some(_) if effective.as_deref() == Some("unset") => {
            let mut clearing = false;
            if let Some(suffix) = &command.suffix {
                for item in &suffix.0 {
                    match item {
                        Item::IoRedirect(_) => {},
                        Item::Word(word)
                            if word.value == "NB_DIR"
                                || word.value == "-v"
                                || word.value == "--" =>
                        {
                            if word.value == "NB_DIR" {
                                clearing = true;
                            }
                        },
                        _ => {
                            return ItemOutcome::Undetermined(
                                "unsupported unset form",
                            );
                        },
                    }
                }
            }
            if clearing {
                *nb_dir = None;
                ItemOutcome::Clear
            } else {
                ItemOutcome::Ignore
            }
        },
        Some(name) if name.value == "source" || name.value == "." => {
            ItemOutcome::Undetermined("sourced file may set NB_DIR")
        },
        Some(_) => {
            // Any other command: nothing persists positionally,
            // but `${NB_DIR:=...}` side effects in its assignments
            // do. Words carrying `NB_DIR` (possible code execution
            // such as `eval`) fall back; anything already scanned
            // above is ignored.
            if let Some(reason) = scan_side_effects(command, home, nb_dir) {
                return ItemOutcome::Undetermined(reason);
            }
            // An `NB_DIR` assignment under an unknown command
            // (`readonly`, `declare`, ...) claims a persistent
            // binding with unclear semantics: reject rather than
            // ignore or honor it.
            if assigns_nb_dir(command) {
                return ItemOutcome::Undetermined(
                    "unsupported NB_DIR assignment",
                );
            }
            if words_mention_nb_dir(command) {
                ItemOutcome::Undetermined("unsupported NB_DIR use")
            } else {
                ItemOutcome::Ignore
            }
        },
    }
}

/// Whether any assignment in the command targets (part of)
/// `NB_DIR` by name: `NB_DIR=...` or `NB_DIR[0]=...` in any
/// position.
fn assigns_nb_dir(command: &brush_parser::ast::SimpleCommand) -> bool {
    use brush_parser::ast::CommandPrefixOrSuffixItem as Item;
    let mut parts = command
        .prefix
        .as_ref()
        .map(|prefix| prefix.0.as_slice())
        .unwrap_or(&[])
        .iter()
        .chain(
            command
                .suffix
                .as_ref()
                .map(|suffix| suffix.0.as_slice())
                .unwrap_or(&[]),
        );
    parts.any(|item| {
        matches!(item,
            Item::AssignmentWord(assignment, _)
                if mentions_nb_dir(&assignment.name.to_string()))
    })
}

/// Whether the command words (name and bare arguments, not
/// assignments, which are scanned separately) mention `NB_DIR`.
fn words_mention_nb_dir(command: &brush_parser::ast::SimpleCommand) -> bool {
    use brush_parser::ast::CommandPrefixOrSuffixItem as Item;
    if command
        .word_or_name
        .as_ref()
        .is_some_and(|word| mentions_nb_dir(&word.value))
    {
        return true;
    }
    let mut parts = command
        .prefix
        .as_ref()
        .map(|prefix| prefix.0.as_slice())
        .unwrap_or(&[])
        .iter()
        .chain(
            command
                .suffix
                .as_ref()
                .map(|suffix| suffix.0.as_slice())
                .unwrap_or(&[]),
        );
    parts.any(|item| {
        matches!(item, Item::Word(word) if mentions_nb_dir(&word.value))
    })
}

/// Whether a command re-routes a preceding `NB_DIR=` prefix
/// away from transient treatment. `export` persists it (bare
/// word capture or pending suffix binding); `readonly` gets its
/// own declining branch. Every other command name runs with
/// transient prefix assignments. (`declare`/`typeset`/`local`
/// were verified transient; `unset` drops the prefix too.)
fn is_declaration_builtin(name: &str) -> bool {
    matches!(name, "export" | "readonly")
}

/// Process prefix assignments for a declaration builtin: values
/// are evaluated for side effects against the live table, and a
/// plain `NB_DIR` value is collected as the transient prefix
/// binding (last wins) without committing it. The caller decides
/// whether a bare word captures it or a suffix binding overrides
/// it.
fn collect_transient_prefix(
    command: &brush_parser::ast::SimpleCommand,
    home: Option<&Path>,
    nb_dir: &mut Option<String>,
    form: &'static str,
) -> Result<Option<String>, &'static str> {
    use brush_parser::ast::CommandPrefixOrSuffixItem as Item;
    let mut transient = None;
    if let Some(prefix) = &command.prefix {
        for item in &prefix.0 {
            match item {
                Item::IoRedirect(_) => {},
                Item::AssignmentWord(assignment, _) => {
                    match classify_assignment(assignment, home, nb_dir) {
                        AssignOutcome::Value(value) => {
                            if is_nb_dir(&assignment.name) {
                                transient = Some(value);
                            }
                        },
                        AssignOutcome::Undetermined(reason) => {
                            return Err(reason);
                        },
                        AssignOutcome::Unrelated => {},
                    }
                },
                _ => return Err(form),
            }
        }
    }
    Ok(transient)
}

/// Whether an assignment name is the plain `NB_DIR` variable.
fn is_nb_dir(name: &brush_parser::ast::AssignmentName) -> bool {
    matches!(name,
        brush_parser::ast::AssignmentName::VariableName(name)
            if name == "NB_DIR")
}

/// Whether a raw word is demonstrably literal: a bare
/// identifier or a simple flag, with nothing to expand, quote,
/// glob, or brace-expand. Only these are proven unrelated
/// without evaluation.
fn is_demonstrably_literal(word: &str) -> bool {
    let identifier = word
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && word
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    let flag = word.len() > 1
        && word.starts_with('-')
        && word[1..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    identifier || flag
}

/// Outcome of classifying one assignment.
enum AssignOutcome {
    Value(String),
    Undetermined(&'static str),
    Unrelated,
}

/// Classify one assignment: `NB_DIR=<literal>` assigns (last wins),
/// anything dynamic falls back, anything else is unrelated — except
/// that values mentioning `NB_DIR` are still evaluated, because
/// `${NB_DIR:=...}` side effects persist no matter which variable
/// is being assigned.
fn classify_assignment(
    assignment: &brush_parser::ast::Assignment,
    home: Option<&Path>,
    nb_dir: &mut Option<String>,
) -> AssignOutcome {
    if !is_nb_dir(&assignment.name) {
        // `NB_DIR[0]=...`: assignment into the variable through a
        // subscript. Arrays cannot be modeled (Bash retypes the
        // scalar on first subscript assignment), so fall back
        // rather than keep a stale value.
        if mentions_nb_dir(&assignment.name.to_string()) {
            return AssignOutcome::Undetermined(
                "unsupported NB_DIR assignment",
            );
        }
        if mentions_nb_dir(&format!("{:?}", assignment.value)) {
            match &assignment.value {
                brush_parser::ast::AssignmentValue::Scalar(word) => {
                    match eval_word(word, home, nb_dir) {
                        Ok(_) => AssignOutcome::Unrelated,
                        Err(reason) => AssignOutcome::Undetermined(reason),
                    }
                },
                brush_parser::ast::AssignmentValue::Array(_) => {
                    AssignOutcome::Undetermined("array assignment")
                },
            }
        } else {
            AssignOutcome::Unrelated
        }
    } else if assignment.append {
        AssignOutcome::Undetermined("append assignment")
    } else {
        match &assignment.value {
            brush_parser::ast::AssignmentValue::Scalar(word) => {
                match eval_word(word, home, nb_dir) {
                    Ok(value) => AssignOutcome::Value(value),
                    Err(reason) => AssignOutcome::Undetermined(reason),
                }
            },
            brush_parser::ast::AssignmentValue::Array(_) => {
                AssignOutcome::Undetermined("array assignment")
            },
        }
    }
}

/// Evaluate a word to a literal string against `home` and the
/// current `NB_DIR` table. Anything that cannot be determined
/// without execution (expansions other than a leading `~`,
/// `$HOME`, or `$NB_DIR` self-defaults, command substitution,
/// arithmetic, globs left to the shell) fails instead of being
/// guessed at.
fn eval_word(
    word: &brush_parser::ast::Word,
    home: Option<&Path>,
    nb_dir: &mut Option<String>,
) -> Result<String, &'static str> {
    let pieces = brush_parser::word::parse(
        &word.value,
        &brush_parser::ParserOptions::default(),
    )
    .map_err(|_| "unparseable value")?;
    eval_pieces(&pieces, home, nb_dir, false)
}

/// Evaluate word pieces. `quoted` selects double-quote semantics
/// (no tilde expansion, backslash only special before `$`, `` ` ``,
/// `"`, `\`, newline).
fn eval_pieces(
    pieces: &[brush_parser::word::WordPieceWithSource],
    home: Option<&Path>,
    nb_dir: &mut Option<String>,
    quoted: bool,
) -> Result<String, &'static str> {
    use brush_parser::word::WordPiece;
    let mut out = String::new();
    let mut first = true;
    for piece in pieces {
        match &piece.piece {
            WordPiece::Text(text) => out.push_str(text),
            WordPiece::SingleQuotedText(text) => out.push_str(text),
            WordPiece::DoubleQuotedSequence(inner) => {
                out.push_str(&eval_pieces(inner, home, nb_dir, true)?);
            },
            WordPiece::GettextDoubleQuotedSequence(_) => {
                return Err("gettext quoting");
            },
            WordPiece::AnsiCQuotedText(_) => {
                return Err("ansi-c quoting");
            },
            WordPiece::TildeExpansion(tilde) => {
                use brush_parser::word::TildeExpr;
                match tilde {
                    TildeExpr::Home if first && !quoted => {
                        let home = home.ok_or("home directory unavailable")?;
                        out.push_str(&home.to_string_lossy());
                    },
                    TildeExpr::Home => {
                        out.push('~');
                    },
                    _ => return Err("tilde expansion"),
                }
            },
            WordPiece::ParameterExpansion(expansion) => {
                out.push_str(&eval_parameter(
                    expansion, home, nb_dir, quoted,
                )?);
            },
            WordPiece::CommandSubstitution(_)
            | WordPiece::BackquotedCommandSubstitution(_)
            | WordPiece::ArithmeticExpression(_) => {
                return Err("command substitution");
            },
            WordPiece::EscapeSequence(escaped) => {
                let rest = escaped.strip_prefix('\\').unwrap_or(escaped);
                if quoted {
                    match rest {
                        "$" | "`" | "\"" | "\\" => out.push_str(rest),
                        "\n" => {},
                        _ => out.push_str(escaped),
                    }
                } else if rest == "\n" {
                } else {
                    out.push_str(rest);
                }
            },
        }
        first = false;
    }
    Ok(out)
}

/// Evaluate one parameter expansion. `$HOME`/`${HOME}` expand
/// against `home`; bare `$NB_DIR`/`${NB_DIR}` read the rc table
/// (empty when unset); `${NB_DIR:-word}`, `${NB_DIR-word}`,
/// `${NB_DIR:=word}`, `${NB_DIR=word}`, `${NB_DIR:+word}`, and
/// `${NB_DIR+word}` evaluate the literal default/alternative
/// (process `NB_DIR` is unset in this branch, so self-defaults
/// always take the default the first time and track later
/// assignments). `quoted` is the quoting context of the expansion
/// site: defaults re-parsed from raw text inherit it, so an
/// unquoted `${NB_DIR:-~/x}` expands `~` while a double-quoted
/// one keeps it literal (matching Bash; a literal `~` then fails
/// downstream as a relative path instead of being absolutized).
/// Anything else fails instead of being guessed at.
fn eval_parameter(
    expansion: &brush_parser::word::ParameterExpr,
    home: Option<&Path>,
    nb_dir: &mut Option<String>,
    quoted: bool,
) -> Result<String, &'static str> {
    use brush_parser::word::{Parameter, ParameterExpr, ParameterTestType};
    // `:-` and friends treat empty as unset; `-` and friends
    // only treat truly-unset as missing.
    let is_missing = |test: &ParameterTestType| match test {
        ParameterTestType::UnsetOrNull => {
            nb_dir.as_deref().is_none_or(|v| v.is_empty())
        },
        ParameterTestType::Unset => nb_dir.is_none(),
    };
    match expansion {
        ParameterExpr::Parameter {
            parameter: Parameter::Named(name),
            indirect: false,
        } if name == "HOME" => {
            let home = home.ok_or("home directory unavailable")?;
            Ok(home.to_string_lossy().into_owned())
        },
        ParameterExpr::Parameter {
            parameter: Parameter::Named(name),
            indirect: false,
        } if name == "NB_DIR" => Ok(nb_dir.clone().unwrap_or_default()),
        ParameterExpr::UseDefaultValues {
            parameter: Parameter::Named(name),
            indirect: false,
            test_type,
            default_value,
        } if name == "NB_DIR" => {
            if is_missing(test_type) {
                eval_default_word(default_value, home, nb_dir, quoted)
            } else {
                Ok(nb_dir.clone().unwrap_or_default())
            }
        },
        ParameterExpr::AssignDefaultValues {
            parameter: Parameter::Named(name),
            indirect: false,
            test_type,
            default_value,
        } if name == "NB_DIR" => {
            if is_missing(test_type) {
                let value =
                    eval_default_word(default_value, home, nb_dir, quoted)?;
                *nb_dir = Some(value.clone());
                Ok(value)
            } else {
                Ok(nb_dir.clone().unwrap_or_default())
            }
        },
        ParameterExpr::UseAlternativeValue {
            parameter: Parameter::Named(name),
            indirect: false,
            test_type,
            alternative_value,
        } if name == "NB_DIR" => {
            let present = match test_type {
                ParameterTestType::UnsetOrNull => {
                    nb_dir.as_deref().is_some_and(|v| !v.is_empty())
                },
                ParameterTestType::Unset => nb_dir.is_some(),
            };
            if present {
                eval_default_word(alternative_value, home, nb_dir, quoted)
            } else {
                Ok(String::new())
            }
        },
        _ => Err("non-HOME expansion"),
    }
}

/// Evaluate a literal default/alternative word against the
/// same table (allows `${NB_DIR:-$HOME/x}` and nesting).
/// Evaluate a literal default/alternative word against the
/// same table (allows `${NB_DIR:-$HOME/x}` and nesting). `quoted`
/// carries the quoting context of the expansion site (see
/// [`eval_parameter`]). A re-parsed default cannot see the outer
/// quotes, so in a double-quoted context any nested quoting form
/// (single/ANSI-C/gettext quotes, inner double-quote sequences)
/// would be misread — standalone they quote, in context they are
/// literal characters — and selects fallback instead. Plain text,
/// `~`, `$HOME`, and nested defaults keep working.
fn eval_default_word(
    raw: &Option<String>,
    home: Option<&Path>,
    nb_dir: &mut Option<String>,
    quoted: bool,
) -> Result<String, &'static str> {
    match raw {
        None => Ok(String::new()),
        Some(text) => {
            let pieces = brush_parser::word::parse(
                text,
                &brush_parser::ParserOptions::default(),
            )
            .map_err(|_| "unparseable default")?;
            if quoted && default_has_nested_quoting(&pieces) {
                return Err("nested quoting in double-quoted default");
            }
            eval_pieces(pieces.as_slice(), home, nb_dir, quoted)
        },
    }
}

/// Whether re-parsed default pieces contain a quoting form that
/// only quotes standalone. Inside double quotes these characters
/// are literal, so the standalone reading would be wrong.
fn default_has_nested_quoting(
    pieces: &[brush_parser::word::WordPieceWithSource],
) -> bool {
    use brush_parser::word::WordPiece;
    pieces.iter().any(|piece| {
        matches!(
            piece.piece,
            WordPiece::SingleQuotedText(_)
                | WordPiece::AnsiCQuotedText(_)
                | WordPiece::GettextDoubleQuotedSequence(_)
                | WordPiece::DoubleQuotedSequence(_)
        )
    })
}
/// Whether `NB_DIR` appears as a standalone word (not inside a
/// longer identifier such as `MY_NB_DIR`).
fn mentions_nb_dir(code: &str) -> bool {
    let mut start = 0;
    while let Some(index) = code[start..].find("NB_DIR") {
        let index = start + index;
        let before = code[..index].chars().next_back();
        let after = code[index + "NB_DIR".len()..].chars().next();
        let boundary = |c: Option<char>| {
            c.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
        };
        if boundary(before) && boundary(after) {
            return true;
        }
        start = index + 1;
    }
    false
}
