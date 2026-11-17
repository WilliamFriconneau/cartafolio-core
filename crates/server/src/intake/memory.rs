//! The node's own hand on its memory: the tools an assistant calls, called in the same process with the rights of the
//! person the intake works for (every space they own, a space made later included). No key, no address, no network.

use base64::Engine as _;
use memcp_core::access::{self, Caller};
use memcp_core::tools::{self, ToolError};
use memcp_core::{Brain, users};
use serde_json::{Value, json};

/// A tool the memory refused, or a failure inside it, in the memory's own words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure(pub String);

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Failure {}

impl From<ToolError> for Failure {
    fn from(e: ToolError) -> Self {
        Failure(e.to_string())
    }
}

/// The name the notes and files the intake writes carry as their author.
pub const AUTHOR: &str = "intake";

/// The memory of one person, called from inside the node.
#[derive(Clone)]
pub struct Memory {
    brain: Brain,
    user_id: String,
}

impl Memory {
    /// The intake of `person` (a login name or an id); `None`: the first administrator.
    pub fn open(brain: &Brain, person: Option<&str>) -> Result<Memory, Failure> {
        let ctl = brain.open_control().map_err(|e| Failure(e.to_string()))?;
        let user = users::resolve(&ctl, person.unwrap_or(users::OWNER))
            .map_err(|e| Failure(e.to_string()))?;
        if user.disabled {
            return Err(Failure(format!("{} is disabled", user.name)));
        }
        Ok(Memory {
            brain: brain.clone(),
            user_id: user.id,
        })
    }

    pub fn brain(&self) -> &Brain {
        &self.brain
    }

    /// The person's login name.
    pub fn person(&self) -> Result<String, Failure> {
        let ctl = self
            .brain
            .open_control()
            .map_err(|e| Failure(e.to_string()))?;
        users::resolve(&ctl, &self.user_id)
            .map(|u| u.name)
            .map_err(|e| Failure(e.to_string()))
    }

    /// The person's Inbox, made if it is not there yet.
    pub fn triage_space(&self) -> Result<String, Failure> {
        let person = self.person()?;
        memcp_core::triage::ensure(&self.brain, &self.user_id, &person)
            .map(|s| s.to_string())
            .map_err(|e| Failure(e.to_string()))
    }

    /// The rights are read at each call: a space made since is reached too.
    fn caller(&self) -> Result<Caller, Failure> {
        let ctl = self
            .brain
            .open_control()
            .map_err(|e| Failure(e.to_string()))?;
        let grants = users::access_of(&ctl, &self.user_id).map_err(|e| Failure(e.to_string()))?;
        Ok(Caller {
            id: AUTHOR.into(),
            agent: AUTHOR.into(),
            user_id: self.user_id.clone(),
            all_spaces: false,
            known: grants.keys().cloned().collect(),
            grants,
            paths: Default::default(),
            read_only_forced: false,
            no_person_guide: true,
            rate_per_min: access::DEFAULT_RATE_PER_MIN,
            max_text: access::DEFAULT_MAX_TEXT,
        })
    }

    /// One tool, as an assistant would call it.
    pub fn call(&mut self, tool: &str, args: Value) -> Result<Value, Failure> {
        let caller = self.caller()?;
        tools::call(&self.brain, &caller, tool, &args).map_err(Failure::from)
    }

    /// Proposes the app guide of the intake for `space`: the person approves it on their page, then the assistants of the
    /// space are told. Only a connection limited to the path `guide` of the space may propose one, so the call is made
    /// with exactly that right.
    pub fn propose_guide(&mut self, space: &str, text: &str) -> Result<Value, Failure> {
        let mut caller = self.caller()?;
        let s = memcp_core::Space::new(space).map_err(|e| Failure(e.to_string()))?;
        caller.paths.insert(s, vec![("guide".to_string(), true)]);
        tools::call(
            &self.brain,
            &caller,
            "guide_propose",
            &json!({ "space": space, "text": text }),
        )
        .map_err(Failure::from)
    }

    /// A file kept in the memory: what it says of it, and its bytes.
    pub fn file_get(&mut self, id: &str) -> Result<(Value, Vec<u8>), Failure> {
        let answer = self.call("file_get", json!({ "id": id }))?;
        let block = &answer["mcp_content"][0];
        let engine = base64::engine::general_purpose::STANDARD;
        let bytes = if let Some(blob) = block["resource"]["blob"]
            .as_str()
            .or_else(|| block["data"].as_str())
        {
            engine
                .decode(blob)
                .map_err(|_| Failure("file_get: the bytes are not base64".into()))?
        } else if let Some(text) = block["text"].as_str() {
            text.as_bytes().to_vec()
        } else {
            return Err(Failure("file_get: no bytes in the answer".into()));
        };
        Ok((answer["file"].clone(), bytes))
    }

    /// Keeps a file under `tags` of `space`, with a line saying what it is.
    pub fn file_put(
        &mut self,
        space: &str,
        name: &str,
        note: &str,
        tags: &[&str],
        bytes: &[u8],
    ) -> Result<Value, Failure> {
        self.call(
            "file_put",
            json!({
                "space": space, "name": name, "note": note, "tags": tags,
                "data_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
        )
    }
}
