//! Collect-then-commit [`Transaction`] plan and apply engine.

mod apply;
mod commit;
mod index;
mod plan;
mod virtual_tree;

pub(crate) use index::{
    filename_for_import, filename_for_title, join_folder_file,
    suffixed_filename, windows_portability_refusal,
};
pub use plan::Transaction;
pub(crate) use plan::validate_create_path;
