//! Pure Lemma MCP tools: `Engine` + arguments in, catalog or text out.
//!
//! No I/O, no JSON-RPC, no session, no remembered engine.

mod catalog;
mod error;
mod tools;

pub use catalog::{list_resources, list_tools, read_resource, ResourceDefinition, ToolDefinition};
pub use error::{ResourceError, ToolError};
pub use tools::{check, evaluate, guide, run, show, source, specs};
