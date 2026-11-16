//! Backup: a consistent copy of every database, outside the data directory, N kept.

mod common;

use std::fs;

use common::{Res, Temp, brain, mode, spaces};
use rusqlite::Connection;

#[test]
fn copies_every_database_readable_and_private() -> Res {
    let t = Temp::new("backup");
    let b = brain(&t)?;
    b.open_space(&common::sp("health"))?
        .execute("INSERT INTO meta (k, v) VALUES ('probe', 'present')", [])?;

    let root = t.path().join("backups");
    let copy = b.backup(&root, 14, 1_000)?;

    #[cfg(unix)]
    {
        assert_eq!(mode(&root)?, 0o700);
        assert_eq!(mode(&copy)?, 0o700);
    }
    for s in spaces() {
        let f = copy.join(format!("{s}.sqlite"));
        #[cfg(unix)]
        assert_eq!(mode(&f)?, 0o600, "{}", f.display());
        let c = Connection::open(&f)?;
        let ok: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        assert_eq!(ok, "ok");
    }
    let c = Connection::open(copy.join("health.sqlite"))?;
    let v: String = c.query_row("SELECT v FROM meta WHERE k = 'probe'", [], |r| r.get(0))?;
    assert_eq!(v, "present");
    assert!(copy.join("control.sqlite").is_file());
    assert!(copy.join("config.toml").is_file());
    Ok(())
}

#[test]
fn refuses_a_destination_inside_the_data() -> Res {
    let t = Temp::new("backup");
    let b = brain(&t)?;
    assert!(b.backup(&b.dir().join("backups"), 14, 1_000).is_err());
    assert!(b.backup(b.dir(), 14, 1_000).is_err());
    assert!(
        !b.dir().join("backups").exists(),
        "nothing is created inside the data"
    );
    Ok(())
}

#[test]
fn keeps_only_the_most_recent() -> Res {
    let t = Temp::new("backup");
    let b = brain(&t)?;
    let root = t.path().join("backups");
    fs::create_dir_all(root.join("do-not-touch"))?;
    for i in 0..5 {
        b.backup(&root, 3, 1_000 + i * 86_400)?;
    }
    let mut names: Vec<String> = fs::read_dir(&root)?
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "backup-1970-01-03T001640",
            "backup-1970-01-04T001640",
            "backup-1970-01-05T001640",
            "do-not-touch",
        ]
    );
    Ok(())
}

#[test]
fn two_backups_in_the_same_second_do_not_overwrite() -> Res {
    let t = Temp::new("backup");
    let b = brain(&t)?;
    let root = t.path().join("backups");
    b.backup(&root, 14, 1_000)?;
    assert!(b.backup(&root, 14, 1_000).is_err());
    Ok(())
}
