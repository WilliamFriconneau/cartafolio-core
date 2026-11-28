#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::todo,
    clippy::dbg_macro,
    clippy::print_stdout
)]

mod dispatch;
mod rpc;
mod schema;
mod tools;

use pensine_access::Accounts;
use std::sync::Arc;

pub struct Mcp {
    accounts: Arc<Accounts>,
}

impl Mcp {
    pub fn new(accounts: Arc<Accounts>) -> Self {
        Self { accounts }
    }
}

mod texts {
    pub const INVALID_REQUEST: &str = "Invalid Request";
    pub const INVALID_PARAMS: &str = "Invalid params";
    pub const METHOD_NOT_FOUND: &str = "Method not found";
    pub const UNKNOWN_TOOL: &str = "Unknown tool";
    pub const INVALID_ARGUMENTS: &str = "Invalid tool arguments";
    pub const NOT_FOUND: &str = "not found";
    pub const CONFLICT: &str = "Conflict";
    pub const INTERNAL_ERROR: &str = "Internal error";
    pub const WRITE_REQUIRED: &str = "Write access required";
    pub const INVALID_BASE64: &str = "Invalid base64 content";

    pub fn download_limit(size: u64, limit: u64) -> String {
        format!("File is {size} bytes. The download limit is {limit} bytes.")
    }
}
