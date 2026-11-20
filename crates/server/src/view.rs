//! The money extension of the space view contract (`memcp.view/1`).
//!
//! The core reads facts; bank statements live in `memcp_finances` and are reached through this node.
//! When a caller reads the dashboard, targets or register of a money space, the core's answer is
//! extended here with the finance capabilities and, for `type=movement`, with the ledger's register.
//! Nothing is renamed and no core tool changes.

use memcp_core::tools::ToolError;
use memcp_finances::Ledger;
use serde_json::{Value, json};

/// Adds the finance indicators to a money space's `stats` answer: the balances read in the last
/// bank export, with its date. The statement itself stays in `finances_balances`.
pub(crate) fn augment_stats(
    ledger: &Ledger<'_>,
    mut base: Value,
    now: i64,
) -> Result<Value, ToolError> {
    let balances = ledger.balances()?;
    base["stats"]["finances"] = json!({
        "accounts": balances["accounts"],
        "last_import": balances["last_import"],
        "months": ledger.monthly_totals(now, 6)?,
        "source": "bank export",
    });
    Ok(base)
}

/// Adds the finance capabilities to a money space's `capabilities` answer.
pub(crate) fn augment(ledger: &Ledger<'_>, mut base: Value) -> Value {
    let space = ledger.space.as_str();
    if let Some(types) = base["item_types"].as_object_mut() {
        types.insert(
            "movement".into(),
            json!({
                "id": "mv_… (derived from the account, date, amount, label and occurrence; stable across reads)",
                "fields": ["date", "amount", "account", "title"],
                "description": "A line of a bank statement, from the money projection.",
            }),
        );
    }
    base["extensions"] = json!([{
        "id": "finances",
        "contract": "memcp.view/finances/1",
        "version": 1,
        "description": "The bank statements of the money space: balances, movements and a monthly statement.",
        "views": [{
            "id": "register",
            "title": "Bank movements",
            "type": "movement",
            "tool": "space_view",
            "arguments": { "space": space, "view": "register", "type": "movement" },
            "returns": ["movement"],
            "filters": ["from", "to", "account", "filter"],
            "pagination": {
                "kind": "cursor",
                "limit": { "min": 1, "max": 200, "default": 50 },
                "cursor": "opaque: take `page.next_cursor` from the answer and pass it back as `cursor`",
            },
            "provenance": "the bank export the movement was imported from; the account number is masked",
        }],
        "tools": [
            { "name": "finances_balances", "description": "Balance of each account as read in the last bank export." },
            { "name": "finances_movements", "description": "Account movements from the bank exports, newest first." },
            { "name": "finances_monthly", "description": "Statement of a month, per account." },
            { "name": "finances_overdraft_exit", "description": "When each overdrawn account comes back above zero (an estimate)." },
            { "name": "finances_rules", "description": "The person's own rules for the monthly statement." },
            { "name": "finances_import", "description": "Imports a bank export already stored in the space (write access)." },
        ],
    }]);
    if let Some(tools) = base["tools"].as_array_mut() {
        for name in [
            "finances_balances",
            "finances_movements",
            "finances_monthly",
            "finances_overdraft_exit",
            "finances_rules",
            "finances_import",
        ] {
            if !tools.iter().any(|t| t.as_str() == Some(name)) {
                tools.push(json!(name));
            }
        }
    }
    base["contract_extension"] = json!("memcp.view/finances/1");
    base
}

/// A chronology cursor: `sort_at|source|id`, where `source` is `fact` or `movement`. A cursor of
/// the legacy register (`at:id`) is left to the core.
fn parse_cursor(cursor: &str) -> Option<(String, String, String)> {
    let mut parts = cursor.splitn(3, '|');
    let at = parts.next()?.to_string();
    let source = parts.next()?.to_string();
    let id = parts.next()?.to_string();
    (!at.is_empty() && (source == "fact" || source == "movement") && !id.is_empty())
        .then_some((at, source, id))
}

fn sort_of(item: &Value, source: &str) -> String {
    let at = if source == "movement" {
        item["date"]
            .as_str()
            .map(|d| format!("{d}T00:00:00+00:00"))
            .unwrap_or_default()
    } else {
        item["at"].as_str().unwrap_or("").to_string()
    };
    let id = item["id"].as_str().unwrap_or("");
    format!("{at}|{source}|{id}")
}

/// The money chronology: the core's facts and the ledger's movements in one page, newest first.
pub(crate) fn merge_chronology(
    ledger: &Ledger<'_>,
    base: Value,
    args: &Value,
) -> Result<Value, ToolError> {
    let limit = args
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(50)
        .clamp(1, 200) as usize;
    let cursor = args
        .get("cursor")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .and_then(parse_cursor);
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("");
    let (from, to, account, filter) = (s("from"), s("to"), s("account"), s("filter"));
    // A page may need movements older than a fact cursor: take them up to that day.
    let move_to = match &cursor {
        Some((at, source, _)) if source == "fact" => at.get(..10).unwrap_or(to),
        _ => to,
    };
    let move_cursor = match &cursor {
        Some((_, source, id)) if source == "movement" => id.as_str(),
        _ => "",
    };
    let after = cursor
        .as_ref()
        .map(|(at, source, id)| format!("{at}|{source}|{id}"));
    // The movements, page after page until one is past the cursor (or they end): a day with more than a page of movements
    // newer than the cursor would otherwise fill the whole page with lines the cursor then removes, and the chronology would
    // stop with movements left.
    let mut movement_items: Vec<Value> = Vec::new();
    let mut movements = ledger.movements_page(account, from, move_to, filter, 200, move_cursor)?;
    for _ in 0..100 {
        let page_items = movements["items"].as_array().cloned().unwrap_or_default();
        let past = match (&after, page_items.last()) {
            (Some(after), Some(last)) => sort_of(last, "movement").as_str() < after.as_str(),
            _ => true,
        };
        movement_items.extend(page_items);
        let next = movements["page"]["next_cursor"].as_str().map(String::from);
        if past || next.is_none() {
            break;
        }
        movements = ledger.movements_page(
            account,
            from,
            move_to,
            filter,
            200,
            next.as_deref().unwrap_or(""),
        )?;
    }
    let facts = base["items"].as_array().cloned().unwrap_or_default();
    let mut items: Vec<(String, Value)> = Vec::new();
    for item in facts {
        let key = sort_of(&item, "fact");
        items.push((key, item));
    }
    for item in movement_items {
        let key = sort_of(&item, "movement");
        items.push((key, item));
    }
    if let Some((at, source, id)) = &cursor {
        let after = format!("{at}|{source}|{id}");
        items.retain(|(key, _)| key.as_str() < after.as_str());
    }
    items.sort_by(|a, b| b.0.cmp(&a.0));
    // A source that was cut at its own page size may hold more than what was merged here: the
    // page then says so, and the next cursor resumes after the last item shown.
    let source_has_more = base["page"]["has_more"].as_bool().unwrap_or(false)
        || movements["page"]["has_more"].as_bool().unwrap_or(false);
    let more = items.len() > limit || (source_has_more && !items.is_empty());
    items.truncate(limit);
    let next = more.then(|| items.last().map(|(key, _)| key.clone()).unwrap_or_default());
    let items: Vec<Value> = items.into_iter().map(|(_, item)| item).collect();
    Ok(json!({
        "contract": base["contract"],
        "contract_version": base["contract_version"],
        "space": base["space"],
        "kind": base["kind"],
        "label": base["label"],
        "view": "chronology",
        "generated_at": base["generated_at"],
        "filters": { "from": from, "to": to, "type": "all", "tag": s("tag"), "q": s("q") },
        "items": items,
        "provenance": { "kind": "mixed", "space": base["space"], "source": "facts and bank movements" },
        "page": { "limit": limit, "has_more": more, "next_cursor": next },
        "notice": base["notice"],
    }))
}
