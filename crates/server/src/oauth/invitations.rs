use super::{hash, invitation_code, INVITE_ATTEMPT_LIMIT, INVITE_TTL};
use rusqlite::{OptionalExtension, Transaction};

pub(super) enum Failure {
    Rejected,
    Limited(i64),
}

pub(super) fn migrate(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    let has_selector = has_column(tx, "selector")?;
    if has_selector {
        tx.execute_batch(
            "DELETE FROM oauth_invites WHERE selector IS NOT NULL;
             DROP INDEX IF EXISTS oauth_invite_selector;
             ALTER TABLE oauth_invites DROP COLUMN selector;",
        )?;
    }
    if !has_column(tx, "failed_attempts")? {
        tx.execute_batch(
            "ALTER TABLE oauth_invites ADD COLUMN failed_attempts INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    if !has_column(tx, "rights")? {
        tx.execute_batch("ALTER TABLE oauth_invites ADD COLUMN rights TEXT;")?;
    }
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS oauth_invite_failures (failed_at INTEGER NOT NULL);",
    )
}

fn has_column(tx: &Transaction<'_>, name: &str) -> rusqlite::Result<bool> {
    tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('oauth_invites') WHERE name = ?1)",
        [name],
        |row| row.get(0),
    )
}

type Invitation = (String, Option<String>);

pub(super) fn consume(
    tx: &Transaction<'_>,
    submitted: &str,
) -> rusqlite::Result<Result<Invitation, Failure>> {
    // 31^12 is about 2^59. At most five guesses per ten minutes reach the code check.
    // An attacker can delay a legitimate sign-in, but the limiter gives no way to complete one.
    let now: i64 = tx.query_row("SELECT unixepoch()", [], |row| row.get(0))?;
    tx.execute(
        "DELETE FROM oauth_invite_failures WHERE failed_at <= ?1",
        [now - INVITE_TTL],
    )?;
    let (count, first): (u32, Option<i64>) = tx.query_row(
        "SELECT COUNT(*), MIN(failed_at) FROM oauth_invite_failures",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if let Some(first) = first.filter(|_| count >= INVITE_ATTEMPT_LIMIT) {
        return Ok(Err(Failure::Limited(first + INVITE_TTL - now)));
    }
    if let Some(normalized) = invitation_code::normalize(submitted) {
        let digest = hash(&normalized);
        let invitation = tx
            .query_row(
                "DELETE FROM oauth_invites WHERE hash = ?1 AND expires_at > ?2
             AND failed_attempts < ?3 RETURNING account, rights",
                (digest, now, INVITE_ATTEMPT_LIMIT),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some(invitation) = invitation {
            return Ok(Ok(invitation));
        }
        // Only an exact hash match can identify an invitation for its own counter.
        tx.execute(
            "UPDATE oauth_invites SET failed_attempts = failed_attempts + 1
             WHERE hash = ?1 AND failed_attempts < ?2",
            (digest, INVITE_ATTEMPT_LIMIT),
        )?;
    }
    tx.execute("INSERT INTO oauth_invite_failures VALUES (?1)", [now])?;
    Ok(Err(Failure::Rejected))
}
