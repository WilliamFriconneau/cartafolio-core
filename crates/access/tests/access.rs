#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_access::{AccessError, Accounts, Grant};
use tempfile::TempDir;

fn setup() -> (TempDir, Accounts) {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Accounts::open(dir.path()).unwrap();
    let account = accounts.account_create("demo").unwrap();
    account.store.space_create("aa", "A", None).unwrap();
    account
        .store
        .space_create("ab", "Child", Some("aa"))
        .unwrap();
    account.store.space_create("bb", "B", None).unwrap();
    (dir, accounts)
}

fn grant(space: &str) -> Grant {
    Grant {
        space: space.into(),
        write: true,
    }
}

#[test]
fn a_key_is_shown_once_and_only_its_hash_is_stored() {
    let (dir, accounts) = setup();
    let (connection, key) = accounts.connection_create_owner("demo", "Owner").unwrap();
    assert_eq!(key.as_str().len(), 67);
    assert!(key
        .as_str()
        .strip_prefix("pk_")
        .unwrap()
        .bytes()
        .all(|b| b.is_ascii_hexdigit()));
    assert!(!format!("{key:?}").contains(key.as_str()));
    assert_eq!(accounts.connections("demo").unwrap(), vec![connection]);
    let db = rusqlite::Connection::open(dir.path().join("access.db")).unwrap();
    let hash: Vec<u8> = db
        .query_row("SELECT key_hash FROM connections", [], |r| r.get(0))
        .unwrap();
    assert_eq!(hash.len(), 32);
    drop(accounts);
    let reopened = Accounts::open(dir.path()).unwrap();
    assert!(reopened.authenticate(key.as_str()).unwrap().can_write("bb"));
}

#[test]
fn a_grant_covers_children() {
    let (_dir, accounts) = setup();
    let (_, key) = accounts
        .connection_create("demo", "Writer", vec![grant("aa")], false)
        .unwrap();
    let principal = accounts.authenticate(key.as_str()).unwrap();
    assert!(principal.can_read("ab"));
    assert!(principal.can_write("ab"));
    assert!(!principal.can_read("bb"));
}

#[test]
fn a_read_only_connection_can_never_write() {
    let (_dir, accounts) = setup();
    let (_, key) = accounts
        .connection_create("demo", "Reader", vec![grant("*")], true)
        .unwrap();
    let principal = accounts.authenticate(key.as_str()).unwrap();
    assert!(principal.can_read("ab"));
    assert!(!principal.can_write("ab"));
    assert!(!principal.can_write_root());
}

#[test]
fn a_revoked_key_stops_working_at_once() {
    let (_dir, accounts) = setup();
    let (connection, key) = accounts.connection_create_owner("demo", "Owner").unwrap();
    assert!(accounts.authenticate(key.as_str()).is_ok());
    accounts.connection_revoke("demo", &connection.id).unwrap();
    assert!(matches!(
        accounts.authenticate(key.as_str()),
        Err(AccessError::Unauthorized)
    ));
    assert!(accounts.connections("demo").unwrap().is_empty());
}
