//! Libraries: a read-only mounted folder indexed in a space, searched, raised into facts, and drawn by `links`.

mod common;

use std::fs;
use std::path::Path;

use common::{Res, Temp, brain};
use memcp_core::access::{self, Rights};
use memcp_core::library;
use memcp_core::tools;
use memcp_core::users::OWNER;
use memcp_core::{Brain, Space};
use serde_json::{Value, json};

fn write(root: &Path, rel: &str, body: &str) -> Res {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().ok_or("no parent")?)?;
    fs::write(p, body)?;
    Ok(())
}

fn snapshot(root: &Path) -> Vec<(String, Vec<u8>, std::time::SystemTime)> {
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(m) = fs::metadata(&p) {
                out.push((
                    p.display().to_string(),
                    fs::read(&p).unwrap_or_default(),
                    m.modified().unwrap_or(std::time::UNIX_EPOCH),
                ));
            }
        }
    }
    out.sort();
    out
}

fn first_space(b: &Brain) -> Result<Space, Box<dyn std::error::Error>> {
    Ok(b.spaces()?.into_iter().next().ok_or("no space")?)
}

fn owner(b: &Brain) -> Result<access::Caller, Box<dyn std::error::Error>> {
    let ctl = b.open_control()?;
    let tok = access::connect_for(&ctl, OWNER, "tester", &Rights::owner(), None, false, false)?;
    Ok(access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("refused")?)
}

fn mount(t: &Temp) -> Result<(Brain, Space, std::path::PathBuf), Box<dyn std::error::Error>> {
    let b = brain(t)?;
    let space = first_space(&b)?;
    let lib = t.path().join("vault");
    write(
        &lib,
        "Projects/Roof.md",
        "# Roof repair\nWe need tiles. See [[Budget]] and [[Ideas|my ideas]].\n",
    )?;
    write(
        &lib,
        "Projects/Budget.md",
        "---\ntitle: Yearly budget\n---\nPlumber and roof costs.\n",
    )?;
    write(
        &lib,
        "Ideas.md",
        "Garden ideas, back to [[Projects/Roof]] and [[Nowhere]].\n",
    )?;
    write(&lib, "notes.txt", "Plain text about bees.\n")?;
    write(&lib, "scan.pdf", "%PDF-1.4 binary")?;
    write(&lib, ".obsidian/workspace.md", "hidden config roof")?;
    let conn = b.open_space(&space)?;
    let id = library::add_root(&conn, &lib, 1000)?;
    library::rescan(&conn, id, 1000)?;
    Ok((b, space, lib))
}

#[test]
fn indexing_never_touches_the_originals_and_rescan_is_incremental() -> Res {
    let t = Temp::new("library-scan");
    let b = brain(&t)?;
    let space = first_space(&b)?;
    let lib = t.path().join("vault");
    write(&lib, "a.md", "# A\nhello\n")?;
    write(&lib, "b.md", "# B\nworld\n")?;
    let before = snapshot(&lib);
    let conn = b.open_space(&space)?;
    let id = library::add_root(&conn, &lib, 1)?;
    let first = library::rescan(&conn, id, 2)?;
    assert_eq!((first.added, first.unchanged), (2, 0));
    assert_eq!(snapshot(&lib), before, "the folder was changed");
    let again = library::rescan(&conn, id, 3)?;
    assert_eq!((again.added, again.updated, again.unchanged), (0, 0, 2));
    // A changed file is read again; a removed one is forgotten, and no longer found.
    std::thread::sleep(std::time::Duration::from_millis(20));
    write(&lib, "a.md", "# A\nhello again, now with giraffes\n")?;
    fs::remove_file(lib.join("b.md"))?;
    let third = library::rescan(&conn, id, 4)?;
    assert_eq!((third.updated, third.removed, third.unchanged), (1, 1, 0));
    assert_eq!(library::search(&conn, "giraffes", 5)?.len(), 1);
    assert!(library::search(&conn, "world", 5)?.is_empty());
    // Mounting a folder inside a mounted one is refused.
    assert!(library::add_root(&conn, &lib.join("sub-missing"), 5).is_err());
    fs::create_dir_all(lib.join("sub"))?;
    assert!(library::add_root(&conn, &lib.join("sub"), 5).is_err());
    Ok(())
}

#[test]
fn search_finds_text_title_and_name_and_leaves_hidden_folders_out() -> Res {
    let t = Temp::new("library-search");
    let (b, space, _) = mount(&t)?;
    let conn = b.open_space(&space)?;
    let hits = library::search(&conn, "tiles", 10)?;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "Roof repair");
    assert!(hits[0].snippet.contains("tiles"));
    assert!(std::path::Path::new(&hits[0].path).ends_with("Projects/Roof.md"));
    assert_eq!(
        library::search(&conn, "yearly budget", 10)?[0].title,
        "Yearly budget"
    );
    assert_eq!(
        library::search(&conn, "bee", 10)?.len(),
        1,
        "plurals and stems"
    );
    // A pdf is found by its name only.
    assert_eq!(library::search(&conn, "scan", 10)?[0].ext, "pdf");
    // The hidden folder is not indexed.
    assert!(library::search(&conn, "hidden", 10)?.is_empty());
    Ok(())
}

#[test]
fn raising_makes_a_normal_fact_linked_to_the_original_and_leaves_it() -> Res {
    let t = Temp::new("library-raise");
    let (b, space, lib) = mount(&t)?;
    let who = owner(&b)?;
    let before = snapshot(&lib);
    let hit = tools::call(&b, &who, "library_search", &json!({ "query": "tiles" }))
        .map_err(|e| format!("{e:?}"))?;
    let path = hit["results"][0]["path"]
        .as_str()
        .ok_or("no path")?
        .to_string();
    let raised = tools::call(
        &b,
        &who,
        "library_raise",
        &json!({ "space": space.as_str(), "path": path, "summary": "Roof needs new tiles before winter.", "tags": ["home/roof"] }),
    )
    .map_err(|e| format!("{e:?}"))?;
    assert_eq!(raised["result"], "raised");
    let tags: Vec<&str> = raised["fact"]["tags"]
        .as_array()
        .ok_or("tags")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(tags.contains(&"home/roof") && tags.contains(&"library/raised"));
    assert!(
        raised["fact"]["text"]
            .as_str()
            .ok_or("text")?
            .contains(&path)
    );
    // It is found by recall like any fact, and the original is unchanged.
    let found = tools::call(&b, &who, "recall", &json!({ "query": "tiles winter" }))
        .map_err(|e| format!("{e:?}"))?;
    assert!(found["results"].as_array().is_some_and(|r| !r.is_empty()));
    assert_eq!(snapshot(&lib), before);
    // Never in the inbox, never a path that is not indexed.
    let bad = tools::call(
        &b,
        &who,
        "library_raise",
        &json!({ "space": space.as_str(), "path": path, "summary": "x y", "tags": ["inbox"] }),
    );
    assert!(bad.is_err());
    let none = tools::call(
        &b,
        &who,
        "library_raise",
        &json!({ "space": space.as_str(), "path": "/etc/passwd", "summary": "x" }),
    );
    assert!(none.is_err());
    Ok(())
}

#[test]
fn a_read_only_connection_searches_but_cannot_raise() -> Res {
    let t = Temp::new("library-rights");
    let (b, space, _) = mount(&t)?;
    let ctl = b.open_control()?;
    let rights = Rights::parse(&format!("{}:r", space.as_str()))?;
    let tok = access::connect_for(&ctl, OWNER, "reader", &rights, None, false, false)?;
    let reader = access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("refused")?;
    assert!(tools::call(&b, &reader, "library_search", &json!({ "query": "tiles" })).is_ok());
    assert!(matches!(
        tools::call(
            &b,
            &reader,
            "library_raise",
            &json!({ "path": "x", "summary": "y" })
        ),
        Err(tools::ToolError::Unknown)
    ));
    let names: Vec<&str> = tools::definitions(&reader).iter().map(|d| d.name).collect();
    assert!(names.contains(&"links") && !names.contains(&"library_raise"));
    Ok(())
}

#[test]
fn links_draws_spaces_tags_library_folders_and_what_was_raised() -> Res {
    let t = Temp::new("library-links");
    let (b, space, _) = mount(&t)?;
    let who = owner(&b)?;
    let other = b
        .spaces()?
        .into_iter()
        .find(|s| *s != space)
        .ok_or("one space only")?;
    for (sp, text) in [
        (&space, "Roof tiles are ordered."),
        (&other, "Roof insurance renewed."),
    ] {
        tools::call(
            &b,
            &who,
            "remember",
            &json!({ "space": sp.as_str(), "text": text, "tags": ["home/roof"] }),
        )
        .map_err(|e| format!("{e:?}"))?;
    }
    let hit = library::search(&b.open_space(&space)?, "budget", 5)?;
    tools::call(&b, &who, "library_raise", &json!({
        "space": space.as_str(), "path": hit[0].path, "summary": "Budget of the year, plumber first.", "tags": ["home/budget"] }))
    .map_err(|e| format!("{e:?}"))?;
    let g = tools::call(&b, &who, "links", &json!({})).map_err(|e| format!("{e:?}"))?;
    let nodes = g["nodes"].as_array().ok_or("nodes")?;
    let has = |id: &str| nodes.iter().any(|n| n["id"] == id);
    assert!(has(&format!("space:{space}")) && has("tag:home/roof"));
    assert!(has(&format!("library:{space}:vault")), "root folder node");
    assert!(has(&format!("library:{space}:vault/Projects")));
    let edges = g["edges"].as_array().ok_or("edges")?;
    let of = |ty: &str| edges.iter().filter(|e| e["type"] == ty).collect::<Vec<_>>();
    // Two spaces hold facts under home/roof.
    assert!(of("shared_tag").iter().any(|e| {
        e["tags"]
            .as_array()
            .is_some_and(|t| t.iter().any(|x| x == "home/roof"))
    }));
    // Roof (Projects) -> Ideas (root), and Ideas -> Roof: one link each way between the two folders.
    let links = of("link");
    assert_eq!(links.len(), 2, "{links:?}");
    assert!(links.iter().all(|e| e["count"] == 1));
    assert_eq!(of("raised").len(), 1);
    assert_eq!(
        of("raised")[0]["source"],
        format!("library:{space}:vault/Projects")
    );
    // `Nowhere` leads to no file.
    let root = nodes
        .iter()
        .find(|n| n["id"] == format!("library:{space}:vault"))
        .ok_or("root")?;
    assert_eq!(root["unresolved_links"], 1);
    Ok(())
}

#[test]
fn links_shows_only_what_the_caller_can_read() -> Res {
    let t = Temp::new("library-links-rights");
    let (b, space, _) = mount(&t)?;
    let other = b
        .spaces()?
        .into_iter()
        .find(|s| *s != space)
        .ok_or("one space only")?;
    let ctl = b.open_control()?;
    let rights = Rights::parse(&format!("{}:r", other.as_str()))?;
    let tok = access::connect_for(&ctl, OWNER, "narrow", &rights, None, false, false)?;
    let narrow = access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("refused")?;
    let g = tools::call(&b, &narrow, "links", &json!({})).map_err(|e| format!("{e:?}"))?;
    let dump = g.to_string();
    assert!(!dump.contains(&format!("space:{space}")), "{dump}");
    assert!(!dump.contains("library:"), "{dump}");
    assert!(dump.contains(&format!("space:{other}")));
    assert!(tools::call(&b, &narrow, "links", &json!({ "space": space.as_str() })).is_err());
    let none = tools::call(&b, &narrow, "library_search", &json!({ "query": "tiles" }))
        .map_err(|e| format!("{e:?}"))?;
    assert_eq!(none["results"].as_array().map(Vec::len), Some(0));
    Ok(())
}
