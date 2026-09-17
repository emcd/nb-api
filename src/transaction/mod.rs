//! Collect-then-commit [`Transaction`] plan and apply engine.

mod apply;
mod commit;
mod index;
mod plan;
mod virtual_tree;

pub(crate) use index::{
    filename_for_title, index_id_in_folder, join_folder_file, suffixed_filename,
};
pub use plan::Transaction;
