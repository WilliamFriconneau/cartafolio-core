//! A database made by every older version of the program opens, upgrades and keeps everything it held.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use rusqlite::{Connection, params};

use crate::access;
use crate::brain::{Brain, migrate};
use crate::schema::{CONTROL_MIGRATIONS, SPACE_MIGRATIONS};
use crate::space::Space;
use crate::text::{Language, stems, text_hash};

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("memcp-migration-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn remove_db(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    for s in ["-wal", "-shm", "-journal"] {
        let mut p = path.as_os_str().to_os_string();
        p.push(s);
        let _ = std::fs::remove_file(PathBuf::from(p));
    }
}

fn version(conn: &Connection) -> usize {
    conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
        .map(|v| usize::try_from(v).unwrap())
        .unwrap()
}

/// Inserts a fact the way every version of the program stored one (columns of the first schema).
fn raw_fact(conn: &Connection, id: &str, text: &str, status: &str, replaced_by: Option<&str>) {
    conn.execute(
        "INSERT INTO facts (id, space, kind, text, text_hash, source_kind, source_agent, author, created_at, valid_from,
                            status, status_at, status_by, status_reason, replaced_by)
         VALUES (?1, 'health', 'fact', ?2, ?3, 'agent', 'agent:old', 'old', 1700000000, 1700000000, ?4,
                 CASE WHEN ?4 = 'active' THEN NULL ELSE 1700000100 END,
                 CASE WHEN ?4 = 'active' THEN NULL ELSE 'old' END,
                 CASE WHEN ?4 = 'retired' THEN 'obsolete' ELSE NULL END, ?5)",
        params![id, text, text_hash(text), status, replaced_by],
    )
    .unwrap();
    let rowid = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO facts_fts (rowid, stem) VALUES (?1, ?2)",
        params![rowid, stems(text).join(" ")],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO fact_events (fact_id, at, actor, action, detail) VALUES (?1, 1700000000, 'old', 'create', '{}')",
        [id],
    )
    .unwrap();
}

#[test]
fn a_space_made_by_any_older_version_upgrades_and_keeps_every_fact() {
    let latest = SPACE_MIGRATIONS.len();
    for n in 1..=latest {
        let dir = temp(&format!("space-{n}"));
        let brain = Brain::init(&dir, &[Space::new("health").unwrap()], Language::English).unwrap();
        let health = Space::new("health").unwrap();
        let path = dir.join(health.file_name());
        remove_db(&path);
        {
            // The file as version `n` of the program left it.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
                .unwrap();
            migrate(&conn, &SPACE_MIGRATIONS[..n]).unwrap();
            assert_eq!(version(&conn), n);
            conn.execute("INSERT INTO meta (k, v) VALUES ('space', 'health')", [])
                .unwrap();
            raw_fact(
                &conn,
                "f_old_1",
                "Resting heart rate was fifty-eight in March.",
                "active",
                None,
            );
            raw_fact(
                &conn,
                "f_old_3",
                "Resting heart rate was fifty-six in April.",
                "active",
                None,
            );
            raw_fact(
                &conn,
                "f_old_2",
                "Resting heart rate was sixty in January.",
                "replaced",
                Some("f_old_3"),
            );
            raw_fact(
                &conn,
                "f_old_4",
                "The scale was broken for a week.",
                "retired",
                None,
            );
            if n >= 5 {
                conn.execute(
                    "INSERT INTO fact_tags (fact_id, tag) VALUES ('f_old_1', 'journal'), ('f_old_1', 'sport/run')",
                    [],
                )
                .unwrap();
            }
            if n >= 6 {
                conn.execute(
                    "INSERT INTO fact_origin (fact_id, app, ext_id, at) VALUES ('f_old_3', 'watch', 'hr/42', '2026-04-01')",
                    [],
                )
                .unwrap();
            }
            if n >= 3 {
                conn.execute(
                    "INSERT INTO files (id, name, mime, size, sha256, author, created_at)
                     VALUES ('file_old', 'scan.txt', 'text/plain', 3, 'abc', 'old', 1700000000)",
                    [],
                )
                .unwrap();
            }
            if n >= 7 {
                conn.execute(
                    "INSERT INTO app_guides (app, text, updated_at) VALUES ('watch', 'Tag heart rate.', 1)",
                    [],
                )
                .unwrap();
            }
        }

        // The program opens it: it upgrades to the latest version.
        let conn = brain.open_space(&health).unwrap();
        assert_eq!(version(&conn), latest, "from version {n}");
        let rows: Vec<(String, String)> = conn
            .prepare("SELECT id, status FROM facts ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(
            rows,
            [
                ("f_old_1", "active"),
                ("f_old_2", "replaced"),
                ("f_old_3", "active"),
                ("f_old_4", "retired")
            ]
            .map(|(a, b)| (a.to_string(), b.to_string())),
            "from version {n}"
        );
        // Search still finds them (the full-text index was kept), history is whole.
        let hits = crate::facts::recall(
            &conn,
            "heart",
            &crate::facts::RecallOptions::default(),
            1_800_000_000,
        )
        .unwrap();
        assert_eq!(
            hits.len(),
            2,
            "from version {n}: the two active heart-rate facts"
        );
        let events: i64 = conn
            .query_row("SELECT count(*) FROM fact_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events, 4);
        if n >= 5 {
            let tags: i64 = conn
                .query_row(
                    "SELECT count(*) FROM fact_tags WHERE fact_id = 'f_old_1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(tags, 2, "from version {n}: the tags");
        }
        if n >= 6 {
            let o: String = conn
                .query_row(
                    "SELECT ext_id FROM fact_origin WHERE fact_id = 'f_old_3'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(o, "hr/42");
        }
        // The new tables are there and the log of changes was made for what existed.
        let logged: i64 = conn
            .query_row("SELECT count(*) FROM sync_log", [], |r| r.get(0))
            .unwrap();
        assert!(
            logged >= 4,
            "from version {n}: {logged} changes logged for 4 existing facts"
        );
        // The limit on the size of a text was lifted by version 8: an old file takes a long text now.
        let long = "word ".repeat(1500);
        conn.execute(
            "INSERT INTO facts (id, space, kind, text, text_hash, source_kind, source_agent, author, created_at, valid_from)
             VALUES ('f_long', 'health', 'fact', ?1, ?2, 'agent', 'agent:x', 'x', 1700000500, 1700000500)",
            params![long, text_hash(&long)],
        )
        .unwrap();
        // A normal write works on it.
        let s = health.clone();
        let made = crate::facts::remember(
            &conn,
            &s,
            "me",
            crate::facts::NewFact::agent("A new line after the upgrade.", "agent:t"),
            &[],
        )
        .unwrap();
        assert!(matches!(made, crate::facts::Remembered::Created(_)));
        let check: String = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(check, "ok");
        // Opening again changes nothing (idempotent).
        drop(conn);
        let again = brain.open_space(&health).unwrap();
        assert_eq!(version(&again), latest);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn a_control_database_made_by_any_older_version_upgrades_and_keeps_its_connections() {
    let latest = CONTROL_MIGRATIONS.len();
    let token = "hcp_abcd_0123456789abcdef0123456789abcdef";
    for n in 1..=latest {
        let dir = temp(&format!("control-{n}"));
        let brain = Brain::init(&dir, &[Space::new("work").unwrap()], Language::English).unwrap();
        let path = dir.join("control.sqlite");
        remove_db(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
                .unwrap();
            migrate(&conn, &CONTROL_MIGRATIONS[..n]).unwrap();
            assert_eq!(version(&conn), n);
            conn.execute("INSERT INTO meta (k, v) VALUES ('kind', 'control')", [])
                .unwrap();
            // What the first version stored: a space, a connection with its grant and one in the audit.
            conn.execute(
                "INSERT INTO spaces (name, created_at) VALUES ('work', 1700000000)",
                [],
            )
            .unwrap();
            if n >= 2 {
                // From the second version on, a space has members: the owner reaches it.
                conn.execute(
                    "INSERT OR IGNORE INTO space_members (space, user_id, can_write, owner) VALUES ('work', 'owner', 1, 1)",
                    [],
                )
                .unwrap();
            }
            conn.execute(
                "INSERT INTO connections (id, agent, hash, all_spaces, created_at) VALUES ('c1', 'old-agent', ?1, 0, 1700000000)",
                [access::hash_token(token)],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO grants (connection_id, space, can_read, can_write) VALUES ('c1', 'work', 1, 1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO auth_audit (ts, agent, event) VALUES (1700000000, 'old-agent', 'connect')",
                [],
            )
            .unwrap();
        }
        let conn = brain.open_control().unwrap();
        assert_eq!(version(&conn), latest, "from version {n}");
        let caller = access::authenticate(&conn, token, 1_900_000_000)
            .unwrap()
            .unwrap_or_else(|| panic!("from version {n}: the old key no longer opens"));
        assert_eq!(caller.agent, "old-agent");
        assert_eq!(
            caller.user_id, "owner",
            "from version {n}: the connection belongs to the owner"
        );
        let work = Space::new("work").unwrap();
        assert!(
            caller.can_read(&work) && caller.can_write(&work),
            "from version {n}: its rights"
        );
        let audit: i64 = conn
            .query_row(
                "SELECT count(*) FROM auth_audit WHERE agent = 'old-agent'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(audit >= 1);
        // The space it held is still registered and opens.
        assert!(brain.has_space(&work).unwrap());
        let _ = brain.open_space(&work).unwrap();
        let check: String = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(check, "ok");
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn a_database_newer_than_the_program_is_refused_and_left_alone() {
    let dir = temp("newer");
    let brain = Brain::init(&dir, &[Space::new("work").unwrap()], Language::English).unwrap();
    let path = dir.join(Space::new("work").unwrap().file_name());
    {
        let conn = Connection::open(&path).unwrap();
        let future = i64::try_from(SPACE_MIGRATIONS.len() + 1).unwrap();
        conn.pragma_update(None, "user_version", future).unwrap();
    }
    let refused = brain.open_space(&Space::new("work").unwrap());
    assert!(refused.is_err());
    let conn = Connection::open(&path).unwrap();
    assert_eq!(version(&conn), SPACE_MIGRATIONS.len() + 1, "left as it was");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_hostile_schema_cannot_call_unsafe_functions_when_a_database_is_opened() {
    // A database that came from elsewhere (adopt, merge, a restored backup) may carry views and triggers of its own.
    // With the schema trusted they may call any function; opened as the program opens it, they may not.
    let dir = temp("hostile");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("x.sqlite");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE t (a); CREATE VIEW v AS SELECT fts3_tokenizer('simple') AS r FROM t;",
        )
        .unwrap();
        conn.execute("INSERT INTO t VALUES (1)", []).unwrap();
    }
    let conn = crate::brain::open_db(&path).unwrap();
    let r: rusqlite::Result<i64> = conn.query_row("SELECT count(*) FROM v", [], |r| r.get(0));
    assert!(
        r.as_ref().is_err_and(|e| e.to_string().contains("unsafe")),
        "{r:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The control migrations before the one that renamed `bridge_pairings` to `pairings` (`CONTROL_V22`).
const BEFORE_RENAME: usize = 21;

/// The pairing table was called `bridge_pairings`: a database made before the rename keeps every code, used or waiting,
/// and the waiting code still opens a connection.
#[test]
fn pairings_made_under_the_old_table_name_are_kept_and_still_redeemed() {
    let dir = temp("pairings-rename");
    let brain = Brain::init(&dir, &[Space::new("work").unwrap()], Language::English).unwrap();
    let path = dir.join("control.sqlite");
    remove_db(&path);
    let code = "ABCD2345";
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .unwrap();
        // The program as it was before the pairing table was renamed: the table under its old name.
        migrate(&conn, &CONTROL_MIGRATIONS[..BEFORE_RENAME]).unwrap();
        assert_eq!(version(&conn), BEFORE_RENAME);
        conn.execute("INSERT INTO meta (k, v) VALUES ('kind', 'control')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO spaces (name, created_at) VALUES ('work', 1700000000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO space_members (space, user_id, can_write, owner) VALUES ('work', 'owner', 1, 1)",
            [],
        )
        .unwrap();
        let rows = [
            (crate::pairing::hash(code), None::<i64>),
            (vec![9u8; 32], Some(1_700_000_100)),
        ];
        for (hash, used) in rows {
            conn.execute(
                "INSERT INTO bridge_pairings (hash, user_id, rights, created_at, expires_at, used_at, agent)
                 VALUES (?1, 'owner', 'work/inbox:rw', 1700000000, 4000000000, ?2, ?3)",
                params![hash, used, used.map(|_| "old-device")],
            )
            .unwrap();
        }
    }
    let conn = brain.open_control().unwrap();
    assert_eq!(version(&conn), CONTROL_MIGRATIONS.len());
    let has = |name: &str| -> bool {
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    };
    assert!(has("pairings") && !has("bridge_pairings"));
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM pairings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 2, "every row is kept");
    let used: String = conn
        .query_row(
            "SELECT agent FROM pairings WHERE used_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(used, "old-device");
    // The waiting code opens a connection, once.
    let paired = crate::pairing::redeem(&conn, code, "phone", 1_800_000_000).unwrap();
    assert_eq!(paired.spaces, vec!["work".to_string()]);
    assert!(crate::pairing::redeem(&conn, code, "phone", 1_800_000_001).is_err());
    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The kind of a connection was not stored: a connection named `peer-…` was taken for a machine whatever made it. The
/// upgrade marks a machine only what the node recorded making as one (a pairing code's row, a `paired` or `joined` in
/// the audit, followed through its renames); a connection that only has the name is an assistant from then on.
#[test]
fn connections_get_their_kind_from_what_made_them_never_from_their_name() {
    let dir = temp("connection-kinds");
    let brain = Brain::init(&dir, &[Space::new("work").unwrap()], Language::English).unwrap();
    let path = dir.join("control.sqlite");
    remove_db(&path);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .unwrap();
        migrate(&conn, &CONTROL_MIGRATIONS[..24]).unwrap();
        conn.execute("INSERT INTO meta (k, v) VALUES ('kind', 'control')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO spaces (name, created_at) VALUES ('work', 1700000000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO space_members (space, user_id, can_write, owner) VALUES ('work', 'owner', 1, 1)",
            [],
        )
        .unwrap();
        for (agent, console) in [
            ("peer-paired-ab12", 0),
            ("study-mac", 0),
            ("peer-fake", 0),
            ("oauth-owner-x", 0),
            ("console-owner", 1),
        ] {
            conn.execute(
                "INSERT INTO connections (id, agent, hash, all_spaces, created_at, user_id, console)
                 VALUES (?1, ?1, ?2, 1, 1700000000, 'owner', ?3)",
                params![agent, access::hash_token(&format!("key-{agent}")), console],
            )
            .unwrap();
        }
        // A pairing code's row names the connection it made.
        conn.execute(
            "INSERT INTO pairings (hash, user_id, rights, created_at, expires_at, used_at, agent)
             VALUES (?1, 'owner', 'all', 1700000000, 1700000600, 1700000100, 'peer-paired-ab12')",
            [vec![7u8; 32]],
        )
        .unwrap();
        // A join recorded in the audit, then a rename by the person: `study-mac` was `peer-joined-cd34`.
        conn.execute_batch(
            "INSERT INTO auth_audit (ts, agent, ip, event, detail) VALUES (1700000200, 'peer-joined-cd34', NULL, 'joined', 'Mac');
             INSERT INTO auth_audit (ts, agent, ip, event, detail) VALUES (1700000300, 'study-mac', NULL, 'rename', 'peer-joined-cd34');
             INSERT INTO oauth_clients (client_id, name, redirect_uris, created_at) VALUES ('c1', 'x', '[]', 1700000000);
             INSERT INTO oauth_refresh (hash, client_id, user_id, agent, created_at) VALUES (X'01', 'c1', 'owner', 'oauth-owner-x', 1700000000);",
        )
        .unwrap();
    }
    let conn = brain.open_control().unwrap();
    assert_eq!(version(&conn), CONTROL_MIGRATIONS.len());
    let kind = |agent: &str| -> String {
        conn.query_row(
            "SELECT kind FROM connections WHERE agent = ?1",
            [agent],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(kind("peer-paired-ab12"), "peer");
    assert_eq!(kind("study-mac"), "peer");
    assert_eq!(kind("peer-fake"), "assistant");
    assert_eq!(kind("oauth-owner-x"), "oauth");
    assert_eq!(kind("console-owner"), "console");
    let caller = |agent: &str| {
        access::authenticate(&conn, &format!("key-{agent}"), 1_800_000_000)
            .unwrap()
            .unwrap()
    };
    assert!(caller("peer-paired-ab12").is_peer());
    assert!(caller("study-mac").is_peer());
    assert!(!caller("peer-fake").is_peer(), "a name proves nothing");
    assert!(!caller("oauth-owner-x").is_peer());
    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
}
