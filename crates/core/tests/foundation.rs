//! The foundation: data directory, spaces, pragmas, permissions, schema invariants.
//! Fictitious data, no network, temporary directories.

mod common;

use std::error::Error;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use common::{Res, SPACES, Temp, brain, mode, sp, spaces};
use memcp_core::text::Language;
use memcp_core::{Brain, SPACE_SCHEMA_VERSION};
use rusqlite::Connection;

#[cfg(unix)]
#[test]
fn init_creates_private_files_in_a_private_directory() -> Res {
    let t = Temp::new("found");
    let b = brain(&t)?;
    let data = b.dir();
    assert_eq!(mode(data)?, 0o700, "data directory");
    for f in SPACES
        .iter()
        .map(|s| format!("{s}.sqlite"))
        .chain(["control.sqlite".into()])
    {
        assert_eq!(mode(&data.join(&f))?, 0o600, "{f}");
    }
    assert_eq!(mode(&data.join("config.toml"))?, 0o600);
    assert_eq!(b.spaces()?, {
        let mut s = spaces();
        s.sort();
        s
    });
    Ok(())
}

#[test]
fn init_without_spaces_is_allowed_and_spaces_can_be_added() -> Res {
    let t = Temp::new("found");
    let b = Brain::init(&t.path().join("data"), &[], Language::French)?;
    assert!(b.spaces()?.is_empty());
    assert_eq!(b.language()?, Language::French);
    b.add_space(&sp("household"))?;
    assert_eq!(b.spaces()?, vec![sp("household")]);
    assert!(b.add_space(&sp("household")).is_err(), "no duplicate space");
    assert!(b.open_space(&sp("household")).is_ok());
    Ok(())
}

#[test]
fn an_unregistered_space_never_opens() -> Res {
    let t = Temp::new("found");
    let b = brain(&t)?;
    assert!(b.open_space(&sp("household")).is_err());
    // A stray file with the right name is not enough: the space must be registered.
    fs::copy(b.dir().join("home.sqlite"), b.dir().join("garden.sqlite"))?;
    assert!(b.open_space(&sp("garden")).is_err());
    Ok(())
}

#[test]
fn init_refuses_an_initialised_directory_and_changes_nothing() -> Res {
    let t = Temp::new("found");
    let b = brain(&t)?;
    let before = fs::read(b.dir().join("personal.sqlite"))?;
    assert!(Brain::init(b.dir(), &spaces(), Language::English).is_err());
    assert_eq!(
        fs::read(b.dir().join("personal.sqlite"))?,
        before,
        "database untouched"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn init_refuses_a_directory_open_to_others() -> Res {
    let t = Temp::new("found");
    let data = t.path().join("data");
    fs::create_dir_all(&data)?;
    fs::set_permissions(&data, fs::Permissions::from_mode(0o755))?;
    assert!(Brain::init(&data, &spaces(), Language::English).is_err());
    assert!(!data.join("control.sqlite").exists(), "nothing is created");
    Ok(())
}

#[test]
fn opens_every_space_and_reads_user_version() -> Res {
    let t = Temp::new("found");
    let b = Brain::open(brain(&t)?.dir())?;
    for s in spaces() {
        let c = b.open_space(&s)?;
        let v: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        assert_eq!(v, SPACE_SCHEMA_VERSION, "{s}");
        let declared: String =
            c.query_row("SELECT v FROM meta WHERE k='space'", [], |r| r.get(0))?;
        assert_eq!(declared, s.as_str());
    }
    let n: u32 = b
        .open_control()?
        .query_row("SELECT count(*) FROM connections", [], |r| r.get(0))?;
    assert_eq!(n, 0);
    Ok(())
}

#[test]
fn pragmas_are_applied() -> Res {
    let t = Temp::new("found");
    let c = brain(&t)?.open_space(&sp("health"))?;
    let jm: String = c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    assert_eq!(jm.to_lowercase(), "wal");
    let fk: i64 = c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    assert_eq!(fk, 1);
    let sd: i64 = c.query_row("PRAGMA secure_delete", [], |r| r.get(0))?;
    assert_eq!(sd, 1);
    let mm: i64 = c.query_row("PRAGMA mmap_size", [], |r| r.get(0))?;
    assert_eq!(mm, 0);
    Ok(())
}

#[test]
fn refuses_a_file_stored_under_the_wrong_name() -> Res {
    let t = Temp::new("found");
    let b = brain(&t)?;
    // home.sqlite copied over health.sqlite: the declared space no longer matches.
    fs::copy(b.dir().join("home.sqlite"), b.dir().join("health.sqlite"))?;
    assert!(b.open_space(&sp("health")).is_err());
    assert!(b.open_space(&sp("home")).is_ok());
    Ok(())
}

#[test]
fn open_refuses_an_uninitialised_directory() {
    let t = Temp::new("found");
    assert!(Brain::open(t.path()).is_err());
}

// --- the schema keeps its promises ---

fn insert_fact(c: &Connection, id: &str, text: &str, hash: &str) -> rusqlite::Result<usize> {
    c.execute(
        "INSERT INTO facts (id, space, kind, text, text_hash, source_kind, source_agent, author, created_at, valid_from)
         VALUES (?1, 'personal', 'fact', ?2, ?3, 'agent', 'agent:test', 'owner', 1000, 1000)",
        (id, text, hash),
    )
}

fn personal() -> Result<(Temp, Connection), Box<dyn Error>> {
    let t = Temp::new("found");
    let c = brain(&t)?.open_space(&sp("personal"))?;
    Ok((t, c))
}

#[test]
fn events_are_append_only() -> Res {
    let (_t, c) = personal()?;
    insert_fact(&c, "f_aaaaaaaaaaaa", "Sam likes green tea.", "h1")?;
    c.execute(
        "INSERT INTO fact_events (fact_id, at, actor, action, detail) VALUES ('f_aaaaaaaaaaaa', 1000, 'owner', 'create', '{}')",
        [],
    )?;
    assert!(c.execute("UPDATE fact_events SET actor='x'", []).is_err());
    assert!(c.execute("DELETE FROM fact_events", []).is_err());
    Ok(())
}

#[test]
fn one_active_fact_per_exact_text() -> Res {
    let (_t, c) = personal()?;
    insert_fact(
        &c,
        "f_aaaaaaaaaaaa",
        "Dr Martin sees patients on Tuesdays.",
        "h2",
    )?;
    assert!(
        insert_fact(
            &c,
            "f_bbbbbbbbbbbb",
            "Dr Martin sees patients on Tuesdays.",
            "h2"
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn empty_text_is_refused_and_the_database_has_no_size_limit() -> Res {
    let (_t, c) = personal()?;
    assert!(insert_fact(&c, "f_aaaaaaaaaaaa", "", "h3").is_err());
    assert!(insert_fact(&c, "f_bbbbbbbbbbbb", &"a".repeat(100_000), "h4").is_ok());
    Ok(())
}

#[test]
fn a_replaced_fact_must_have_a_replacement() -> Res {
    let (_t, c) = personal()?;
    insert_fact(&c, "f_aaaaaaaaaaaa", "The bakery opens at 9.", "h5")?;
    let r = c.execute(
        "UPDATE facts SET status='replaced' WHERE id='f_aaaaaaaaaaaa'",
        [],
    );
    assert!(r.is_err(), "replaced without replaced_by is inconsistent");
    Ok(())
}

#[test]
fn full_text_search_is_available() -> Res {
    let (_t, c) = personal()?;
    insert_fact(&c, "f_aaaaaaaaaaaa", "Sam has tea at four.", "h6")?;
    let rowid: i64 = c.query_row(
        "SELECT rowid FROM facts WHERE id='f_aaaaaaaaaaaa'",
        [],
        |r| r.get(0),
    )?;
    c.execute(
        "INSERT INTO facts_fts (rowid, stem) VALUES (?1, 'sam tea four')",
        [rowid],
    )?;
    let found: i64 = c.query_row(
        "SELECT rowid FROM facts_fts WHERE facts_fts MATCH 'tea'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(found, rowid);
    Ok(())
}

#[test]
fn the_control_database_has_its_tables() -> Res {
    let t = Temp::new("found");
    let ctl = brain(&t)?.open_control()?;
    for table in ["connections", "grants", "auth_audit", "devices"] {
        let n: u32 = ctl.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
        assert_eq!(n, 0, "{table}");
    }
    let n: u32 = ctl.query_row("SELECT count(*) FROM spaces", [], |r| r.get(0))?;
    assert_eq!(n, 4);
    Ok(())
}
