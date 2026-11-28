#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_access::{AccessError, Accounts};

#[test]
fn alias_rules_reject_reserved_invalid_and_taken_names() {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Accounts::open(dir.path()).unwrap();
    accounts.account_create("first").unwrap();
    accounts.account_create("second").unwrap();
    for name in [
        "mcp",
        "admin",
        "authorize",
        "token",
        "register",
        "healthz",
        "oauth",
        "static",
        "assets",
        "api",
        "well-known",
        "www",
    ] {
        let error = accounts.set_alias("first", name).unwrap_err();
        assert!(error.to_string().contains("reserved"));
    }
    for name in [
        "",
        "a",
        "-aa",
        "aa-",
        "Mixed",
        "a_b",
        "a/b",
        "été",
        "abcdefghijklmnopqrstuvwxyz1234567",
    ] {
        assert!(matches!(
            accounts.set_alias("first", name),
            Err(AccessError::Invalid(_))
        ));
    }
    accounts.set_alias("first", "example-42").unwrap();
    let error = accounts.set_alias("second", "example-42").unwrap_err();
    assert!(error.to_string().contains("taken"));
    assert_eq!(accounts.alias_of("first").unwrap(), "example-42");
    assert_eq!(accounts.account_by_alias("example-42").unwrap().id, "first");
    accounts.set_alias("first", "renamed").unwrap();
    assert!(matches!(
        accounts.account_by_alias("example-42"),
        Err(AccessError::NotFound)
    ));
    drop(accounts);
    let reopened = Accounts::open(dir.path()).unwrap();
    assert_eq!(reopened.alias_of("first").unwrap(), "renamed");
}

#[test]
fn a_new_ha_user_gets_a_default_alias_from_their_name() {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Accounts::open(dir.path()).unwrap();
    let first = accounts
        .account_for_ha_user("test-user-a", "  Été / Démo  ")
        .unwrap();
    let second = accounts
        .account_for_ha_user("test-user-b", "Été Démo")
        .unwrap();
    let fallback = accounts.account_for_ha_user("test-user-c", "☃").unwrap();
    assert_eq!(accounts.alias_of(&first.id).unwrap(), "ete-demo");
    assert_eq!(accounts.alias_of(&second.id).unwrap(), "ete-demo-2");
    assert_eq!(accounts.alias_of(&fallback.id).unwrap(), fallback.id);
    accounts
        .account_for_ha_user("test-user-a", "Different name")
        .unwrap();
    assert_eq!(accounts.alias_of(&first.id).unwrap(), "ete-demo");
}

#[test]
fn the_alias_migration_backfills_existing_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Accounts::open(dir.path()).unwrap();
    let ha = accounts
        .account_for_ha_user("fixture-ha", "Été Démo")
        .unwrap();
    accounts.account_create("manual").unwrap();
    let (_, key) = accounts.connection_create_owner(&ha.id, "Owner").unwrap();
    drop(accounts);

    let db = rusqlite::Connection::open(dir.path().join("access.db")).unwrap();
    db.execute_batch("DROP TABLE account_aliases; PRAGMA user_version = 2;")
        .unwrap();
    drop(db);

    let accounts = Accounts::open(dir.path()).unwrap();
    assert_eq!(accounts.alias_of(&ha.id).unwrap(), "ete-demo");
    assert_eq!(accounts.alias_of("manual").unwrap(), "manual");
    assert_eq!(accounts.authenticate(key.as_str()).unwrap().account, ha.id);
}
