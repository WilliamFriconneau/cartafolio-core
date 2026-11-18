#![allow(clippy::expect_used, clippy::unwrap_used)]

//! Red-flag detection: only a real negation denies, doubt fires, a harmless idiom stays quiet but never hides a real case.
//! Fictional sentences only. Ported with the same cases and results as the reference tests of the previous health app.

use memcp_health::{Lang, builtin, detect};

fn ids(text: &str) -> Vec<String> {
    builtin()
        .red_flag_rules()
        .check(text)
        .into_iter()
        .map(|h| h.id)
        .collect()
}

#[test]
fn chest_pain_fires_on_plain_words() {
    assert_eq!(
        ids("J'ai une douleur à la poitrine en montant les escaliers"),
        vec!["chest_pain"]
    );
    assert_eq!(
        ids("douleur thoracique depuis ce matin"),
        vec!["chest_pain"]
    );
    assert_eq!(ids("POITRINE SERRÉE après la séance"), vec!["chest_pain"]);
}

#[test]
fn negated_sign_does_not_fire() {
    assert!(ids("pas de douleur thoracique aujourd'hui").is_empty());
    assert!(ids("Aucune douleur dans la poitrine, juste les jambes").is_empty());
    assert!(ids("séance sans douleur thoracique").is_empty());
    assert!(ids("je n'ai pas d'idées noires").is_empty());
}

#[test]
fn doubt_still_fires() {
    assert_eq!(
        ids("je ne sais pas si c'est une douleur dans la poitrine ou le dos"),
        vec!["chest_pain"]
    );
    assert_eq!(
        ids("peut-être un peu mal à la poitrine ?"),
        vec!["chest_pain"]
    );
}

#[test]
fn negation_does_not_cross_clauses() {
    assert_eq!(
        ids("Pas de douleur thoracique, mais j'ai envie d'en finir"),
        vec!["suicidal_ideation"]
    );
    assert_eq!(
        ids("pas de fièvre. Douleur thoracique à l'effort"),
        vec!["chest_pain"]
    );
}

#[test]
fn negator_must_be_close_to_the_sign() {
    assert_eq!(
        ids("je ne dors pas bien et j'ai une douleur thoracique"),
        vec!["chest_pain"]
    );
}

#[test]
fn suicidal_ideation_fires() {
    assert_eq!(ids("j'ai envie d'en finir"), vec!["suicidal_ideation"]);
    assert_eq!(ids("Idées noires ce soir"), vec!["suicidal_ideation"]);
    assert_eq!(ids("parfois je veux mourir"), vec!["suicidal_ideation"]);
}

#[test]
fn other_flags_fire() {
    assert_eq!(
        ids("Je me suis ÉVANOUI après la séance"),
        vec!["loss_of_consciousness"]
    );
    assert_eq!(
        ids("bouche de travers et je n'arrive plus à parler"),
        vec!["stroke_signs"]
    );
    assert_eq!(
        ids("le pire mal de tête de ma vie"),
        vec!["thunderclap_headache"]
    );
    assert_eq!(ids("j'ai craché du sang ce matin"), vec!["bleeding"]);
    assert_eq!(ids("j'étouffe"), vec!["breathing"]);
    assert_eq!(
        ids("mes lèvres qui gonflent après les cacahuètes"),
        vec!["anaphylaxis"]
    );
}

#[test]
fn ordinary_entries_do_not_fire() {
    for t in [
        "Squat 70x6a8 8 8 9",
        "mal de tête léger en fin de journée",
        "fatigue 7/10, nuit courte",
        "bonne soirée avec des amis",
        "courbatures aux jambes",
        "4x1000 en 4'45, récup 2'",
        "",
    ] {
        assert!(ids(t).is_empty(), "unexpected hit on {t:?}");
    }
}

#[test]
fn words_match_whole_tokens_only() {
    assert!(ids("antisuicidex est une marque inventée").is_empty());
    assert_eq!(
        ids("passé une mauvaise nuit, douleur thoracique"),
        vec!["chest_pain"]
    );
}

#[test]
fn hits_are_deduplicated_and_ordered_by_rule() {
    let hits = builtin()
        .red_flag_rules()
        .check("idées noires, en finir, et douleur à la poitrine");
    let got: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(got, vec!["chest_pain", "suicidal_ideation"]);
    assert!(hits.iter().all(|h| !h.message.is_empty()));
}

#[test]
fn referential_is_well_formed() {
    let set = builtin().red_flag_rules();
    assert!(set.rules().len() >= 8);
    for r in set.rules() {
        assert!(
            !r.patterns.is_empty() || !r.together.is_empty(),
            "{} has no pattern",
            r.id
        );
        assert!(!r.message.is_empty(), "{} has no message", r.id);
        assert!(!r.sources.is_empty(), "{} has no source", r.id);
        for s in &r.sources {
            assert!(
                set.sources().iter().any(|x| &x.id == s),
                "{} cites unknown source {s}",
                r.id
            );
        }
        // messages never name a disease
        let m = r.message.to_lowercase();
        for banned in [
            "infarctus",
            "avc",
            "crise cardiaque",
            "embolie",
            "dépression",
            "anévrisme",
        ] {
            assert!(
                !m.contains(banned),
                "{} message names a disease: {banned}",
                r.id
            );
        }
    }
}

/// Spoken French, as dictation writes it, drops the « ne »: these must fire.
#[test]
fn spoken_french_without_ne_fires() {
    for (text, rule) in [
        ("j'arrive plus à respirer", "breathing"),
        ("j'arrive pas à respirer", "breathing"),
        ("je peux plus respirer", "breathing"),
        ("je respire plus", "breathing"),
        ("j'ai du mal à respirer depuis ce matin", "breathing"),
        ("je n'arrive plus à respirer", "breathing"),
        ("je peux plus parler", "stroke_signs"),
        (
            "j'arrive plus à parler depuis tout à l'heure",
            "stroke_signs",
        ),
        ("les mots sortent plus", "stroke_signs"),
        ("je peux plus lever le bras", "stroke_signs"),
        ("je veux plus vivre", "suicidal_ideation"),
        ("envie de crever", "suicidal_ideation"),
        ("j'ai envie de me foutre en l'air", "suicidal_ideation"),
        ("je vais me faire du mal", "suicidal_ideation"),
        ("j'ai plus envie de vivre", "suicidal_ideation"),
        ("ils seraient mieux sans moi", "suicidal_ideation"),
        ("j'ai pris tous mes cachets", "overdose"),
        ("j'ai avalé toute la boîte", "overdose"),
        ("je vois plus rien de l'œil gauche", "sudden_vision_loss"),
    ] {
        assert!(
            ids(text).contains(&rule.to_string()),
            "{text:?} should fire {rule}, got {:?}",
            ids(text)
        );
    }
}

/// « plus » is « more » as often as « no longer »: a sign that gets worse fires.
#[test]
fn a_sign_that_gets_worse_fires() {
    for text in [
        "J'ai de plus en plus mal à la poitrine",
        "de plus en plus de douleurs thoraciques",
        "j'ai encore plus mal à la poitrine",
        "en plus de la fatigue, douleur thoracique",
        "j'ai plus mal à la poitrine qu'hier",
        "j'ai plus de douleur thoracique",
    ] {
        assert_eq!(ids(text), vec!["chest_pain"], "{text:?}");
    }
}

/// Only a real negation denies: « ne … plus (de) », « pas de », « sans », « aucun ».
#[test]
fn a_real_negation_denies() {
    for text in [
        "je n'ai plus d'idées noires",
        "je n'ai plus de douleur thoracique",
        "il n'y a plus de douleur dans la poitrine",
        "je n'ai plus mal à la poitrine",
        "j'ai pas de douleur thoracique",
        "je n'ai aucun mal à respirer",
        "pas de mal à respirer",
        "je ne me suis pas évanoui",
        "je n'ai pas pris tous mes cachets",
        "pas de vision double",
    ] {
        assert!(ids(text).is_empty(), "{text:?} fired {:?}", ids(text));
    }
}

/// A negation of the verb is not a negation of the sign.
#[test]
fn a_denied_verb_does_not_deny_the_sign() {
    assert_eq!(
        ids("je ne supporte pas la douleur thoracique"),
        vec!["chest_pain"]
    );
    assert_eq!(
        ids("je ne supporte plus la douleur dans la poitrine"),
        vec!["chest_pain"]
    );
    assert_eq!(
        ids("je ne dors pas et j'ai de plus en plus mal à la poitrine"),
        vec!["chest_pain"]
    );
}

/// Idioms that only look like a sign stay quiet.
#[test]
fn harmless_idioms_do_not_fire() {
    for text in [
        "on a eu envie de mourir de rire",
        "je veux mourir de rire devant ce film",
        "hâte d'en finir avec ce dossier",
        "envie d'en finir avec les travaux",
        "j'ai lu un article sur la prévention du suicide",
        "journée mondiale de prévention du suicide au travail",
        "j'ai du mal à parler de mes émotions",
        "je n'arrive plus à parler à ma sœur",
        "intoxication alimentaire après le resto",
        "ce bruit m'empoisonne la vie",
        "je respire plus facilement depuis la séance",
        "je vois plus rien à faire pour ce projet",
        "je me suis cogné la tête, pas vomi",
    ] {
        assert!(ids(text).is_empty(), "{text:?} fired {:?}", ids(text));
    }
}

/// ...but never at the cost of a real case.
#[test]
fn idioms_never_hide_a_real_case() {
    for text in [
        "envie d'en finir avec la vie",
        "envie d'en finir avec tout",
        "envie d'en finir une bonne fois pour toutes",
        "je pense au suicide",
        "je lis sur la prévention du suicide et je veux me suicider",
        "envie de mourir, de rire jamais",
        "j'ai envie de mourir",
    ] {
        assert_eq!(ids(text), vec!["suicidal_ideation"], "{text:?}");
    }
    assert_eq!(
        ids("j'ai du mal à parler, la bouche de travers"),
        vec!["stroke_signs"]
    );
    assert_eq!(ids("je respire plus du tout"), vec!["breathing"]);
}

#[test]
fn new_flags_fire() {
    for (text, rule) in [
        ("des taches violacées et 39 de fièvre", "purpura_fever"),
        (
            "taches qui ne s'effacent pas quand j'appuie. Nuque raide",
            "purpura_fever",
        ),
        ("il a fait des convulsions", "seizure"),
        ("crise d'épilepsie ce matin", "seizure"),
        (
            "elle est désorientée depuis ce midi",
            "confusion_drowsiness",
        ),
        ("impossible à réveiller", "confusion_drowsiness"),
        (
            "je me suis cogné la tête hier. Depuis j'ai vomi deux fois",
            "head_injury",
        ),
        ("choc à la tête et somnolence", "head_injury"),
        ("j'ai pris trop de médicaments", "overdose"),
        ("vision double depuis une heure", "sudden_vision_loss"),
    ] {
        assert!(
            ids(text).contains(&rule.to_string()),
            "{text:?} should fire {rule}, got {:?}",
            ids(text)
        );
    }
}

/// A combination needs every part, and a denied part does not count.
#[test]
fn a_combination_needs_every_part() {
    assert!(ids("des taches violacées sur le bras").is_empty());
    assert!(ids("taches violacées, pas de fièvre").is_empty());
    assert!(ids("coup sur la tête au foot").is_empty());
    assert!(ids("j'ai vomi ce matin").is_empty());
}

/// Every rule says which number comes first: the 3114 for dark thoughts, the 15 for the rest; rules sending to the 15
/// come first, so the first hit gives the first number.
#[test]
fn every_rule_says_which_number_comes_first() {
    let set = builtin().red_flag_rules();
    for r in set.rules() {
        let want = if r.id == "suicidal_ideation" {
            "3114"
        } else {
            "15"
        };
        assert_eq!(r.first, want, "{}", r.id);
    }
    let hits = set.check("je veux mourir, j'ai pris tous mes cachets");
    assert_eq!(hits[0].first, "15");
    let hits = set.check("des idées noires ce soir");
    assert_eq!(hits[0].first, "3114");
}

/// The messages say whom to call, as the sources do; the dark-thoughts message welcomes without asserting.
#[test]
fn messages_say_whom_to_call() {
    let set = builtin().red_flag_rules();
    let msg = |id: &str| {
        set.rules()
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.message.clone())
            .unwrap_or_default()
    };
    for r in set.rules() {
        if r.sources.iter().any(|s| s.starts_with("missing")) {
            continue;
        }
        assert!(
            r.message.contains("le 15 (ou le 112)") || r.message.contains("3114"),
            "{} does not say whom to call",
            r.id
        );
    }
    assert!(msg("stroke_signs").contains("notez l'heure du début"));
    let s = msg("suicidal_ideation");
    assert!(s.starts_with("Ce que vous avez écrit compte."));
    assert!(s.contains("3114"));
    assert!(!s.contains("Vous parlez"));
    assert!(!s.contains("pas seul"));
}

/// Breathless at rest, or unable to finish a sentence, opens the help screen; breathless on the stairs does not.
#[test]
fn breathless_at_rest_fires_and_on_effort_does_not() {
    for text in [
        "Ce matin je suis essoufflée même au repos, assise dans le fauteuil.",
        "essoufflé au repos depuis hier soir",
        "un essoufflement au repos qui ne passe pas",
        "je n'arrive pas à finir mes phrases tellement je souffle",
    ] {
        assert_eq!(ids(text), vec!["breathing"], "{text}");
    }
    for text in [
        "Essoufflée en montant l'escalier de la gare, il a fallu s'arrêter au palier.",
        "pas essoufflée au repos, seulement dans la côte",
    ] {
        assert!(ids(text).is_empty(), "{text}: {:?}", ids(text));
    }
}

#[test]
fn every_sheet_has_unique_questions_and_the_intro_says_answers_change_nothing() {
    let set = builtin().red_flag_rules();
    let chest = set
        .rules()
        .iter()
        .find(|r| r.id == "chest_pain")
        .map(|r| {
            r.characterization
                .iter()
                .map(|q| q.id.as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert_eq!(
        chest,
        vec![
            "radiation",
            "effort",
            "repos",
            "inspiration",
            "depuis_quand"
        ]
    );
    for rule in set.rules() {
        let mut seen: Vec<&str> = Vec::new();
        for q in &rule.characterization {
            assert!(
                !q.id.trim().is_empty() && !q.text.trim().is_empty(),
                "{}",
                rule.id
            );
            assert!(
                !seen.contains(&q.id.as_str()),
                "{} repeats {}",
                rule.id,
                q.id
            );
            seen.push(&q.id);
        }
    }
    assert!(set.characterization_intro().contains("ne changent rien"));
}

#[test]
fn detect_carries_the_red_flag_with_its_number_and_only_in_french() {
    let d = detect("douleur thoracique depuis ce matin", Lang::Fr);
    assert_eq!(d.red_flags.len(), 1);
    assert_eq!(d.red_flags[0].id, "chest_pain");
    assert_eq!(d.red_flags[0].first, "15");
    // No English or Spanish red-flag vocabulary exists (docs/HEALTH.md): nothing is claimed.
    assert!(detect("douleur thoracique", Lang::En).red_flags.is_empty());
    assert!(detect("douleur thoracique", Lang::Es).red_flags.is_empty());
    assert_eq!(detect("douleur thoracique", Lang::Auto).red_flags.len(), 1);
}

#[test]
fn an_invalid_referential_is_rejected() {
    use memcp_health::{Referential, Sources};
    let src = |red: &'static str| Sources {
        manifestations: include_str!("../data/manifestations.json"),
        manifestations_fr: include_str!("../data/manifestations.fr.json"),
        manifestations_en: include_str!("../data/manifestations.en.json"),
        measures: include_str!("../data/measures.json"),
        vitals: include_str!("../data/vitals.json"),
        red_flags_fr: red,
    };
    assert!(Referential::from_sources(&src("{}")).is_err());
    assert!(Referential::from_sources(&src("not json")).is_err());
    let one = |rule: &str| {
        format!(r#"{{"schema":"memcp.health.red_flags/1","version":1,"rules":[{rule}]}}"#)
    };
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    // no number first
    assert!(
        Referential::from_sources(&src(leak(one(
            r#"{"id":"a","patterns":["x"],"message":"m","first":"","sources":["s"]}"#
        ))))
        .is_err()
    );
    // a combination of one group
    assert!(
        Referential::from_sources(&src(leak(one(
            r#"{"id":"a","together":[["x"]],"message":"m","first":"15","sources":["s"]}"#
        ))))
        .is_err()
    );
    assert!(
        Referential::from_sources(&src(leak(one(
            r#"{"id":"a","together":[["x"],["y"]],"message":"m","first":"15","sources":["s"]}"#
        ))))
        .is_ok()
    );
}
