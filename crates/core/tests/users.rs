//! People and the spaces they own: each person reaches their own spaces and nothing else, and no
//! token goes beyond its person.

mod common;

use std::error::Error;

use common::{Res, Temp, sp};
use memcp_core::Brain;
use memcp_core::access::{self, Rights};
use memcp_core::text::Language;
use memcp_core::tools::{self, ToolError};
use memcp_core::users::{self, OWNER};
use rusqlite::Connection;
use serde_json::json;

struct Home {
    _t: Temp,
    brain: Brain,
    ctl: Connection,
    sam: String,
    alex: String,
}

/// Sam (admin) owns `sam`, `home` and `finances`; Alex (member) owns `alex`. Nothing is shared.
fn home() -> Result<Home, Box<dyn Error>> {
    let t = Temp::new("users");
    let brain = Brain::init(&t.path().join("data"), &[], Language::English)?;
    let ctl = brain.open_control()?;
    let sam = users::create_user(&ctl, OWNER, "sam", "Sam", true)?.id;
    let alex = users::create_user(&ctl, OWNER, "alex", "Alex", false)?.id;
    brain.add_space_for(&sp("sam"), &sam)?;
    brain.add_space_for(&sp("home"), &sam)?;
    brain.add_space_for(&sp("finances"), &sam)?;
    brain.add_space_for(&sp("alex"), &alex)?;
    Ok(Home {
        _t: t,
        brain,
        ctl,
        sam,
        alex,
    })
}

fn token(h: &Home, user: &str, agent: &str, spec: &str) -> memcp_core::Result<String> {
    let r = Rights::parse(spec)?;
    access::connect_for(&h.ctl, user, agent, &r, None, false, false)
}

fn caller(h: &Home, t: &str) -> Result<access::Caller, Box<dyn Error>> {
    access::authenticate(&h.ctl, t, 1_900_000_000)?.ok_or_else(|| "refused".into())
}

fn spaces_of(h: &Home, user: &str) -> Result<Vec<String>, Box<dyn Error>> {
    Ok(users::access_of(&h.ctl, user)?
        .into_keys()
        .map(|s| s.to_string())
        .collect())
}

#[test]
fn all_means_everything_the_person_reaches_never_more() -> Res {
    let h = home()?;
    let c = caller(&h, &token(&h, &h.alex, "alex-claude", "all")?)?;
    assert!(c.can_write(&sp("alex")));
    assert!(
        !c.can_read(&sp("sam")) && !c.can_read(&sp("home")),
        "Sam's spaces stay Sam's"
    );
    let r = tools::call(
        &h.brain,
        &c,
        "recall",
        &json!({ "query": "x", "space": "sam" }),
    );
    assert!(matches!(r, Err(ToolError::Denied(_))));
    Ok(())
}

#[test]
fn a_disabled_person_is_refused_everywhere_and_nothing_is_deleted() -> Res {
    let h = home()?;
    let t = token(&h, &h.alex, "alex-agent", "all")?;
    assert!(users::set_disabled(&h.ctl, &h.sam, &h.alex, true)?);
    assert!(access::authenticate(&h.ctl, &t, 1_900_000_000)?.is_none());
    assert!(users::access_of(&h.ctl, &h.alex)?.is_empty());
    assert!(token(&h, &h.alex, "again", "alex:rw").is_err());
    assert!(users::set_disabled(&h.ctl, &h.sam, &h.alex, false)?);
    assert!(
        access::authenticate(&h.ctl, &t, 1_900_000_000)?.is_some(),
        "back as before"
    );
    assert!(h.brain.has_space(&sp("alex"))?);
    Ok(())
}

#[test]
fn the_last_administrator_is_protected() -> Res {
    let h = home()?;
    assert!(
        users::set_disabled(&h.ctl, &h.sam, OWNER, true)?,
        "another admin remains"
    );
    assert!(users::set_admin(&h.ctl, &h.sam, &h.sam, false).is_err());
    assert!(users::set_disabled(&h.ctl, &h.sam, &h.sam, true).is_err());
    Ok(())
}

#[test]
fn an_administrator_who_cannot_sign_in_is_no_way_back() -> Res {
    let h = home()?;
    // Two administrators: Ana, linked to a login, and Bo, not linked (nobody can open the page as Bo).
    let ana = users::create_user(&h.ctl, &h.sam, "ana-adm", "Ana", true)?;
    let bo = users::create_user(&h.ctl, &h.sam, "bo-adm", "Bo", true)?;
    users::set_ha_user(&h.ctl, &ana.id, Some("ha-ana"))?;
    // Everyone else who could sign in as an administrator is out of the way.
    for u in users::list_users(&h.ctl)? {
        if u.admin && u.id != ana.id && u.id != bo.id {
            users::set_admin(&h.ctl, &h.sam, &u.id, false).ok();
        }
    }
    // Ana is the only administrator who can sign in: she cannot become a member, be disabled or be unlinked,
    // although Bo is an administrator too.
    assert!(users::set_admin(&h.ctl, &ana.id, &ana.id, false).is_err());
    assert!(users::set_disabled(&h.ctl, &ana.id, &ana.id, true).is_err());
    assert!(users::set_ha_user(&h.ctl, &ana.id, None).is_err());
    // Once Bo can sign in, Ana may step down.
    users::set_ha_user(&h.ctl, &bo.id, Some("ha-bo"))?;
    assert!(users::set_admin(&h.ctl, &ana.id, &ana.id, false)?);
    Ok(())
}

#[test]
fn an_agent_name_belongs_to_one_person() -> Res {
    let h = home()?;
    token(&h, &h.sam, "phone", "sam:rw")?;
    assert!(token(&h, &h.alex, "phone", "alex:rw").is_err());
    let r = Rights::parse("alex:rw")?;
    assert!(
        access::connect_for(&h.ctl, &h.alex, "phone", &r, None, false, true).is_err(),
        "not even with rotate"
    );
    assert_eq!(
        access::owner_of(&h.ctl, "phone")?.as_deref(),
        Some(h.sam.as_str())
    );
    Ok(())
}

#[test]
fn a_home_assistant_login_becomes_a_person_once() -> Res {
    let h = home()?;
    let admins = vec!["Jordan".to_string()];
    let a = users::from_home_assistant(&h.ctl, "ha-123", "Jordan", "Jordan B.", &admins)?
        .ok_or("none")?;
    assert!(a.admin && a.name == "jordan" && a.display == "Jordan B.");
    let again = users::from_home_assistant(&h.ctl, "ha-123", "Jordan", "Jordan B.", &admins)?
        .ok_or("none")?;
    assert_eq!(again.id, a.id, "same person");
    let b =
        users::from_home_assistant(&h.ctl, "ha-456", "Élise Martin", "", &admins)?.ok_or("none")?;
    assert!(!b.admin);
    assert_eq!(b.name, "elise-martin");
    users::set_disabled(&h.ctl, &h.sam, &b.id, true)?;
    assert!(
        users::from_home_assistant(&h.ctl, "ha-456", "Élise Martin", "", &admins)?.is_none(),
        "disabled stays out"
    );
    Ok(())
}

#[test]
fn a_database_from_before_users_upgrades_without_loss() -> Res {
    // A control database at schema 1 (no users), with one connection, then reopened.
    let t = Temp::new("upgrade");
    let data = t.path().join("data");
    let brain = Brain::init(&data, &[sp("personal")], Language::English)?;
    let tok = access::connect(
        &brain.open_control()?,
        "legacy",
        &Rights::owner(),
        None,
        false,
        false,
    )?;
    {
        let c = Connection::open(data.join("control.sqlite"))?;
        c.execute_batch(
            "PRAGMA foreign_keys = OFF;
             DROP TABLE grant_paths; DROP TABLE guides; DROP TABLE guide_approvals; DROP TABLE person_requests; DROP TABLE pairing_codes; DROP TABLE bridge_pairings; ALTER TABLE connections DROP COLUMN no_person_guide; ALTER TABLE connections DROP COLUMN all_inboxes; DROP TABLE oauth_refresh; DROP TABLE oauth_codes; DROP TABLE oauth_clients;
             DROP TABLE group_members; DROP TABLE group_spaces; DROP TABLE groups; DROP TABLE space_members;
             ALTER TABLE connections DROP COLUMN user_id; ALTER TABLE connections DROP COLUMN console;
             ALTER TABLE devices DROP COLUMN user_id; ALTER TABLE spaces DROP COLUMN label; ALTER TABLE spaces DROP COLUMN parent; ALTER TABLE spaces DROP COLUMN position; ALTER TABLE spaces DROP COLUMN kind;
             DROP TABLE users; PRAGMA user_version = 1;",
        )?;
    }
    let ctl = Brain::open(&data)?.open_control()?;
    let c = access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("legacy token refused")?;
    assert_eq!(c.user_id, OWNER);
    assert!(
        c.can_write(&sp("personal")),
        "the owner got every existing space"
    );
    Ok(())
}

#[test]
fn a_home_assistant_login_takes_over_an_unlinked_person_of_the_same_name() -> Res {
    let h = home()?;
    // Sam exists from the command line, not yet linked to Home Assistant.
    let u = users::from_home_assistant(&h.ctl, "ha-sam", "Sam", "Sam", &[])?.ok_or("none")?;
    assert_eq!(u.id, h.sam, "same person, same spaces");
    // A second login with the same name gets a new person: the first link stands.
    let other = users::from_home_assistant(&h.ctl, "ha-other", "Sam", "", &[])?.ok_or("none")?;
    assert_ne!(other.id, h.sam);
    assert_eq!(other.name, "sam-2");
    Ok(())
}

#[test]
fn on_a_fresh_installation_the_first_login_becomes_the_administrator() -> Res {
    let t = Temp::new("first");
    let brain = Brain::init(&t.path().join("data"), &[sp("personal")], Language::English)?;
    let ctl = brain.open_control()?;
    let first = users::from_home_assistant(&ctl, "ha-1", "Robin", "Robin", &[])?.ok_or("none")?;
    assert_eq!(
        (first.id.as_str(), first.admin, first.name.as_str()),
        (OWNER, true, "robin")
    );
    assert!(
        users::access_of(&ctl, &first.id)?.contains_key(&sp("personal")),
        "takes the initial spaces"
    );
    let second = users::from_home_assistant(&ctl, "ha-2", "Kim", "Kim", &[])?.ok_or("none")?;
    assert!(!second.admin, "only the first");
    Ok(())
}

#[test]
fn a_read_only_opening_cannot_write() -> Res {
    let h = home()?;
    let (_, t) = access::open_space_connection(&h.ctl, &h.alex, &sp("alex"), false, None)?;
    let c = caller(&h, &t)?;
    assert!(c.can_read(&sp("alex")));
    assert!(!c.can_write(&sp("alex")));
    Ok(())
}

#[test]
fn a_small_vault_is_not_buried_by_a_large_one() -> Res {
    // Scores of two vaults do not compare: a word that is rare in a large vault scores far
    // higher there than the same word in a small one. A hit as good must not be pushed out.
    let h = home()?;
    let c = caller(&h, &token(&h, &h.sam, "sam-agent", "all")?)?;
    let put = |space: &str, text: String| -> Res {
        let r = tools::call(
            &h.brain,
            &c,
            "remember",
            &json!({ "space": space, "text": text }),
        )?;
        assert_eq!(r["result"], "created", "{r}");
        Ok(())
    };
    for i in 0..150 {
        put("home", format!("filler{i} gamma{i} delta{i}"))?;
    }
    for t in ["pivot apple", "pivot banana", "pivot cherry"] {
        put("home", t.to_string())?;
    }
    put("sam", "pivot seed".to_string())?;
    let r = tools::call(
        &h.brain,
        &c,
        "recall",
        &json!({ "query": "pivot", "limit": 3 }),
    )?;
    let spaces: Vec<&str> = r["results"]
        .as_array()
        .ok_or("results")?
        .iter()
        .filter_map(|x| x["space"].as_str())
        .collect();
    assert_eq!(spaces.len(), 3);
    assert!(
        spaces.contains(&"sam"),
        "the small vault's hit is in the first three: {spaces:?}"
    );
    // One vault asked alone keeps its own order and its own hits.
    let one = tools::call(
        &h.brain,
        &c,
        "recall",
        &json!({ "query": "pivot", "space": "home", "limit": 5 }),
    )?;
    assert_eq!(one["results"].as_array().ok_or("results")?.len(), 3);
    Ok(())
}

#[test]
fn an_unknown_home_assistant_login_waits_and_creates_nobody() -> Res {
    let h = home()?;
    let admins = vec!["boss".to_string()];
    let first = users::ha_login(&h.ctl, "ha-9", "Stranger", "Stranger S.", &admins, false)?;
    assert!(matches!(first, users::HaLogin::Pending), "{first:?}");
    assert!(
        users::user_by_name(&h.ctl, "stranger")?.is_none(),
        "no person yet"
    );
    let again = users::ha_login(&h.ctl, "ha-9", "Stranger", "Stranger S.", &admins, false)?;
    assert!(matches!(again, users::HaLogin::Pending));
    let waiting = users::list_requests(&h.ctl)?;
    assert_eq!(waiting.len(), 1, "one request, not one per visit");
    assert_eq!(waiting[0].status, "pending");
    assert_eq!(
        users::request_status(&h.ctl, "ha-9")?.as_deref(),
        Some("pending")
    );

    // Approving creates the person, linked to the login, and the request goes away.
    let u = users::approve_request(&h.ctl, OWNER, "ha-9", false)?;
    assert_eq!(u.name, "stranger");
    assert!(!u.admin);
    assert!(users::list_requests(&h.ctl)?.is_empty());
    let back = users::ha_login(&h.ctl, "ha-9", "Stranger", "", &admins, false)?;
    assert!(matches!(back, users::HaLogin::Person(p) if p.id == u.id));
    assert!(
        users::approve_request(&h.ctl, OWNER, "ha-9", false).is_err(),
        "nothing left to approve"
    );
    Ok(())
}

#[test]
fn a_refused_login_stays_refused_and_asks_no_more() -> Res {
    let h = home()?;
    let admins = vec!["boss".to_string()];
    users::ha_login(&h.ctl, "ha-7", "Visitor", "", &admins, false)?;
    assert!(users::refuse_request(&h.ctl, OWNER, "ha-7")?);
    for _ in 0..3 {
        let r = users::ha_login(&h.ctl, "ha-7", "Visitor", "", &admins, false)?;
        assert!(matches!(r, users::HaLogin::Refused), "{r:?}");
    }
    assert!(users::user_by_name(&h.ctl, "visitor")?.is_none());
    assert!(
        users::approve_request(&h.ctl, OWNER, "ha-7", false).is_err(),
        "a refused request is not approved by accident"
    );
    assert!(
        !users::refuse_request(&h.ctl, OWNER, "ha-7")?,
        "already refused"
    );
    Ok(())
}

#[test]
fn the_approved_ways_in_do_not_wait() -> Res {
    let h = home()?;
    // Named in the administrators of the options: in at once, as an administrator.
    let admins = vec!["boss".to_string()];
    let boss = users::ha_login(&h.ctl, "ha-1", "boss", "Boss", &admins, false)?;
    assert!(
        matches!(boss, users::HaLogin::Person(ref p) if p.admin),
        "{boss:?}"
    );
    // A person an administrator created by that name, not linked yet: the login becomes them.
    let jo = users::create_user(&h.ctl, OWNER, "jo", "Jo", false)?;
    let linked = users::ha_login(&h.ctl, "ha-2", "Jo", "Jo", &admins, false)?;
    assert!(
        matches!(linked, users::HaLogin::Person(ref p) if p.id == jo.id),
        "{linked:?}"
    );
    // On a fresh installation (no administrator named, owner not linked), the first login
    // becomes the owner.
    let g = Temp::new("first-login");
    let brain = Brain::init(&g.path().join("data"), &[sp("home")], Language::English)?;
    let ctl = brain.open_control()?;
    let first = users::ha_login(&ctl, "ha-first", "Robin", "Robin", &[], false)?;
    assert!(
        matches!(first, users::HaLogin::Person(ref p) if p.id == OWNER),
        "{first:?}"
    );
    // The second unknown login, once the owner is linked, waits.
    let second = users::ha_login(&ctl, "ha-second", "Kim", "Kim", &[], false)?;
    assert!(matches!(second, users::HaLogin::Pending), "{second:?}");
    Ok(())
}

#[test]
fn a_flood_of_unknown_logins_is_not_kept() -> Res {
    let h = home()?;
    let admins = vec!["boss".to_string()];
    for i in 0..130 {
        users::ha_login(
            &h.ctl,
            &format!("ha-flood-{i}"),
            &format!("Flood {i}"),
            "",
            &admins,
            false,
        )?;
    }
    assert_eq!(users::list_requests(&h.ctl)?.len(), 100);
    Ok(())
}

#[test]
fn a_fact_that_concerns_another_vault_is_found_from_it_and_grants_nothing() -> Res {
    let h = home()?;
    let all = caller(&h, &token(&h, &h.sam, "sam-agent", "all")?)?;
    let only_sam = caller(&h, &token(&h, &h.sam, "sam-narrow", "sam:rw")?)?;
    // Stored in `home`, and says it also concerns `sam`.
    let r = tools::call(
        &h.brain,
        &all,
        "remember",
        &json!({ "space": "home", "text": "the boiler was serviced in march", "tags": ["house/boiler", "@sam"] }),
    )?;
    assert_eq!(r["result"], "created", "{r}");
    assert_eq!(r["fact"]["tags"], json!(["@sam", "house/boiler"]));
    let hits = |c: &access::Caller, space: Option<&str>| -> Result<Vec<String>, Box<dyn Error>> {
        let mut args = json!({ "query": "boiler" });
        if let Some(s) = space {
            args["space"] = json!(s);
        }
        let r = tools::call(&h.brain, c, "recall", &args)?;
        Ok(r["results"]
            .as_array()
            .ok_or("results")?
            .iter()
            .filter_map(|x| x["space"].as_str().map(String::from))
            .collect())
    };
    assert_eq!(
        hits(&all, Some("sam"))?,
        ["home"],
        "asked from `sam`, found in `home`"
    );
    assert_eq!(hits(&all, Some("home"))?, ["home"]);
    assert!(
        hits(&all, Some("finances"))?.is_empty(),
        "not concerning `finances`"
    );
    // Concern never grants access: a connection that cannot read `home` finds nothing there.
    assert!(hits(&only_sam, Some("sam"))?.is_empty());
    assert!(hits(&only_sam, None)?.is_empty());
    // A tag filter works through the tool, and a bad tag is explained.
    let by_tag = tools::call(
        &h.brain,
        &all,
        "recall",
        &json!({ "query": "boiler", "tags": ["house"] }),
    )?;
    assert_eq!(by_tag["results"].as_array().map(Vec::len), Some(1));
    let bad = tools::call(
        &h.brain,
        &all,
        "remember",
        &json!({ "space": "home", "text": "something else entirely", "tags": ["Bad Tag!"] }),
    );
    assert!(matches!(bad, Err(ToolError::Invalid(m)) if m.contains("lowercase segments")));
    // The vocabulary comes back with the counts.
    let stats = tools::call(&h.brain, &all, "stats", &json!({}))?;
    assert_eq!(stats["home"]["tags"]["house"], 1);
    assert_eq!(stats["home"]["tags"]["house/boiler"], 1);
    assert!(stats["finances"].get("tags").is_none(), "no tags, no key");
    Ok(())
}

#[test]
fn each_person_reaches_only_their_own_spaces() -> Res {
    let h = home()?;
    assert_eq!(spaces_of(&h, &h.sam)?, ["finances", "home", "sam"]);
    assert_eq!(spaces_of(&h, &h.alex)?, ["alex"]);
    assert!(spaces_of(&h, OWNER)?.is_empty());
    Ok(())
}

#[test]
fn a_token_cannot_be_granted_beyond_its_person() -> Res {
    let h = home()?;
    assert!(
        token(&h, &h.alex, "a1", "sam:r").is_err(),
        "no access to sam"
    );
    assert!(
        token(&h, &h.alex, "a2", "home:r").is_err(),
        "no access to Sam's other spaces"
    );
    assert!(token(&h, &h.alex, "a3", "alex:rw").is_ok());
    Ok(())
}

#[test]
fn opening_a_vault_gives_a_connection_limited_to_it() -> Res {
    let h = home()?;
    let (agent, t) = access::open_space_connection(&h.ctl, &h.alex, &sp("alex"), true, None)?;
    assert!(agent.starts_with("alex-"), "named after the vault: {agent}");
    let c = caller(&h, &t)?;
    assert!(c.can_write(&sp("alex")));
    assert!(!c.can_read(&sp("sam")));
    assert!(matches!(
        tools::call(
            &h.brain,
            &c,
            "recall",
            &json!({ "query": "x", "space": "sam" })
        ),
        Err(ToolError::Denied(_))
    ));
    Ok(())
}

#[test]
fn a_vault_cannot_be_opened_beyond_the_person() -> Res {
    let h = home()?;
    assert!(
        access::open_space_connection(&h.ctl, &h.alex, &sp("sam"), false, None).is_err(),
        "alex does not reach sam"
    );
    assert!(access::open_space_connection(&h.ctl, &h.alex, &sp("alex"), false, None).is_ok());
    Ok(())
}

#[test]
fn names_are_checked() -> Res {
    let h = home()?;
    assert!(users::create_user(&h.ctl, OWNER, "Sam", "", false).is_err());
    assert!(
        users::create_user(&h.ctl, OWNER, "sam", "", false).is_err(),
        "taken"
    );
    Ok(())
}

#[test]
fn an_heir_attaches_a_new_account_only_after_a_long_absence() -> Res {
    let h = home()?;
    let now = 1_900_000_000;
    users::set_heir(&h.ctl, &h.sam, Some(&h.alex))?;
    assert_eq!(users::heir_of(&h.ctl, &h.sam)?, Some(h.alex.clone()));
    assert_eq!(users::heir_to(&h.ctl, &h.alex)?.len(), 1);
    // Sam answers to an account that is then lost; a new one waits.
    users::set_ha_user(&h.ctl, &h.sam, Some("ha-old"))?;
    let wait = users::ha_login(&h.ctl, "ha-new", "sam", "Sam", &["root".to_string()], false)?;
    assert!(matches!(wait, users::HaLogin::Pending), "{wait:?}");
    // Sam was seen a moment ago: no.
    users::touch_seen(&h.ctl, &h.sam, now)?;
    assert!(users::attach_request(&h.ctl, &h.alex, "ha-new", &h.sam, now + 60).is_err());
    // Not the heir: no.
    assert!(users::attach_request(&h.ctl, OWNER, "ha-new", &h.sam, now + 40 * 86_400).is_err());
    // The heir, after thirty days: yes. The account is Sam's now, and no longer waits.
    let sam = users::attach_request(&h.ctl, &h.alex, "ha-new", &h.sam, now + 31 * 86_400)?;
    assert_eq!(sam.ha_user.as_deref(), Some("ha-new"));
    assert!(users::list_requests(&h.ctl)?.is_empty());
    // Not oneself, not nobody.
    assert!(users::set_heir(&h.ctl, &h.sam, Some(&h.sam)).is_err());
    assert!(users::set_heir(&h.ctl, &h.sam, Some("nobody")).is_err());
    users::set_heir(&h.ctl, &h.sam, None)?;
    assert_eq!(users::heir_of(&h.ctl, &h.sam)?, None);
    Ok(())
}

#[test]
fn a_recovery_key_lets_a_new_account_take_the_memory_over_once() -> Res {
    let h = home()?;
    assert!(!users::has_recovery_key(&h.ctl, &h.sam)?);
    let key = users::make_recovery_key(&h.ctl, &h.sam)?;
    assert_eq!(key.len(), 4 * 5 + 4, "five groups of four: {key}");
    assert!(users::has_recovery_key(&h.ctl, &h.sam)?);
    // A wrong key takes nothing.
    assert!(users::claim_with_recovery_key(&h.ctl, "ha-x", "AAAA-AAAA-AAAA-AAAA-AAAA")?.is_none());
    // The right one does, however it is typed, and it is used up.
    let typed = key.to_lowercase().replace('-', " ");
    let sam = users::claim_with_recovery_key(&h.ctl, "ha-new", &typed)?.ok_or("refused")?;
    assert_eq!(sam.id, h.sam);
    assert_eq!(sam.ha_user.as_deref(), Some("ha-new"));
    assert!(!users::has_recovery_key(&h.ctl, &h.sam)?);
    assert!(users::claim_with_recovery_key(&h.ctl, "ha-other", &key)?.is_none());
    // Making another key replaces the earlier one.
    let first = users::make_recovery_key(&h.ctl, &h.alex)?;
    let second = users::make_recovery_key(&h.ctl, &h.alex)?;
    assert!(users::claim_with_recovery_key(&h.ctl, "ha-1", &first)?.is_none());
    assert!(users::claim_with_recovery_key(&h.ctl, "ha-2", &second)?.is_some());
    Ok(())
}

#[test]
fn with_auto_creation_every_new_account_gets_a_person_unless_it_was_refused() -> Res {
    let h = home()?;
    let admins = vec!["boss".to_string()];
    // A new account is let in at once, a member with a memory of their own to make.
    let new = users::ha_login(&h.ctl, "ha-new", "Jo", "Jo", &admins, true)?;
    let users::HaLogin::Person(jo) = new else {
        return Err("not let in".into());
    };
    assert!(!jo.admin);
    assert_eq!(jo.ha_user.as_deref(), Some("ha-new"));
    assert!(users::list_requests(&h.ctl)?.is_empty(), "nothing waits");
    // The same account again is the same person.
    let again = users::ha_login(&h.ctl, "ha-new", "Jo", "Jo", &admins, true)?;
    assert!(
        matches!(&again, users::HaLogin::Person(p) if p.id == jo.id),
        "{again:?}"
    );
    // One an administrator refused stays refused, whatever the setting.
    users::ha_login(&h.ctl, "ha-bad", "Mallory", "", &admins, false)?;
    users::refuse_request(&h.ctl, OWNER, "ha-bad")?;
    let refused = users::ha_login(&h.ctl, "ha-bad", "Mallory", "", &admins, true)?;
    assert!(matches!(refused, users::HaLogin::Refused), "{refused:?}");
    Ok(())
}
