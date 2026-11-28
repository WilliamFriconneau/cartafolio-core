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
        "Keep standing targets in one pinned note per space, tagged 'targets' and written in the \
         language the person uses there. Read it first with list{space, pinned:true}. Keep it short \
         and factual: one line per target with a plain label, start date, last seen date and a line of evidence \
         from the notes. No diagnosis, advice or invented facts. Review the last 30 days with \
         list{space, since}, using cursor if needed, or recall. Use Unix seconds for since. Update with supersede in the same \
         space, keeping all tags including 'targets'. The pin stays. Drop targets with no sign for \
         a long time. The old version stays in history. If asked for targets and no targets note \
         exists, create it with remember, tag it 'targets', then pin it. Sort other notes with \
         tags_set and move.";

    pub const PIN_DESCRIPTION: &str =
        "Pin an active note in place. Omit pinned to pin it, or pass false to unpin it. \
         Pin the targets note after creating it.";
    pub const LIST_DESCRIPTION: &str =
        "List notes in a space or across accessible spaces. With pinned true, list pinned notes \
         newest pin first. With pinned false, list unpinned notes. Pass space and pinned true \
         to find the targets note for that space.";
    pub const SUPERSEDE_DESCRIPTION: &str =
        "Replace a note with a new version in the same space, keeping its pin and history.";
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
