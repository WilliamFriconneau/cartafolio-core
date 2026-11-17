//! Pairing a node: a code shown once, exchanged once, for a peer of the person.

mod common;

use common::{Res, Temp, brain};
use memcp_core::access::{self, Rights};
use memcp_core::pairing::{self, CODE_TTL};
use memcp_core::tools::{self, ToolError};
use memcp_core::users;
use serde_json::json;

const NOW: i64 = 1_900_000_000;

#[test]
fn a_code_is_exchanged_once_for_a_peer_key() -> Res {
    let t = Temp::new("pairing");
    let brain = brain(&t)?;
    let ctl = brain.open_control()?;
    let (code, _) = pairing::create_for_peer(&brain, users::OWNER, "owner", NOW)?;
    assert_eq!(code.len(), 9, "ABCD-EFGH: {code}");
    assert!(
        code.chars()
            .all(|c| c == '-' || "ABCDEFGHJKMNPQRSTUVWXYZ23456789".contains(c))
    );
    // Typed in lower case, without the dash: the same code.
    let typed = code.replace('-', " ").to_lowercase();
    assert!(pairing::holds(&ctl, &typed, NOW + 1));
    let paired = pairing::redeem(&ctl, &typed, "Laptop de Sam", NOW + 5)?;
    assert!(
        paired.agent.starts_with("peer-laptop-de-sam-"),
        "{}",
        paired.agent
    );
    // Once only, and the same answer as an unknown code.
    let again = pairing::redeem(&ctl, &code, "other", NOW + 7);
    let unknown = pairing::redeem(&ctl, "AAAA-AAAA", "other", NOW + 7);
    assert_eq!(
        again.err().map(|e| e.to_string()),
        unknown.err().map(|e| e.to_string())
    );
    assert!(!pairing::holds(&ctl, &code, NOW + 8));
    Ok(())
}

#[test]
fn a_code_expires() -> Res {
    let t = Temp::new("pairing-expire");
    let brain = brain(&t)?;
    let ctl = brain.open_control()?;
    let (code, _) = pairing::create_for_peer(&brain, users::OWNER, "owner", NOW)?;
    assert!(pairing::redeem(&ctl, &code, "x", NOW + CODE_TTL).is_err());
    Ok(())
}

#[test]
fn a_peer_reads_and_writes_every_space_of_the_person() -> Res {
    let t = Temp::new("pairing-triage");
    let brain = brain(&t)?;
    let (code, triage) = pairing::create_for_peer(&brain, users::OWNER, "owner", NOW)?;
    assert_eq!(triage, "owner-a-trier");
    let ctl = brain.open_control()?;
    let p = pairing::redeem(&ctl, &code, "phone", NOW + 1)?;
    assert!(p.agent.starts_with("peer-phone-"), "{}", p.agent);
    let c = access::authenticate(&ctl, &p.token, NOW + 2)?.ok_or("key")?;
    let box_ = memcp_core::Space::new(&triage)?;
    assert!(c.can_write(&box_));
    assert!(c.readable().contains(&box_));
    // A second device gets the same box, never a second one.
    let (_, again) = pairing::create_for_peer(&brain, users::OWNER, "owner", NOW + 3)?;
    assert_eq!(again, triage);
    assert!(memcp_core::triage::is_triage(&brain, &box_));
    Ok(())
}

#[test]
fn what_waits_in_a_trier_is_sent_to_the_inbox_of_a_vault_and_leaves() -> Res {
    use memcp_core::facts::{self, NewFact, Remembered};
    let t = Temp::new("triage-send");
    let brain = brain(&t)?;
    let (_, triage) = pairing::create_for_peer(&brain, users::OWNER, "owner", NOW)?;
    let box_ = memcp_core::Space::new(&triage)?;
    let work = brain.add_vault(users::OWNER, "owner", "work")?;
    let mut n = NewFact::agent("2026-11-03, rappeler le fournisseur", "device-phone");
    n.tags = vec!["inbox".into()];
    let Remembered::Created(note) =
        facts::remember(&brain.open_space(&box_)?, &box_, "device-phone", n, &[])?
    else {
        return Err("not created".into());
    };
    let waiting = memcp_core::triage::waiting(&brain, &box_, 20)?;
    assert_eq!(waiting["notes"].as_array().map(Vec::len), Some(1));
    assert!(
        memcp_core::triage::send(&brain, &work, &note.id, &box_, "owner", "owner").is_err(),
        "only out of À trier"
    );
    let new = memcp_core::triage::send(&brain, &box_, &note.id, &work, "owner", "owner")?;
    let moved = facts::read_fact(&brain.open_space(&work)?, &new)?.ok_or("moved")?;
    assert_eq!(moved.text, note.text, "as it is");
    assert!(
        moved.tags.contains(&"inbox".to_string()),
        "into the vault's inbox"
    );
    let left = facts::read_fact(&brain.open_space(&box_)?, &note.id)?.ok_or("kept")?;
    assert_eq!(left.status, "retired");
    let waiting = memcp_core::triage::waiting(&brain, &box_, 20)?;
    assert_eq!(waiting["notes"].as_array().map(Vec::len), Some(0));
    Ok(())
}

#[test]
fn a_write_that_names_no_space_goes_to_a_trier_when_the_key_writes_there() -> Res {
    use memcp_core::facts;
    let t = Temp::new("triage-default-space");
    let brain = brain(&t)?;
    let ctl = brain.open_control()?;
    let key = |agent: &str, spec: &str| -> Result<access::Caller, Box<dyn std::error::Error>> {
        let token = access::connect(&ctl, agent, &Rights::parse(spec)?, None, false, false)?;
        Ok(access::authenticate(&ctl, &token, NOW)?.ok_or("key")?)
    };
    let note = json!({ "text": "2026-11-03, a note from a device", "tags": ["inbox"] });
    let file = json!({ "name": "memo.txt", "data_base64": "aGVsbG8=", "tags": ["inbox"] });
    // Several spaces and no « À trier »: the write is refused, as it always was.
    let all = key("assistant", "all")?;
    for (tool, args) in [("remember", &note), ("file_put", &file)] {
        assert!(
            matches!(
                tools::call(&brain, &all, tool, args),
                Err(ToolError::Invalid(_))
            ),
            "{tool}: no space named, none to fall back on"
        );
    }
    // The person's « À trier » now exists: a key on all spaces writes there, and nowhere else.
    let (_, triage) = pairing::create_for_peer(&brain, users::OWNER, "owner", NOW)?;
    let box_ = memcp_core::Space::new(&triage)?;
    let all = key("assistant-2", "all")?;
    let r = tools::call(&brain, &all, "remember", &note)?;
    assert_eq!(r["result"], "created", "{r}");
    let id = r["fact"]["id"].as_str().ok_or("id")?.to_string();
    assert!(facts::read_fact(&brain.open_space(&box_)?, &id)?.is_some());
    let r = tools::call(&brain, &all, "file_put", &file)?;
    assert_eq!(r["result"], "kept", "{r}");
    let waiting = memcp_core::triage::waiting(&brain, &box_, 20)?;
    assert_eq!(waiting["notes"].as_array().map(Vec::len), Some(1));
    assert_eq!(waiting["files"].as_array().map(Vec::len), Some(1));
    for vault in common::SPACES {
        let conn = brain.open_space(&common::sp(vault))?;
        let n: i64 = conn.query_row("SELECT count(*) FROM facts", [], |r| r.get(0))?;
        assert_eq!(n, 0, "nothing reached {vault}");
    }
    // A space that is named is still the one written to.
    let named = json!({ "text": "2026-11-03, a fact for home", "space": "home" });
    let r = tools::call(&brain, &all, "remember", &named)?;
    let id = r["fact"]["id"].as_str().ok_or("id")?.to_string();
    assert!(facts::read_fact(&brain.open_space(&common::sp("home"))?, &id)?.is_some());
    // Several vaults and no right on « À trier »: still refused.
    let two = key("two", "home:rw,work:rw")?;
    assert!(matches!(
        tools::call(&brain, &two, "remember", &note),
        Err(ToolError::Invalid(_))
    ));
    Ok(())
}
