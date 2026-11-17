//! The folder of a space on this node: `folder_get` and `folder_set` (see docs/FOLDERS.md).

use super::*;
use crate::folders;

pub(super) fn folder(
    brain: &Brain,
    caller: &Caller,
    name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    if name == "folder_get" {
        let spaces = whole_spaces(caller, read_spaces(caller, args)?);
        if args.get("space").is_some() {
            let one = spaces
                .first()
                .ok_or_else(|| ToolError::Denied("no read right on that space".into()))?;
            return Ok(folders::info(brain, one)?);
        }
        let mut out = vec![];
        for s in spaces {
            if folders::get(brain, &s)?.is_some() {
                out.push(folders::info(brain, &s)?);
            }
        }
        return Ok(json!({ "folders": out }));
    }
    let space = write_space(brain, caller, args)?;
    if caller.paths.contains_key(&space) {
        return Err(ToolError::Denied(
            "no right to set a folder on a space reached by paths".into(),
        ));
    }
    let path = match args.get("path") {
        Some(Value::Null) => None,
        Some(Value::String(p)) if !p.trim().is_empty() => Some(std::path::PathBuf::from(p.trim())),
        _ => {
            return Err(ToolError::Invalid(
                "\"path\" (text, or null for the default folder) is required".into(),
            ));
        }
    };
    let mode = match args.get("mode").and_then(Value::as_str) {
        Some(m) => folders::Mode::parse(m)?,
        None => folders::Mode::ReadWrite,
    };
    Ok(folders::set(
        brain,
        &space,
        path.as_deref(),
        mode,
        util::now(),
    )?)
}
