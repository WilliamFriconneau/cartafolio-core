//! Where a space's notes go. What touches the network or the phone's files
//! stays in the app; the core keeps the settings and decides what leaves.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorKind {
    /// The note stays in the app: nothing to deliver.
    Local,
    Memcp,
    McpMemory,
    Markdown,
    Drive,
}

impl ConnectorKind {
    pub fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(s.into())).ok()
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Memcp => "memcp",
            Self::McpMemory => "mcp_memory",
            Self::Markdown => "markdown",
            Self::Drive => "drive",
        }
    }
}

/// An export, or the folder is the truth and the app follows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Export,
    SourceOfTruth,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(s.into())).ok()
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Export => "export",
            Self::SourceOfTruth => "source_of_truth",
        }
    }
}

/// One destination of a space. Its secrets stay in the app's keychain, under its id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorConfig {
    pub id: String,
    pub kind: ConnectorKind,
    pub mode: Mode,
    /// A folder bookmark, an address, a Drive folder id: the app's to read.
    #[serde(default)]
    pub target: String,
    /// The target is on iCloud Drive: refused for a sensitive space (rule 5.1.3(ii)).
    #[serde(default)]
    pub icloud: bool,
}

impl ConnectorConfig {
    /// Whether a note goes out through it (a local connector keeps it in the app).
    pub fn delivers(&self) -> bool {
        self.kind != ConnectorKind::Local
    }
}
