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

    pub fn accounts(&self) -> &Accounts {
        &self.accounts
    }
}

mod texts {
    pub const INSTRUCTIONS: &str =
        "Pinned notes are the person's standing targets. Read them first with list{pinned:true}. \
         Use the space argument when focusing on a particular space. To see what recurs, look at \
         the last 30 days with list{since} or use recall with search words and the same since cutoff. \
         Set since to the cutoff in Unix seconds. Use tags_set to organize a note by topic and move \
         to put it in the right space. Describe what the notes support and never invent a diagnosis.";

    pub const PIN_DESCRIPTION: &str =
        "Pin an active note in place. Omit pinned to pin it, or pass false to unpin it.";
    pub const LIST_DESCRIPTION: &str =
        "List notes in a space or across accessible spaces. With pinned true, list pinned notes \
         newest pin first. With pinned false, list unpinned notes.";
    pub const RECALL_DESCRIPTION: &str =
        "Search notes by relevance with a boost for pinned notes. Set pinned to true to search \
         only pinned notes or false to search only unpinned notes.";
    pub const SPACES_DESCRIPTION: &str =
        "List accessible spaces with counts of active notes, files, and pinned notes.";

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
