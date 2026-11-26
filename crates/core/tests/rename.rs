//! Renaming a space: the file, the files folder and every row that names it follow; the Inbox of a memory made before its
//! kind was in English comes out as `inbox`.

mod common;

use common::{Res, Temp, brain};
use pensine_core::access::{self, Rights};
use pensine_core::users::OWNER;
use pensine_core::{Space, tools, triage, util};
use serde_json::json;

#[test]
fn a_space_is_renamed_on_disk_and_in_the_control_database_and_an_old_box_becomes_the_inbox() -> Res
{
    let t = Temp::new("rename");
    let b = brain(&t)?;
    let old = Space::new("owner-a-trier")?;
    b.add_space_for(&old, OWNER)?;
    b.open_control()?.execute(
        "UPDATE spaces SET kind = 'a-trier' WHERE name = 'owner-a-trier'",
        [],
    )?;
    assert_eq!(
        triage::of(&b, OWNER)?.map(|s| s.to_string()),
        Some("owner-a-trier".into()),
        "the old kind is the Inbox"
    );

    // A device that writes to the box, and a note in it.
    let ctl = b.open_control()?;
    let tok = access::connect_for(
        &ctl,
        OWNER,
        "device",
        &Rights::parse("owner-a-trier/inbox:rw")?,
        None,
        false,
        false,
    )?;
    let device = access::authenticate(&ctl, &tok, util::now())?.ok_or("refused")?;
    let w = tools::call(
        &b,
        &device,
        "remember",
        &json!({ "space": "owner-a-trier", "text": "Une dictée qui attend", "tags": ["inbox"] }),
    )?;
    let id = w["fact"]["id"].as_str().ok_or("id")?.to_string();
    std::fs::create_dir_all(b.dir().join("files").join("owner-a-trier"))?;
    std::fs::write(
        b.dir().join("files").join("owner-a-trier").join("blob"),
        b"x",
    )?;

    let new = Space::new("owner-inbox")?;
    // A count first: nothing moves.
    let dry = b.rename_space(OWNER, &old, &new, true)?;
    assert!(dry.dry_run && dry.grants >= 1 && dry.has_files, "{dry:?}");
    assert!(
        b.dir().join("owner-a-trier.sqlite").exists()
            && !b.dir().join("owner-inbox.sqlite").exists()
    );

    let done = b.rename_space(OWNER, &old, &new, false)?;
    assert_eq!(done.kind, "inbox");
    assert!(
        !b.dir().join("owner-a-trier.sqlite").exists()
            && b.dir().join("owner-inbox.sqlite").exists()
    );
    assert!(
        b.dir()
            .join("files")
            .join("owner-inbox")
            .join("blob")
            .exists()
    );
    assert!(!b.dir().join("files").join("owner-a-trier").exists());
    // The notes, with their ids, are under the new name, and say so.
    let conn = b.open_space(&new)?;
    let space_of: String =
        conn.query_row("SELECT space FROM facts WHERE id = ?1", [&id], |r| r.get(0))?;
    assert_eq!(space_of, "owner-inbox");
    // The control database: the space, its kind, its label, and the grants of the device.
    assert_eq!(b.space_kind(&new)?, "inbox");
    assert!(!b.has_space(&old)?);
    assert_eq!(
        triage::of(&b, OWNER)?.map(|s| s.to_string()),
        Some("owner-inbox".into())
    );
    let grants: i64 = b.open_control()?.query_row(
        "SELECT count(*) FROM grants WHERE space = 'owner-inbox'",
        [],
        |r| r.get(0),
    )?;
    let stale: i64 = b.open_control()?.query_row(
        "SELECT count(*) FROM grants WHERE space = 'owner-a-trier'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!((grants >= 1, stale), (true, 0));
    // The device, with the same key, writes to the box under its new name.
    let ctl = b.open_control()?;
    let device = access::authenticate(&ctl, &tok, util::now())?.ok_or("the key still works")?;
    tools::call(
        &b,
        &device,
        "remember",
        &json!({ "space": "owner-inbox", "text": "Une autre dictée", "tags": ["inbox"] }),
    )?;
    // A name that is taken, or the same name, is refused.
    assert!(
        b.rename_space(OWNER, &new, &Space::new("health")?, false)
            .is_err()
    );
    assert!(b.rename_space(OWNER, &new, &new, false).is_err());
    Ok(())
}
