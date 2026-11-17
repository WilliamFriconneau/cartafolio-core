//! The catalogue of vault kinds: a vault is made from a kind, named after its person, and the
//! tools of a kind follow the vaults a caller reaches.

mod common;

use std::error::Error;
use std::fs;

use common::{Res, Temp, sp};
use memcp_core::access::{self, Rights};
use memcp_core::catalogue::{self, CUSTOM, Feature};
use memcp_core::text::Language;
use memcp_core::users::{self, OWNER};
use memcp_core::{Brain, Error as CoreError};

fn brain(name: &str) -> Result<(Temp, Brain), Box<dyn Error>> {
    let t = Temp::new(name);
    let b = Brain::init(&t.path().join("data"), &[sp("legacy")], Language::English)?;
    Ok((t, b))
}

#[test]
fn the_default_catalogue_follows_what_a_person_keeps_track_of() -> Res {
    let (_t, b) = brain("cat-default")?;
    let kinds = b.catalogue()?;
    let keys: Vec<&str> = kinds.iter().map(|k| k.key.as_str()).collect();
    assert_eq!(
        keys,
        ["me", "health", "people", "home", "money", "work", "hobbies"]
    );
    let feature = |k: &str| kinds.iter().find(|x| x.key == k).and_then(|x| x.feature);
    assert_eq!(feature("health"), None);
    assert_eq!(feature("money"), Some(Feature::Money));
    assert_eq!(feature("work"), None);
    Ok(())
}

#[test]
fn a_catalogue_file_replaces_the_default_and_its_errors_name_the_line() -> Res {
    let (t, b) = brain("cat-file")?;
    fs::write(
        t.path().join("data").join("catalogue.txt"),
        "# for a classroom\nlessons | Lessons | What I teach\npupils | Pupils | The class | money\n",
    )?;
    let keys: Vec<String> = b.catalogue()?.into_iter().map(|k| k.key).collect();
    assert_eq!(keys, ["lessons", "pupils"]);
    for (text, why) in [
        ("", "no kind"),
        ("a | A", "expected"),
        ("Bad Key | A | b", "lowercase letters"),
        ("custom | A | b", "reserved"),
        ("a | A | b\na | A | b", "already defined"),
        ("a | A | b | wealth", "the last field is `money`"),
        ("a |  | b", "label is empty"),
    ] {
        let e = catalogue::parse(text)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(e.contains(why), "{text:?} gave {e:?}");
    }
    fs::write(t.path().join("data").join("catalogue.txt"), "x | X\n")?;
    assert!(
        b.catalogue().is_err(),
        "a broken file is an error, not a silent default"
    );
    Ok(())
}

#[test]
fn a_vault_is_named_after_its_person_and_its_kind() -> Res {
    assert_eq!(
        catalogue::vault_name("sam", "health")?.as_str(),
        "sam-health"
    );
    let long = catalogue::vault_name(&"a".repeat(40), "hobbies")?;
    assert!(
        long.as_str().len() <= 32 && long.as_str().ends_with("-hobbies"),
        "{long}"
    );
    let odd = catalogue::vault_name("42 Élise", "me")?;
    assert!(
        odd.as_str().starts_with('p') && odd.as_str().ends_with("-me"),
        "{odd}"
    );
    Ok(())
}

#[test]
fn a_person_makes_their_vaults_from_the_catalogue_only() -> Res {
    let (_t, b) = brain("cat-vault")?;
    let ctl = b.open_control()?;
    let sam = users::create_user(&ctl, OWNER, "sam", "Sam", false)?;
    let v = b.add_vault(&sam.id, "sam", "health")?;
    assert_eq!(v.as_str(), "sam-health");
    assert_eq!(b.space_kind(&v)?, "health");
    assert!(users::is_space_owner(&ctl, &v, &sam.id)?);
    for (kind, why) in [("notes", "unknown kind"), ("custom", "unknown kind")] {
        let e = b
            .add_vault(&sam.id, "sam", kind)
            .err()
            .map(|e| e.to_string());
        assert!(
            e.is_some_and(|m| m.contains(why) && m.contains("hobbies")),
            "{kind}"
        );
    }
    let again = b.add_vault(&sam.id, "sam", "health");
    assert!(matches!(again, Err(CoreError::Invalid(m)) if m.contains("already has")));
    // A space that predates kinds is `custom`; a kind can be given to it, from the catalogue.
    let old = sp("legacy");
    assert_eq!(b.space_kind(&old)?, CUSTOM);
    b.set_space_kind(&old, "work")?;
    assert_eq!(b.space_kind(&old)?, "work");
    assert!(b.set_space_kind(&old, "nonsense").is_err());
    b.set_space_kind(&old, CUSTOM)?;
    assert!(b.set_space_kind(&sp("missing"), "work").is_err());
    Ok(())
}

#[test]
fn the_vault_of_a_kind_is_the_one_the_caller_owns() -> Res {
    let (_t, b) = brain("cat-resolve")?;
    let ctl = b.open_control()?;
    let sam = users::create_user(&ctl, OWNER, "sam", "Sam", false)?;
    let alex = users::create_user(&ctl, OWNER, "alex", "Alex", false)?;
    let sam_health = b.add_vault(&sam.id, "sam", "health")?;
    let alex_health = b.add_vault(&alex.id, "alex", "health")?;
    let token = |user: &str, agent: &str, spec: &str| -> Result<access::Caller, Box<dyn Error>> {
        let r = Rights::parse(spec)?;
        let t = access::connect_for(&ctl, user, agent, &r, None, false, false)?;
        Ok(access::authenticate(&ctl, &t, 1_900_000_000)?.ok_or("refused")?)
    };
    // Each person finds their own, and the other's is not theirs.
    let s = token(&sam.id, "s1", "all")?;
    let a = token(&alex.id, "a1", "all")?;
    assert_eq!(
        access::space_of_kind(&ctl, &s, "health")?,
        Some(sam_health.clone())
    );
    assert_eq!(
        access::space_of_kind(&ctl, &a, "health")?,
        Some(alex_health.clone())
    );
    // A connection that is not opened on a vault of that kind finds none: its tools are not offered.
    let work_only = {
        b.add_vault(&sam.id, "sam", "work")?;
        token(&sam.id, "s3", "sam-work:rw")?
    };
    assert_eq!(access::space_of_kind(&ctl, &work_only, "health")?, None);
    assert_eq!(
        access::space_of_kind(&ctl, &s, "money")?,
        None,
        "no vault of that kind at all"
    );
    Ok(())
}

#[test]
fn a_database_from_before_kinds_gets_custom_for_every_space() -> Res {
    let (t, b) = brain("cat-upgrade")?;
    drop(b);
    {
        let c = rusqlite::Connection::open(t.path().join("data").join("control.sqlite"))?;
        c.execute_batch(
            "ALTER TABLE users DROP COLUMN heir; ALTER TABLE users DROP COLUMN recovery_hash; ALTER TABLE users DROP COLUMN last_seen; DROP TABLE grant_paths; DROP TABLE guides; DROP TABLE guide_approvals; ALTER TABLE oauth_codes DROP COLUMN agent; ALTER TABLE spaces DROP COLUMN label; ALTER TABLE spaces DROP COLUMN parent; ALTER TABLE spaces DROP COLUMN position; ALTER TABLE spaces DROP COLUMN kind; DROP TABLE folder_state; DROP TABLE space_folders; DROP TABLE pairings; DROP TABLE sync_state; DROP TABLE peer_meta; DROP TABLE peer_directory; ALTER TABLE connections DROP COLUMN internal; ALTER TABLE connections DROP COLUMN no_person_guide; ALTER TABLE connections DROP COLUMN all_inboxes; PRAGMA user_version = 6;",
        )?;
    }
    let b = Brain::open(&t.path().join("data"))?;
    assert_eq!(b.space_kind(&sp("legacy"))?, CUSTOM);
    Ok(())
}

#[test]
fn one_vault_of_a_kind_per_person_even_when_the_kind_was_given_afterwards() -> Res {
    let (_t, b) = brain("cat-once")?;
    // `legacy` belongs to the bootstrap person; give it a kind after the fact.
    b.set_space_kind(&sp("legacy"), "work")?;
    let e = b
        .add_vault(OWNER, "owner", "work")
        .err()
        .map(|e| e.to_string());
    assert!(
        e.is_some_and(|m| m.contains("already has a work space (legacy)")),
        "the existing vault counts"
    );
    // Another kind is fine, and so is the same kind for someone else.
    assert_eq!(
        b.add_vault(OWNER, "owner", "hobbies")?.as_str(),
        "owner-hobbies"
    );
    let ctl = b.open_control()?;
    let kim = users::create_user(&ctl, OWNER, "kim", "Kim", false)?;
    assert_eq!(b.add_vault(&kim.id, "kim", "work")?.as_str(), "kim-work");
    Ok(())
}
