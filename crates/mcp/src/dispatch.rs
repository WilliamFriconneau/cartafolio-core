use crate::texts;
use crate::{schema::TOOLS, tools::Scope, Mcp};
use pensine_access::{AccessError, Principal};
use pensine_store::StoreError;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt;

#[derive(Debug)]
pub(crate) enum ToolError {
    Store(StoreError),
    Message(String),
    Internal,
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(StoreError::NotFound) => f.write_str(texts::NOT_FOUND),
            Self::Store(StoreError::Conflict) => f.write_str(texts::CONFLICT),
            Self::Store(StoreError::Invalid(message)) | Self::Message(message) => {
                f.write_str(message)
            }
            _ => f.write_str(texts::INTERNAL_ERROR),
        }
    }
}

impl From<StoreError> for ToolError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<AccessError> for ToolError {
    fn from(error: AccessError) -> Self {
        match error {
            AccessError::NotFound => StoreError::NotFound.into(),
            _ => Self::Internal,
        }
    }
}

pub(crate) type Result<T> = std::result::Result<T, ToolError>;

impl Mcp {
    pub(crate) fn call(&self, principal: &Principal, name: &str, args: Value) -> Result<Value> {
        let tool = TOOLS
            .iter()
            .find(|tool| tool.name == name)
            .ok_or_else(|| ToolError::Message(texts::UNKNOWN_TOOL.into()))?;
        tool.validate(&args)?;
        let account = self.accounts.account(&principal.account)?;
        if !principal.belongs_to(&account) {
            return Err(StoreError::NotFound.into());
        }
        let _operations = account.lock_operations()?;
        let scope = Scope {
            principal,
            store: &account.store,
        };
        match name {
            "remember" => {
                let input: Remember = parse(args)?;
                let mut results = scope.remember(&input.space, vec![input.note])?;
                output(results.pop().ok_or(ToolError::Internal)?)
            }
            "remember_many" => {
                let input: RememberMany = parse(args)?;
                output(scope.remember(&input.space, input.notes)?)
            }
            "recall" => output(scope.recall(parse(args)?)?),
            "list" => scope.list(parse(args)?),
            "read" => {
                let id: Id = parse(args)?;
                output(scope.visible_fact(scope.fact(&id.id)?)?)
            }
            "pin" => {
                let input: Pin = parse(args)?;
                scope.write_fact(&input.id)?;
                output(
                    scope
                        .visible_fact(scope.store.pin(&input.id, input.pinned.unwrap_or(true))?)?,
                )
            }
            "history" => {
                let id: Id = parse(args)?;
                output(scope.history(&id.id)?)
            }
            "supersede" => output(scope.supersede(parse(args)?)?),
            "retire" => {
                let input: Reason = parse(args)?;
                scope.write_fact(&input.id)?;
                scope.store.retire(&input.id, &input.reason)?;
                Ok(json!({}))
            }
            "restore" => {
                let input: Id = parse(args)?;
                scope.write_fact(&input.id)?;
                output(scope.visible_fact(scope.store.restore(&input.id)?)?)
            }
            "tags_set" => {
                let input: TagsSet = parse(args)?;
                scope.write_fact(&input.id)?;
                output(scope.visible_fact(scope.store.tags_set(&input.id, input.tags)?)?)
            }
            "move" => {
                let input: Move = parse(args)?;
                output(scope.move_facts(&input.ids, &input.to_space)?)
            }
            "spaces" => output(scope.spaces_view()?),
            "space_create" => {
                let input: SpaceCreate = parse(args)?;
                scope.create_space(&input.name, &input.label, input.parent.as_deref())?;
                Ok(json!({}))
            }
            "space_move" => {
                let parent = args.get("parent").map(Value::as_str);
                let input: SpaceMove = parse(args.clone())?;
                scope.move_space(&input.name, input.label.as_deref(), parent)?;
                Ok(json!({}))
            }
            "file_put" => output(scope.put_file(parse(args)?)?),
            "file_get" => {
                let input: Id = parse(args)?;
                scope.get_file(&input.id)
            }
            "file_list" => output(scope.list_files(parse(args)?)?),
            "file_retire" => {
                let input: Reason = parse(args)?;
                scope.retire_file(&input.id, &input.reason)?;
                Ok(json!({}))
            }
            "access" => self.access(principal, &scope),
            _ => Err(ToolError::Internal),
        }
    }

    fn access(&self, principal: &Principal, scope: &Scope<'_>) -> Result<Value> {
        let alias = self
            .accounts
            .alias_of(&principal.account)
            .unwrap_or_else(|_| principal.account.clone());
        let grants: Vec<Value> = principal
            .grants
            .iter()
            .map(|grant| json!({"space": grant.space, "write": grant.write}))
            .collect();
        Ok(json!({
            "account": principal.account,
            "alias": alias,
            "connection": principal.connection,
            "read_only": principal.read_only(),
            "grants": grants,
            "visible_spaces": scope.spaces()?.len()
        }))
    }
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T> {
    serde_json::from_value(args).map_err(|_| ToolError::Message(texts::INVALID_ARGUMENTS.into()))
}

pub(crate) fn output(value: impl Serialize) -> Result<Value> {
    serde_json::to_value(value).map_err(|_| ToolError::Internal)
}

#[derive(Deserialize)]
struct Remember {
    space: String,
    #[serde(flatten)]
    note: Note,
}

#[derive(Deserialize)]
struct RememberMany {
    space: String,
    notes: Vec<Note>,
}

#[derive(Deserialize)]
pub(crate) struct Note {
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub at: Option<i64>,
    pub origin: Option<(String, String)>,
    pub author: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct Recall {
    pub pinned: Option<bool>,
    pub query: String,
    pub space: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub since: Option<i64>,
    pub limit: Option<usize>,
}

#[derive(Deserialize)]
pub(crate) struct List {
    pub space: Option<String>,
    pub status: Option<String>,
    pub origin_aliases: Option<bool>,
    pub pinned: Option<bool>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub since: Option<i64>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

#[derive(Deserialize)]
struct Id {
    id: String,
}

#[derive(Deserialize)]
struct Pin {
    id: String,
    pinned: Option<bool>,
}

#[derive(Deserialize)]
struct Reason {
    id: String,
    reason: String,
}

#[derive(Deserialize)]
pub(crate) struct Supersede {
    pub(crate) id: String,
    pub(crate) text: String,
    pub(crate) tags: Option<Vec<String>>,
    pub(crate) reason: String,
    pub(crate) at: Option<i64>,
    pub(crate) origin: Option<(String, String)>,
    pub(crate) author: Option<String>,
}

#[derive(Deserialize)]
struct TagsSet {
    id: String,
    tags: Vec<String>,
}

#[derive(Deserialize)]
struct Move {
    ids: Vec<String>,
    to_space: String,
}

#[derive(Deserialize)]
struct SpaceCreate {
    name: String,
    label: String,
    parent: Option<String>,
}

#[derive(Deserialize)]
struct SpaceMove {
    name: String,
    label: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct FilePut {
    pub space: String,
    pub name: String,
    pub mime: Option<String>,
    pub base64: String,
    pub if_absent: Option<bool>,
    pub note: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct FileList {
    pub space: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}
