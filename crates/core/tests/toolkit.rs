//! The toolkit of the professions: `skills`, and the tools of the nurse (`target_set`, `signs_review`, `sign_set`).

mod common;

use common::{Res, Temp, brain, sp};
use pensine_core::access::{self, Caller, Rights};
use pensine_core::tools;
use pensine_core::users::OWNER;
use pensine_core::{Brain, util};
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
fn target_traits_can_be_replaced_cleared_and_left_alone() -> Res {
    let t = Temp::new("target-traits");
    let b = brain(&t)?;
    let c = caller(&b, "assistant", "all")?;
    let set = |args| call(&b, &c, "target_set", args);
    set(json!({ "space": "health", "problem": "Sommeil" }))?;
    let before = targets(&b, &c)?;
    let id = before[0]["id"].as_str().ok_or("card id")?;
    set(json!({ "id": id, "signs": ["Mal de tête", "Heavy legs"], "factors": ["Late coffee"] }))?;
    let added = targets(&b, &c)?;
    assert_eq!(added[0]["signs"], json!(["headache", "heavy-legs"]));
    assert_eq!(added[0]["factors"], json!(["late-coffee"]));
    assert_eq!(added[0]["text"], "Sommeil");

    set(json!({ "id": id, "factors": ["Night shifts"], "column": "reached" }))?;
    let changed = targets(&b, &c)?;
    assert_eq!(changed[0]["id"], id);
    assert_eq!(changed[0]["signs"], added[0]["signs"]);
    assert_eq!(changed[0]["factors"], json!(["night-shifts"]));

    call(
        &b,
        &c,
        "living_set",
        json!({ "space": "health", "page": "targets",
        "move": [{ "id": id, "column": "open" }] }),
    )?;
    assert_eq!(targets(&b, &c)?[0]["factors"], json!(["night-shifts"]));
    set(json!({ "id": id, "signs": ["fatigue"], "factors": [] }))?;
    let removed = targets(&b, &c)?;
    assert_eq!(removed[0]["signs"], json!(["fatigue"]));
    assert_eq!(removed[0]["factors"], json!([]));
    set(json!({ "id": id, "signs": [] }))?;
    assert_eq!(targets(&b, &c)?[0]["signs"], json!([]));

    assert!(set(json!({ "id": "unknown", "factors": ["coffee"] })).is_err());
    assert!(set(json!({ "space": "work", "id": id, "signs": [] })).is_err());
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
