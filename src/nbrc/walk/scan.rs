//! Opaque-dispatch pre-scan for notebook rc evaluation.
//!
//! Detects sourcing, re-dispatching, and in-shell-executing
//! commands anywhere in the program (behind guards, inside
//! compounds and function bodies, excluding forked subshells,
//! backgrounded items, and coprocesses) by effective command
//! identity. See the parent module docs for the declared subset.

use std::path::Path;

use super::{eval_word, mentions_nb_dir};

/// Resolve the effective command name by evaluating the name
/// word (quote removal, concatenation, `$HOME`, live `$NB_DIR`
/// table). Returns the evaluation error when the name is dynamic:
/// dispatch then cannot be determined without execution.
pub(crate) fn effective_command_name(
    command: &brush_parser::ast::SimpleCommand,
    home: Option<&Path>,
    nb_dir: &mut Option<String>,
) -> Result<Option<String>, &'static str> {
    match &command.word_or_name {
        None => Ok(None),
        Some(word) => eval_word(word, home, nb_dir).map(Some),
    }
}

/// Opaque-dispatch reason for one simple command, if any.
/// Identity is effective, not textual: quotes, concatenation,
/// `$HOME`, and the live table resolve first. `source`/`.`
/// read other files; `builtin`/`command` re-dispatch opaquely;
/// `eval`/`exec` execute in the current shell. `return` aborts
/// sourcing unless it only returns from a function body, so
/// `in_function` exempts `return` alone; `exit` always exits the
/// shell, even from a function, and is never exempt. A name that
/// fails evaluation (`$OTHER`, `$(...)`) or that mentions
/// `NB_DIR` without being the static word itself (`$NB_DIR ...`)
/// cannot be resolved to anything transparent. A static
/// transparent name (including a bare `NB_DIR` command, which
/// Bash fails to look up) is left to the per-item walk. Anything
/// else is transparent here (external commands cannot persist
/// shell state; assignments are scanned separately).
pub(crate) fn opaque_dispatch_reason(
    command: &brush_parser::ast::SimpleCommand,
    home: Option<&Path>,
    nb_dir: &mut Option<String>,
    in_function: bool,
) -> Option<&'static str> {
    let name = command.word_or_name.as_ref()?;
    match effective_command_name(command, home, nb_dir) {
        Ok(Some(resolved)) => match resolved.as_str() {
            "source" | "." => Some("sourced file may set NB_DIR"),
            "builtin" | "command" | "eval" | "exec" => {
                Some("opaque command may set NB_DIR")
            },
            "return" if !in_function => {
                Some("control flow may skip assignments")
            },
            "exit" => Some("control flow may skip assignments"),
            _ if mentions_nb_dir(&name.value) && name.value != resolved => {
                Some("opaque command may set NB_DIR")
            },
            _ => None,
        },
        Ok(None) => None,
        Err(_) => Some("opaque command may set NB_DIR"),
    }
}

/// Whether the program dispatches opaquely anywhere: a sourcing,
/// re-dispatching, or in-shell-executing command at the top level,
/// behind a guard, or inside any compound command or function
/// body. Sourced files are never read; the opaque presence alone
/// selects fallback for the whole file.
pub(crate) fn program_opaque_reason(
    program: &brush_parser::ast::Program,
    home: Option<&Path>,
    nb_dir: &mut Option<String>,
) -> Option<&'static str> {
    program
        .complete_commands
        .iter()
        .find_map(|list| list_opaque_reason(list, home, nb_dir, false))
}

/// Whether a command list dispatches opaquely. Backgrounded
/// (`&`) items run in a subshell, which cannot affect the parent
/// store, and are skipped.
fn list_opaque_reason(
    list: &brush_parser::ast::CompoundList,
    home: Option<&Path>,
    nb_dir: &mut Option<String>,
    in_function: bool,
) -> Option<&'static str> {
    use brush_parser::ast::SeparatorOperator;
    list.0.iter().find_map(
        |brush_parser::ast::CompoundListItem(andor, sep)| {
            if matches!(sep, SeparatorOperator::Async) {
                return None;
            }
            andor_opaque_reason(andor, home, nb_dir, in_function)
        },
    )
}

/// Whether an and/or list dispatches opaquely.
fn andor_opaque_reason(
    andor: &brush_parser::ast::AndOrList,
    home: Option<&Path>,
    nb_dir: &mut Option<String>,
    in_function: bool,
) -> Option<&'static str> {
    if let Some(reason) =
        pipeline_opaque_reason(&andor.first, home, nb_dir, in_function)
    {
        return Some(reason);
    }
    andor.additional.iter().find_map(|branch| match branch {
        brush_parser::ast::AndOr::And(pipeline)
        | brush_parser::ast::AndOr::Or(pipeline) => {
            pipeline_opaque_reason(pipeline, home, nb_dir, in_function)
        },
    })
}

/// Whether a pipeline dispatches opaquely.
fn pipeline_opaque_reason(
    pipeline: &brush_parser::ast::Pipeline,
    home: Option<&Path>,
    nb_dir: &mut Option<String>,
    in_function: bool,
) -> Option<&'static str> {
    pipeline.seq.iter().find_map(|command| {
        command_opaque_reason(command, home, nb_dir, in_function)
    })
}

/// Whether a command is, or contains, an opaque dispatch.
fn command_opaque_reason(
    command: &brush_parser::ast::Command,
    home: Option<&Path>,
    nb_dir: &mut Option<String>,
    in_function: bool,
) -> Option<&'static str> {
    match command {
        brush_parser::ast::Command::Simple(simple) => {
            opaque_dispatch_reason(simple, home, nb_dir, in_function)
        },
        brush_parser::ast::Command::Compound(compound, _) => {
            compound_opaque_reason(compound, home, nb_dir, in_function)
        },
        brush_parser::ast::Command::Function(definition) => {
            compound_opaque_reason(&definition.body.0, home, nb_dir, true)
        },
        brush_parser::ast::Command::ExtendedTest(_, _) => None,
    }
}

/// Whether a compound command contains an opaque dispatch.
/// Subshell bodies run in a forked subshell and coprocess bodies
/// run asynchronously: neither can affect the parent store, so
/// both are skipped.
fn compound_opaque_reason(
    compound: &brush_parser::ast::CompoundCommand,
    home: Option<&Path>,
    nb_dir: &mut Option<String>,
    in_function: bool,
) -> Option<&'static str> {
    use brush_parser::ast::CompoundCommand;
    match compound {
        CompoundCommand::Arithmetic(_) => None,
        CompoundCommand::ArithmeticForClause(command) => {
            do_group_opaque_reason(&command.body, home, nb_dir, in_function)
        },
        CompoundCommand::BraceGroup(command) => {
            list_opaque_reason(&command.list, home, nb_dir, in_function)
        },
        CompoundCommand::Subshell(_) => None,
        CompoundCommand::ForClause(command) => {
            do_group_opaque_reason(&command.body, home, nb_dir, in_function)
        },
        CompoundCommand::CaseClause(command) => {
            command.cases.iter().find_map(|item| {
                item.cmd.as_ref().and_then(|list| {
                    list_opaque_reason(list, home, nb_dir, in_function)
                })
            })
        },
        CompoundCommand::IfClause(command) => {
            list_opaque_reason(&command.condition, home, nb_dir, in_function)
                .or_else(|| {
                    list_opaque_reason(
                        &command.then,
                        home,
                        nb_dir,
                        in_function,
                    )
                })
                .or_else(|| {
                    command.elses.as_ref().and_then(|elses| {
                        elses.iter().find_map(|clause| {
                            clause
                                .condition
                                .as_ref()
                                .and_then(|condition| {
                                    list_opaque_reason(
                                        condition,
                                        home,
                                        nb_dir,
                                        in_function,
                                    )
                                })
                                .or_else(|| {
                                    list_opaque_reason(
                                        &clause.body,
                                        home,
                                        nb_dir,
                                        in_function,
                                    )
                                })
                        })
                    })
                })
        },
        CompoundCommand::WhileClause(command)
        | CompoundCommand::UntilClause(command) => list_opaque_reason(
            &command.0,
            home,
            nb_dir,
            in_function,
        )
        .or_else(|| {
            do_group_opaque_reason(&command.1, home, nb_dir, in_function)
        }),
        CompoundCommand::Coprocess(_) => None,
    }
}

/// Whether a do-group body contains an opaque dispatch.
fn do_group_opaque_reason(
    group: &brush_parser::ast::DoGroupCommand,
    home: Option<&Path>,
    nb_dir: &mut Option<String>,
    in_function: bool,
) -> Option<&'static str> {
    list_opaque_reason(&group.list, home, nb_dir, in_function)
}
