//! `docs/TOOLS.md` and `tools::definitions` must say the same thing: the list of tools and the
//! names of their fields. Run with `MEMCP_DUMP_TOOLS=<file>` to write the schemas as JSON (what the
//! page is generated from).

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::{Res, Temp, brain};
use memcp_core::access::{self, Rights};
use memcp_core::tools;
use memcp_core::users::OWNER;
use serde_json::{Value, json};

/// Every tool of the core, as seen by the union of an owner, a read-only key and a key that may
/// propose the guide of its app, each used from the node's own machine (where every tool is offered).
fn all_tools() -> Result<BTreeMap<String, Value>, Box<dyn std::error::Error>> {
    let t = Temp::new("tools-doc");
    let b = brain(&t)?;
    let ctl = b.open_control()?;
    let mut out = BTreeMap::new();
    for (agent, spec) in [
        ("doc-owner", "all"),
        ("doc-reader", "health:r"),
        ("doc-app", "health/guide:rw,health/sources/app:rw"),
    ] {
        let tok = access::connect_for(
            &ctl,
            OWNER,
            agent,
            &Rights::parse(spec)?,
            None,
            false,
            false,
        )?;
        let mut caller = access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("refused")?;
        caller.local = true;
        for d in tools::definitions(&caller) {
            out.entry(d.name.to_string())
                .or_insert_with(|| json!({ "description": d.description, "schema": d.schema }));
        }
    }
    Ok(out)
}

#[test]
fn tools_md_lists_every_tool_and_every_field() -> Res {
    let tools = all_tools()?;
    if let Ok(path) = std::env::var("MEMCP_DUMP_TOOLS") {
        std::fs::write(path, serde_json::to_string_pretty(&tools)?)?;
    }
    let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/TOOLS.md"))?;
    // Sections: "### `name`", fields: rows "| `field` |" until the next heading.
    let mut documented: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in doc.lines() {
        if let Some(name) = line.strip_prefix("### `").and_then(|r| r.split('`').next()) {
            current = Some(name.to_string());
            documented.entry(name.to_string()).or_default();
        } else if line.starts_with('#') {
            current = None;
        } else if let (Some(cur), Some(row)) = (&current, line.strip_prefix("| `"))
            && let Some(field) = row.split('`').next()
        {
            documented
                .entry(cur.clone())
                .or_default()
                .insert(field.to_string());
        }
    }
    for (name, def) in &tools {
        let fields: BTreeSet<String> = def["schema"]["properties"]
            .as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        let Some(doc_fields) = documented.get(name) else {
            panic!("docs/TOOLS.md has no section for {name}");
        };
        // Only the top-level fields are compared: nested ones are written as `origin.app`.
        let top: BTreeSet<String> = doc_fields
            .iter()
            .filter(|f| !f.contains('.'))
            .cloned()
            .collect();
        assert_eq!(
            &fields, &top,
            "fields of {name} differ between the code and docs/TOOLS.md"
        );
    }
    for name in documented.keys() {
        // The bank statement tools live in the finances crate.
        if name.starts_with("finances_") {
            continue;
        }
        assert!(
            tools.contains_key(name),
            "docs/TOOLS.md documents {name}, which is no tool"
        );
    }
    Ok(())
}
