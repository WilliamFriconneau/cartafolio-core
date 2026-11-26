#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::*;

fn health() -> SpaceInfo {
    SpaceInfo {
        name: "health".into(),
        label: "Santé".into(),
        kind: "health".into(),
    }
}

fn home() -> SpaceInfo {
    SpaceInfo {
        name: "home".into(),
        label: "Maison".into(),
        kind: "home".into(),
    }
}

fn tag(s: &SpaceInfo, text: &str) -> Suggestion {
    suggest(s, &Rules::default(), &[], text)
}

fn field(s: &Suggestion, k: &str) -> Option<String> {
    s.fields.get(k).cloned()
}

#[test]
fn the_tag_of_the_space_comes_first() {
    let s = tag(&home(), "rien de particulier");
    assert_eq!(s.tags, vec!["home"]);
    assert!(s.fields.is_empty());
}

#[test]
fn a_pain_with_its_level_french_and_english() {
    let s = tag(&health(), "douleur 2/10 depuis ce matin");
    assert_eq!(s.tags, vec!["health", "signe/douleur"]);
    assert_eq!(field(&s, "level").as_deref(), Some("2/10"));
    for text in [
        "J'ai une douleur à 7 sur 10",
        "pain 7/10 this morning",
        "pain level 7 out of 10",
        "douleur de 7 / 10",
    ] {
        let s = tag(&health(), text);
        assert!(s.tags.contains(&"signe/douleur".to_string()), "{text}");
        assert_eq!(field(&s, "level").as_deref(), Some("7/10"), "{text}");
    }
}

#[test]
fn a_level_said_in_words_and_the_specific_sign_wins() {
    let s = tag(&health(), "Forte douleur thoracique ce soir");
    assert!(s.tags.contains(&"signe/douleur-thoracique".to_string()));
    assert!(!s.tags.contains(&"signe/douleur".to_string()));
    assert_eq!(field(&s, "level").as_deref(), Some("forte"));
    let s = tag(&health(), "mild headache");
    assert!(s.tags.contains(&"signe/cephalee".to_string()));
    assert_eq!(field(&s, "level").as_deref(), Some("mild"));
    let s = tag(&health(), "mal à la tête et nausées");
    assert!(s.tags.contains(&"signe/cephalee".to_string()));
    assert!(s.tags.contains(&"signe/nausee".to_string()));
    assert!(!s.tags.contains(&"signe/douleur".to_string()));
}

#[test]
fn several_levels_are_named_by_sign() {
    let s = tag(&health(), "douleur 3/10 et fatigue 6/10");
    assert_eq!(field(&s, "level"), None);
    assert_eq!(field(&s, "level_douleur").as_deref(), Some("3/10"));
    assert_eq!(field(&s, "level_fatigue").as_deref(), Some("6/10"));
}

#[test]
fn blood_pressure_as_said() {
    for (text, value) in [
        ("tension 12/8 ce matin", "12/8"),
        ("Ma tension est de 12 sur 8", "12/8"),
        ("tension 120/80 mmHg", "120/80"),
        ("blood pressure 130 over 85", "130/85"),
        ("BP 14/9", "14/9"),
        ("tension : 13 8", "13/8"),
    ] {
        let s = tag(&health(), text);
        assert!(s.tags.contains(&"mesure/tension".to_string()), "{text}");
        assert_eq!(field(&s, "tension").as_deref(), Some(value), "{text}");
    }
    let s = tag(&health(), "tension 120/80 mmHg");
    assert_eq!(field(&s, "tension_unit").as_deref(), Some("mmhg"));
    assert!(s.field_tags().contains(&"field/tension/120/80".to_string()));
}

#[test]
fn weight_pulse_sugar_temperature_with_units_and_decimals() {
    let s = tag(&health(), "poids 72,5 kg");
    assert_eq!(field(&s, "poids").as_deref(), Some("72.5"));
    assert_eq!(field(&s, "poids_unit").as_deref(), Some("kg"));
    assert!(s.field_tags().contains(&"field/poids/72_5".to_string()));
    let s = tag(&health(), "I weigh 80.2 kg today");
    assert_eq!(field(&s, "poids").as_deref(), Some("80.2"));
    let s = tag(&health(), "Pouls 64 bpm au repos");
    assert_eq!(field(&s, "pouls").as_deref(), Some("64"));
    assert_eq!(field(&s, "pouls_unit").as_deref(), Some("bpm"));
    let s = tag(&health(), "heart rate 58");
    assert_eq!(field(&s, "pouls").as_deref(), Some("58"));
    let s = tag(&health(), "glycémie 1,05 g/l à jeun");
    assert_eq!(field(&s, "glycemie").as_deref(), Some("1.05"));
    assert_eq!(field(&s, "glycemie_unit").as_deref(), Some("gl"));
    let s = tag(&health(), "blood sugar 5.8 mmol/l");
    assert_eq!(field(&s, "glycemie").as_deref(), Some("5.8"));
    assert_eq!(field(&s, "glycemie_unit").as_deref(), Some("mmoll"));
    let s = tag(&health(), "température 37.2 °C");
    assert_eq!(field(&s, "temperature").as_deref(), Some("37.2"));
    assert_eq!(field(&s, "temperature_unit").as_deref(), Some("c"));
    let s = tag(&health(), "J'ai 38,5 de fièvre");
    assert!(s.tags.contains(&"mesure/temperature".to_string()));
    assert!(s.tags.contains(&"signe/fievre".to_string()));
    assert_eq!(field(&s, "temperature").as_deref(), Some("38.5"));
    let s = tag(&health(), "fever of 39.1");
    assert_eq!(field(&s, "temperature").as_deref(), Some("39.1"));
    let s = tag(&health(), "saturation 97 %");
    assert_eq!(field(&s, "saturation").as_deref(), Some("97"));
}

#[test]
fn dates_and_times_are_never_values() {
    for text in [
        "tension du 2026-11-17",
        "poids le 17/11/2026",
        "rendez-vous à 12h30, tension à vérifier",
        "pouls mesuré à 12:30",
        "douleur depuis 2026-11-17",
        "le 12h30 douleur",
        "température le 2026-11-17 à 12h30",
    ] {
        let s = tag(&health(), text);
        assert!(s.fields.is_empty(), "{text}: {:?}", s.fields);
        assert!(
            !s.tags.iter().any(|t| t.starts_with("mesure/")),
            "{text}: {:?}",
            s.tags
        );
    }
    // A time next to a sign is no level.
    let s = tag(&health(), "douleur à 12h30");
    assert!(s.fields.is_empty());
}

#[test]
fn numbers_out_of_range_or_far_from_their_word_are_ignored() {
    for text in [
        "poids 5 kg",
        "pouls 400",
        "tension 5/9",
        "la tension monte, rendez-vous dans 3 jours",
        "il pèse lourd, 12 kg de pommes",
        "température 12",
        "glycémie à jeun, et 3 repas",
    ] {
        let s = tag(&health(), text);
        assert!(
            !s.tags.iter().any(|t| t.starts_with("mesure/")),
            "{text}: {:?}",
            s.tags
        );
    }
}

#[test]
fn health_patterns_apply_to_health_spaces_only() {
    let s = tag(&home(), "douleur 2/10 tension 12/8");
    assert_eq!(s.tags, vec!["home"]);
    assert!(s.fields.is_empty());
}

#[test]
fn the_keywords_tags_and_vocabularies_of_the_space() {
    let rules = Rules {
        tags: vec!["home/energie".into()],
        keywords: vec!["zendure".into(), "chaudière".into(), "volets".into()],
        sources: None,
    };
    let s = suggest(
        &home(),
        &rules,
        &[],
        "La batterie Zendure ne charge plus, énergie à vérifier, chaudiere ok",
    );
    assert_eq!(s.tags, vec!["home", "zendure", "chaudiere", "home/energie"]);
    // A vocabulary term: a sign in a health space, a plain tag elsewhere; the plural is the same word.
    let vocab = vec!["céphalée".to_string(), "dyspnée".to_string()];
    let s = suggest(
        &health(),
        &Rules::default(),
        &vocab,
        "Dyspnées et céphalées",
    );
    assert!(s.tags.contains(&"signe/dyspnee".to_string()));
    assert!(s.tags.contains(&"signe/cephalee".to_string()));
    let s = suggest(&home(), &Rules::default(), &vocab, "une céphalée");
    assert!(s.tags.contains(&"cephalee".to_string()));
}

#[test]
fn the_tags_are_bounded() {
    let rules = Rules {
        tags: vec![],
        keywords: (0..40).map(|i| format!("mot{i}")).collect(),
        sources: None,
    };
    let text: String = (0..40).map(|i| format!("mot{i} ")).collect();
    let s = suggest(&home(), &rules, &[], &text);
    assert_eq!(s.tags.len(), MAX_SUGGESTED + 1);
    assert_eq!(s.tags[0], "home");
}

#[test]
fn fields_are_tags_and_back() {
    assert_eq!(
        field_tag("level", "2/10").as_deref(),
        Some("field/level/2/10")
    );
    assert_eq!(
        field_tag("Poids", "72,5").as_deref(),
        Some("field/poids/72_5")
    );
    assert_eq!(field_tag("poids", "").as_deref(), None);
    let tags = vec![
        "health".to_string(),
        "field/tension/12/8".to_string(),
        "field/poids/72_5".to_string(),
    ];
    let f = fields_of(&tags);
    assert_eq!(f["tension"], "12/8");
    assert_eq!(f["poids"], "72.5");
}

#[test]
fn hints_add_and_deny_but_never_take_the_space_tag_out() {
    let s = Suggestion {
        tags: vec!["health".into(), "signe/douleur".into()],
        fields: BTreeMap::new(),
    };
    let hints = Hints {
        add: vec!["suivi/dos".into()],
        deny: ["signe/douleur".to_string(), "health".to_string()].into(),
    };
    let s = s.with_hints(&hints, Some("health"));
    assert_eq!(s.tags, vec!["health", "suivi/dos"]);
}

#[test]
fn it_learns_from_the_person_and_stays_bounded() {
    let dir = std::env::temp_dir().join(format!("autotag-learn-{}", crate::util::new_fact_id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let store = Store::open(&dir);
    let note = "La batterie Zendure est vide, la chaudière aussi";
    // Nothing is known yet; one correction is not enough.
    assert_eq!(store.hints("home", note), Hints::default());
    store.learn_tags("home", note, &["home/energie".to_string()], &[]);
    assert!(store.hints("home", note).add.is_empty());
    store.learn_tags("home", note, &["home/energie".to_string()], &[]);
    assert_eq!(store.hints("home", note).add, vec!["home/energie"]);
    // Another space knows nothing of it.
    assert!(store.hints("work", note).add.is_empty());
    // The person takes it off twice: it goes, and is denied.
    for _ in 0..3 {
        store.learn_tags("home", note, &[], &["home/energie".to_string()]);
    }
    let h = store.hints("home", note);
    assert!(h.add.is_empty());
    assert!(h.deny.contains("home/energie"));
    // The memory's plumbing is never learned.
    assert!(!store.learn_tags(
        "home",
        note,
        &["inbox".to_string(), "sources/x".to_string()],
        &[]
    ));
    // Routes: sent twice to a space, the words lead there; two spaces tied lead nowhere.
    assert!(store.route(note).is_empty());
    store.learn_route(note, "home");
    store.learn_route(note, "home");
    assert_eq!(store.route(note)[0].0, "home");
    store.learn_route(note, "work");
    store.learn_route(note, "work");
    assert!(store.route(note).is_empty());
    // Bounded: far more words than allowed leave at most MAX_WORDS.
    for k in 0..100 {
        let text: String = (0..100).map(|i| format!("mot{k}x{i}aaa ")).collect();
        store.learn_route(&text, "home");
    }
    let data: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("autotag-learned.json")).expect("file"),
    )
    .expect("json");
    assert!(data["routes"].as_object().expect("routes").len() <= learn::MAX_WORDS);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_red_flag_is_tagged_first_with_its_field() {
    let s = tag(&health(), "douleur thoracique depuis ce matin");
    assert_eq!(s.tags[1], "alerte/chest_pain", "{:?}", s.tags);
    assert!(s.tags.contains(&"signe/douleur-thoracique".to_string()));
    assert_eq!(field(&s, "red_flag").as_deref(), Some("chest_pain"));
    assert!(
        s.field_tags()
            .contains(&"field/red_flag/chest_pain".to_string())
    );
    // The 15 comes before the 3114: the first alert is the field.
    let s = tag(&health(), "idées noires, et j'ai pris tous mes cachets");
    assert!(s.tags.contains(&"alerte/suicidal_ideation".to_string()));
    assert!(s.tags.contains(&"alerte/overdose".to_string()));
    assert_eq!(field(&s, "red_flag").as_deref(), Some("overdose"));
    // A denied sign is no alert; and no alert outside a health space.
    assert!(
        !tag(&health(), "pas de douleur thoracique")
            .tags
            .iter()
            .any(|t| t.starts_with("alerte/"))
    );
    assert_eq!(tag(&home(), "douleur thoracique").tags, vec!["home"]);
}

#[test]
fn a_denied_sign_is_not_tagged_and_the_richer_referential_finds_more() {
    let s = tag(&health(), "pas de fièvre aujourd'hui");
    assert_eq!(s.tags, vec!["health"], "{:?}", s.tags);
    let s = tag(
        &health(),
        "j'ai mal au crâne, je suis crevé, j'ai dormi 6h30",
    );
    for t in ["signe/cephalee", "signe/fatigue", "mesure/sommeil"] {
        assert!(s.tags.contains(&t.to_string()), "{t}: {:?}", s.tags);
    }
    assert_eq!(field(&s, "sommeil").as_deref(), Some("6.5"));
    assert_eq!(field(&s, "sommeil_unit").as_deref(), Some("h"));
    // Signs of the referential the first tagger did not have are tagged by their id.
    let s = tag(&health(), "je me sens seul et j'ai honte");
    assert!(s.tags.contains(&"signe/loneliness".to_string()));
    assert!(s.tags.contains(&"signe/shame".to_string()));
    // A level said after a comma goes to the sign of the phrase before.
    let s = tag(&health(), "je suis anxieux, 2 sur 10");
    assert_eq!(field(&s, "level").as_deref(), Some("2/10"));
}

#[test]
fn every_tag_of_the_referential_is_a_valid_tag() {
    for m in pensine_health::builtin().manifestations() {
        assert!(
            crate::facts::normalize_tag(&format!("signe/{}", m.tag)).is_ok(),
            "{}",
            m.tag
        );
    }
    for d in pensine_health::builtin().measure_defs() {
        assert!(
            crate::facts::normalize_tag(&format!("mesure/{}", d.tag)).is_ok(),
            "{}",
            d.tag
        );
    }
    for r in pensine_health::builtin().red_flag_rules().rules() {
        assert!(
            crate::facts::normalize_tag(&format!("alerte/{}", r.id)).is_ok(),
            "{}",
            r.id
        );
    }
}
