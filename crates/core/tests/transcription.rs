//! Guided transcription: the short ordered list of words a speech model is told to expect, drawn from the memory the
//! caller reads (names, rules keywords, vocabulary terms, health terms), cached and rebuilt when the memory changed.

mod common;

use common::{Res, Temp, brain, sp};
use memcp_core::access::{self, Caller, Rights};
use memcp_core::rules::{self, Rules};
use memcp_core::tools::{self, ToolError};
use memcp_core::users::OWNER;
use memcp_core::{Brain, transcription, util, vocabulary};
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

fn note(b: &Brain, c: &Caller, space: &str, text: &str) -> Res {
    let r = tools::call(b, c, "remember", &json!({ "space": space, "text": text }))?;
    assert_eq!(r["result"], "created", "{r}");
    Ok(())
}

fn hints(b: &Brain, c: &Caller, args: &Value) -> Result<Value, ToolError> {
    tools::call(b, c, "transcription_hints", args)
}

fn list(v: &Value, key: &str) -> Vec<String> {
    v[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_str().map(String::from))
        .collect()
}

fn group(v: &Value, name: &str) -> Vec<String> {
    list(&v["groups"], name)
}

fn at(words: &[String], w: &str) -> usize {
    words
        .iter()
        .position(|x| x == w)
        .unwrap_or_else(|| panic!("{w} is not in {words:?}"))
}

/// A person's notes, the kind that recur: a few names, days and months at the start of sentences.
fn story(b: &Brain, c: &Caller) -> Res {
    for text in [
        "Hier j'ai vu Paul Martin au parc, avec Sophie.",
        "Paul Martin viendra mardi pour la fête.",
        "On dîne chez Paul Martin avec Sophie vendredi soir.",
        "Appeler le garage de Lisbonne demain matin.",
        "Lundi on part à Lisbonne en famille, Janvier sera froid.",
        "Pendant ce temps Lisbonne attend. Bonjour tout le monde.",
        "Demain pendant la réunion on parle du budget. Bonjour à tous.",
        "Le plombier de Lisbonne a installé la Zendure.",
        "Pendant l'été la Zendure produit beaucoup.",
    ] {
        note(b, c, "personal", text)?;
    }
    Ok(())
}

#[test]
fn names_that_recur_come_first_and_sentence_starts_days_and_months_are_not_names() -> Res {
    let t = Temp::new("hints-names");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    story(&b, &c)?;
    let h = hints(&b, &c, &json!({}))?;
    let words = list(&h, "words");
    let names = group(&h, "names");
    for w in ["Paul Martin", "Sophie", "Lisbonne", "Zendure"] {
        assert!(names.contains(&w.to_string()), "{w}: {names:?}");
    }
    for w in [
        "Pendant", "Bonjour", "Demain", "Lundi", "Janvier", "Hier", "Appeler", "Le",
    ] {
        assert!(
            !words.iter().any(|x| x.eq_ignore_ascii_case(w)),
            "{w} is a false positive: {words:?}"
        );
    }
    // The phrase said three times before the place said four times alone? the more frequent first, the phrase wins a tie.
    assert!(at(&names, "Lisbonne") < at(&names, "Sophie"), "{names:?}");
    assert!(
        at(&names, "Paul Martin") < at(&names, "Zendure"),
        "{names:?}"
    );
    Ok(())
}

#[test]
fn recent_notes_weigh_more_than_old_ones() -> Res {
    let t = Temp::new("hints-recent");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    for text in [
        "On a revu Alphonse chez les voisins.",
        "Alphonse a apporté le gâteau hier.",
        "On a croisé Bruno au marché.",
        "Bruno a réparé le vélo ce matin.",
    ] {
        note(&b, &c, "personal", text)?;
    }
    // The two notes about Alphonse are a year old.
    let conn = b.open_space(&sp("personal"))?;
    conn.execute(
        "UPDATE facts SET created_at = created_at - 400 * 86400 WHERE text LIKE '%Alphonse%'",
        [],
    )?;
    let names = group(&hints(&b, &c, &json!({}))?, "names");
    assert!(at(&names, "Bruno") < at(&names, "Alphonse"), "{names:?}");
    Ok(())
}

#[test]
fn tags_of_people_and_places_count_and_the_same_word_is_listed_once() -> Res {
    let t = Temp::new("hints-tags");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    for (text, tags) in [
        ("Déjeuner du dimanche.", json!(["personnes/camille-roux"])),
        ("Appel du soir.", json!(["personnes/camille-roux"])),
        ("Promenade au bord de l'eau.", json!(["lieux/saint-malo"])),
        ("Zendure branchée.", json!(["produits/zendure"])),
    ] {
        let r = tools::call(
            &b,
            &c,
            "remember",
            &json!({"space": "personal", "text": text, "tags": tags}),
        )?;
        assert_eq!(r["result"], "created", "{r}");
    }
    note(
        &b,
        &c,
        "personal",
        "On a vu Camille Roux, puis Camille Roux encore.",
    )?;
    // The same word as a vocabulary term and as a keyword.
    let mut conn = b.open_space(&sp("personal"))?;
    vocabulary::import(
        &mut conn,
        "words",
        "test",
        &[("zendure".into(), String::new(), String::new())],
        1,
    )?;
    rules::write(
        &conn,
        &sp("personal"),
        "me",
        &Rules {
            tags: vec![],
            keywords: vec!["ZENDURE".into(), "garage".into()],
        },
    )?;
    let h = hints(&b, &c, &json!({}))?;
    let words = list(&h, "words");
    assert!(words.contains(&"Camille Roux".to_string()), "{words:?}");
    assert!(words.contains(&"Saint Malo".to_string()), "{words:?}");
    let lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let mut once = lower.clone();
    once.sort();
    once.dedup();
    assert_eq!(lower.len(), once.len(), "duplicates in {words:?}");
    assert!(group(&h, "names").contains(&"Zendure".to_string()));
    assert!(!group(&h, "vocabulary").contains(&"zendure".to_string()));
    assert!(
        !group(&h, "keywords")
            .iter()
            .any(|w| w.to_lowercase() == "zendure")
    );
    assert!(group(&h, "keywords").contains(&"garage".to_string()));
    Ok(())
}

#[test]
fn the_keywords_of_the_rules_and_the_vocabulary_terms_are_ordered_by_use_then_length() -> Res {
    let t = Temp::new("hints-vocab");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    note(
        &b,
        &c,
        "work",
        "La chaudière est en panne, la chaudière fuit.",
    )?;
    note(
        &b,
        &c,
        "work",
        "Le devis du plombier arrive demain, le plombier passe.",
    )?;
    let mut conn = b.open_space(&sp("work"))?;
    let terms: Vec<(String, String, String)> = [
        "chaudière",
        "plombier",
        "fuit",
        "panne",
        "robinet",
        "thermostat différentiel",
        "x",
        "un terme beaucoup trop long pour être dit dans une dictée normale",
    ]
    .iter()
    .map(|t| (t.to_string(), String::new(), String::new()))
    .collect();
    vocabulary::import(&mut conn, "maison", "test", &terms, 1)?;
    rules::write(
        &conn,
        &sp("work"),
        "me",
        &Rules {
            tags: vec![],
            keywords: vec!["devis".into(), "chaudière".into(), "facture client".into()],
        },
    )?;
    let h = hints(&b, &c, &json!({"space": "work"}))?;
    let vocab = group(&h, "vocabulary");
    // « chaudière » is a keyword too: it is listed once, with the keywords. Said twice, said once (shortest first),
    // never said (shortest first).
    assert_eq!(vocab[..3], ["plombier", "fuit", "panne"], "{vocab:?}");
    assert!(at(&vocab, "robinet") < at(&vocab, "thermostat différentiel"));
    assert!(
        !vocab.iter().any(|w| w.chars().count() > 40),
        "an item is 40 characters at most: {vocab:?}"
    );
    let keywords = group(&h, "keywords");
    assert_eq!(
        keywords,
        ["chaudière", "devis", "facture client"],
        "most used first"
    );
    // The first group that has a word keeps it.
    let words = list(&h, "words");
    assert_eq!(words.iter().filter(|w| *w == "chaudière").count(), 1);
    Ok(())
}

#[test]
fn health_terms_come_from_the_health_referential_in_the_language_asked() -> Res {
    let t = Temp::new("hints-health");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    note(&b, &c, "health", "Migraine forte ce matin.")?;
    note(&b, &c, "health", "Encore une migraine, et de la toux.")?;
    // The labels are the referential's, as written there; compared without case.
    let low = |v: Vec<String>| v.into_iter().map(|w| w.to_lowercase()).collect::<Vec<_>>();
    let fr = hints(&b, &c, &json!({"lang": "fr"}))?;
    let health = low(group(&fr, "health"));
    assert!(health.contains(&"mal de tête".to_string()), "{health:?}");
    assert!(health.contains(&"glycémie".to_string()), "{health:?}");
    assert!(!health.contains(&"headache".to_string()));
    // Used signs first.
    assert!(at(&health, "mal de tête") < at(&health, "constipation"));
    let en = hints(&b, &c, &json!({"lang": "en"}))?;
    let health = low(group(&en, "health"));
    assert!(health.contains(&"headache".to_string()), "{health:?}");
    assert!(!health.contains(&"mal de tête".to_string()));
    // Nobody who cannot read a health space is given health terms.
    let w = caller(&b, "work-only", "work:r")?;
    assert!(group(&hints(&b, &w, &json!({}))?, "health").is_empty());
    Ok(())
}

#[test]
fn the_list_is_cut_at_the_limit_and_the_prompt_stays_short() -> Res {
    let t = Temp::new("hints-limit");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    story(&b, &c)?;
    let mut conn = b.open_space(&sp("work"))?;
    let terms: Vec<(String, String, String)> = (0..600)
        .map(|i| (format!("terme numero {i}"), String::new(), String::new()))
        .collect();
    vocabulary::import(&mut conn, "many", "test", &terms, 1)?;
    let five = hints(&b, &c, &json!({"limit": 5}))?;
    assert_eq!(list(&five, "words").len(), 5, "{five}");
    let default = hints(&b, &c, &json!({}))?;
    assert!(list(&default, "words").len() <= 120);
    assert!(list(&default, "words").len() > 60, "{default}");
    let huge = hints(&b, &c, &json!({"limit": 100_000}))?;
    assert!(list(&huge, "words").len() <= 500);
    assert!(list(&huge, "words").len() > 120);
    for w in list(&huge, "words") {
        assert!(w.chars().count() <= 40 && !w.contains(','), "{w}");
    }
    // About three characters a token for a French text: 220 tokens is well under 700 characters.
    let text = huge["text"].as_str().unwrap_or_default();
    assert!(
        text.chars().count() <= 700,
        "{} chars",
        text.chars().count()
    );
    assert!(text.starts_with(&list(&huge, "words")[0]));
    assert!(text.contains(", "));
    // The words of the text are the first words of the list, in the same order.
    let in_text: Vec<&str> = text.split(", ").collect();
    let words = list(&huge, "words");
    assert_eq!(
        in_text,
        words[..in_text.len()]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn a_caller_gets_hints_from_the_spaces_it_reads_only() -> Res {
    let t = Temp::new("hints-rights");
    let b = brain(&t)?;
    let owner = caller(&b, "owner", "all")?;
    note(
        &b,
        &owner,
        "work",
        "Réunion avec Ysolde Karamazov, puis Ysolde Karamazov rappelle.",
    )?;
    note(
        &b,
        &owner,
        "personal",
        "Dîner chez Bertrand Oulipo, Bertrand Oulipo cuisine.",
    )?;
    note(
        &b,
        &owner,
        "health",
        "Rendez-vous avec le docteur Prosper Valmont, Prosper Valmont a rappelé.",
    )?;
    let all = list(&hints(&b, &owner, &json!({}))?, "words");
    for w in ["Ysolde Karamazov", "Bertrand Oulipo", "Prosper Valmont"] {
        assert!(all.contains(&w.to_string()), "{w}: {all:?}");
    }
    // A key limited to one space sees that space's hints, and no other.
    let only_work = caller(&b, "work-key", "work:r")?;
    let w = list(&hints(&b, &only_work, &json!({}))?, "words");
    assert!(w.contains(&"Ysolde Karamazov".to_string()), "{w:?}");
    assert!(
        !w.iter()
            .any(|x| x.contains("Oulipo") || x.contains("Valmont")),
        "{w:?}"
    );
    // Two spaces read, the sensitive one (health) is not mixed in.
    let two = caller(&b, "two", "work:r,personal:r")?;
    let w = list(&hints(&b, &two, &json!({}))?, "words");
    assert!(w.contains(&"Bertrand Oulipo".to_string()), "{w:?}");
    assert!(!w.iter().any(|x| x.contains("Valmont")), "{w:?}");
    // A space that is not read is refused in the words of every other unreadable thing.
    let err = hints(&b, &only_work, &json!({"space": "health"}));
    assert!(matches!(err, Err(ToolError::Denied(_))), "{err:?}");
    // One space of the owner's, by name.
    let h = list(&hints(&b, &owner, &json!({"space": "personal"}))?, "words");
    assert!(h.contains(&"Bertrand Oulipo".to_string()));
    assert!(!h.iter().any(|x| x.contains("Karamazov")), "{h:?}");
    // A connection limited to paths inside a space is not shown the tool.
    let ctl = b.open_control()?;
    let tok = access::connect_for(
        &ctl,
        OWNER,
        "paths",
        &Rights::parse("work/sources/app:rw")?,
        None,
        false,
        false,
    )?;
    let scoped = access::authenticate(&ctl, &tok, util::now())?.ok_or("refused")?;
    assert!(hints(&b, &scoped, &json!({})).is_err());
    assert!(
        !tools::definitions(&scoped)
            .iter()
            .any(|d| d.name == "transcription_hints")
    );
    Ok(())
}

#[test]
fn the_answer_is_cached_and_rebuilt_when_notes_rules_or_vocabulary_changed() -> Res {
    let t = Temp::new("hints-cache");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    story(&b, &c)?;
    let first = hints(&b, &c, &json!({"space": "personal"}))?;
    let builds = transcription::builds(&b);
    let second = hints(&b, &c, &json!({"space": "personal"}))?;
    assert_eq!(
        transcription::builds(&b),
        builds,
        "nothing changed: no rebuild"
    );
    assert_eq!(first["version"], second["version"]);
    assert_eq!(first["built_at"], second["built_at"]);
    assert_eq!(first["words"], second["words"]);
    // A new note.
    note(
        &b,
        &c,
        "personal",
        "Rencontre avec Ottilie Brandt. Ottilie Brandt revient.",
    )?;
    let third = hints(&b, &c, &json!({"space": "personal"}))?;
    assert!(transcription::builds(&b) > builds);
    assert_ne!(first["version"], third["version"]);
    assert!(list(&third, "words").contains(&"Ottilie Brandt".to_string()));
    // A retired note.
    let again = transcription::builds(&b);
    let found = tools::call(
        &b,
        &c,
        "recall",
        &json!({"query": "Ottilie", "space": "personal"}),
    )?;
    for r in found["results"].as_array().into_iter().flatten() {
        tools::call(&b, &c, "retire", &json!({"id": r["id"], "reason": "test"}))?;
    }
    let fourth = hints(&b, &c, &json!({"space": "personal"}))?;
    assert!(transcription::builds(&b) > again);
    assert!(!list(&fourth, "words").contains(&"Ottilie Brandt".to_string()));
    // The rules.
    let conn = b.open_space(&sp("personal"))?;
    rules::write(
        &conn,
        &sp("personal"),
        "me",
        &Rules {
            tags: vec![],
            keywords: vec!["jardinage".into()],
        },
    )?;
    let fifth = hints(&b, &c, &json!({"space": "personal"}))?;
    assert!(group(&fifth, "keywords").contains(&"jardinage".to_string()));
    // A vocabulary.
    let mut conn = b.open_space(&sp("personal"))?;
    vocabulary::import(
        &mut conn,
        "garden",
        "test",
        &[("sécateur".into(), String::new(), String::new())],
        1,
    )?;
    let sixth = hints(&b, &c, &json!({"space": "personal"}))?;
    assert!(group(&sixth, "vocabulary").contains(&"sécateur".to_string()));
    assert_ne!(fifth["version"], sixth["version"]);
    // Another space changing leaves this one's cache alone, and the whole-person list follows both.
    let builds = transcription::builds(&b);
    note(
        &b,
        &c,
        "work",
        "Visite de Wilhelmine Stark, Wilhelmine Stark est passée.",
    )?;
    let _ = hints(&b, &c, &json!({"space": "personal"}))?;
    assert_eq!(transcription::builds(&b), builds);
    let all = hints(&b, &c, &json!({}))?;
    assert!(list(&all, "words").contains(&"Wilhelmine Stark".to_string()));
    Ok(())
}

#[test]
fn the_tool_is_shown_to_whoever_reads_a_space() -> Res {
    let t = Temp::new("hints-shown");
    let b = brain(&t)?;
    let reader = caller(&b, "reader", "work:r")?;
    let def = tools::definitions(&reader)
        .into_iter()
        .find(|d| d.name == "transcription_hints")
        .ok_or("not shown")?;
    let props = def.schema["properties"]
        .as_object()
        .ok_or("no properties")?;
    let mut keys: Vec<&String> = props.keys().collect();
    keys.sort();
    assert_eq!(keys, ["lang", "limit", "space"]);
    // An empty memory gives an empty, well-formed answer.
    let h = hints(&b, &reader, &json!({}))?;
    assert_eq!(h["words"], json!([]));
    assert_eq!(h["text"], "");
    for g in ["names", "keywords", "vocabulary", "health"] {
        assert_eq!(h["groups"][g], json!([]), "{g}");
    }
    assert!(h["built_at"].is_string() && h["version"].is_u64(), "{h}");
    Ok(())
}
