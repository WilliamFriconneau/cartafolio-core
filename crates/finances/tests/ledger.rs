//! Bank statements in a memory space, on a fictitious export (tests/fixtures/statements.xlsx).

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use pensine_core::access::{self, Caller, Rights};
use pensine_core::text::Language;
use pensine_core::tools::ToolError;
use pensine_core::{Brain, Space, files};
use pensine_finances::Ledger;
use serde_json::{Value, json};

type Res = Result<(), Box<dyn Error>>;
static N: AtomicU32 = AtomicU32::new(0);
const WORKBOOK: &[u8] = include_bytes!("fixtures/statements.xlsx");

/// 2026-10-31T12:00:00Z
const NOW: i64 = 1_793_448_000;

fn sp(n: &str) -> Space {
    match Space::new(n) {
        Ok(s) => s,
        Err(e) => panic!("{n}: {e}"),
    }
}

struct Env {
    dir: PathBuf,
    brain: Brain,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn env() -> Result<Env, Box<dyn Error>> {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("pensine-finances-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let brain = Brain::init(&dir, &[sp("finances"), sp("home")], Language::English)?;
    Ok(Env { dir, brain })
}

fn ledger(e: &Env) -> Ledger<'_> {
    Ledger {
        brain: &e.brain,
        space: sp("finances"),
    }
}

fn caller(e: &Env, spec: &str) -> Result<Caller, Box<dyn Error>> {
    let ctl = e.brain.open_control()?;
    let n = N.fetch_add(1, Ordering::SeqCst);
    let t = access::connect(
        &ctl,
        &format!("agent-{n}"),
        &Rights::parse(spec)?,
        None,
        false,
        false,
    )?;
    access::authenticate(&ctl, &t, NOW)?.ok_or_else(|| "refused".into())
}

/// Stores the workbook and imports it; returns the import report.
fn imported(e: &Env) -> Result<Value, Box<dyn Error>> {
    let f = files::put(
        &e.brain,
        &sp("finances"),
        "test",
        "accounts.xlsx",
        "export",
        WORKBOOK,
    )?;
    Ok(ledger(e).import(&f.id, NOW)?)
}

fn account<'a>(report: &'a Value, name: &str) -> Result<&'a Value, Box<dyn Error>> {
    report["accounts"]
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|x| x["account"].as_str().is_some_and(|s| s.contains(name)))
        })
        .ok_or_else(|| format!("no account {name}").into())
}

fn set_rules(e: &Env) -> Res {
    let writer = caller(e, "finances:rw")?;
    ledger(e).call(
        &writer,
        "finances_rules",
        &json!({ "rules": {
            "caps": [{ "name": "PayPal", "match": ["paypal"], "limit": 200 }],
            "watch": [{ "name": "Amazon", "match": ["amazon", "amzn"], "except": ["audible"] }],
            "internal": ["livret", "compte"],
        } }),
        NOW,
    )?;
    Ok(())
}

#[test]
fn an_export_is_read_per_account_with_names_balances_and_masked_numbers() -> Res {
    let e = env()?;
    let report = imported(&e)?;
    assert_eq!(report["new_movements"], 11);
    let everyday = account(&report, "Everyday account")?;
    assert_eq!(everyday["account"], "Everyday account …3344");
    assert_eq!(everyday["balance"], -300.0);
    assert_eq!(everyday["as_of"], "2026-09-30");
    assert_eq!(everyday["new_movements"], 10);
    let savings = account(&report, "Savings")?;
    assert_eq!(
        savings["new_movements"], 1,
        "a text date (dd/mm/yyyy) is read too"
    );
    assert!(
        !report.to_string().contains("11122233344"),
        "the account number never comes out in clear"
    );
    Ok(())
}

#[test]
fn importing_the_same_export_again_adds_nothing() -> Res {
    let e = env()?;
    imported(&e)?;
    let again = imported(&e)?;
    assert_eq!(again["new_movements"], 0);
    let rows = ledger(&e).movements("everyday", "", "", "cafe", 50)?;
    assert_eq!(
        rows["movements"].as_array().map(Vec::len),
        Some(2),
        "two identical lines on one day are two movements"
    );
    Ok(())
}

#[test]
fn balances_and_movements_can_be_filtered() -> Res {
    let e = env()?;
    imported(&e)?;
    let l = ledger(&e);
    let b = l.balances()?;
    assert_eq!(b["accounts"].as_array().map(Vec::len), Some(2));
    assert_eq!(b["last_import"]["file"], "accounts.xlsx");
    let paypal = l.movements("", "", "", "paypal", 50)?;
    assert_eq!(paypal["movements"].as_array().map(Vec::len), Some(2));
    let by_digits = l.movements("7788", "2026-09-01", "2026-09-30", "", 50)?;
    let rows = by_digits["movements"].as_array().ok_or("rows")?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["label"], "VIR DE COMPTE COURANT");
    assert_eq!(rows[0]["amount"], 300.0);
    assert_eq!(
        l.movements("nothing", "", "", "", 50)?["movements"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
    Ok(())
}

#[test]
fn the_monthly_statement_applies_the_persons_own_rules() -> Res {
    let e = env()?;
    imported(&e)?;
    set_rules(&e)?;
    // NOW is in October: the last complete month is September.
    let s = ledger(&e).monthly("", NOW)?;
    assert_eq!(s["month"], "2026-09");
    let everyday = s["accounts"]
        .as_array()
        .and_then(|a| {
            a.iter().find(|x| {
                x["account"]
                    .as_str()
                    .is_some_and(|n| n.contains("Everyday"))
            })
        })
        .ok_or("everyday")?;
    assert_eq!(everyday["income"], 2000.0);
    assert_eq!(everyday["outflow"], -2681.45);
    assert_eq!(everyday["net"], -681.45);
    assert_eq!(everyday["caps"][0]["spent"], 220.5);
    assert_eq!(everyday["caps"][0]["exceeded"], true);
    let amazon = everyday["watch"][0]["movements"]
        .as_array()
        .ok_or("watch")?;
    assert_eq!(amazon.len(), 1, "Audible is an exception");
    assert_eq!(amazon[0]["amount"], -35.0);
    assert_eq!(everyday["fees"]["count"], 1);
    assert_eq!(everyday["fees"]["total"], 8.0);
    assert_eq!(everyday["internal_transfers"]["count"], 1);
    assert_eq!(everyday["internal_transfers"]["net"], -300.0);
    assert_eq!(
        everyday["overdrawn_days"], 6,
        "25 to 30 September, rebuilt backwards from the balance of the export"
    );
    assert_eq!(everyday["largest_outflows"][0]["amount"], -2100.0);
    assert_eq!(
        everyday["largest_outflows"][0]["label"], "LOYER SEPTEMBRE",
        "spaces in a label are collapsed"
    );
    Ok(())
}

#[test]
fn without_rules_nothing_personal_is_assumed() -> Res {
    let e = env()?;
    imported(&e)?;
    let s = ledger(&e).monthly("2026-09", NOW)?;
    let first = &s["accounts"][0];
    assert_eq!(first["caps"], json!([]));
    assert_eq!(first["watch"], json!([]));
    assert_eq!(
        ledger(&e).monthly("2025-01", NOW)?["accounts"],
        json!([]),
        "a month without movement"
    );
    assert!(matches!(
        ledger(&e).monthly("sept", NOW),
        Err(ToolError::Invalid(_))
    ));
    Ok(())
}

#[test]
fn rights_decide_who_reads_who_imports_and_who_sets_the_rules() -> Res {
    let e = env()?;
    let (reader, writer, other) = (
        caller(&e, "finances:r")?,
        caller(&e, "finances:rw")?,
        caller(&e, "home:rw")?,
    );
    let l = ledger(&e);
    let names =
        |c: &Caller| -> Vec<&'static str> { l.definitions(c).iter().map(|d| d.name).collect() };
    assert!(names(&other).is_empty(), "no right on the space, no tool");
    assert!(names(&reader).contains(&"finances_monthly"));
    assert!(!names(&reader).contains(&"finances_import"));
    assert!(names(&writer).contains(&"finances_import"));
    assert!(matches!(
        l.call(&reader, "finances_import", &json!({ "file_id": "x" }), NOW),
        Err(ToolError::Unknown)
    ));
    assert!(matches!(
        l.call(
            &reader,
            "finances_rules",
            &json!({ "rules": { "fees": ["X"] } }),
            NOW
        ),
        Err(ToolError::Denied(_))
    ));
    assert!(matches!(
        l.call(
            &writer,
            "finances_rules",
            &json!({ "rules": { "caps": [{ "name": "x", "match": ["x"] }] } }),
            NOW
        ),
        Err(ToolError::Invalid(_))
    ));
    assert!(matches!(
        l.call(
            &writer,
            "finances_rules",
            &json!({ "rules": { "mystery": [] } }),
            NOW
        ),
        Err(ToolError::Invalid(_))
    ));
    Ok(())
}

#[test]
fn only_a_workbook_is_imported() -> Res {
    let e = env()?;
    let text = files::put(
        &e.brain,
        &sp("finances"),
        "test",
        "notes.txt",
        "",
        b"not a workbook",
    )?;
    assert!(matches!(
        ledger(&e).import(&text.id, NOW),
        Err(ToolError::Invalid(_))
    ));
    assert!(matches!(
        ledger(&e).import("missing", NOW),
        Err(ToolError::Invalid(_))
    ));
    Ok(())
}
