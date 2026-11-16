//! `triage_claim`: a sorter outside the memory reserves notes of « À trier »; another one does not get the same.

mod common;

use common::{Res, Temp, brain};
use memcp_core::access::{self, Rights};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::tools;
use memcp_core::triage;
use memcp_core::users::OWNER;
use serde_json::json;

#[test]
fn a_note_reserved_by_one_sorter_is_not_given_to_another_until_it_is_released() -> Res {
    let t = Temp::new("triage-claim");
    let b = brain(&t)?;
    let space = triage::ensure(&b, OWNER, "owner")?;
    let conn = b.open_space(&space)?;
    let mut n = NewFact::agent("Call the plumber on Friday.", "agent:phone");
    n.tags = vec!["inbox".into()];
    assert!(matches!(
        facts::remember(&conn, &space, "phone", n, &[])?,
        Remembered::Created(_)
    ));
    let ctl = b.open_control()?;
    let rights = Rights::parse(&format!("{}:rw", space.as_str()))?;
    let mk = |agent: &str| -> Result<access::Caller, Box<dyn std::error::Error>> {
        let tok = access::connect_for(&ctl, OWNER, agent, &rights, None, false, false)?;
        Ok(access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("refused")?)
    };
    let (a, c) = (mk("sorter-a")?, mk("sorter-b")?);
    let got = |who: &access::Caller| tools::call(&b, who, "triage_claim", &json!({}));
    assert_eq!(got(&a)?["claimed"].as_array().map(Vec::len), Some(1));
    assert_eq!(got(&c)?["claimed"].as_array().map(Vec::len), Some(0));
    tools::call(&b, &a, "triage_release", &json!({}))?;
    assert_eq!(got(&c)?["claimed"].as_array().map(Vec::len), Some(1));
    Ok(())
}
