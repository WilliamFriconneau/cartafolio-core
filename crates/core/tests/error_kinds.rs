//! Every error path answers with one kind of the vocabulary of `docs/CORE-API.md`.

mod common;

use common::{Res, Temp, brain};
use pensine_core::access::{self, Rights};
use pensine_core::tools::{self, ToolError};
use pensine_core::users::OWNER;
use pensine_core::{Brain, Error, ErrorKind, Space};
use serde_json::{Value, json};

/// The rows of the table "Error vocabulary": (code, http status).
fn documented() -> Vec<(String, u16)> {
    let doc = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/CORE-API.md"
    ))
    .unwrap_or_default();
    let part = doc.split("## Error vocabulary").nth(1).unwrap_or("");
    part.lines()
        .filter_map(|l| {
            let mut cells = l.split('|').map(str::trim);
            cells.next()?;
            let code = cells
                .next()?
                .strip_prefix('`')?
                .split('`')
                .next()?
                .to_string();
            let status = cells.next()?.parse().ok()?;
            Some((code, status))
        })
        .collect()
}

#[test]
fn the_documented_table_is_the_vocabulary() {
    let docs = documented();
    for k in ErrorKind::ALL {
        assert!(
            docs.contains(&(k.code().to_string(), k.http_status())),
            "{} is missing or has another status in docs/CORE-API.md",
            k.code()
        );
    }
    assert_eq!(docs.len(), ErrorKind::ALL.len());
}

#[test]
fn every_core_error_has_a_kind() {
    let errors = [
        Error::Io(std::io::Error::other("x")),
        Error::Sqlite(rusqlite::Error::InvalidQuery),
        Error::AlreadyInitialized("d".into()),
        Error::NotInitialized("d".into()),
        Error::DirectoryTooOpen {
            path: "d".into(),
            mode: 0o755,
        },
        Error::Schema("x".into()),
        Error::Invalid("x".into()),
        Error::UnknownFact("x".into()),
        Error::UnknownSpace("x".into()),
    ];
    for e in &errors {
        assert!(ErrorKind::ALL.contains(&e.kind()), "{e}");
    }
}

fn caller(
    b: &Brain,
    agent: &str,
    spec: &str,
) -> Result<access::Caller, Box<dyn std::error::Error>> {
    let ctl = b.open_control()?;
    let tok = access::connect_for(
        &ctl,
        OWNER,
        agent,
        &Rights::parse(spec)?,
        None,
        false,
        false,
    )?;
    Ok(access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("refused")?)
}

#[test]
fn every_tool_error_path_returns_a_kind_of_the_table() -> Res {
    let t = Temp::new("error-kinds");
    let b = brain(&t)?;
    let owner = caller(&b, "owner-key", "all")?;
    let reader = caller(&b, "reader-key", "health:r")?;
    let writer = caller(&b, "w-key", "health:rw")?;
    let docs = documented();
    let cases: Vec<(&str, &access::Caller, &str, Value, ErrorKind)> = vec![
        (
            "hidden tool",
            &reader,
            "remember",
            json!({ "text": "x" }),
            ErrorKind::UnknownTool,
        ),
        (
            "missing tool",
            &owner,
            "no_such_tool",
            json!({}),
            ErrorKind::UnknownTool,
        ),
        (
            "missing text",
            &owner,
            "remember",
            json!({ "space": "home" }),
            ErrorKind::Invalid,
        ),
        (
            "bad tag",
            &owner,
            "remember",
            json!({ "space": "home", "text": "x y", "tags": ["a b/"] }),
            ErrorKind::Invalid,
        ),
        (
            "bad date",
            &owner,
            "recall",
            json!({ "query": "x", "as_of": "soon" }),
            ErrorKind::Invalid,
        ),
        (
            "unknown fact",
            &owner,
            "read_fact",
            json!({ "id": "nope" }),
            ErrorKind::Invalid,
        ),
        (
            "unreadable space",
            &reader,
            "recall",
            json!({ "query": "x", "space": "work" }),
            ErrorKind::Denied,
        ),
        (
            "unwritable space",
            &writer,
            "remember",
            json!({ "text": "x y", "space": "work" }),
            ErrorKind::Denied,
        ),
        (
            "retire an unknown fact",
            &owner,
            "retire",
            json!({ "id": "nope", "reason": "r" }),
            ErrorKind::Invalid,
        ),
    ];
    for (name, c, tool, args, want) in cases {
        let Err(e) = tools::call(&b, c, tool, &args) else {
            panic!("{name}: expected an error");
        };
        assert_eq!(e.kind(), want, "{name}: {e}");
        assert!(
            docs.iter().any(|(code, _)| code == e.kind().code()),
            "{name}"
        );
    }
    Ok(())
}

#[test]
fn a_length_limit_is_the_limited_kind() -> Res {
    let t = Temp::new("error-limited");
    let b = brain(&t)?;
    let ctl = b.open_control()?;
    let tok = access::connect_for(
        &ctl,
        OWNER,
        "short-key",
        &Rights::parse("home:rw")?,
        None,
        false,
        false,
    )?;
    access::set_limit(&ctl, "short-key", "max-text", 5)?;
    let c = access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("refused")?;
    let e = tools::call(
        &b,
        &c,
        "remember",
        &json!({ "text": "far too long for five" }),
    );
    assert!(matches!(e, Err(ToolError::Limited(_))), "{e:?}");
    assert_eq!(e.err().map(|e| e.kind()), Some(ErrorKind::Limited));
    Ok(())
}

#[test]
fn core_errors_carry_their_kind() -> Res {
    assert_eq!(
        Space::new("Bad Name").err().map(|e| e.kind()),
        Some(ErrorKind::Invalid)
    );
    let t = Temp::new("error-core");
    let b = brain(&t)?;
    let conn = b.open_space(&common::sp("home"))?;
    let e = pensine_core::facts::retire(&conn, "me", "nope", "why").err();
    assert_eq!(e.map(|e| e.kind()), Some(ErrorKind::NotFound));
    let again = Brain::init(
        &t.path().join("data"),
        &[],
        pensine_core::text::Language::English,
    );
    assert_eq!(again.err().map(|e| e.kind()), Some(ErrorKind::Conflict));
    let missing = Brain::open(&t.path().join("none"));
    assert_eq!(
        missing.err().map(|e| e.kind()),
        Some(ErrorKind::Unavailable)
    );
    Ok(())
}
