//! Schemas and migrations. The database version is `PRAGMA user_version`;
//! migration number N (from 1) is `MIGRATIONS[N - 1]`.

/// Migrations of a space file.
pub(crate) const SPACE_MIGRATIONS: &[&str] = &[
    SPACE_V1, SPACE_V2, SPACE_V3, SPACE_V4, SPACE_V5, SPACE_V6, SPACE_V7, SPACE_V8,
];

/// Migrations of the control database (`control.sqlite`).
pub(crate) const CONTROL_MIGRATIONS: &[&str] = &[
    CONTROL_V1,
    CONTROL_V2,
    CONTROL_V3,
    CONTROL_V4,
    CONTROL_V5,
    CONTROL_V6,
    CONTROL_V7,
    CONTROL_V8,
    CONTROL_V9,
    CONTROL_V10,
    CONTROL_V11,
    CONTROL_V12,
    CONTROL_V13,
    CONTROL_V14,
    CONTROL_V15,
    CONTROL_V16,
    CONTROL_V17,
];

pub const SPACE_SCHEMA_VERSION: u32 = SPACE_MIGRATIONS.len() as u32;
pub const CONTROL_SCHEMA_VERSION: u32 = CONTROL_MIGRATIONS.len() as u32;

const SPACE_V1: &str = r"
CREATE TABLE meta (k TEXT PRIMARY KEY, v TEXT NOT NULL) STRICT;

-- A fact, a decision, a piece of context, or a passage imported from a note.
CREATE TABLE facts (
  rowid        INTEGER PRIMARY KEY,
  id           TEXT NOT NULL UNIQUE,
  space        TEXT NOT NULL,
  kind         TEXT NOT NULL CHECK (kind IN ('fact','decision','context','passage')),
  type         TEXT,
  fields       TEXT,
  text         TEXT NOT NULL CHECK (length(text) BETWEEN 1 AND 4000),
  text_hash    TEXT NOT NULL,
  source_kind  TEXT NOT NULL CHECK (source_kind IN ('note','agent')),
  source_path  TEXT,
  source_hash  TEXT,
  source_range TEXT,
  source_ref   TEXT,
  source_agent TEXT,
  author       TEXT NOT NULL,
  created_at   INTEGER NOT NULL,
  valid_from   INTEGER NOT NULL,
  valid_to     INTEGER,
  replaces     TEXT REFERENCES facts(id),
  replaced_by  TEXT REFERENCES facts(id),
  status       TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('proposed','active','replaced','retired')),
  status_at    INTEGER,
  status_by    TEXT,
  status_reason TEXT,
  CHECK (source_kind <> 'note'  OR (source_path IS NOT NULL AND source_hash IS NOT NULL)),
  CHECK (source_kind <> 'agent' OR source_agent IS NOT NULL),
  CHECK ((status = 'replaced') = (replaced_by IS NOT NULL)),
  CHECK (valid_to IS NULL OR valid_to >= valid_from)
) STRICT;

CREATE UNIQUE INDEX facts_active_hash ON facts(text_hash) WHERE status = 'active';
CREATE UNIQUE INDEX facts_active_ref  ON facts(source_ref) WHERE status = 'active' AND source_ref IS NOT NULL;
CREATE INDEX facts_status ON facts(status, created_at);
CREATE INDEX facts_path   ON facts(source_path);

-- Append-only history of every fact.
CREATE TABLE fact_events (
  id       INTEGER PRIMARY KEY,
  fact_id  TEXT NOT NULL REFERENCES facts(id),
  at       INTEGER NOT NULL,
  actor    TEXT NOT NULL,
  action   TEXT NOT NULL CHECK (action IN ('create','supersede','retire','repoint','set_validity','import_replace','import_retire')),
  detail   TEXT NOT NULL
) STRICT;
CREATE TRIGGER fact_events_no_update BEFORE UPDATE ON fact_events BEGIN SELECT RAISE(ABORT,'append-only'); END;
CREATE TRIGGER fact_events_no_delete BEFORE DELETE ON fact_events BEGIN SELECT RAISE(ABORT,'append-only'); END;

CREATE VIRTUAL TABLE facts_fts USING fts5(stem, tokenize = 'unicode61 remove_diacritics 2');

-- Notes already imported (path, digest), to import again only what changed.
CREATE TABLE notes (
  path TEXT PRIMARY KEY, hash TEXT NOT NULL, mtime INTEGER NOT NULL,
  imported_at INTEGER NOT NULL, blocks INTEGER NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('imported','missing'))
) STRICT;

CREATE TABLE audit (
  id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, client TEXT NOT NULL, tool TEXT NOT NULL,
  ok INTEGER NOT NULL, fact_ids TEXT, args_hash TEXT, detail TEXT
) STRICT;

CREATE TABLE embeddings (
  fact_id TEXT PRIMARY KEY REFERENCES facts(id), model TEXT NOT NULL, dim INTEGER NOT NULL, vec BLOB NOT NULL
) STRICT;

-- Documents a device sends (a full export, then numbered stream updates).
CREATE TABLE device_docs (
  id TEXT PRIMARY KEY, received_at INTEGER NOT NULL, schema TEXT NOT NULL,
  stream TEXT, sha TEXT NOT NULL, body TEXT NOT NULL
) STRICT;
CREATE INDEX device_docs_sha ON device_docs(sha);
CREATE TABLE device_streams (
  stream TEXT PRIMARY KEY, sequence INTEGER NOT NULL, base TEXT,
  current TEXT NOT NULL, updated_at INTEGER NOT NULL
) STRICT;

-- Proposals agents leave for the device; the person accepts or refuses them there.
CREATE TABLE outbox (
  id TEXT PRIMARY KEY, kind TEXT NOT NULL, created_at INTEGER NOT NULL,
  state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','accepted','refused')),
  author TEXT NOT NULL, content TEXT NOT NULL, answered_at INTEGER
) STRICT;
";

/// The health record: what a person (or an agent on their word) records about their
/// health, besides what devices measure. One row per entry; nothing is deleted:
/// an entry is resolved, stopped or retracted. Mapped to FHIR at export only.
const SPACE_V2: &str = r"
CREATE TABLE record (
  id TEXT PRIMARY KEY,
  -- encounter (consultation, admission), procedure (surgery, exam), test_result (lab, imaging),
  -- medication, allergy, condition (problem, diagnosis), request (a question for a clinician), note
  kind TEXT NOT NULL CHECK (kind IN ('encounter','procedure','test_result','medication','allergy','condition','request','note')),
  label TEXT NOT NULL CHECK (length(label) BETWEEN 1 AND 300),
  detail TEXT NOT NULL DEFAULT '' CHECK (length(detail) <= 2000),
  start TEXT, end TEXT,
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','resolved','stopped','retracted')),
  -- for conditions and allergies: an agent only proposes, the person confirms
  verification TEXT NOT NULL DEFAULT 'confirmed' CHECK (verification IN ('confirmed','unconfirmed','refuted')),
  author TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, reason TEXT
) STRICT;
CREATE INDEX record_kind ON record(kind, status);
";

/// Files kept in a space (photos, documents); the bytes are in `files/<space>/<sha256>`.
const SPACE_V3: &str = r"
CREATE TABLE files (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, mime TEXT NOT NULL, size INTEGER NOT NULL,
  sha256 TEXT NOT NULL, note TEXT NOT NULL DEFAULT '', fact_id TEXT REFERENCES facts(id),
  author TEXT NOT NULL, created_at INTEGER NOT NULL,
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','retired')), retired_reason TEXT
) STRICT;
CREATE INDEX files_status ON files(status, created_at);
";

/// What devices send, stored once: an item that comes back in the next export
/// is a reference, not a copy. `stored` says how a document's body reads.
const SPACE_V4: &str = r"
CREATE TABLE device_items (sha TEXT PRIMARY KEY, body TEXT NOT NULL) STRICT, WITHOUT ROWID;
ALTER TABLE device_docs ADD COLUMN stored TEXT NOT NULL DEFAULT 'raw' CHECK (stored IN ('raw','items'));
";

const CONTROL_V1: &str = r"
CREATE TABLE meta (k TEXT PRIMARY KEY, v TEXT NOT NULL) STRICT;

CREATE TABLE spaces (
  name TEXT PRIMARY KEY, created_at INTEGER NOT NULL
) STRICT;

-- One connection = one agent identity + one token. Only the token digest is kept.
-- `all_spaces` = read and write every space, present and future (the owner).
-- `upstream` is no longer read or written (the server proxies nothing); old rows keep their value
-- and the column goes at the next clean break (see docs/MODEL.md, Breaking change policy).
CREATE TABLE connections (
  id TEXT PRIMARY KEY, agent TEXT NOT NULL UNIQUE,
  hash BLOB NOT NULL UNIQUE,
  all_spaces INTEGER NOT NULL DEFAULT 0,
  upstream TEXT,
  created_at INTEGER NOT NULL, expires_at INTEGER, revoked_at INTEGER, last_used_at INTEGER, last_ip TEXT,
  read_only_forced INTEGER NOT NULL DEFAULT 0,
  rate_per_min INTEGER, writes_per_day INTEGER, max_text INTEGER
) STRICT;

CREATE TABLE grants (
  connection_id TEXT NOT NULL REFERENCES connections(id), space TEXT NOT NULL REFERENCES spaces(name),
  can_read INTEGER NOT NULL, can_write INTEGER NOT NULL, PRIMARY KEY (connection_id, space)
) STRICT;

CREATE TABLE auth_audit (ts INTEGER NOT NULL, agent TEXT, ip TEXT, event TEXT NOT NULL, detail TEXT) STRICT;

-- A device (phone app, sensor bridge) that pushes documents into one space.
CREATE TABLE devices (
  id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, space TEXT NOT NULL REFERENCES spaces(name),
  state TEXT NOT NULL CHECK (state IN ('active','revoked')),
  token_hash BLOB NOT NULL UNIQUE,
  created_at INTEGER NOT NULL, revoked_at INTEGER, last_used_at INTEGER, last_ip TEXT,
  rate_per_min INTEGER
) STRICT;
";

/// People, their spaces and the groups that share spaces. Everything that existed
/// before belongs to the bootstrap administrator `owner`.
const CONTROL_V2: &str = r"
CREATE TABLE users (
  id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, display TEXT NOT NULL DEFAULT '',
  role TEXT NOT NULL CHECK (role IN ('admin','member')),
  -- The Home Assistant user behind it (ingress), if any.
  ha_user TEXT UNIQUE,
  created_at INTEGER NOT NULL, disabled_at INTEGER
) STRICT;

-- Direct access of a person to a space. `owner`: created it (may share it).
CREATE TABLE space_members (
  space TEXT NOT NULL REFERENCES spaces(name), user_id TEXT NOT NULL REFERENCES users(id),
  can_write INTEGER NOT NULL, owner INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (space, user_id)
) STRICT;

-- A group shares a set of spaces with its members (a household: home, finances).
CREATE TABLE groups (name TEXT PRIMARY KEY, created_at INTEGER NOT NULL) STRICT;
CREATE TABLE group_spaces (
  grp TEXT NOT NULL REFERENCES groups(name), space TEXT NOT NULL REFERENCES spaces(name),
  PRIMARY KEY (grp, space)
) STRICT;
CREATE TABLE group_members (
  grp TEXT NOT NULL REFERENCES groups(name), user_id TEXT NOT NULL REFERENCES users(id),
  can_write INTEGER NOT NULL, PRIMARY KEY (grp, user_id)
) STRICT;

ALTER TABLE connections ADD COLUMN user_id TEXT REFERENCES users(id);
-- A console token opens the admin page only, never MCP.
ALTER TABLE connections ADD COLUMN console INTEGER NOT NULL DEFAULT 0;
ALTER TABLE devices ADD COLUMN user_id TEXT REFERENCES users(id);

INSERT INTO users (id, name, display, role, created_at) VALUES ('owner', 'owner', 'Owner', 'admin', CAST(strftime('%s','now') AS INTEGER));
INSERT INTO space_members (space, user_id, can_write, owner) SELECT name, 'owner', 1, 1 FROM spaces;
UPDATE connections SET user_id = 'owner';
UPDATE devices SET user_id = 'owner';
";

/// OAuth 2.1 for remote MCP clients (Claude on the web): registered clients,
/// one-time authorization codes and refresh tokens (digests only).
const CONTROL_V3: &str = r"
CREATE TABLE oauth_clients (
  client_id TEXT PRIMARY KEY, name TEXT NOT NULL, redirect_uris TEXT NOT NULL, created_at INTEGER NOT NULL
) STRICT;
CREATE TABLE oauth_codes (
  hash BLOB PRIMARY KEY, client_id TEXT NOT NULL REFERENCES oauth_clients(client_id),
  user_id TEXT NOT NULL REFERENCES users(id), redirect_uri TEXT NOT NULL, challenge TEXT NOT NULL,
  expires_at INTEGER NOT NULL, used INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE TABLE oauth_refresh (
  hash BLOB PRIMARY KEY, client_id TEXT NOT NULL REFERENCES oauth_clients(client_id),
  user_id TEXT NOT NULL REFERENCES users(id), agent TEXT NOT NULL,
  created_at INTEGER NOT NULL, revoked_at INTEGER
) STRICT;
";

/// One-time pairing codes for devices (a QR code shown on the admin page): only the
/// digest of the code is kept, it expires and it is claimed once.
const CONTROL_V4: &str = r"
CREATE TABLE pairing_codes (
  hash BLOB PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id), space TEXT NOT NULL,
  created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, used_at INTEGER
) STRICT;
";

/// What a person chose on the consent page, kept with the authorization code until the
/// token is issued: the vaults (`health:rw,work:r`), whether the upstream tools (home,
/// mail, finances) were ticked, and whether the connection is read-only. An empty
/// `rights` (a code issued before this existed) grants nothing.
const CONTROL_V5: &str = r"
ALTER TABLE oauth_codes ADD COLUMN rights TEXT NOT NULL DEFAULT '';
ALTER TABLE oauth_codes ADD COLUMN upstream INTEGER NOT NULL DEFAULT 0;
ALTER TABLE oauth_codes ADD COLUMN read_only INTEGER NOT NULL DEFAULT 0;
";

/// A Home Assistant login the installation does not know yet waits here for an
/// administrator, instead of becoming a person at once. Approving creates the person and
/// removes the row; refusing keeps it, so the same login does not ask again every time.
const CONTROL_V6: &str = r"
CREATE TABLE person_requests (
  ha_user TEXT PRIMARY KEY, name TEXT NOT NULL, display TEXT NOT NULL DEFAULT '',
  first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','refused'))
) STRICT;
";

/// Tags: a short hierarchical label on a fact (`work/dev/ci`), for organising inside a space.
/// A tag matches itself and its descendants; `@space` says that a fact also concerns another
/// space, which makes it findable from there and grants no access.
const SPACE_V5: &str = r"
CREATE TABLE fact_tags (
  fact_id TEXT NOT NULL REFERENCES facts(id),
  tag TEXT NOT NULL,
  PRIMARY KEY (fact_id, tag)
) STRICT, WITHOUT ROWID;
CREATE INDEX fact_tags_tag ON fact_tags(tag, fact_id);
";

/// Where a fact came from in an app's own terms: the app, the id the app gave that record, and
/// the instant of the data point. A source that sends the same id again changes nothing (or
/// replaces its own earlier version), and its latest `at` is the point to resume from.
const SPACE_V6: &str = r"
CREATE TABLE fact_origin (
  fact_id TEXT PRIMARY KEY REFERENCES facts(id),
  app TEXT NOT NULL,
  ext_id TEXT NOT NULL,
  at TEXT NOT NULL,
  layer TEXT NOT NULL DEFAULT 'source' CHECK (layer IN ('source','dossier'))
) STRICT;
CREATE INDEX fact_origin_key ON fact_origin(app, ext_id);
CREATE INDEX fact_origin_at ON fact_origin(app, at);
";

/// The kind of a vault (from the catalogue). A vault that existed before kinds, or one an
/// operator named by hand, is `custom`, and carries no built-in tools.
const CONTROL_V7: &str = r"
ALTER TABLE spaces ADD COLUMN kind TEXT NOT NULL DEFAULT 'custom';
";

/// Guides: the standing instructions of a person, for their assistants. One per vault and one
/// for the person, written only through the admin page (no tool writes one). A new text
/// never overwrites: the old row stays, no longer current. Clearing leaves no current row.
const CONTROL_V8: &str = r"
CREATE TABLE guides (
  id INTEGER PRIMARY KEY,
  scope TEXT NOT NULL,
  body TEXT NOT NULL CHECK (length(body) BETWEEN 1 AND 16384),
  author TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  current INTEGER NOT NULL DEFAULT 1,
  ended_at INTEGER
) STRICT;
CREATE UNIQUE INDEX guides_current ON guides(scope) WHERE current = 1;
CREATE INDEX guides_scope ON guides(scope, id);
";

/// A grant limited to a path inside a vault (`health/sources/bevel:rw`): the connection reads and
/// writes only the facts filed under that tag path. The vault's own `grants` row says the
/// connection reaches it at all; these rows narrow it.
const CONTROL_V9: &str = r"
CREATE TABLE grant_paths (
  connection_id TEXT NOT NULL REFERENCES connections(id),
  space TEXT NOT NULL,
  path TEXT NOT NULL,
  can_write INTEGER NOT NULL,
  PRIMARY KEY (connection_id, space, path)
) STRICT, WITHOUT ROWID;
";

/// Not losing a person's memory with their account. A person may name an heir, who can attach the
/// memory to another account when the person has not been seen for a long while; and may make a
/// recovery key, whose digest is kept, with which a new account takes the memory over. `last_seen`
/// is the last time they opened their page.
const CONTROL_V10: &str = r"
ALTER TABLE users ADD COLUMN heir TEXT;
ALTER TABLE users ADD COLUMN recovery_hash BLOB;
ALTER TABLE users ADD COLUMN last_seen INTEGER;
";

/// The guide an app keeps for what is written for it: one document per connection, proposed by the
/// app through its own tool. It is not a fact: no search finds it, it sits in no layer. It is only
/// data until the person reads it on their page and approves it.
const SPACE_V7: &str = r"
CREATE TABLE app_guides (
  app TEXT PRIMARY KEY,
  text TEXT NOT NULL CHECK (length(text) BETWEEN 1 AND 4000),
  updated_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;
";

/// What the person approved of those guides. An approval holds for the text as it was read (its
/// digest): if the app changes it, it waits again.
const CONTROL_V11: &str = r"
CREATE TABLE guide_approvals (
  space TEXT NOT NULL,
  app TEXT NOT NULL,
  body_hash TEXT NOT NULL,
  approved_by TEXT NOT NULL,
  approved_at INTEGER NOT NULL,
  PRIMARY KEY (space, app)
) STRICT, WITHOUT ROWID;
";

/// The name a connector was given by its address (`/mcp/<name>`), kept with the authorization code
/// until the token is issued: the connection is then called that. Empty: named after the client.
const CONTROL_V12: &str = r"
ALTER TABLE oauth_codes ADD COLUMN agent TEXT NOT NULL DEFAULT '';
";

/// A bridge paired for « all my vaults »: it writes to the inbox of each, and of the ones the person makes later.
const CONTROL_V15: &str = r"
ALTER TABLE connections ADD COLUMN all_inboxes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE bridge_pairings ADD COLUMN all_inboxes INTEGER NOT NULL DEFAULT 0;
";

/// A connection the person's own guide is not told to (a work assistant, on an account others may read).
const CONTROL_V14: &str = r"
ALTER TABLE connections ADD COLUMN no_person_guide INTEGER NOT NULL DEFAULT 0;
";

/// Pairing a bridge: a short code shown on the person's page, exchanged once by a device for a
/// connection of its own. Only the hash of the code is kept; the rights are a spec (`Rights::parse`).
const CONTROL_V13: &str = r"
CREATE TABLE bridge_pairings (
  hash BLOB PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id),
  rights TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  used_at INTEGER,
  agent TEXT
) STRICT;
";

/// No size limit on what the person keeps (2026-11-07: the person asked to lift every
/// limit). SQLite cannot drop a CHECK with ALTER TABLE; its documented way for a change that leaves
/// the stored rows as they are is to edit the table's text in `sqlite_schema` (ALTER TABLE, § « Making Other
/// Kinds Of Table Schema Changes »). `migrate` raises `schema_version` after it, so every connection reloads it.
const SPACE_V8: &str = r"
PRAGMA writable_schema = ON;
UPDATE sqlite_schema SET sql = replace(sql, 'CHECK (length(text) BETWEEN 1 AND 4000)', 'CHECK (length(text) >= 1)')
  WHERE type = 'table' AND name IN ('facts', 'app_guides');
UPDATE sqlite_schema SET sql = replace(replace(sql,
    'CHECK (length(label) BETWEEN 1 AND 300)', 'CHECK (length(label) >= 1)'),
    ' CHECK (length(detail) <= 2000)', '')
  WHERE type = 'table' AND name = 'record';
PRAGMA writable_schema = OFF;
";

/// The person's guides have no size limit either (see `SPACE_V8`).
const CONTROL_V16: &str = r"
PRAGMA writable_schema = ON;
UPDATE sqlite_schema SET sql = replace(sql, 'CHECK (length(body) BETWEEN 1 AND 16384)', 'CHECK (length(body) >= 1)')
  WHERE type = 'table' AND name = 'guides';
PRAGMA writable_schema = OFF;
";

/// How a person lays out their spaces: a label to show (the name never changes: it is the file and
/// what rights refer to), a parent space and a position among its siblings.
const CONTROL_V17: &str = r"
ALTER TABLE spaces ADD COLUMN label TEXT;
ALTER TABLE spaces ADD COLUMN parent TEXT;
ALTER TABLE spaces ADD COLUMN position INTEGER NOT NULL DEFAULT 0;
";
