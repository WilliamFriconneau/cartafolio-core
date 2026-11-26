//! The folder of a space on a node: setting, cloud flag, both directions of the mirror, conflicts, deletions, the
//! read-only mode, loop protection, permissions, and that the setting never travels.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{Res, Temp, brain, sp};
use pensine_core::access::{self, Rights};
use pensine_core::facts::{self, NewFact, Remembered};
use pensine_core::folders::{self, Mode, Options, Report};
use pensine_core::library;
use pensine_core::tools::{self, ToolError};
use pensine_core::users::OWNER;
use pensine_core::{Brain, Space, files, util};
use serde_json::json;

const NOW: Options = Options { settle_secs: 0 };

fn pass(b: &Brain, s: &Space) -> Result<Report, Box<dyn std::error::Error>> {
    Ok(folders::sync_space(b, s, util::now(), NOW)?)
}

fn later() {
    std::thread::sleep(std::time::Duration::from_millis(30));
}

fn note(
    b: &Brain,
    s: &Space,
    text: &str,
    tags: &[&str],
) -> Result<String, Box<dyn std::error::Error>> {
    let conn = b.open_space(s)?;
    let mut n = NewFact::agent(text, "agent:tester");
    n.tags = tags.iter().map(|t| t.to_string()).collect();
    match facts::remember(&conn, s, "tester", n, &[])? {
        Remembered::Created(f) => Ok(f.id),
        other => Err(format!("not created: {other:?}").into()),
    }
}

fn md_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    v.sort();
    v
}

fn active(b: &Brain, s: &Space) -> Result<Vec<facts::Fact>, Box<dyn std::error::Error>> {
    let conn = b.open_space(s)?;
    let mut st = conn.prepare("SELECT id FROM facts WHERE status = 'active' AND type IS NULL")?;
    let ids: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let mut out = vec![];
    for id in ids {
        out.extend(facts::read_fact(&conn, &id)?);
    }
    Ok(out)
}

fn find_text(
    b: &Brain,
    s: &Space,
    needle: &str,
) -> Result<Option<facts::Fact>, Box<dyn std::error::Error>> {
    Ok(active(b, s)?.into_iter().find(|f| f.text.contains(needle)))
}

fn setup(t: &Temp, mode: Mode) -> Result<(Brain, Space, PathBuf), Box<dyn std::error::Error>> {
    let b = brain(t)?;
    let s = sp("home");
    let dir = t.path().join("mirror").join("home-notes");
    folders::set(&b, &s, Some(&dir), mode, util::now())?;
    Ok((b, s, dir))
}

// ---- pure helpers ---------------------------------------------------------------------------------------------

#[test]
fn slug_short_id_and_hash_are_stable() {
    assert_eq!(
        folders::slug("Boiler serviced in March!"),
        "boiler-serviced-in-march"
    );
    assert_eq!(folders::slug("Énergie : été 2026"), "energie-ete-2026");
    assert_eq!(folders::slug("???"), "note");
    assert!(folders::slug(&"word ".repeat(40)).len() <= 48);
    assert!(!folders::slug("a  b").contains("--"));
    assert_eq!(folders::short_id("f_AB12cd34ef56"), "ab12cd");
    assert_eq!(
        folders::hash_bytes(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn front_matter_is_read_in_its_three_shapes_and_a_plain_file_is_all_body() {
    let p =
        folders::parse_note("---\nid: f_x1\ntags: [a/b, c]\n---\n\nHello there.\nSecond line.\n");
    assert_eq!(p.id.as_deref(), Some("f_x1"));
    assert_eq!(p.tags, Some(vec!["a/b".to_string(), "c".to_string()]));
    assert_eq!(p.body.trim(), "Hello there.\nSecond line.");
    let list = folders::parse_note("---\ntags:\n  - one\n  - \"two\"\n---\nBody");
    assert_eq!(list.tags, Some(vec!["one".to_string(), "two".to_string()]));
    let csv = folders::parse_note("---\ntags: x, y\n---\nBody\r\n");
    assert_eq!(csv.tags, Some(vec!["x".to_string(), "y".to_string()]));
    let plain = folders::parse_note("Just text\n---\nnot a front matter");
    assert_eq!((plain.id, plain.tags), (None, None));
    assert!(plain.body.starts_with("Just text"));
    // A front matter that is never closed is text.
    assert_eq!(folders::parse_note("---\nid: x\nno end").id, None);
    // No `tags` key: the note keeps its tags; an empty list is a list.
    assert_eq!(folders::parse_note("---\nid: x\n---\nB").tags, None);
    assert_eq!(
        folders::parse_note("---\ntags: []\n---\nB").tags,
        Some(vec![])
    );
}

#[test]
fn a_rendered_note_reads_back_the_same() -> Res {
    let t = Temp::new("folders-render");
    let b = brain(&t)?;
    let s = sp("home");
    let id = note(&b, &s, "Rendered text.", &["home/a", "home/b"])?;
    let f = facts::read_fact(&b.open_space(&s)?, &id)?.ok_or("fact")?;
    let md = folders::render_note(&f, f.created_at, 3);
    assert!(md.starts_with("---\nid: "));
    for key in [
        "space: home",
        "tags: [home/a, home/b]",
        "version: 3",
        "created: ",
    ] {
        assert!(md.contains(key), "{key} in {md}");
    }
    let p = folders::parse_note(&md);
    assert_eq!(p.id.as_deref(), Some(id.as_str()));
    assert_eq!(
        p.tags,
        Some(vec!["home/a".to_string(), "home/b".to_string()])
    );
    assert_eq!(p.body.trim(), "Rendered text.");
    Ok(())
}

#[test]
fn clouds_are_told_from_the_path() {
    let cases = [
        (
            "/Users/x/Library/CloudStorage/GoogleDrive-me@example.org/My Drive/notes",
            Some("google-drive"),
        ),
        ("G:\\My Drive\\notes", None), // not a unix path: the component is the whole string
        ("/home/x/Google Drive/notes", Some("google-drive")),
        (
            "/Users/x/Library/Mobile Documents/com~apple~CloudDocs/notes",
            Some("icloud"),
        ),
        ("/Users/x/iCloud Drive/notes", Some("icloud")),
        ("/Users/x/OneDrive/notes", Some("onedrive")),
        (
            "/Users/x/Library/CloudStorage/OneDrive-Contoso/notes",
            Some("onedrive"),
        ),
        ("/Users/x/OneDrive - Contoso/notes", Some("onedrive")),
        ("/home/x/Dropbox/notes", Some("dropbox")),
        ("/home/x/Dropbox (Team)/notes", Some("dropbox")),
        ("/home/x/Documents/notes", None),
        ("/home/x/dropbox-like-but-not/notes", Some("dropbox")),
        ("/home/x/my-drive-notes", None),
    ];
    for (p, want) in cases {
        if p.contains('\\') {
            continue;
        }
        assert_eq!(folders::cloud_of(Path::new(p)), want, "{p}");
    }
    assert!(
        folders::cloud_warning(Some("icloud"))
            .is_some_and(|w| w.contains("iCloud") && w.contains("device"))
    );
    assert!(folders::cloud_warning(None).is_none());
}

// ---- the setting ----------------------------------------------------------------------------------------------

#[test]
fn a_space_has_the_default_folder_until_one_is_chosen_and_can_go_back() -> Res {
    let t = Temp::new("folders-default");
    let b = brain(&t)?;
    let s = sp("home");
    let before = folders::info(&b, &s)?;
    assert_eq!(before["custom"], false);
    assert_eq!(before["mode"], json!(null));
    assert_eq!(before["path"], before["default_path"]);
    assert!(
        before["default_path"]
            .as_str()
            .ok_or("path")?
            .starts_with(b.dir().to_str().ok_or("dir")?)
    );
    assert!(
        folders::sync_space(&b, &s, util::now(), NOW).is_err(),
        "nothing to mirror by default"
    );

    let dir = t.path().join("elsewhere");
    let set = folders::set(&b, &s, Some(&dir), Mode::ReadWrite, util::now())?;
    assert_eq!(set["custom"], true);
    assert_eq!(set["mode"], "readwrite");
    assert_eq!(set["cloud"], json!(null));
    assert_eq!(set["warning"], json!(null));
    assert!(dir.is_dir(), "a missing folder is made");

    // Back to the default: the node stops mirroring, the folder is left as it is.
    note(&b, &s, "Kept in the folder.", &[])?;
    pass(&b, &s)?;
    assert_eq!(md_files(&dir).len(), 1);
    let back = folders::set(&b, &s, None, Mode::ReadWrite, util::now())?;
    assert_eq!(back["custom"], false);
    assert_eq!(md_files(&dir).len(), 1);
    assert!(folders::get(&b, &s)?.is_none());
    Ok(())
}

#[test]
fn a_folder_that_would_hurt_is_refused() -> Res {
    let t = Temp::new("folders-refuse");
    let b = brain(&t)?;
    let s = sp("home");
    assert!(folders::set(&b, &s, Some(Path::new("relative/dir")), Mode::ReadWrite, 1).is_err());
    assert!(
        folders::set(&b, &s, Some(b.dir()), Mode::ReadWrite, 1).is_err(),
        "the data directory"
    );
    assert!(folders::set(&b, &s, Some(&b.dir().join("inside")), Mode::ReadWrite, 1).is_err());
    let file = t.path().join("a-file");
    fs::write(&file, "x")?;
    assert!(folders::set(&b, &s, Some(&file), Mode::ReadWrite, 1).is_err());
    let one = t.path().join("shared");
    folders::set(&b, &s, Some(&one), Mode::ReadWrite, 1)?;
    // Another space cannot take the same folder, nor one inside or around it.
    let other = sp("work");
    assert!(folders::set(&b, &other, Some(&one), Mode::ReadWrite, 1).is_err());
    assert!(folders::set(&b, &other, Some(&one.join("sub")), Mode::ReadWrite, 1).is_err());
    assert!(folders::set(&b, &other, Some(t.path()), Mode::ReadWrite, 1).is_err());
    assert!(
        folders::set(
            &b,
            &sp("nowhere"),
            Some(&t.path().join("z")),
            Mode::ReadWrite,
            1
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn a_cloud_folder_is_allowed_flagged_and_works() -> Res {
    let t = Temp::new("folders-cloud");
    let b = brain(&t)?;
    let s = sp("home");
    let dir = t
        .path()
        .join("CloudStorage")
        .join("GoogleDrive-me@example.org")
        .join("My Drive")
        .join("notes");
    let set = folders::set(&b, &s, Some(&dir), Mode::ReadWrite, util::now())?;
    assert_eq!(set["cloud"], "google-drive");
    assert!(
        set["warning"]
            .as_str()
            .ok_or("warning")?
            .contains("Google Drive")
    );
    note(&b, &s, "Written into a drive folder.", &[])?;
    pass(&b, &s)?;
    assert_eq!(md_files(&dir).len(), 1, "read and write are allowed");
    assert_eq!(folders::info(&b, &s)?["cloud"], "google-drive");
    Ok(())
}

// ---- the engine -----------------------------------------------------------------------------------------------

#[test]
fn the_memory_is_shown_in_the_folder_and_what_the_node_wrote_is_not_read_back() -> Res {
    let t = Temp::new("folders-out");
    let (b, s, dir) = setup(&t, Mode::ReadWrite)?;
    let id = note(&b, &s, "Boiler serviced in March.", &["home/boiler"])?;
    files::put(&b, &s, "tester", "plan.txt", "", b"a plan of the house")?;
    let r = pass(&b, &s)?;
    assert_eq!(r.written, 2, "{r:?}");
    let mds = md_files(&dir);
    assert_eq!(mds.len(), 1);
    let name = mds[0]
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("name")?
        .to_string();
    assert_eq!(
        name,
        format!("boiler-serviced-in-march-{}.md", folders::short_id(&id))
    );
    let body = fs::read_to_string(&mds[0])?;
    assert!(
        body.contains(&format!("id: {id}"))
            && body.contains("tags: [home/boiler]")
            && body.contains("version: 1")
    );
    assert!(body.contains("Boiler serviced in March."));
    assert_eq!(
        fs::read(dir.join("files").join("plan.txt"))?,
        b"a plan of the house"
    );
    // The facts that describe files are not notes of the folder.
    assert_eq!(md_files(&dir).len(), 1);

    // Loop protection: what was written is not an edit. Nothing is written, nothing comes back, no new version.
    for _ in 0..3 {
        later();
        let again = pass(&b, &s)?;
        assert_eq!(
            (
                again.written,
                again.edited,
                again.new_notes,
                again.new_files,
                again.conflicts,
                again.retired
            ),
            (0, 0, 0, 0, 0, 0),
            "{again:?}"
        );
    }
    assert_eq!(facts::history(&b.open_space(&s)?, &id)?.len(), 1);
    let info = folders::info(&b, &s)?;
    assert_eq!(info["files"], 2);
    assert!(info["last_scan"].is_string());
    Ok(())
}

#[test]
fn an_edit_in_the_folder_becomes_a_new_version_by_folder_sync() -> Res {
    let t = Temp::new("folders-edit");
    let (b, s, dir) = setup(&t, Mode::ReadWrite)?;
    let id = note(&b, &s, "Boiler serviced in March.", &["home/boiler"])?;
    pass(&b, &s)?;
    let path = md_files(&dir).remove(0);
    later();
    let edited = fs::read_to_string(&path)?.replace("March", "April");
    fs::write(&path, edited)?;
    let r = pass(&b, &s)?;
    assert_eq!(r.edited, 1, "{r:?}");
    let conn = b.open_space(&s)?;
    let chain = facts::history(&conn, &id)?;
    assert_eq!(chain.len(), 2);
    let head = chain.last().ok_or("head")?;
    assert_eq!(head.text, "Boiler serviced in April.");
    assert_eq!(head.author, folders::AUTHOR);
    assert_eq!(head.tags, vec!["home/boiler".to_string()], "tags kept");
    assert_eq!(chain[0].status, "replaced");
    // Not read back as another change, and the front matter is the user's until the memory writes again.
    later();
    assert_eq!(pass(&b, &s)?.edited, 0);
    assert_eq!(facts::history(&conn, &id)?.len(), 2);

    // The tags of the front matter are the tags of the note; whitespace changes are no change.
    later();
    let again = fs::read_to_string(&path)?
        .replace("tags: [home/boiler]", "tags: [home/boiler, home/heating]");
    fs::write(&path, again)?;
    assert_eq!(pass(&b, &s)?.edited, 1);
    let now_head = find_text(&b, &s, "April")?.ok_or("head")?;
    assert_eq!(
        now_head.tags,
        vec!["home/boiler".to_string(), "home/heating".to_string()]
    );
    later();
    fs::write(
        &path,
        fs::read_to_string(&path)?.replace("April.", "April.\n\n\n"),
    )?;
    assert_eq!(
        pass(&b, &s)?.edited,
        0,
        "the same words are not a new version"
    );
    Ok(())
}

#[test]
fn a_new_file_in_the_folder_becomes_a_note_or_a_file_and_is_not_written_twice() -> Res {
    let t = Temp::new("folders-new");
    let (b, s, dir) = setup(&t, Mode::ReadWrite)?;
    fs::write(
        dir.join("shed.md"),
        "---\ntags: [home/ideas]\n---\n\nBuild a shed in the garden.\n",
    )?;
    fs::create_dir_all(dir.join("deep/er"))?;
    fs::write(dir.join("deep/er/plain.md"), "A note with no front matter.")?;
    fs::write(dir.join("scan.txt"), "an invoice as text")?;
    fs::write(dir.join(".hidden.md"), "never taken")?;
    fs::write(dir.join("half.md.part"), "being copied")?;
    fs::write(dir.join("empty.md"), "---\ntags: [a]\n---\n")?;
    let r = pass(&b, &s)?;
    assert_eq!((r.new_notes, r.new_files), (2, 1), "{r:?}");
    let shed = find_text(&b, &s, "shed")?.ok_or("shed")?;
    assert_eq!(shed.tags, vec!["home/ideas".to_string()]);
    assert_eq!(shed.author, folders::AUTHOR);
    assert!(find_text(&b, &s, "no front matter")?.is_some());
    assert!(
        find_text(&b, &s, "never taken")?.is_none() && find_text(&b, &s, "being copied")?.is_none()
    );
    let listed = files::list(&b, &s, None, 10)?;
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0].name.as_str(), listed[0].author.as_str()),
        ("scan.txt", folders::AUTHOR)
    );
    // The empty note is reported, once, and does not stop the pass.
    assert!(
        r.errors.iter().any(|e| e.contains("empty.md")),
        "{:?}",
        r.errors
    );
    assert!(
        folders::info(&b, &s)?["errors"]
            .as_array()
            .ok_or("errors")?
            .iter()
            .any(|e| e.as_str().is_some_and(|e| e.contains("empty.md")))
    );
    // They are already in the folder: nothing more is written for them.
    later();
    let again = pass(&b, &s)?;
    assert_eq!(
        (again.written, again.new_notes, again.new_files),
        (0, 0, 0),
        "{again:?}"
    );
    assert_eq!(
        md_files(&dir).len(),
        2 + 1,
        "shed.md, empty.md and nothing else at the top"
    );
    Ok(())
}

#[test]
fn a_note_written_in_the_memory_after_the_first_pass_comes_into_the_folder_and_a_change_follows()
-> Res {
    let t = Temp::new("folders-follow");
    let (b, s, dir) = setup(&t, Mode::ReadWrite)?;
    let id = note(&b, &s, "First wording.", &[])?;
    pass(&b, &s)?;
    let conn = b.open_space(&s)?;
    let newer = facts::supersede(
        &conn,
        &s,
        "tester",
        &id,
        NewFact::agent("Second wording.", "agent:tester"),
        Some("better"),
    )?;
    let r = pass(&b, &s)?;
    assert_eq!(r.written, 1, "{r:?}");
    let mds = md_files(&dir);
    assert_eq!(mds.len(), 1, "the same file is rewritten, not a second one");
    let body = fs::read_to_string(&mds[0])?;
    assert!(
        body.contains("Second wording.")
            && body.contains(&format!("id: {}", newer.id))
            && body.contains("version: 2")
    );
    let more = note(&b, &s, "A second note.", &[])?;
    pass(&b, &s)?;
    assert_eq!(md_files(&dir).len(), 2);
    assert!(fs::read_dir(&dir)?.flatten().any(|e| {
        e.file_name()
            .to_string_lossy()
            .ends_with(&format!("{}.md", folders::short_id(&more)))
    }));
    Ok(())
}

#[test]
fn a_note_changed_on_both_sides_keeps_both() -> Res {
    let t = Temp::new("folders-conflict");
    let (b, s, dir) = setup(&t, Mode::ReadWrite)?;
    let id = note(&b, &s, "Plan A for the roof.", &["home/roof"])?;
    pass(&b, &s)?;
    let path = md_files(&dir).remove(0);
    later();
    fs::write(
        &path,
        fs::read_to_string(&path)?.replace("Plan A", "Plan B from the folder"),
    )?;
    let conn = b.open_space(&s)?;
    facts::supersede(
        &conn,
        &s,
        "tester",
        &id,
        NewFact::agent("Plan C from the memory.", "agent:tester"),
        None,
    )?;
    let r = pass(&b, &s)?;
    assert_eq!(r.conflicts, 1, "{r:?}");
    // The memory's version stays the note; the folder's copy is a note of its own, tagged conflict, with its tags.
    let mem = find_text(&b, &s, "Plan C")?.ok_or("memory version")?;
    assert!(!mem.tags.contains(&"conflict".to_string()));
    let copy = find_text(&b, &s, "Plan B from the folder")?.ok_or("folder copy")?;
    assert!(
        copy.tags.contains(&"conflict".to_string()) && copy.tags.contains(&"home/roof".to_string()),
        "{:?}",
        copy.tags
    );
    // The file shows the memory's version again, and the copy has its own file.
    assert!(fs::read_to_string(&path)?.contains("Plan C from the memory."));
    assert_eq!(md_files(&dir).len(), 2);
    later();
    assert_eq!(pass(&b, &s)?.conflicts, 0);
    Ok(())
}

#[test]
fn a_deleted_file_retires_its_note_with_a_reason_and_never_the_reverse() -> Res {
    let t = Temp::new("folders-delete");
    let (b, s, dir) = setup(&t, Mode::ReadWrite)?;
    let gone = note(&b, &s, "To be deleted.", &[])?;
    let kept = note(&b, &s, "To be kept.", &[])?;
    let retired_here = note(&b, &s, "Retired in the memory.", &[])?;
    let file = files::put(&b, &s, "tester", "old.txt", "", b"old text")?;
    pass(&b, &s)?;
    for p in md_files(&dir) {
        if fs::read_to_string(&p)?.contains("To be deleted.") {
            fs::remove_file(p)?;
        }
    }
    fs::remove_file(dir.join("files").join("old.txt"))?;
    let conn = b.open_space(&s)?;
    facts::retire(&conn, "tester", &retired_here, "no longer true")?;
    let r = pass(&b, &s)?;
    assert_eq!(r.retired, 2, "{r:?}");
    let f = facts::read_fact(&conn, &gone)?.ok_or("fact")?;
    assert_eq!(f.status, "retired");
    assert_eq!(f.status_by.as_deref(), Some(folders::AUTHOR));
    assert!(
        f.status_reason
            .as_deref()
            .is_some_and(|r| r.contains("deleted from the folder"))
    );
    assert_eq!(
        facts::read_fact(&conn, &kept)?.ok_or("kept")?.status,
        "active"
    );
    assert_eq!(
        files::info(&b, &s, &file.id)?.ok_or("file")?.status,
        "retired"
    );
    // The text stays readable: nothing was erased.
    assert_eq!(f.text, "To be deleted.");
    // A note retired in the memory leaves its file where it is.
    assert!(
        md_files(&dir)
            .iter()
            .any(|p| fs::read_to_string(p).is_ok_and(|t| t.contains("Retired in the memory.")))
    );
    later();
    let again = pass(&b, &s)?;
    assert_eq!(
        (again.retired, again.written, again.new_notes),
        (0, 0, 0),
        "{again:?}"
    );
    Ok(())
}

#[test]
fn a_renamed_file_keeps_its_note() -> Res {
    let t = Temp::new("folders-rename");
    let (b, s, dir) = setup(&t, Mode::ReadWrite)?;
    let id = note(&b, &s, "A note that moves.", &[])?;
    pass(&b, &s)?;
    let from = md_files(&dir).remove(0);
    fs::create_dir_all(dir.join("archive"))?;
    fs::rename(&from, dir.join("archive").join("moved.md"))?;
    let r = pass(&b, &s)?;
    assert_eq!((r.retired, r.new_notes), (0, 0), "{r:?}");
    assert_eq!(
        facts::read_fact(&b.open_space(&s)?, &id)?
            .ok_or("fact")?
            .status,
        "active"
    );
    assert!(dir.join("archive").join("moved.md").is_file());
    Ok(())
}

#[test]
fn a_vanished_folder_or_a_mass_deletion_retires_nothing() -> Res {
    let t = Temp::new("folders-mass");
    let (b, s, dir) = setup(&t, Mode::ReadWrite)?;
    let ids: Vec<String> = [
        "apple orchard",
        "river crossing",
        "winter tyres",
        "garden shed",
        "broken window",
        "tax return",
    ]
    .iter()
    .map(|w| note(&b, &s, &format!("Remember the {w} for later."), &[]))
    .collect::<Result<_, _>>()?;
    pass(&b, &s)?;
    for p in md_files(&dir) {
        fs::remove_file(p)?;
    }
    let r = pass(&b, &s)?;
    assert_eq!(r.retired, 0);
    assert!(
        r.errors.iter().any(|e| e.contains("gone at once")),
        "{:?}",
        r.errors
    );
    let conn = b.open_space(&s)?;
    for id in &ids {
        assert_eq!(facts::read_fact(&conn, id)?.ok_or("fact")?.status, "active");
    }
    // The folder itself gone (a drive unmounted): an error, nothing changed.
    fs::remove_dir_all(&dir)?;
    assert!(folders::sync_space(&b, &s, util::now(), NOW).is_err());
    for id in &ids {
        assert_eq!(facts::read_fact(&conn, id)?.ok_or("fact")?.status, "active");
    }
    Ok(())
}

#[test]
fn a_file_the_node_did_not_make_is_never_overwritten() -> Res {
    let t = Temp::new("folders-keep");
    let (b, s, dir) = setup(&t, Mode::ReadWrite)?;
    files::put(&b, &s, "tester", "plan.txt", "", b"from the memory")?;
    fs::create_dir_all(dir.join("files"))?;
    fs::write(dir.join("files").join("plan.txt"), "the person's own file")?;
    // The person's file is taken as it is (a new file of the space), the memory's file gets another name.
    pass(&b, &s)?;
    assert_eq!(
        fs::read_to_string(dir.join("files").join("plan.txt"))?,
        "the person's own file"
    );
    let names: Vec<String> = fs::read_dir(dir.join("files"))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    Ok(())
}

#[test]
fn read_only_writes_nothing_either_way_and_the_library_search_still_works() -> Res {
    let t = Temp::new("folders-read");
    let b = brain(&t)?;
    let s = sp("home");
    let dir = t.path().join("vault");
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("Roof.md"), "# Roof\nWe need tiles.\n")?;
    note(&b, &s, "A note of the memory.", &[])?;
    let set = folders::set(&b, &s, Some(&dir), Mode::Read, util::now())?;
    assert_eq!(set["mode"], "read");
    let before: Vec<_> = fs::read_dir(&dir)?
        .flatten()
        .map(|e| e.file_name())
        .collect();
    let r = pass(&b, &s)?;
    assert_eq!(r, Report::default());
    let after: Vec<_> = fs::read_dir(&dir)?
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(before, after, "nothing was written in the folder");
    assert!(
        find_text(&b, &s, "tiles")?.is_none(),
        "nothing came into the memory"
    );
    // A change in a read-only folder is indexed, never a note.
    later();
    fs::write(dir.join("New.md"), "Fresh words about giraffes.")?;
    pass(&b, &s)?;
    assert!(find_text(&b, &s, "giraffes")?.is_none());
    let conn = b.open_space(&s)?;
    assert_eq!(library::search(&conn, "giraffes", 5)?.len(), 1);
    assert_eq!(library::search(&conn, "tiles", 5)?.len(), 1);
    // The library tools see it too.
    let ctl = b.open_control()?;
    let tok = access::connect_for(&ctl, OWNER, "tester", &Rights::owner(), None, false, false)?;
    let who = access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("refused")?;
    let hit = tools::call(&b, &who, "library_search", &json!({ "query": "tiles" }))?;
    assert_eq!(hit["results"][0]["space"], "home");
    Ok(())
}

#[test]
fn the_library_index_follows_the_one_setting_and_library_add_is_its_alias() -> Res {
    let t = Temp::new("folders-library");
    let b = brain(&t)?;
    let s = sp("home");
    let one = t.path().join("one");
    let two = t.path().join("two");
    fs::create_dir_all(&one)?;
    fs::create_dir_all(&two)?;
    fs::write(one.join("a.md"), "alpha words")?;
    fs::write(two.join("b.md"), "beta words")?;
    folders::set(&b, &s, Some(&one), Mode::Read, 1)?;
    pass(&b, &s)?;
    folders::set(&b, &s, Some(&two), Mode::Read, 2)?;
    pass(&b, &s)?;
    let conn = b.open_space(&s)?;
    assert_eq!(library::roots(&conn)?.len(), 1, "one folder, one index");
    assert!(library::search(&conn, "alpha", 5)?.is_empty());
    assert_eq!(library::search(&conn, "beta", 5)?.len(), 1);
    folders::set(&b, &s, None, Mode::Read, 3)?;
    assert!(library::roots(&conn)?.is_empty());
    Ok(())
}

// ---- rights and travel -----------------------------------------------------------------------------------------

/// A connection used by a program on the node's own machine (the standard input, or the loopback address).
fn caller(b: &Brain, name: &str, spec: &str) -> Result<access::Caller, Box<dyn std::error::Error>> {
    let mut c = remote_caller(b, name, spec)?;
    c.local = true;
    Ok(c)
}

/// The same connection used from another machine (a cloud assistant, a laptop on the network).
fn remote_caller(
    b: &Brain,
    name: &str,
    spec: &str,
) -> Result<access::Caller, Box<dyn std::error::Error>> {
    let ctl = b.open_control()?;
    let tok = access::connect_for(&ctl, OWNER, name, &Rights::parse(spec)?, None, false, false)?;
    Ok(access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("refused")?)
}

#[test]
fn the_tools_follow_the_rights_of_the_connection() -> Res {
    let t = Temp::new("folders-tools");
    let b = brain(&t)?;
    let dir = t.path().join("tool-folder");
    let writer = caller(&b, "writer", "home:rw")?;
    let reader = caller(&b, "reader", "home:r,work:r")?;
    let other = caller(&b, "other", "work:rw")?;
    let names = |c: &access::Caller| -> Vec<&'static str> {
        tools::definitions(c).iter().map(|d| d.name).collect()
    };
    assert!(names(&writer).contains(&"folder_set") && names(&writer).contains(&"folder_get"));
    assert!(!names(&reader).contains(&"folder_set") && names(&reader).contains(&"folder_get"));

    let set = tools::call(
        &b,
        &writer,
        "folder_set",
        &json!({ "space": "home", "path": dir.display().to_string() }),
    )?;
    assert_eq!(
        (
            set["space"].as_str(),
            set["mode"].as_str(),
            set["custom"].as_bool()
        ),
        (Some("home"), Some("readwrite"), Some(true))
    );
    for key in [
        "path",
        "default_path",
        "mode",
        "cloud",
        "warning",
        "last_scan",
        "files",
        "errors",
    ] {
        assert!(set.get(key).is_some(), "{key}");
    }
    // A reader reads it; a reader cannot set; another space's writer can neither set nor read it.
    let got = tools::call(&b, &reader, "folder_get", &json!({ "space": "home" }))?;
    assert_eq!(got["path"], set["path"]);
    assert!(matches!(
        tools::call(
            &b,
            &reader,
            "folder_set",
            &json!({ "space": "home", "path": null })
        ),
        Err(ToolError::Unknown)
    ));
    assert!(matches!(
        tools::call(
            &b,
            &other,
            "folder_set",
            &json!({ "space": "home", "path": null })
        ),
        Err(ToolError::Denied(_))
    ));
    assert!(matches!(
        tools::call(&b, &other, "folder_get", &json!({ "space": "home" })),
        Err(ToolError::Denied(_))
    ));
    let list = tools::call(&b, &reader, "folder_get", &json!({}))?;
    assert_eq!(list["folders"].as_array().map(Vec::len), Some(1));
    let list = tools::call(&b, &other, "folder_get", &json!({}))?;
    assert_eq!(
        list["folders"].as_array().map(Vec::len),
        Some(0),
        "a space it cannot read is not named"
    );
    // Mode, and back to the default with null.
    let ro = tools::call(
        &b,
        &writer,
        "folder_set",
        &json!({ "space": "home", "path": dir.display().to_string(), "mode": "read" }),
    )?;
    assert_eq!(ro["mode"], "read");
    assert!(
        tools::call(
            &b,
            &writer,
            "folder_set",
            &json!({ "space": "home", "path": dir.display().to_string(), "mode": "nope" })
        )
        .is_err()
    );
    assert!(
        tools::call(&b, &writer, "folder_set", &json!({ "space": "home" })).is_err(),
        "path is required"
    );
    let off = tools::call(
        &b,
        &writer,
        "folder_set",
        &json!({ "space": "home", "path": null }),
    )?;
    assert_eq!(off["custom"], false);
    // A key limited to paths inside a space never sees them.
    let scoped = caller(&b, "scoped", "home/sources/app:rw")?;
    assert!(matches!(
        tools::call(&b, &scoped, "folder_get", &json!({})),
        Err(ToolError::Unknown)
    ));
    Ok(())
}

#[test]
fn the_setting_is_in_no_sync_event_and_in_no_replicated_table() -> Res {
    let t = Temp::new("folders-travel");
    let (b, s, dir) = setup(&t, Mode::ReadWrite)?;
    note(&b, &s, "A note that travels.", &[])?;
    files::put(&b, &s, "tester", "doc.txt", "", b"a file that travels")?;
    pass(&b, &s)?;
    later();
    let path = md_files(&dir).remove(0);
    fs::write(
        &path,
        fs::read_to_string(&path)?.replace("travels", "journeys"),
    )?;
    pass(&b, &s)?;
    let conn = b.open_space(&s)?;
    let mut st = conn.prepare("SELECT kind, payload FROM sync_log")?;
    let rows: Vec<(String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    assert!(rows.len() >= 3, "the changes themselves are in the log");
    let needle = dir.to_str().ok_or("dir")?;
    for (kind, payload) in &rows {
        assert!(
            !payload.contains(needle)
                && !payload.contains("space_folders")
                && !payload.contains("folder_state"),
            "{kind}: {payload}"
        );
        assert!(!kind.contains("folder"), "{kind}");
    }
    let mut st = conn.prepare("SELECT field FROM sync_fields")?;
    let fields: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    assert!(fields.iter().all(|f| !f.contains("folder")), "{fields:?}");
    // The tables of the setting are in the control database, not in a space file.
    let ctl = b.open_control()?;
    let n: i64 = ctl.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name IN ('space_folders','folder_state')",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(n, 2);
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name IN ('space_folders','folder_state')",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(n, 0);
    // The layout of the space (what does travel) carries no folder.
    let layout = b.layout(OWNER)?;
    assert!(!format!("{layout:?}").contains(needle));
    Ok(())
}

#[test]
fn only_the_page_and_programs_of_this_machine_point_a_space_at_a_folder() -> Res {
    let t = Temp::new("folders-remote");
    let b = brain(&t)?;
    let dir = t.path().join("from-afar");
    let args = json!({ "space": "home", "path": dir.display().to_string(), "mode": "read" });
    let names = |c: &access::Caller| -> Vec<&'static str> {
        tools::definitions(c).iter().map(|d| d.name).collect()
    };
    // A key used from another machine, even one that writes the space whole: no folder_set, listed or called.
    let afar = remote_caller(&b, "afar", "home:rw")?;
    assert!(!names(&afar).contains(&"folder_set"));
    assert!(names(&afar).contains(&"folder_get"));
    assert!(tools::call(&b, &afar, "folder_set", &args).is_err());
    // A connection made by an OAuth consent never may, even through the loopback address.
    let ctl = b.open_control()?;
    let tok = access::connect_oauth(
        &ctl,
        OWNER,
        "signed-in",
        &Rights::parse("home:rw")?,
        Some(30),
        false,
        false,
    )?;
    let mut oauth = access::authenticate(&ctl, &tok, util::now())?.ok_or("refused")?;
    oauth.local = true;
    assert!(!names(&oauth).contains(&"folder_set"));
    assert!(tools::call(&b, &oauth, "folder_set", &args).is_err());
    assert!(!dir.exists(), "nothing was made on the disk");
    assert!(folders::get(&b, &sp("home"))?.is_none());
    Ok(())
}

#[test]
fn a_space_never_sits_in_or_over_a_folder_of_keys_or_of_the_system() -> Res {
    let t = Temp::new("folders-secret");
    let b = brain(&t)?;
    let s = sp("home");
    let home = ["HOME", "USERPROFILE"]
        .iter()
        .find_map(std::env::var_os)
        .map(PathBuf::from)
        .ok_or("HOME or USERPROFILE")?;
    let mut places = vec![
        home.join(".ssh"),
        home.join(".ssh").join("inside"),
        home.join(".config").join("memcp"),
        home.clone(),
    ];
    // A path from the root is absolute only on Unix; on Windows it has no drive and is refused before.
    if cfg!(unix) {
        places.push(PathBuf::from("/etc"));
    }
    for path in places {
        let err = folders::set(&b, &s, Some(&path), Mode::Read, 1)
            .err()
            .ok_or_else(|| format!("{} was taken", path.display()))?;
        assert!(err.to_string().contains("keys"), "{err}");
    }
    assert!(folders::get(&b, &s)?.is_none());
    // A folder of the person's documents is fine.
    let docs = t.path().join("documents");
    folders::set(&b, &s, Some(&docs), Mode::Read, 1)?;
    Ok(())
}
