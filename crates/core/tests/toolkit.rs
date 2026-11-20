//! The toolkit of the professions: `skills`, and the tools of the nurse (`target_set`, `signs_review`, `sign_set`).

mod common;

use common::{Res, Temp, brain, sp};
use memcp_core::access::{self, Caller, Rights};
use memcp_core::tools;
use memcp_core::users::OWNER;
use memcp_core::{Brain, util};
use serde_json::{Value, json};

fn caller(b: &Brain, name: &str, rights: &str) -> Result<Caller, Box<dyn std::error::Error>> {
    let ctl = b.open_control()?;
    let tok = access::connect_for(
        &ctl,
        OWNER,
        name,
        &Rights::parse(rights)?,
        None,
        false,
        false,
    )?;
    Ok(access::authenticate(&ctl, &tok, util::now())?.ok_or("refused")?)
}

fn call(b: &Brain, c: &Caller, tool: &str, args: Value) -> Result<Value, tools::ToolError> {
    tools::call(b, c, tool, &args)
}

fn targets(b: &Brain, c: &Caller) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let v = call(
        b,
        c,
        "space_view",
        json!({ "space": "health", "view": "targets" }),
    )?;
    Ok(v["items"].as_array().cloned().unwrap_or_default())
}

#[test]
fn a_target_is_set_as_a_nurse_writes_it() -> Res {
    let t = Temp::new("toolkit-target");
    let b = brain(&t)?;
    b.set_space_kind(&sp("health"), "health")?;
    let c = caller(&b, "assistant", "all")?;
    let made = call(
        &b,
        &c,
        "target_set",
        json!({ "problem": "Achats impulsifs", "precision": "arrêter, pour le budget", "signs": ["purchase_urge", "fatigue"] }),
    )?;
    assert_eq!(made["result"], "created", "{made}");
    assert_eq!(made["text"], "Achats impulsifs — arrêter, pour le budget");
    let list = targets(&b, &c)?;
    let target = list
        .iter()
        .find(|i| {
            i["text"]
                .as_str()
                .is_some_and(|t| t.starts_with("Achats impulsifs"))
        })
        .ok_or("target")?;
    let signs = target["signs"].to_string();
    assert!(
        signs.contains("purchase_urge") && signs.contains("fatigue"),
        "{signs}"
    );
    let card = target["id"].as_str().ok_or("card id")?.to_string();

    // A title that states an aim is refused, with the way to write it.
    let err = call(
        &b,
        &c,
        "target_set",
        json!({ "problem": "Arrêter d'acheter" }),
    )
    .err()
    .ok_or("refused")?;
    assert!(err.to_string().contains("precision"), "{err}");
    // A sign outside the referential is refused with the closest ones.
    let err = call(
        &b,
        &c,
        "target_set",
        json!({ "problem": "Sommeil", "signs": ["fatigue", "not_a_sign"] }),
    )
    .err()
    .ok_or("refused")?;
    assert!(err.to_string().contains("referential"), "{err}");

    // Changing it keeps its card id and its history.
    let up = call(
        &b,
        &c,
        "target_set",
        json!({ "id": card, "problem": "Achats impulsifs", "precision": "envies, le soir", "signs": ["purchase_urge"] }),
    )?;
    assert_eq!(up["result"], "updated", "{up}");
    let after = targets(&b, &c)?;
    let again = after
        .iter()
        .find(|i| {
            i["text"]
                .as_str()
                .is_some_and(|t| t.contains("envies, le soir"))
        })
        .ok_or("updated")?;
    assert_eq!(again["id"], json!(card), "the card keeps its id");
    assert!(!again["signs"].to_string().contains("fatigue"), "{again}");
    Ok(())
}

#[test]
fn the_signs_of_a_note_are_reviewed_and_corrected() -> Res {
    let t = Temp::new("toolkit-signs");
    let b = brain(&t)?;
    b.set_space_kind(&sp("health"), "health")?;
    let c = caller(&b, "assistant", "all")?;
    let w = call(
        &b,
        &c,
        "remember",
        json!({ "space": "health", "text": "Très fatigué ce matin, mal au crâne depuis hier." }),
    )?;
    let id = w["fact"]["id"].as_str().ok_or("id")?.to_string();
    let review = call(&b, &c, "signs_review", json!({ "space": "health" }))?;
    let note = review["notes"]
        .as_array()
        .ok_or("notes")?
        .iter()
        .find(|n| n["id"] == json!(id))
        .ok_or("note")?;
    assert!(note["signs"].to_string().contains("fatigue"), "{note}");
    assert!(
        !note["evidence"].as_array().ok_or("evidence")?.is_empty(),
        "{note}"
    );

    let fixed = call(
        &b,
        &c,
        "sign_set",
        json!({ "id": id, "remove": ["fatigue"], "add": ["stress"], "learn": false }),
    );
    // `stress` may not be a sign of the referential: then it is refused and nothing changes.
    let fixed = match fixed {
        Ok(v) => v,
        Err(e) => {
            assert!(e.to_string().contains("referential"), "{e}");
            call(
                &b,
                &c,
                "sign_set",
                json!({ "id": id, "remove": ["fatigue"], "learn": false }),
            )?
        }
    };
    assert_eq!(fixed["result"], "superseded", "{fixed}");
    assert!(
        !fixed["fact"]["tags"].to_string().contains("signe/fatigue"),
        "{fixed}"
    );
    Ok(())
}

#[test]
fn a_sign_the_person_took_off_a_note_is_not_put_back() -> Res {
    let t = Temp::new("toolkit-refused");
    let b = brain(&t)?;
    b.set_space_kind(&sp("health"), "health")?;
    let c = caller(&b, "assistant", "all")?;
    let w = call(
        &b,
        &c,
        "remember",
        json!({ "space": "health", "text": "Très fatigué ce matin, mal au crâne depuis hier." }),
    )?;
    let id = w["fact"]["id"].as_str().ok_or("id")?.to_string();
    // The person takes the fatigue off this note (the default: what they do is learned).
    let off = call(
        &b,
        &c,
        "sign_set",
        json!({ "id": id, "remove": ["fatigue"] }),
    )?;
    let tags = off["fact"]["tags"].to_string();
    assert!(!tags.contains("signe/fatigue"), "{tags}");
    assert!(tags.contains("signe-refuse/fatigue"), "{tags}");

    // The person corrects the text later: the tagger reads it again and leaves the refused sign alone.
    let id = off["fact"]["id"].as_str().ok_or("id")?.to_string();
    let again = call(
        &b,
        &c,
        "supersede",
        json!({ "id": id, "text": "Très fatigué ce matin, mal au crâne depuis hier, un peu mieux ce soir." }),
    )?;
    let tags = again["fact"]["tags"].to_string();
    assert!(!tags.contains("signe/fatigue"), "{tags}");
    assert!(tags.contains("signe-refuse/fatigue"), "{tags}");

    // The review still shows it, with what the person refused, so that it can come back.
    let review = call(&b, &c, "signs_review", json!({ "space": "health" }))?;
    let note = review["notes"]
        .as_array()
        .ok_or("notes")?
        .iter()
        .find(|n| n["id"] == json!(again["fact"]["id"]))
        .ok_or("note")?;
    assert!(note["refused"].to_string().contains("fatigue"), "{note}");

    // Putting the sign back takes the mark off.
    let back = call(
        &b,
        &c,
        "sign_set",
        json!({ "id": again["fact"]["id"], "add": ["fatigue"] }),
    )?;
    let tags = back["fact"]["tags"].to_string();
    assert!(tags.contains("signe/fatigue"), "{tags}");
    assert!(!tags.contains("signe-refuse/fatigue"), "{tags}");

    // A whole list of tags that leaves the sign out is the same word: the mark goes on.
    let listed: Vec<String> = back["fact"]["tags"]
        .as_array()
        .ok_or("tags")?
        .iter()
        .filter_map(Value::as_str)
        .filter(|t| *t != "signe/fatigue")
        .map(String::from)
        .collect();
    let off = call(
        &b,
        &c,
        "tags_set",
        json!({ "id": back["fact"]["id"], "tags": listed }),
    )?;
    let tags = off["fact"]["tags"].to_string();
    assert!(!tags.contains("signe/fatigue"), "{tags}");
    assert!(tags.contains("signe-refuse/fatigue"), "{tags}");
    Ok(())
}

#[test]
fn every_space_announces_its_profession_and_the_person_can_add_profiles() -> Res {
    let t = Temp::new("toolkit-skills");
    let b = brain(&t)?;
    b.set_space_kind(&sp("health"), "health")?;
    let c = caller(&b, "assistant", "all")?;
    let cap = call(
        &b,
        &c,
        "space_view",
        json!({ "space": "health", "view": "capabilities" }),
    )?;
    let profiles: Vec<&str> = cap["profession"]["profiles"]
        .as_array()
        .ok_or("profiles")?
        .iter()
        .filter_map(|p| p["id"].as_str())
        .collect();
    assert_eq!(profiles, ["nurse", "coach", "doctor"], "{cap}");
    assert_eq!(cap["profession"]["default"], "nurse");
    let s = call(
        &b,
        &c,
        "skills",
        json!({ "space": "health", "profile": "doctor" }),
    )?;
    assert_eq!(
        s["spaces"][0]["profession"]["profiles"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "{s}"
    );

    // The person's own profile, from the file beside the memory: a new profile of health, and a replaced one.
    std::fs::write(
        b.dir().join(memcp_core::profession::FILE),
        json!([
            { "id": "physio", "kind": "health", "title": "Physio", "brief": "You follow rehabilitation.",
              "principles": ["Pain stops the exercise."], "skills": [{ "id": "n", "title": "Note", "when": "After an exercise.", "tool": "remember" }] },
            { "id": "broken", "kind": "health" }
        ])
        .to_string(),
    )?;
    let s = call(&b, &c, "skills", json!({ "space": "health" }))?;
    let ids: Vec<&str> = s["spaces"][0]["profession"]["profiles"]
        .as_array()
        .ok_or("profiles")?
        .iter()
        .filter_map(|p| p["id"].as_str())
        .collect();
    assert_eq!(
        ids,
        ["nurse", "coach", "doctor", "physio"],
        "a malformed profile is left out: {s}"
    );
    Ok(())
}

#[test]
fn every_tool_a_profession_names_exists() -> Res {
    let t = Temp::new("toolkit-names");
    let b = brain(&t)?;
    b.set_space_kind(&sp("health"), "health")?;
    let c = caller(&b, "owner", "all")?;
    let known: Vec<String> = tools::definitions(&c)
        .iter()
        .map(|d| d.name.to_string())
        .collect();
    for name in memcp_core::profession::tool_names() {
        // The bank statement tools live in the finances crate.
        if name.starts_with("finances_") {
            continue;
        }
        assert!(
            known.iter().any(|k| k == name),
            "a skill names {name}, which is no tool"
        );
    }
    Ok(())
}

#[test]
fn the_professions_are_exported_as_a_claude_plugin_marketplace() -> Res {
    let t = Temp::new("toolkit-export");
    let out = t.path().join("skills");
    let n = memcp_core::profession::export(&out, "20261119.2.0", None)?;
    assert!(n >= 9, "{n} skills");
    let skill = std::fs::read_to_string(
        out.join("plugins/memcp-metiers/skills/memcp-health-nurse/SKILL.md"),
    )?;
    assert!(
        skill.starts_with("---\nname: memcp-health-nurse\ndescription: Use when"),
        "{skill}"
    );
    assert!(
        skill.contains("`target_set`") && skill.contains("## Principles"),
        "{skill}"
    );
    let market: Value = serde_json::from_str(&std::fs::read_to_string(
        out.join(".claude-plugin/marketplace.json"),
    )?)?;
    assert_eq!(market["plugins"][0]["source"], "./plugins/memcp-metiers");
    assert!(
        out.join("plugins/memcp-metiers/.claude-plugin/plugin.json")
            .exists()
    );
    Ok(())
}

#[test]
fn a_profession_for_a_game_master_is_proposed_accepted_and_applies_to_its_space_only() -> Res {
    let t = Temp::new("toolkit-gamemaster");
    let b = brain(&t)?;
    b.set_space_kind(&sp("personal"), "hobbies")?;
    b.set_space_kind(&sp("work"), "hobbies")?;
    let c = caller(&b, "assistant", "all")?;
    let profile = json!({
        "id": "gamemaster", "kind": "hobbies", "spaces": ["personal"],
        "title": "Game master", "brief": "You prepare and run role-playing games from the rulebooks the person keeps.",
        "principles": ["The rulebooks are the authority; say which book and page."],
        "references": [{ "title": "Favourite rulebooks", "description": "PDFs of the games, in a mounted library.",
                         "search": { "tool": "library_search", "arguments": { "query": "…" } } }],
        "skills": [
            { "id": "character", "title": "Create a character", "when": "The person wants a new character.",
              "steps": ["Ask the game and the concept.", "Search the rulebook for the creation rules.", "Write the sheet and keep it with `remember`."],
              "template": "Name:\nConcept:\nAttributes:\nBackground:", "tags": ["jdr/personnage"] },
            { "id": "rules", "title": "Look a rule up", "when": "A rule is in question.", "tool": "library_search" }
        ]
    });
    let r = call(&b, &c, "profession_propose", json!({ "profile": profile }))?;
    assert_eq!(r["result"], "proposed", "{r}");
    // Until the person accepts, it does nothing, and it is listed for them.
    let s = call(&b, &c, "skills", json!({ "space": "personal" }))?;
    let ids = |s: &Value| -> Vec<String> {
        s["spaces"][0]["profession"]["profiles"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|p| p["id"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    assert!(!ids(&s).contains(&"gamemaster".to_string()), "{s}");
    assert_eq!(s["proposals_waiting"][0]["id"], "gamemaster", "{s}");
    // A malformed profile is refused.
    assert!(
        call(
            &b,
            &c,
            "profession_propose",
            json!({ "profile": { "id": "x", "kind": "hobbies" } })
        )
        .is_err()
    );
    // The person accepts.
    assert!(memcp_core::profession::decide(
        b.dir(),
        "hobbies",
        "gamemaster",
        true
    )?);
    let s = call(&b, &c, "skills", json!({ "space": "personal" }))?;
    assert!(ids(&s).contains(&"gamemaster".to_string()), "{s}");
    let gm = s["spaces"][0]["profession"]["profiles"]
        .as_array()
        .ok_or("p")?
        .iter()
        .find(|p| p["id"] == "gamemaster")
        .ok_or("gm")?;
    assert_eq!(gm["source"], "person");
    assert_eq!(
        gm["skills"][0]["steps"].as_array().map(Vec::len),
        Some(3),
        "{gm}"
    );
    assert_eq!(gm["skills"][0]["tags"][0], "jdr/personnage");
    assert_eq!(gm["references"][0]["search"]["tool"], "library_search");
    // It applies to the space it names, not to another space of the same kind.
    let cap = call(
        &b,
        &c,
        "space_view",
        json!({ "space": "personal", "view": "capabilities" }),
    )?;
    assert!(
        cap["profession"]["profiles"]
            .to_string()
            .contains("gamemaster"),
        "{cap}"
    );
    let other = call(
        &b,
        &c,
        "space_view",
        json!({ "space": "work", "view": "capabilities" }),
    )?;
    assert!(
        !other["profession"]["profiles"]
            .to_string()
            .contains("gamemaster"),
        "another space of the same kind has the default profiles only: {other}"
    );
    // And it becomes a Claude skill like the others.
    let out = t.path().join("skills");
    memcp_core::profession::export(&out, "x", Some(b.dir()))?;
    let md = std::fs::read_to_string(
        out.join("plugins/memcp-metiers/skills/memcp-hobbies-gamemaster/SKILL.md"),
    )?;
    assert!(
        md.contains("1. Ask the game and the concept.") && md.contains("## Reference material"),
        "{md}"
    );
    Ok(())
}

#[test]
fn the_usual_spaces_of_a_fresh_memory_have_their_kind_so_the_toolkit_works_at_once() -> Res {
    let t = Temp::new("toolkit-kinds");
    let b = brain(&t)?;
    for (space, kind) in [
        ("health", "health"),
        ("home", "home"),
        ("work", "work"),
        ("personal", "me"),
    ] {
        assert_eq!(b.space_kind(&sp(space))?, kind, "{space}");
    }
    let c = caller(&b, "assistant", "all")?;
    let made = call(
        &b,
        &c,
        "target_set",
        json!({ "problem": "Sommeil", "signs": ["sleep_changes"] }),
    )?;
    assert_eq!(made["result"], "created", "{made}");
    Ok(())
}
