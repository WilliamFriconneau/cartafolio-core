//! `rules_get` / `rules_set`: the filing rules of a space, versioned.

mod common;

use common::{Res, Temp, brain};
use memcp_core::access::{self, Rights};
use memcp_core::tools;
use memcp_core::triage;
use memcp_core::users::OWNER;
use serde_json::json;

#[test]
fn rules_are_written_as_a_new_version_and_read_back() -> Res {
    let t = Temp::new("rules-tools");
    let b = brain(&t)?;
    let space = triage::ensure(&b, OWNER, "owner")?;
    let ctl = b.open_control()?;
    let rights = Rights::parse(&format!("{}:rw", space.as_str()))?;
    let tok = access::connect_for(&ctl, OWNER, "sorter", &rights, None, false, false)?;
    let c = access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("refused")?;
    let s = space.as_str();
    let first = tools::call(
        &b,
        &c,
        "rules_set",
        &json!({ "space": s, "keywords": ["Plumber", "#boiler"], "tags": ["home"] }),
    )?;
    assert_eq!(first["rules"]["keywords"], json!(["boiler", "plumber"]));
    tools::call(
        &b,
        &c,
        "rules_set",
        &json!({ "space": s, "keywords": ["roof"] }),
    )?;
    let got = tools::call(&b, &c, "rules_get", &json!({ "space": s }))?;
    assert_eq!(got["rules"]["keywords"], json!(["roof"]));
    assert_eq!(
        got["rules"]["tags"],
        json!(["home"]),
        "a list left out stays"
    );
    Ok(())
}
