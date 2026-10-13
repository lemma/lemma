//! List, row, and import-row integration tests. One binary so the shared
//! `support/lists.rs` helpers are checked against every consumer.

#[path = "../support/explanation.rs"]
mod explanation_support;
#[path = "../support/lists.rs"]
mod lists;

mod import_row_missing_data;
mod list_access;
mod list_commands;
mod list_declarations;
mod list_evaluate_data;
mod list_keys;
mod list_output;
mod list_recursion;
mod list_rows_with;
mod list_syntax;
mod list_temporal;
mod list_worked_examples;
mod scalar_with_command;
