//! Import of a Markdown folder. A fictitious vault generated in a temporary directory.

mod common;

use std::error::Error;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::{Res, Temp, brain, sp};
use memcp_core::facts::{self, RecallOptions};
use memcp_core::import::{self, ImportOptions, ImportRules, chunk, date_from_name};
use memcp_core::{Brain, Space};

const RULES: &str = "
# fictitious vault
map Work = work
map Health = health
map Home = home
default = personal
exclude = Archives
instructions = House rules
";

struct Env {
    t: Temp,
    vault: PathBuf,
    brain: Brain,
    rules: ImportRules,
}

impl Drop for Env {
    fn drop(&mut self) {
        // A 000 file would get in the way of the removal.
        let _ = chmod_all(&self.vault);
        let _ = &self.t;
    }
}

fn chmod_all(dir: &Path) -> std::io::Result<()> {
    for e in fs::read_dir(dir)? {
        let p = e?.path();
        let _ = fs::set_permissions(
            &p,
            fs::Permissions::from_mode(if p.is_dir() { 0o755 } else { 0o644 }),
        );
        if p.is_dir() {
            chmod_all(&p)?;
        }
    }
    Ok(())
}

fn env() -> Result<Env, Box<dyn Error>> {
    let t = Temp::new("import");
    let vault = t.path().join("vault");
    fs::create_dir_all(&vault)?;
    let brain = brain(&t)?;
    Ok(Env {
        t,
        vault,
        brain,
        rules: ImportRules::parse(RULES)?,
    })
}

fn put(e: &Env, rel: &str, content: &str) -> Result<(), Box<dyn Error>> {
    let p = e.vault.join(rel);
    fs::create_dir_all(p.parent().ok_or("parent")?)?;
    fs::write(p, content)?;
    Ok(())
}

fn import_with(e: &Env, opts: &ImportOptions) -> memcp_core::Result<import::Report> {
    import::import(&e.brain, &e.vault, &e.rules, opts)
}

fn run(e: &Env, dry: bool) -> Result<import::Report, Box<dyn Error>> {
    Ok(import_with(
        e,
        &ImportOptions {
            dry_run: dry,
            yes: !dry,
            ..Default::default()
        },
    )?)
}

fn count(e: &Env, space: &str, sql: &str) -> Result<i64, Box<dyn Error>> {
    Ok(e.brain
        .open_space(&sp(space))?
        .query_row(sql, [], |r| r.get(0))?)
}

fn passages(e: &Env, space: &str) -> Result<i64, Box<dyn Error>> {
    count(
        e,
        space,
        "SELECT count(*) FROM facts WHERE kind='passage' AND status='active'",
    )
}

fn recall_in(e: &Env, space: &Space, q: &str) -> Result<Vec<facts::Hit>, Box<dyn Error>> {
    let opts = RecallOptions {
        limit: 5,
        ..Default::default()
    };
    Ok(facts::recall(
        &e.brain.open_space(space)?,
        q,
        &opts,
        2_000_000_000,
    )?)
}

const NOTE: &str = "---\ntags: [test]\ncreated: 2026-10-01\n---\n# Sam\n\nSam cooks on Sundays for the whole family.\n\n## Projects\n\nMoon Bakery: redo the kitchen worktop before Christmas.\n\n## Groceries\n\nMilk\n";

// --- rules ---------------------------------------------------------------------------------

#[test]
fn rules_route_by_the_longest_folder_prefix() -> Res {
    let r = ImportRules::parse(
        "map Journal = personal\nmap Journal/Health = health\ndefault = personal\n",
    )?;
    assert_eq!(r.space_for("Journal/Health/2026-10-30.md"), sp("health"));
    assert_eq!(r.space_for("Journal/2026-10-30.md"), sp("personal"));
    assert_eq!(
        r.space_for("journal/health/x.md"),
        sp("health"),
        "folded comparison"
    );
    assert_eq!(r.space_for("Elsewhere/x.md"), sp("personal"));
    Ok(())
}

#[test]
fn rules_require_a_default_and_valid_lines() {
    assert!(
        ImportRules::parse("map Work = work\n").is_err(),
        "no default"
    );
    assert!(ImportRules::parse("default = personal\nnonsense\n").is_err());
    assert!(ImportRules::parse("default = Not A Space\n").is_err());
    assert!(ImportRules::parse("default = personal\ncolour = blue\n").is_err());
}

#[test]
fn rules_naming_a_missing_space_are_refused_before_reading() -> Res {
    let e = env()?;
    put(
        &e,
        "Garden/Roses.md",
        "# Roses\n\nThe roses are pruned in March every year.\n",
    )?;
    let rules = ImportRules::parse("map Garden = garden\ndefault = personal\n")?;
    let opts = ImportOptions {
        yes: true,
        ..Default::default()
    };
    assert!(import::import(&e.brain, &e.vault, &rules, &opts).is_err());
    Ok(())
}

// --- chunking ------------------------------------------------------------------------------

#[test]
fn chunking_follows_headings_and_drops_the_frontmatter() {
    let blocks = chunk("General/Sam.md", NOTE);
    let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
    assert!(
        texts
            .iter()
            .all(|t| !t.contains("tags:") && !t.contains("created:")),
        "{texts:?}"
    );
    assert!(texts[0].starts_with("Sam — Sam cooks"), "{texts:?}");
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("Projects — Moon Bakery"))
    );
    // "Groceries" (4-character body) is too short: joined to the previous block.
    assert!(
        texts.last().is_some_and(|t| t.contains("Milk")),
        "{texts:?}"
    );
    assert_eq!(blocks[0].reference, "General/Sam.md#Sam/1");
    // Line numbers of the file (frontmatter included).
    assert!(blocks[0].range.starts_with("5-"), "{}", blocks[0].range);
}

#[test]
fn links_images_comments_and_code_are_handled() {
    let md = "# Links\n\nSee [[Other note|this note]] and [[Page#section]] or [a site](https://x.example) ![[photo.png]] ![alt](a.png) end. <!-- hidden --> %%also%% still visible, one more sentence.\n\n```\n# not a heading\n```\n\nText after the code, long enough to stand on its own.\n";
    let all = chunk("a.md", md)
        .iter()
        .map(|b| b.text.clone())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(all.contains("this note") && !all.contains("[["), "{all}");
    assert!(all.contains("Page") && !all.contains("#section"), "{all}");
    assert!(all.contains("a site") && !all.contains("https"), "{all}");
    assert!(
        !all.contains("photo.png") && !all.contains("a.png"),
        "{all}"
    );
    assert!(!all.contains("hidden") && !all.contains("also"), "{all}");
    assert!(
        all.contains("# not a heading"),
        "code is not cut as a heading: {all}"
    );
}

#[test]
fn a_long_paragraph_is_split_and_repeated_headings_get_distinct_references() {
    let long = "word ".repeat(720); // 3600 characters
    let md = format!(
        "# A\n\n{long}\n\n# A\n\nSecond section with the same heading, long enough to exist.\n"
    );
    let blocks = chunk("b.md", &md);
    assert!(blocks.len() >= 3, "{}", blocks.len());
    assert!(
        blocks.iter().all(|b| b.text.chars().count() <= 1600),
        "bounded blocks"
    );
    let refs: std::collections::BTreeSet<_> = blocks.iter().map(|b| b.reference.clone()).collect();
    assert_eq!(refs.len(), blocks.len(), "unique references: {refs:?}");
}

// --- import --------------------------------------------------------------------------------

#[test]
fn dry_run_writes_nothing_then_a_real_run_needs_a_dry_run_or_yes() -> Res {
    let e = env()?;
    put(&e, "General/Sam.md", NOTE)?;
    let opts = |dry, yes| ImportOptions {
        dry_run: dry,
        yes,
        ..Default::default()
    };
    assert!(
        import_with(&e, &opts(false, false)).is_err(),
        "no recent dry run"
    );
    let r = import_with(&e, &opts(true, false))?;
    assert!(r.dry_run && r.created > 0 && r.read == 1);
    assert_eq!(passages(&e, "personal")?, 0, "dry run: nothing written");
    assert_eq!(count(&e, "personal", "SELECT count(*) FROM notes")?, 0);
    let r = import_with(&e, &opts(false, false))?;
    assert!(r.created > 0, "accepted after the dry run");
    assert_eq!(passages(&e, "personal")?, i64::try_from(r.created)?);
    Ok(())
}

#[test]
fn notes_go_to_the_space_of_their_folder() -> Res {
    let e = env()?;
    put(
        &e,
        "Work/Meeting.md",
        "# Meeting\n\nMinutes of the steering committee about this week's schedule.\n",
    )?;
    put(
        &e,
        "Health/Exercise.md",
        "# Exercise\n\nForty-minute walk, steady heart rate the whole time.\n",
    )?;
    put(
        &e,
        "Home/Boiler.md",
        "# Boiler\n\nYearly boiler service planned in November with the technician.\n",
    )?;
    put(
        &e,
        "Money/Budget.md",
        "# Budget\n\nThe November budget keeps a hundred euros for leisure.\n",
    )?;
    let r = run(&e, false)?;
    assert_eq!(r.created, 4);
    for (space, word) in [
        ("work", "committee"),
        ("health", "walk"),
        ("home", "boiler"),
        ("personal", "budget"),
    ] {
        assert_eq!(passages(&e, space)?, 1, "{space}");
        let hits = recall_in(&e, &sp(space), word)?;
        assert_eq!(hits.len(), 1, "{space} / {word}");
        assert_eq!(hits[0].fact.kind, "passage");
        assert_eq!(hits[0].fact.author, "import");
        assert!(hits[0].fact.source_path.is_some() && hits[0].fact.source_ref.is_some());
    }
    assert!(
        recall_in(&e, &sp("personal"), "committee")?.is_empty(),
        "no copy elsewhere"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn protected_files_are_never_read() -> Res {
    let e = env()?;
    let mut e = e;
    e.rules = ImportRules::parse(&format!("{RULES}protect = bank card\n"))?;
    let protected = [
        (
            "Notes/Secrets.md",
            "# Secrets\n\nwifi password: toosecret123 and door code 4567.\n",
        ),
        (
            "Notes/Door codes.md",
            "# Codes\n\nDoor A: 1234 and gate B: 5678 for the garage.\n",
        ),
        (
            "Notes/BANK CARD details.md",
            "# Card\n\nContent never to be read at all.\n",
        ),
        (
            "General/Router password.md",
            "# Router\n\nAdmin login and password of the house router.\n",
        ),
    ];
    for (f, c) in protected {
        put(&e, f, c)?;
    }
    put(
        &e,
        "General/Ordinary.md",
        "# Ordinary\n\nAn ordinary note that must be imported without any problem.\n",
    )?;
    // Unreadable: if the importer opened them, it would fail.
    for (f, _) in protected {
        fs::set_permissions(e.vault.join(f), fs::Permissions::from_mode(0o000))?;
    }
    let r = run(&e, false)?;
    assert_eq!(r.protected, 4, "{r}");
    assert_eq!(r.created, 1);
    for secret in ["toosecret123", "4567", "5678", "router"] {
        let n: i64 = e.brain.open_space(&sp("personal"))?.query_row(
            "SELECT count(*) FROM facts WHERE text LIKE ?1",
            [format!("%{secret}%")],
            |r| r.get(0),
        )?;
        assert_eq!(n, 0, "{secret} must not be stored");
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn exclusions_instructions_symlinks_and_big_files_kept() -> Res {
    let e = env()?;
    put(
        &e,
        "Archives/Old.md",
        "# Old\n\nAn old archived note that must not be imported at all.\n",
    )?;
    put(
        &e,
        "General/Journal/2026-10-30.md",
        "# Day\n\nDaily journal of the assistant, imported and dated by its name.\n",
    )?;
    put(
        &e,
        ".obsidian/notes.md",
        "# Config\n\nEditor settings of no interest for the agents' memory.\n",
    )?;
    put(
        &e,
        "Home/CLAUDE.md",
        "# Instructions\n\nThese are instructions for agents, not memory of the house.\n",
    )?;
    put(
        &e,
        "Home/House rules.md",
        "# Rules\n\nThe house rules are agent rules, not memory passages.\n",
    )?;
    put(&e, "General/Text.txt", "not markdown")?;
    put(
        &e,
        "General/Huge.md",
        &format!("# Big\n\n{}\n", "x".repeat(1_100_000)),
    )?;
    put(
        &e,
        "General/Ok.md",
        "# Ok\n\nAn ordinary note, long enough to become a memory passage.\n",
    )?;
    std::os::unix::fs::symlink("/etc/hostname", e.vault.join("General/Link.md"))?;
    let r = run(&e, false)?;
    assert_eq!(r.excluded, 2, "{r}");
    assert_eq!(r.instructions, 2, "{r}");
    assert_eq!(r.too_big, 0, "no file is too big");
    assert_eq!(r.created, 3, "{r}");
    let n = count(
        &e,
        "personal",
        "SELECT count(*) FROM facts WHERE source_path LIKE '%Link.md'",
    )?;
    assert_eq!(n, 0, "a symlink is not followed");
    Ok(())
}

#[test]
fn identical_passages_and_idempotence() -> Res {
    let e = env()?;
    put(&e, "General/A.md", NOTE)?;
    assert!(run(&e, false)?.created >= 2);
    let r2 = run(&e, false)?;
    assert_eq!(
        (r2.created, r2.replaced, r2.retired, r2.repointed),
        (0, 0, 0, 0),
        "{r2}"
    );
    assert_eq!(r2.unchanged, 1);
    // An identical copy elsewhere: duplicate skipped, nothing added.
    put(&e, "General/Copy.md", NOTE)?;
    let r3 = run(&e, false)?;
    assert_eq!(r3.created, 0, "{r3}");
    assert!(r3.duplicates >= 2, "{r3}");
    Ok(())
}

#[test]
fn a_changed_note_replaces_a_vanished_block_retires_and_the_history_is_complete() -> Res {
    let e = env()?;
    put(
        &e,
        "General/A.md",
        "# Title\n\nFirst paragraph long enough to stand alone in a passage.\n\n## Next\n\nSecond paragraph, also long enough to be a passage.\n",
    )?;
    run(&e, false)?;
    // The first one changes, the "Next" section goes.
    put(
        &e,
        "General/A.md",
        "# Title\n\nFirst paragraph CHANGED, still long enough to stand alone in a passage.\n",
    )?;
    let r = run(&e, false)?;
    assert_eq!((r.replaced, r.retired, r.created), (1, 1, 0), "{r}");
    let c = e.brain.open_space(&sp("personal"))?;
    let statuses: Vec<(String, Option<String>)> = c
        .prepare("SELECT status, status_reason FROM facts ORDER BY created_at, rowid")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    assert!(statuses.iter().any(|(s, _)| s == "replaced"));
    assert!(
        statuses
            .iter()
            .any(|(s, why)| s == "retired" && why.as_deref() == Some("source_removed"))
    );
    let actions: Vec<String> = c
        .prepare("SELECT action FROM fact_events ORDER BY id")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    assert!(
        actions.contains(&"import_replace".into()) && actions.contains(&"import_retire".into()),
        "{actions:?}"
    );
    let n: i64 = c.query_row(
        "SELECT count(*) FROM facts WHERE text LIKE '%Second paragraph%'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(n, 1, "old texts stay readable");
    Ok(())
}

#[test]
fn a_moved_note_keeps_its_passages_and_a_deleted_one_retires_them() -> Res {
    let e = env()?;
    put(&e, "General/Old.md", NOTE)?;
    let r1 = run(&e, false)?;
    let before = passages(&e, "personal")?;
    assert_eq!(before, i64::try_from(r1.created)?);
    fs::create_dir_all(e.vault.join("Ideas"))?;
    fs::rename(e.vault.join("General/Old.md"), e.vault.join("Ideas/New.md"))?;
    let r2 = run(&e, false)?;
    assert_eq!((r2.created, r2.retired), (0, 0), "{r2}");
    assert_eq!(r2.repointed, r1.created, "{r2}");
    assert_eq!(passages(&e, "personal")?, before, "same passages, active");
    let c = e.brain.open_space(&sp("personal"))?;
    let paths: Vec<String> = c
        .prepare("SELECT DISTINCT source_path FROM facts WHERE status='active'")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    assert_eq!(paths, vec!["Ideas/New.md".to_string()]);
    let state: String = c.query_row(
        "SELECT state FROM notes WHERE path='General/Old.md'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(state, "missing");

    fs::remove_file(e.vault.join("Ideas/New.md"))?;
    let r3 = run(&e, false)?;
    assert_eq!(r3.missing_notes, 1, "{r3}");
    assert_eq!(passages(&e, "personal")?, 0);
    let retired: i64 = c.query_row(
        "SELECT count(*) FROM facts WHERE status='retired' AND status_reason='source_removed'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(retired, before);
    Ok(())
}

#[test]
fn a_note_that_becomes_protected_leaves_the_memory() -> Res {
    let e = env()?;
    put(
        &e,
        "General/Wifi.md",
        "# Wifi\n\nThe house network covers every room of the ground floor.\n",
    )?;
    run(&e, false)?;
    assert_eq!(passages(&e, "personal")?, 1);
    fs::rename(
        e.vault.join("General/Wifi.md"),
        e.vault.join("General/Wifi password.md"),
    )?;
    let r = run(&e, false)?;
    assert_eq!(r.protected, 1, "{r}");
    assert_eq!(passages(&e, "personal")?, 0, "{r}");
    Ok(())
}

#[test]
fn the_only_scope_limits_the_import_and_the_vanished_notes_check() -> Res {
    let e = env()?;
    put(
        &e,
        "Home/A.md",
        "# A\n\nA note about the house, long enough to become a passage.\n",
    )?;
    put(
        &e,
        "General/B.md",
        "# B\n\nA personal note, long enough to become a passage as well.\n",
    )?;
    let only_home = ImportOptions {
        only: Some("Home".into()),
        yes: true,
        ..Default::default()
    };
    assert_eq!(import_with(&e, &only_home)?.created, 1);
    assert_eq!(passages(&e, "personal")?, 0);
    // A full import then an import limited to Home does not touch what is out of scope.
    run(&e, false)?;
    fs::remove_file(e.vault.join("General/B.md"))?;
    let r = import_with(&e, &only_home)?;
    assert_eq!(r.missing_notes, 0, "{r}");
    assert_eq!(passages(&e, "personal")?, 1, "B is out of scope");
    Ok(())
}

#[test]
fn extra_protections_add_to_the_built_in_list() -> Res {
    let e = env()?;
    put(
        &e,
        "General/Medical file.md",
        "# File\n\nContent the owner decided never to import into memory.\n",
    )?;
    let r = import_with(
        &e,
        &ImportOptions {
            yes: true,
            extra_protected: vec!["Medical file".into()],
            ..Default::default()
        },
    )?;
    assert_eq!((r.protected, r.created), (1, 0), "{r}");
    Ok(())
}

// --- secrets in the content ----------------------------------------------------------------

#[test]
fn secrets_are_detected_in_the_text() {
    for secret in [
        "-----BEGIN OPENSSH PRIVATE KEY-----",
        "The password: toosecret123 for the router",
        "password=Hunter2024x",
        "Wifi password: abc12345",
        "token: 9f8a7b6c5d4e",
        "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9abc",
        "The key is AKIAIOSFODNN7EXAMPLE1234 do not lose it",
        "Le mot de passe : tropsecret123 pour le routeur",
        "jeton = 12ab34cd56ef78",
    ] {
        assert!(import::looks_secret(secret), "must be detected: {secret}");
    }
    for normal in [
        "The password is changed in the router settings.",
        "The session token expires after an hour, see the docs.",
        "Meeting on 29/10 at 2 pm with Dr Martin about the schedule.",
        "See https://example.org/a/very/long/path/with/digits1234567890/here",
        "The secret of the recipe: lots of butter.",
        "Version 2026.10.4 of the core, kernel 7.2.10-1-generic, python 3.14.",
    ] {
        assert!(
            !import::looks_secret(normal),
            "must not be detected: {normal}"
        );
    }
}

#[test]
fn a_passage_that_looks_like_a_secret_stays_out_of_memory() -> Res {
    let e = env()?;
    put(
        &e,
        "Home/Network.md",
        "# Network\n\nThe router is at 192.168.1.1 and the box in bridge mode since September.\n\n## Access\n\nRouter credentials, password: Zx9kLm2Qw8 never to be shared.\n",
    )?;
    let r = run(&e, false)?;
    assert_eq!((r.secrets_masked, r.created), (1, 1), "{r}");
    let n = count(
        &e,
        "home",
        "SELECT count(*) FROM facts WHERE text LIKE '%Zx9kLm2Qw8%'",
    )?;
    assert_eq!(n, 0);
    // A secret added later to an imported note: that passage is left out and the old one retired.
    put(
        &e,
        "Home/Network.md",
        "# Network\n\nThe router is at 192.168.1.1 and the box in bridge mode since September. token: ab12cd34ef56gh78\n",
    )?;
    let r = run(&e, false)?;
    assert_eq!((r.secrets_masked, r.retired), (1, 1), "{r}");
    assert_eq!(passages(&e, "home")?, 0);
    Ok(())
}

#[test]
fn a_note_dated_by_its_name_carries_its_date() {
    assert_eq!(
        date_from_name("Health/Journal/2026-10-30.md"),
        Some(1_793_318_400)
    );
    assert_eq!(
        date_from_name("Work/2026-10-26 Review.md"),
        Some(1_792_972_800)
    );
    assert_eq!(date_from_name("1970-01-01.md"), Some(0));
    assert_eq!(date_from_name("Health/Home page.md"), None);
    assert_eq!(date_from_name("2026-13-01.md"), None);
}
