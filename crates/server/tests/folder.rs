//! The folder of a space on this node: the command line (`folder set|get|sync`, `library add` as its alias) and the
//! background task that keeps the folder in step while the node serves.

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use pensine_core::facts::{self, NewFact, Remembered};
use pensine_core::folders::{self, Mode};
use pensine_core::text::Language;
use pensine_core::{Brain, Space, util};
use pensine_server::{cli, folder_sync};

type Res = Result<(), Box<dyn Error>>;

fn temp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("memcp-folder-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    p
}

fn brain_with_note(dir: &std::path::Path, text: &str) -> Result<(Brain, Space), Box<dyn Error>> {
    let space = Space::new("home")?;
    let b = Brain::init(
        &dir.join("data"),
        std::slice::from_ref(&space),
        Language::English,
    )?;
    let conn = b.open_space(&space)?;
    match facts::remember(
        &conn,
        &space,
        "tester",
        NewFact::agent(text, "agent:tester"),
        &[],
    )? {
        Remembered::Created(_) => Ok((b, space)),
        other => Err(format!("{other:?}").into()),
    }
}

#[test]
fn the_commands_set_show_sync_and_put_back_the_folder_and_library_add_is_its_alias() -> Res {
    let base = temp("cli");
    let (b, space) = brain_with_note(&base, "Service the boiler in March")?;
    let data = base.join("data");
    let dir = base.join("mirror");
    cli::folder_set(&data, "home", Some(&dir), Mode::ReadWrite).map_err(|e| e.1)?;
    let files: Vec<_> = fs::read_dir(&dir)?.flatten().collect();
    assert_eq!(files.len(), 1, "the first pass runs at once");
    assert!(fs::read_to_string(files[0].path())?.contains("Service the boiler in March"));
    cli::folder_get(&data, Some("home")).map_err(|e| e.1)?;
    cli::folder_get(&data, None).map_err(|e| e.1)?;
    cli::folder_sync(&data, None).map_err(|e| e.1)?;
    assert_eq!(
        folders::get(&b, &space)?.map(|f| f.mode),
        Some(Mode::ReadWrite)
    );
    // `library add` is the read-only alias: the space then has one folder, only read.
    let vault = base.join("vault");
    fs::create_dir_all(&vault)?;
    fs::write(vault.join("Roof.md"), "# Roof\nWe need tiles.")?;
    cli::library_add(&data, &vault, "home").map_err(|e| e.1)?;
    let f = folders::get(&b, &space)?.ok_or("folder")?;
    assert_eq!(f.mode, Mode::Read);
    assert_eq!(f.path, fs::canonicalize(&vault)?);
    assert_eq!(
        fs::read_dir(&vault)?.count(),
        1,
        "nothing written in a read-only folder"
    );
    cli::folder_set(&data, "home", None, Mode::ReadWrite).map_err(|e| e.1)?;
    assert!(folders::get(&b, &space)?.is_none());
    assert!(
        cli::folder_sync(&data, Some("home")).is_err(),
        "no folder: nothing to sync"
    );
    assert!(cli::folder_set(&data, "home", Some(&data.join("inside")), Mode::ReadWrite).is_err());
    let _ = fs::remove_dir_all(&base);
    Ok(())
}

#[test]
fn the_background_task_keeps_the_folder_in_step_by_itself() -> Res {
    let base = temp("task");
    let (b, space) = brain_with_note(&base, "A note for the task")?;
    let dir = base.join("mirror");
    folders::set(&b, &space, Some(&dir), Mode::ReadWrite, util::now())?;
    let task = folder_sync::start(&b).ok_or("task")?;
    let wait = |what: &str, ok: &dyn Fn() -> bool| -> Res {
        let until = Instant::now() + Duration::from_secs(20);
        while Instant::now() < until {
            if ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err(format!("timed out: {what}").into())
    };
    let md = || -> Option<PathBuf> {
        fs::read_dir(&dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "md"))
    };
    wait("the note is written", &|| md().is_some())?;
    // An edit in the folder comes back as a new version, by the task alone.
    std::thread::sleep(Duration::from_millis(100));
    let path = md().ok_or("file")?;
    fs::write(&path, fs::read_to_string(&path)?.replace("task", "engine"))?;
    let conn = b.open_space(&space)?;
    wait("the edit comes back", &|| {
        conn.query_row(
            "SELECT count(*) FROM facts WHERE status = 'active' AND text LIKE '%A note for the engine%'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .is_ok_and(|n| n == 1)
    })?;
    task.stop();
    let _ = fs::remove_dir_all(&base);
    Ok(())
}
