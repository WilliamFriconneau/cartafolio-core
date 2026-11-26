#![allow(clippy::expect_used, clippy::unwrap_used)]

//! A person who is not a nurse writes how she feels, in her own words: does the referential find the signs (recall), and
//! does it stay quiet when she denies or says something else? Everyday French, fictional. Ported with the same cases and
//! results as the reference tests of the previous health app. A suggestion only ever proposes.

use std::collections::BTreeSet;

use pensine_health::{Lang, builtin, detect};

fn found(text: &str) -> Vec<String> {
    builtin().suggest(text, Lang::Fr)
}

/// (what she writes, a sign that must be suggested)
const SAYS: &[(&str, &str)] = &[
    ("j'ai mal au crâne depuis ce matin", "headache"),
    ("j'ai la migraine", "headache"),
    ("je suis crevé ce soir", "fatigue"),
    ("je suis claquée", "fatigue"),
    ("je suis épuisé", "fatigue"),
    ("j'ai pas dormi de la nuit", "sleep_changes"),
    (
        "je me réveille à trois heures et je ne me rendors pas",
        "sleep_changes",
    ),
    ("insomnie encore cette nuit", "sleep_changes"),
    ("j'ai le cœur qui bat très fort", "palpitations"),
    ("des palpitations en montant les escaliers", "palpitations"),
    ("j'ai du mal à respirer", "shortness_of_breath"),
    ("je suis essoufflé dès que je marche", "shortness_of_breath"),
    ("j'ai mal au dos, en bas", "lower_back_pain"),
    ("mon lumbago est revenu", "lower_back_pain"),
    ("j'ai envie de vomir", "nausea"),
    ("j'ai la nausée depuis hier", "nausea"),
    ("j'ai vomi deux fois", "vomiting"),
    ("la tête qui tourne quand je me lève", "dizziness"),
    ("j'ai des vertiges", "dizziness"),
    ("j'ai le nez qui coule", "runny_nose"),
    ("je tousse depuis trois jours", "coughing"),
    ("j'ai mal à la gorge", "sore_throat"),
    ("j'ai de la fièvre", "fever"),
    ("j'ai 38,5 de température", "fever"),
    ("j'ai des frissons", "chills"),
    ("je suis très stressé au travail", "anxiety"),
    ("je suis angoissé", "anxiety"),
    ("j'ai le moral à zéro", "sadness"),
    ("je suis triste sans raison", "sadness"),
    ("plus envie de rien", "loss_of_pleasure"),
    ("tout m'énerve en ce moment", "irritability"),
    ("je n'arrive pas à me concentrer", "poor_concentration"),
    ("je me sens seul", "loneliness"),
    ("j'ai le ventre ballonné", "bloating"),
    ("j'ai la diarrhée", "diarrhea"),
    ("je suis constipé", "constipation"),
    ("j'ai des brûlures d'estomac", "heartburn"),
    ("j'ai mal partout", "generalized_body_ache"),
    ("j'ai des courbatures", "generalized_body_ache"),
    ("je suis raide ce matin", "stiffness"),
    ("j'ai des bouffées de chaleur", "hot_flashes"),
    ("des sueurs la nuit", "night_sweats"),
    ("je n'ai plus d'appétit", "appetite_changes"),
    ("j'oublie tout en ce moment", "memory_lapse"),
    ("j'ai mal au ventre", "abdominal_cramps"),
    ("j'ai perdu l'odorat", "loss_of_smell"),
    ("j'ai le nez bouché", "sinus_congestion"),
    ("j'ai honte de ce que j'ai dit", "shame"),
    ("je culpabilise", "guilt"),
    ("je suis en colère", "anger"),
    ("j'ai peur de demain", "fear"),
    ("je suis découragé", "discouragement"),
    ("je me sens nul", "self_devaluation"),
    ("j'ai la gorge qui gratte", "sore_throat"),
    ("j'ai le souffle court", "shortness_of_breath"),
    ("j'ai des douleurs au genou", "pain"),
    ("j'ai mal aux dents", "pain"),
    ("ça tire dans la nuque", "pain"),
    ("j'ai la peau sèche", "dry_skin"),
    ("je perds mes cheveux", "hair_loss"),
    ("j'ai eu un malaise", "fainting"),
    ("j'ai mal aux seins avant mes règles", "breast_pain"),
    (
        "j'ai le cœur qui saute de temps en temps",
        "skipped_heartbeat",
    ),
    ("ça siffle quand je respire", "wheezing"),
    (
        "j'ai des fuites urinaires quand je tousse",
        "bladder_incontinence",
    ),
    ("poussée de boutons sur le front", "acne"),
];

/// Written after the keywords were tuned on the first list and not looked at while tuning.
const HELD_OUT: &[(&str, &str)] = &[
    ("j'ai un mal de tête horrible", "headache"),
    ("je suis mort de fatigue", "fatigue"),
    ("grosse fatigue depuis lundi", "fatigue"),
    ("je dors très mal en ce moment", "sleep_changes"),
    (
        "j'ai le cœur qui s'emballe dès que je stresse",
        "palpitations",
    ),
    ("mal de dos depuis la salle de sport", "lower_back_pain"),
    ("j'ai la nausée quand je mange gras", "nausea"),
    ("j'ai des remontées acides", "heartburn"),
    ("je tousse la nuit", "coughing"),
    ("j'ai une angine", "sore_throat"),
    ("je suis fiévreux", "fever"),
    ("j'ai peur, j'angoisse pour demain", "anxiety"),
    ("je suis déprimé", "sadness"),
    ("je n'ai goût à rien", "loss_of_pleasure"),
    ("je suis à cran", "irritability"),
    (
        "j'ai la tête ailleurs, impossible de me concentrer",
        "poor_concentration",
    ),
    ("je me sens isolé", "loneliness"),
    ("j'ai le ventre gonflé", "bloating"),
    ("j'ai la courante", "diarrhea"),
    ("j'ai du mal à aller à la selle", "constipation"),
    ("mon corps est tout endolori", "generalized_body_ache"),
    ("je suis ankylosé", "stiffness"),
    ("je transpire la nuit", "night_sweats"),
    ("je n'ai pas faim du tout", "appetite_changes"),
    ("je perds la mémoire", "memory_lapse"),
    ("crampes d'estomac", "abdominal_cramps"),
    ("je n'ai plus de goût", "loss_of_taste"),
    ("nez pris", "sinus_congestion"),
    ("j'ai eu un vertige", "dizziness"),
    ("je me suis évanoui", "fainting"),
    ("je suis crevé, mais content", "joy"),
    ("quelle bonne journée, je me sens bien", "wellbeing"),
    ("je suis soulagé", "relief"),
    ("je suis reconnaissant", "gratitude"),
    ("j'ai la boule au ventre", "anxiety"),
    (
        "la fatigue sociale, j'ai besoin d'être seul",
        "social_battery_empty",
    ),
    ("j'ai des douleurs dans les articulations", "pain"),
];

/// (what she writes, a sign that must NOT be suggested)
const DENIES: &[(&str, &str)] = &[
    ("pas de fièvre aujourd'hui", "fever"),
    ("aucune douleur ce matin", "pain"),
    ("sans nausée", "nausea"),
    ("jamais de migraine", "headache"),
    ("pas de toux", "coughing"),
    ("pas d'essoufflement", "shortness_of_breath"),
    ("je n'ai pas de vertiges", "dizziness"),
    ("il n'y a pas de fièvre", "fever"),
];

/// Everyday sentences about something else.
const OTHER: &[&str] = &[
    "j'ai fait les courses et cuisiné des pâtes",
    "belle journée, promenade au parc avec le chien",
    "réunion à 14 h avec l'équipe, rien de spécial",
    "j'ai regardé un film ce soir",
    "rendez-vous chez le coiffeur demain",
    "ma sœur arrive samedi",
];

#[test]
fn everyday_words_are_found() {
    let mut misses = Vec::new();
    for (text, code) in SAYS.iter().chain(HELD_OUT) {
        if !found(text).iter().any(|c| c == code) {
            misses.push(format!("« {text} » → {code}"));
        }
    }
    let total = SAYS.len() + HELD_OUT.len();
    let recall = 1.0 - misses.len() as f64 / total as f64;
    assert!(
        recall >= 0.95,
        "recall {:.0} %: {misses:#?}",
        recall * 100.0
    );
}

#[test]
fn a_denied_sign_is_not_suggested() {
    for (text, code) in DENIES {
        let f = found(text);
        assert!(
            !f.iter().any(|c| c == code),
            "« {text} » suggests {code}: {f:?}"
        );
    }
}

#[test]
fn a_sentence_about_something_else_suggests_nothing() {
    for text in OTHER {
        let f = found(text);
        assert!(f.is_empty(), "« {text} » suggests {f:?}");
    }
}

#[test]
fn every_suggestion_is_a_known_sign_and_unique() {
    let known: BTreeSet<&str> = builtin()
        .manifestations()
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    for (text, _) in SAYS {
        let f = found(text);
        let unique: BTreeSet<&String> = f.iter().collect();
        assert_eq!(unique.len(), f.len(), "« {text} » repeats: {f:?}");
        assert!(f.iter().all(|c| known.contains(c.as_str())), "{f:?}");
    }
}

/// The specific one brings the general one with it (mental fatigue is fatigue too), after it, once.
#[test]
fn a_specific_sign_brings_the_general_one() {
    for (text, specific, general) in [
        (
            "grosse fatigue mentale ce soir",
            "mental_fatigue",
            "fatigue",
        ),
        (
            "fatigue physique depuis lundi",
            "physical_fatigue",
            "fatigue",
        ),
        (
            "de l'irritabilité toute la journée",
            "irritability",
            "mood_changes",
        ),
        ("je suis en colère contre lui", "anger", "mood_changes"),
    ] {
        let f = found(text);
        let at = |c: &str| f.iter().position(|x| x == c);
        assert!(
            at(specific).is_some() && at(general).is_some(),
            "« {text} » → {f:?}"
        );
        assert!(at(specific) < at(general), "the specific first: {f:?}");
        assert_eq!(
            f.iter().filter(|c| c.as_str() == general).count(),
            1,
            "{f:?}"
        );
    }
    assert!(found("je suis de mauvaise humeur").contains(&"mood_changes".to_string()));
    assert!(found("pas de fatigue mentale").is_empty());
}

/// « Mal de dent... ça lance » brings up a pain, not nothing; « mal au ventre » stays the belly's.
#[test]
fn a_toothache_and_a_sore_knee_bring_up_a_pain() {
    assert!(
        found("Mal de dent depuis hier soir, ça lance quand je mâche")
            .contains(&"pain".to_string())
    );
    assert!(found("J'ai mal au genou gauche").contains(&"pain".to_string()));
}

#[test]
fn a_longer_words_sign_keeps_its_own() {
    let c = found("mal au ventre ce matin");
    assert!(c.contains(&"abdominal_cramps".to_string()));
    assert!(!c.contains(&"pain".to_string()), "got {c:?}");
}

#[test]
fn a_tooth_with_a_swollen_cheek_opens_the_help() {
    let flags = detect("Mal de dent depuis deux jours, la joue gonfle", Lang::Fr).red_flags;
    assert!(flags.iter().any(|f| f.id == "dental_swelling"), "{flags:?}");
    assert!(
        detect("Mal de dent, ça lance", Lang::Fr)
            .red_flags
            .is_empty()
    );
}

/// Pleasure lost, pleasure found again: two sides of one bar.
#[test]
fn pleasure_lost_and_found_again_are_one_bar() {
    assert_eq!(
        found("plus envie de rien depuis un mois"),
        vec!["loss_of_pleasure"]
    );
    assert_eq!(
        found("j'ai retrouvé du plaisir à cuisiner"),
        vec!["pleasure"]
    );
    assert!(
        builtin()
            .pairs()
            .iter()
            .any(|p| p.weighs == "loss_of_pleasure" && p.helps == "pleasure")
    );
    assert_eq!(builtin().opposite("pleasure"), Some("loss_of_pleasure"));
    assert_eq!(builtin().opposite("fever"), None);
}

#[test]
fn a_sign_tied_to_a_red_flag_has_no_opposite() {
    for id in [
        "chest_pain",
        "fainting",
        "shortness_of_breath",
        "dark_thoughts",
    ] {
        assert_eq!(builtin().opposite(id), None, "{id}");
    }
}

#[test]
fn the_words_that_brought_a_sign_up_are_kept() {
    let w = builtin().suggest_with_words("j'ai mal au crâne", Lang::Fr);
    assert!(w.contains(&("headache".to_string(), "mal au crane".to_string())));
}

#[test]
fn english_signs_and_a_language_the_referential_lacks() {
    let en = builtin().suggest("mild headache and nausea, no fever", Lang::En);
    assert_eq!(en, vec!["headache", "nausea"]);
    // French words are not English ones, and Spanish has no vocabulary at all.
    assert!(builtin().suggest("j'ai de la fièvre", Lang::En).is_empty());
    assert!(
        builtin()
            .suggest("me duele la cabeza, migraine, headache", Lang::Es)
            .is_empty()
    );
    assert!(detect("fever 38.5", Lang::Es).measures.is_empty());
    // The label follows the words that brought the sign up.
    let d = detect("headache et fatigue", Lang::Auto);
    assert_eq!(d.signs[0].label, "Headache");
    assert_eq!(d.signs[1].label, "Fatigue");
}

#[test]
fn an_ordinary_shopping_need_is_not_an_urge_to_buy() {
    let ids = |text: &str| -> Vec<String> {
        builtin()
            .detect(text, Lang::Fr)
            .signs
            .iter()
            .map(|s| s.id.clone())
            .collect()
    };
    assert!(
        !ids("J'ai besoin d'acheter du lait et du pain").contains(&"purchase_urge".to_string())
    );
    assert!(
        ids("Grosse envie d'acheter une montre ce soir").contains(&"purchase_urge".to_string())
    );
    assert!(ids("Achat compulsif hier soir").contains(&"purchase_urge".to_string()));
}

#[test]
fn a_refused_sign_is_named_and_read_back() {
    assert_eq!(
        pensine_health::refused_sign("signe/fatigue").as_deref(),
        Some("signe-refuse/fatigue")
    );
    assert_eq!(
        pensine_health::sign_of_refusal("signe-refuse/fatigue").as_deref(),
        Some("signe/fatigue")
    );
    // Only a sign is refused, and only a mark comes back: a measure or a plain tag does not.
    assert!(pensine_health::refused_sign("mesure/poids").is_none());
    assert!(pensine_health::sign_of_refusal("signe/fatigue").is_none());
    // A sign and its mark never collide.
    assert_ne!(
        pensine_health::refused_sign("signe/fatigue"),
        Some("signe/fatigue".to_string())
    );
}
