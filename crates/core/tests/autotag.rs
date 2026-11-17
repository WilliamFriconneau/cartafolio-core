//! Automatic tagging through the node: what a write gets, the tools that preview and correct it, and what the node learns.

mod common;

use common::{Res, Temp, brain, sp};
use memcp_core::access::{self, Caller, Rights};
use memcp_core::tools;
use memcp_core::triage;
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

fn remember(
    b: &Brain,
    c: &Caller,
    space: &str,
    text: &str,
    tags: &[&str],
) -> Result<Value, Box<dyn std::error::Error>> {
    let r = tools::call(
        b,
        c,
        "remember",
        &json!({ "space": space, "text": text, "tags": tags }),
    )?;
    assert_eq!(r["result"], "created", "{r}");
    Ok(r["fact"].clone())
}

fn has(v: &Value, tag: &str) -> bool {
    v["tags"]
        .as_array()
        .is_some_and(|a| a.iter().any(|t| t == tag))
}

#[test]
fn a_write_gets_the_tag_of_the_space_and_what_the_text_says() -> Res {
    let t = Temp::new("autotag-write");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    let f = remember(
        &b,
        &c,
        "health",
        "Douleur 3/10 ce matin. Tension 12/8.",
        &[],
    )?;
    for tag in [
        "health",
        "signe/douleur",
        "mesure/tension",
        "field/level/3/10",
        "field/tension/12/8",
    ] {
        assert!(has(&f, tag), "{tag}: {f}");
    }
    assert_eq!(f["fields"]["level"], "3/10");
    assert_eq!(f["fields"]["tension"], "12/8");
    // The caller's own tags stay; the node only adds.
    let f = remember(
        &b,
        &c,
        "health",
        "Pouls 64 bpm au repos, poids 72,5 kg.",
        &["journal", "perso/suivi"],
    )?;
    for tag in [
        "journal",
        "perso/suivi",
        "health",
        "mesure/pouls",
        "mesure/poids",
        "field/poids/72_5",
    ] {
        assert!(has(&f, tag), "{tag}: {f}");
    }
    // Another space: its tag, and no health pattern.
    let f = remember(
        &b,
        &c,
        "home",
        "Chaudière révisée, tension 12/8 sur le circuit.",
        &[],
    )?;
    assert!(has(&f, "home") && !has(&f, "mesure/tension"), "{f}");
    // The keywords of the space's rules note.
    tools::call(
        &b,
        &c,
        "rules_set",
        &json!({ "space": "home", "keywords": ["zendure"] }),
    )?;
    let f = remember(&b, &c, "home", "La Zendure est pleine.", &[])?;
    assert!(has(&f, "home") && has(&f, "zendure"), "{f}");
    // A note on a date is no measure.
    let f = remember(
        &b,
        &c,
        "health",
        "Rendez-vous le 2026-11-17 à 12h30, tension à vérifier.",
        &[],
    )?;
    assert!(has(&f, "health") && !has(&f, "mesure/tension"), "{f}");
    assert_eq!(f["fields"], json!({}));
    Ok(())
}

#[test]
fn nothing_is_added_to_a_trier_nor_to_a_path_connection() -> Res {
    let t = Temp::new("autotag-triage");
    let b = brain(&t)?;
    let (_, box_name) = memcp_core::pairing::create_for_peer(&b, OWNER, "owner", util::now())?;
    let device = caller(&b, "peer-phone-x", &format!("{box_name}:rw"))?;
    let r = tools::call(
        &b,
        &device,
        "remember",
        &json!({ "text": "Douleur 3/10 ce matin", "tags": ["inbox"] }),
    )?;
    assert_eq!(r["fact"]["tags"], json!(["inbox"]), "{r}");
    // A path connection writes under its path only.
    let c = caller(&b, "scoped", "health/sources/app:rw")?;
    let r = tools::call(
        &b,
        &c,
        "remember",
        &json!({ "space": "health", "text": "Douleur 3/10 ce matin", "tags": ["sources/app"] }),
    )?;
    assert_eq!(r["fact"]["tags"], json!(["sources/app"]), "{r}");
    Ok(())
}

#[test]
fn a_deposit_and_a_file_description_are_tagged_too() -> Res {
    let t = Temp::new("autotag-deposit");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    let r = tools::call(
        &b,
        &c,
        "deposit",
        &json!({ "space": "health", "records": [
            { "text": "Poids 80 kg ce matin", "origin": { "app": "scale", "id": "r1", "at": "2026-11-01" } }
        ] }),
    )?;
    assert_eq!(r["failed"], 0, "{r}");
    let got = tools::call(
        &b,
        &c,
        "recall",
        &json!({ "query": "poids", "space": "health", "tags": ["sources/scale"] }),
    )?;
    assert!(
        has(&got["results"][0], "mesure/poids") && has(&got["results"][0], "health"),
        "{got}"
    );
    let r = tools::call(
        &b,
        &c,
        "file_put",
        &json!({ "space": "health", "name": "ordonnance.txt", "note": "Tension 13/8 notée", "data_base64": "aGVsbG8=" }),
    )?;
    assert!(r.to_string().contains("health"), "{r}");
    let list = tools::call(
        &b,
        &c,
        "recall",
        &json!({ "query": "tension", "space": "health" }),
    )?;
    assert!(
        list["results"][0]["tags"]
            .as_array()
            .is_some_and(|a| a.iter().any(|t| t == "mesure/tension")),
        "{list}"
    );
    Ok(())
}

#[test]
fn the_preview_gives_what_the_write_gives() -> Res {
    let t = Temp::new("autotag-preview");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    let text = "Douleur à 4 sur 10 et fatigue, glycémie 1,1 g/l";
    let p = tools::call(
        &b,
        &c,
        "tags_suggest",
        &json!({ "space": "health", "text": text }),
    )?;
    let f = remember(&b, &c, "health", text, &[])?;
    let mut wrote: Vec<String> = f["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(String::from)
        .collect();
    let mut previewed: Vec<String> = p["all_tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(String::from)
        .collect();
    wrote.sort();
    previewed.sort();
    assert_eq!(wrote, previewed);
    assert_eq!(p["fields"]["level"], "4/10");
    assert_eq!(p["fields"]["glycemie"], "1.1");
    assert_eq!(p["tags"][0], "health");
    // A read-only connection can preview; it cannot correct.
    let ro = caller(&b, "reader", "health:r")?;
    assert!(
        tools::call(
            &b,
            &ro,
            "tags_suggest",
            &json!({ "space": "health", "text": text })
        )
        .is_ok()
    );
    assert!(matches!(
        tools::call(&b, &ro, "tags_set", &json!({ "id": "x", "add": ["a"] })),
        Err(tools::ToolError::Unknown)
    ));
    Ok(())
}

#[test]
fn a_correction_supersedes_with_the_new_tags_and_keeps_the_history() -> Res {
    let t = Temp::new("autotag-set");
    let b = brain(&t)?;
    let c = caller(&b, "engine", "all")?;
    let f = remember(&b, &c, "health", "Douleur 3/10 au genou depuis hier", &[])?;
    let id = f["id"].as_str().ok_or("id")?;
    let r = tools::call(
        &b,
        &c,
        "tags_set",
        &json!({
            "id": id, "add": ["suivi/genou"], "remove": ["signe/douleur"],
            "fields": { "level": "4/10" }, "reason": "the engine read the note again", "learn": false
        }),
    )?;
    assert_eq!(r["result"], "superseded", "{r}");
    assert!(
        has(&r["fact"], "suivi/genou") && !has(&r["fact"], "signe/douleur"),
        "{r}"
    );
    assert!(
        has(&r["fact"], "field/level/4/10") && !has(&r["fact"], "field/level/3/10"),
        "{r}"
    );
    assert_eq!(r["fact"]["fields"]["level"], "4/10");
    assert_eq!(r["fact"]["text"], f["text"], "the text is untouched");
    assert_eq!(r["fact"]["status_reason"], Value::Null);
    let new_id = r["fact"]["id"].as_str().ok_or("new id")?;
    let h = tools::call(&b, &c, "history", &json!({ "id": new_id }))?;
    let chain = h["chain"].as_array().ok_or("chain")?;
    assert_eq!(chain.len(), 2, "{h}");
    assert!(
        chain
            .iter()
            .any(|x| has(x, "signe/douleur") && x["status"] == "replaced")
    );
    assert_eq!(
        chain
            .iter()
            .find(|x| x["id"] == id)
            .map(|x| x["status_reason"].clone()),
        Some(json!("the engine read the note again"))
    );
    // The same set again changes nothing; an empty set is refused; a note keeps its text.
    let again = tools::call(
        &b,
        &c,
        "tags_set",
        &json!({ "id": new_id, "tags": r["fact"]["tags"] }),
    )?;
    assert_eq!(again["result"], "unchanged");
    assert!(tools::call(&b, &c, "tags_set", &json!({ "id": new_id, "tags": [] })).is_err());
    // A connection that does not reach the space does not see the note.
    let other = caller(&b, "other", "work:rw")?;
    assert!(
        tools::call(
            &b,
            &other,
            "tags_set",
            &json!({ "id": new_id, "add": ["x"] })
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn the_node_learns_from_the_corrections_of_the_person() -> Res {
    let t = Temp::new("autotag-learn");
    let b = brain(&t)?;
    let c = caller(&b, "person-page", "all")?;
    let texts = [
        "Zendure batterie déchargée au retour de vacances",
        "La batterie Zendure ne charge plus avec le soleil faible",
    ];
    for text in texts {
        let f = remember(&b, &c, "home", text, &[])?;
        tools::call(
            &b,
            &c,
            "tags_set",
            &json!({ "id": f["id"], "add": ["home/energie"] }),
        )?;
    }
    let f = remember(&b, &c, "home", "Zendure batterie pleine à midi", &[])?;
    assert!(
        has(&f, "home/energie"),
        "learned after two corrections: {f}"
    );
    // The preview says it too; another space does not know it.
    let p = tools::call(
        &b,
        &c,
        "tags_suggest",
        &json!({ "space": "home", "text": "Zendure batterie pleine à midi" }),
    )?;
    assert!(p["tags"].to_string().contains("home/energie"), "{p}");
    let w = remember(
        &b,
        &c,
        "work",
        "Zendure batterie pleine à midi, point d'équipe",
        &[],
    )?;
    assert!(!has(&w, "home/energie"), "{w}");
    // The person takes it off a note like this one: it stays off the next.
    let f = remember(&b, &c, "home", "Zendure batterie en charge ce lundi", &[])?;
    assert!(has(&f, "home/energie"));
    tools::call(
        &b,
        &c,
        "tags_set",
        &json!({ "id": f["id"], "remove": ["home/energie"] }),
    )?;
    let f = remember(&b, &c, "home", "Zendure batterie encore vide jeudi", &[])?;
    assert!(!has(&f, "home/energie"), "{f}");
    assert!(has(&f, "home"), "the tag of the space stays: {f}");
    Ok(())
}

#[test]
fn the_janitor_and_an_engine_that_asks_not_to_learn_teach_nothing() -> Res {
    let t = Temp::new("autotag-nolearn");
    let b = brain(&t)?;
    let c = caller(&b, "engine", "all")?;
    let j = caller(&b, "janitor", "all")?;
    for (text, other) in [
        (
            "Facture plombier chaudière hiver",
            "Robinet cuisine: plombier, facture réglée",
        ),
        (
            "Plombier facture évier salle de bain",
            "Devis plombier et facture du garage",
        ),
    ] {
        let f = remember(&b, &c, "home", text, &[])?;
        tools::call(
            &b,
            &c,
            "tags_set",
            &json!({ "id": f["id"], "add": ["home/travaux"], "learn": false }),
        )?;
        let f = remember(&b, &j, "home", other, &[])?;
        tools::call(
            &b,
            &j,
            "tags_set",
            &json!({ "id": f["id"], "add": ["home/travaux"] }),
        )?;
    }
    let f = remember(
        &b,
        &c,
        "home",
        "Plombier facture escalier visite lundi",
        &[],
    )?;
    assert!(!has(&f, "home/travaux"), "{f}");
    Ok(())
}

#[test]
fn where_the_person_sends_notes_is_learned_and_the_janitors_choice_is_not() -> Res {
    let t = Temp::new("autotag-route");
    let b = brain(&t)?;
    let (_, box_name) = memcp_core::pairing::create_for_peer(&b, OWNER, "owner", util::now())?;
    let box_ = sp(&box_name);
    let device = caller(&b, "peer-phone-x", &format!("{box_name}:rw"))?;
    let c = caller(&b, "person-page", "all")?;
    let drop = |text: &str| -> Result<String, Box<dyn std::error::Error>> {
        let r = tools::call(
            &b,
            &device,
            "remember",
            &json!({ "text": text, "tags": ["inbox"] }),
        )?;
        Ok(r["fact"]["id"].as_str().ok_or("id")?.to_string())
    };
    // The janitor sends: not learned.
    for text in ["Facture plombier chaudière", "Plombier facture robinet"] {
        triage::send(
            &b,
            &box_,
            &drop(text)?,
            &sp("home"),
            "memcp-janitor",
            "owner",
        )?;
    }
    let p = tools::call(
        &b,
        &c,
        "tags_suggest",
        &json!({ "text": "Plombier facture escalier visite lundi" }),
    )?;
    assert_eq!(p["route"], json!([]), "{p}");
    // The person sends: learned after two.
    for text in [
        "Contrat assurance auto renouvelé",
        "Assurance auto contrat échéance",
    ] {
        triage::send(&b, &box_, &drop(text)?, &sp("personal"), "owner", "owner")?;
    }
    let p = tools::call(
        &b,
        &c,
        "tags_suggest",
        &json!({ "text": "Assurance auto contrat à relire" }),
    )?;
    assert_eq!(p["route"][0]["space"], "personal", "{p}");
    // A reader of one space is not told about another.
    let only_work = caller(&b, "only-work", "work:r")?;
    let p = tools::call(
        &b,
        &only_work,
        "tags_suggest",
        &json!({ "text": "Assurance auto contrat à relire" }),
    )?;
    assert_eq!(p["route"], json!([]), "{p}");
    Ok(())
}

#[test]
fn the_tags_come_back_in_a_living_page_and_in_recall() -> Res {
    let t = Temp::new("autotag-living");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    remember(&b, &c, "health", "Douleur 2/10 au dos ce soir", &[])?;
    let board = tools::call(
        &b,
        &c,
        "living_get",
        &json!({ "space": "health", "page": "next" }),
    )?;
    let d = &board["delta"][0];
    assert!(has(d, "health") && has(d, "signe/douleur"), "{board}");
    assert_eq!(d["fields"]["level"], "2/10");
    let r = tools::call(
        &b,
        &c,
        "recall",
        &json!({ "query": "douleur", "space": "health" }),
    )?;
    assert!(has(&r["results"][0], "signe/douleur"), "{r}");
    assert_eq!(r["results"][0]["fields"]["level"], "2/10");
    Ok(())
}

#[test]
fn a_note_sent_out_of_a_triage_space_is_tagged_like_one_written_there() -> Res {
    let t = Temp::new("autotag-triage");
    let b = brain(&t)?;
    let (_, box_name) = memcp_core::pairing::create_for_peer(&b, OWNER, "owner", util::now())?;
    let device = caller(&b, "peer-phone-y", &format!("{box_name}:rw"))?;
    let janitor = caller(&b, "memcp-janitor", "all")?;
    // A device writes with no space: the note waits in Inbox, untagged by the space it will go to.
    let r = tools::call(
        &b,
        &device,
        "remember",
        &json!({ "text": "Douleur 3/10 ce matin. Tension 12/8.", "tags": ["inbox"] }),
    )?;
    let id = r["fact"]["id"].as_str().ok_or("id")?;
    let sent = tools::call(
        &b,
        &janitor,
        "triage_send",
        &json!({ "id": id, "to": "health" }),
    )?;
    let new = sent["id"].as_str().ok_or("id")?;
    let f = tools::call(
        &b,
        &janitor,
        "read_fact",
        &json!({ "space": "health", "id": new }),
    )?;
    let fact = if f["fact"].is_object() {
        &f["fact"]
    } else {
        &f
    };
    assert!(has(fact, "health"), "the tag of the space first: {f}");
    assert!(has(fact, "inbox"), "still to be read: {f}");
    assert!(
        fact["tags"].as_array().is_some_and(|a| a
            .iter()
            .any(|t| t.as_str().is_some_and(|s| s.starts_with("field/")))),
        "the levels and measures said: {f}"
    );
    Ok(())
}

#[test]
fn a_connection_to_all_spaces_makes_and_moves_a_sub_space() -> Res {
    let t = Temp::new("space-tools");
    let b = brain(&t)?;
    let c = caller(&b, "assistant", "all")?;
    // The parent is a space of the person too: make it first.
    let top = tools::call(&b, &c, "space_create", &json!({ "label": "Hobbies" }))?;
    let parent = top["space"]["space"]
        .as_str()
        .or(top["space"]["name"].as_str())
        .ok_or("parent name")?
        .to_string();
    let made = tools::call(
        &b,
        &c,
        "space_create",
        &json!({ "label": "Firefighting", "parent": parent, "keywords": ["vsav"] }),
    )?;
    assert_eq!(made["result"], "created", "{made}");
    let name = made["space"]["space"]
        .as_str()
        .or(made["space"]["name"].as_str())
        .ok_or("name")?
        .to_string();
    let moved = tools::call(
        &b,
        &c,
        "space_move",
        &json!({ "space": name, "label": "Fire", "parent": "" }),
    )?;
    assert_eq!(moved["result"], "updated", "{moved}");
    // A connection limited to one space cannot arrange the person's spaces.
    let one = caller(&b, "only-work", "work:rw")?;
    assert!(tools::call(&b, &one, "space_create", &json!({ "label": "X" })).is_err());
    Ok(())
}

#[test]
fn notes_move_previews_then_moves_the_notes_of_a_tag_and_keeps_the_originals() -> Res {
    let t = Temp::new("notes-move");
    let b = brain(&t)?;
    let c = caller(&b, "assistant", "all")?;
    let a = tools::call(&b, &c, "space_create", &json!({ "label": "Alpha" }))?;
    // A connection sees the spaces that exist when it authenticates: connect again after each one.
    let c = caller(&b, "assistant1", "all")?;
    let z = tools::call(&b, &c, "space_create", &json!({ "label": "Zulu" }))?;
    let (a, z) = (
        a["space"]["space"]
            .as_str()
            .or(a["space"]["name"].as_str())
            .ok_or("a")?
            .to_string(),
        z["space"]["space"]
            .as_str()
            .or(z["space"]["name"].as_str())
            .ok_or("z")?
            .to_string(),
    );
    let c = caller(&b, "assistant2", "all")?;
    remember(
        &b,
        &c,
        &a,
        "2026-11-17, first fire drill report",
        &["drills"],
    )?;
    remember(
        &b,
        &c,
        &a,
        "2026-11-18, ladder inspection and pump pressure results",
        &["drills"],
    )?;
    remember(
        &b,
        &c,
        &a,
        "2026-11-19, a note about groceries and bread",
        &["other"],
    )?;
    let preview = tools::call(
        &b,
        &c,
        "notes_move",
        &json!({ "from": a, "to": z, "tag": "drills" }),
    )?;
    assert_eq!(preview["dry_run"], true);
    assert_eq!(preview["matched"], 2, "{preview}");
    assert_eq!(preview["moved"], 0, "a preview moves nothing: {preview}");
    let done = tools::call(
        &b,
        &c,
        "notes_move",
        &json!({ "from": a, "to": z, "tag": "drills", "dry_run": false }),
    )?;
    assert_eq!(done["moved"], 2, "{done}");
    let there = tools::call(
        &b,
        &c,
        "recall",
        &json!({ "space": z, "query": "", "tags": ["drills"], "limit": 10 }),
    )?;
    assert_eq!(
        there["results"].as_array().map(Vec::len),
        Some(2),
        "{there}"
    );
    let left = tools::call(
        &b,
        &c,
        "recall",
        &json!({ "space": a, "query": "", "tags": ["drills"], "limit": 10 }),
    )?;
    assert!(
        left["results"]
            .as_array()
            .is_none_or(|r| r.iter().all(|x| x["status"] != "active")),
        "the originals are retired, not erased: {left}"
    );
    // The same space on both sides, and a key that cannot write on both, are refused.
    assert!(
        tools::call(
            &b,
            &c,
            "notes_move",
            &json!({ "from": a, "to": a, "tag": "x" })
        )
        .is_err()
    );
    let one = caller(&b, "one", &format!("{a}:rw"))?;
    assert!(
        tools::call(
            &b,
            &one,
            "notes_move",
            &json!({ "from": a, "to": z, "tag": "x" })
        )
        .is_err()
    );
    Ok(())
}
