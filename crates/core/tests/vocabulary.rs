//! Vocabularies: a list of terms imported into a space, replaced by the same name, removed, read by a tool.

mod common;

use common::{Res, Temp, brain};
use memcp_core::access::{self, Rights};
use memcp_core::tools;
use memcp_core::triage;
use memcp_core::users::OWNER;
use memcp_core::vocabulary;
use serde_json::json;

#[test]
fn a_vocabulary_is_imported_replaced_listed_and_removed() -> Res {
    let t = Temp::new("vocabulary");
    let b = brain(&t)?;
    let space = triage::ensure(&b, OWNER, "owner")?;
    let mut conn = b.open_space(&space)?;
    let file = "# a comment\nCéphalée\t sign\tR51\n  dyspnée \n\n#fièvre\nfièvre\nCéphalée\n";
    let terms = vocabulary::parse(file);
    assert_eq!(terms.len(), 4, "comments and blanks skipped: {terms:?}");
    assert_eq!(
        vocabulary::import(&mut conn, "signs", "a list", &terms, 100)?,
        3,
        "a repeated term counts once"
    );
    assert_eq!(
        vocabulary::terms(&conn, 10)?,
        ["céphalée", "dyspnée", "fièvre"]
    );
    // The same name again replaces.
    let two = vocabulary::parse("toux\n");
    vocabulary::import(&mut conn, "signs", "other", &two, 200)?;
    assert_eq!(vocabulary::terms(&conn, 10)?, ["toux"]);
    let list = vocabulary::list(&conn)?;
    assert_eq!(
        (
            list[0].name.as_str(),
            list[0].source.as_str(),
            list[0].terms
        ),
        ("signs", "other", 1)
    );
    assert!(vocabulary::import(&mut conn, "Bad Name", "", &two, 1).is_err());
    assert!(vocabulary::remove(&mut conn, "signs")?);
    assert!(!vocabulary::remove(&mut conn, "signs")?);
    assert!(vocabulary::terms(&conn, 10)?.is_empty());
    Ok(())
}

#[test]
fn the_tool_gives_the_terms_to_a_reader_of_the_space_only() -> Res {
    let t = Temp::new("vocabulary-tool");
    let b = brain(&t)?;
    let space = triage::ensure(&b, OWNER, "owner")?;
    let mut conn = b.open_space(&space)?;
    vocabulary::import(
        &mut conn,
        "signs",
        "a list",
        &vocabulary::parse("toux\nfièvre\n"),
        1,
    )?;
    let ctl = b.open_control()?;
    let rights = Rights::parse(&format!("{}:r", space.as_str()))?;
    let tok = access::connect_for(&ctl, OWNER, "reader", &rights, None, false, false)?;
    let c = access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("refused")?;
    let got = tools::call(
        &b,
        &c,
        "vocabulary_terms",
        &json!({ "space": space.as_str() }),
    )?;
    assert_eq!(got["terms"], json!(["fièvre", "toux"]));
    assert_eq!(got["vocabularies"][0]["name"], "signs");
    Ok(())
}
