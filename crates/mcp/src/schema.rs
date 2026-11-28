use crate::texts;
use pensine_access::Principal;
use serde_json::{json, Map, Value};

pub(crate) struct Tool {
    pub name: &'static str,
    pub write: bool,
    fields: &'static [Field],
}

type Field = (&'static str, Kind, bool);

#[derive(Clone, Copy)]
enum Kind {
    Text,
    Boolean,
    PinSetting,
    Texts,
    Integer,
    Origin,
    Status,
    Notes,
    Parent,
    Limit(u16),
}

use Kind::{Boolean, Integer, Limit, Notes, Origin, Parent, PinSetting, Status, Text, Texts};

const NOTE: &[Field] = &[
    ("text", Text, true),
    ("tags", Texts, false),
    ("at", Integer, false),
    ("origin", Origin, false),
    ("author", Text, false),
];

macro_rules! tool {
    ($name:literal, $write:literal, [$($fields:expr),* $(,)?]) => {
        Tool { name: $name, write: $write, fields: &[$($fields),*] }
    };
}

pub(crate) const TOOLS: &[Tool] = &[
    tool!(
        "remember",
        true,
        [
            ("space", Text, true),
            NOTE[0],
            NOTE[1],
            NOTE[2],
            NOTE[3],
            NOTE[4]
        ]
    ),
    tool!(
        "remember_many",
        true,
        [("space", Text, true), ("notes", Notes, true)]
    ),
    tool!(
        "recall",
        false,
        [
            ("query", Text, true),
            ("space", Text, false),
            ("tags", Texts, false),
            ("since", Integer, false),
            ("pinned", Boolean, false),
            ("limit", Limit(50), false)
        ]
    ),
    tool!(
        "list",
        false,
        [
            ("space", Text, false),
            ("tags", Texts, false),
            ("pinned", Boolean, false),
            ("since", Integer, false),
            ("limit", Limit(200), false),
            ("cursor", Text, false),
            ("status", Status, false),
            ("origin_aliases", Boolean, false)
        ]
    ),
    tool!(
        "pin",
        true,
        [("id", Text, true), ("pinned", PinSetting, false)]
    ),
    tool!("read", false, [("id", Text, true)]),
    tool!("history", false, [("id", Text, true)]),
    tool!(
        "supersede",
        true,
        [
            ("id", Text, true),
            ("text", Text, true),
            ("tags", Texts, false),
            ("reason", Text, true)
        ]
    ),
    tool!("retire", true, [("id", Text, true), ("reason", Text, true)]),
    tool!("restore", true, [("id", Text, true)]),
    tool!(
        "tags_set",
        true,
        [("id", Text, true), ("tags", Texts, true)]
    ),
    tool!(
        "move",
        true,
        [("ids", Texts, true), ("to_space", Text, true)]
    ),
    tool!("spaces", false, []),
    tool!(
        "space_create",
        true,
        [
            ("name", Text, true),
            ("label", Text, true),
            ("parent", Text, false)
        ]
    ),
    tool!(
        "space_move",
        true,
        [
            ("name", Text, true),
            ("label", Text, false),
            ("parent", Parent, false)
        ]
    ),
    tool!(
        "file_put",
        true,
        [
            ("space", Text, true),
            ("name", Text, true),
            ("mime", Text, false),
            ("base64", Text, true),
            ("if_absent", Boolean, false),
            ("note", Text, false)
        ]
    ),
    tool!("file_get", false, [("id", Text, true)]),
    tool!(
        "file_list",
        false,
        [
            ("space", Text, false),
            ("limit", Limit(200), false),
            ("cursor", Text, false)
        ]
    ),
    tool!(
        "file_retire",
        true,
        [("id", Text, true), ("reason", Text, true)]
    ),
];

pub(crate) fn tools(principal: &Principal) -> Vec<Value> {
    TOOLS
        .iter()
        .filter(|tool| !tool.write || principal.may_write())
        .map(|tool| {
            let mut spec = json!({"name":tool.name,"inputSchema":object(tool.fields)});
            let description = match tool.name {
                "pin" => Some(texts::PIN_DESCRIPTION),
                "list" => Some(texts::LIST_DESCRIPTION),
                "supersede" => Some(texts::SUPERSEDE_DESCRIPTION),
                "recall" => Some(texts::RECALL_DESCRIPTION),
                "spaces" => Some(texts::SPACES_DESCRIPTION),
                _ => None,
            };
            if let Some(description) = description {
                spec["description"] = json!(description);
            }
            spec
        })
        .collect()
}

fn object(fields: &[Field]) -> Value {
    let properties: Map<_, _> = fields
        .iter()
        .map(|(name, kind, _)| (name.to_string(), property(*kind)))
        .collect();
    let required: Vec<_> = fields
        .iter()
        .filter_map(|(name, _, required)| required.then_some(name))
        .collect();
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}

fn property(kind: Kind) -> Value {
    match kind {
        Text => json!({"type":"string"}),
        Boolean => json!({"type":"boolean"}),
        PinSetting => json!({"type":"boolean","default":true}),
        Texts => json!({"type":"array","items":property(Text)}),
        Integer => json!({"type":"integer"}),
        Origin => json!({"type":"array","items":property(Text),"minItems":2,"maxItems":2}),
        Status => json!({"type":"string","enum":["active","retired","superseded"]}),
        Notes => json!({"type":"array","items":object(NOTE)}),
        Parent => json!({"type":["string","null"]}),
        Limit(maximum) => json!({"type":"integer","minimum":1,"maximum":maximum}),
    }
}

impl Tool {
    pub(crate) fn validate(&self, args: &Value) -> crate::dispatch::Result<()> {
        if valid_object(self.fields, args) {
            return Ok(());
        }
        Err(crate::dispatch::ToolError::Message(
            texts::INVALID_ARGUMENTS.into(),
        ))
    }
}

fn valid_object(fields: &[Field], value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object
        .keys()
        .all(|key| fields.iter().any(|(name, _, _)| key == name))
        && fields
            .iter()
            .all(|(name, kind, required)| match object.get(*name) {
                Some(value) => valid_value(*kind, value),
                None => !required,
            })
}

fn valid_value(kind: Kind, value: &Value) -> bool {
    match kind {
        Text => value.is_string(),
        Boolean | PinSetting => value.is_boolean(),
        Texts => value
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string)),
        Integer => value.is_i64(),
        Origin => value
            .as_array()
            .is_some_and(|items| items.len() == 2 && items.iter().all(Value::is_string)),
        Status => matches!(value.as_str(), Some("active" | "retired" | "superseded")),
        Notes => value
            .as_array()
            .is_some_and(|items| items.iter().all(|item| valid_object(NOTE, item))),
        Parent => value.is_null() || value.is_string(),
        Limit(maximum) => value
            .as_u64()
            .is_some_and(|n| (1..=u64::from(maximum)).contains(&n)),
    }
}
