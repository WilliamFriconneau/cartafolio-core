//! The language a note is read in: French alone when it looks French, every language together otherwise.

use pensine_health::{Lang, lang_of};

#[test]
fn a_french_note_is_read_in_french() {
    for text in [
        "Très fatigué ce matin, mal au crâne depuis hier.",
        "J'ai mal au dos depuis hier, je dors mal.",
        "Fatigue et douleur au genou, ça dure.",
        "Réveil à 3 h, anxiété avant le travail.",
        "Rien à signaler, juste un peu de fatigue.",
        "Courses du samedi : pain, lait, café.",
        "Pain au chocolat ce matin, et une douleur au ventre.",
    ] {
        assert_eq!(lang_of(text), Lang::Fr, "{text}");
    }
}

#[test]
fn a_note_of_another_language_is_read_in_every_language() {
    for text in [
        "Pain in my knee since Monday.",
        "The pain is worse today, I can't sleep.",
        "Started the day with a headache, the pain is worse after lunch.",
        "me duele la cabeza, dolor de cabeza",
        "Kopfschmerzen und Müdigkeit seit gestern",
    ] {
        assert_eq!(lang_of(text), Lang::Auto, "{text}");
    }
}

#[test]
fn a_note_with_nothing_to_read_says_nothing() {
    // A note of a few words without a mark of any language is read in every language (nothing to find anyway).
    for text in ["RAS", "Douleur 6/10", "12/8 tension"] {
        assert_eq!(lang_of(text), Lang::Auto, "{text}");
    }
}

#[test]
fn a_french_word_of_another_sense_is_not_a_sign() {
    // « pain » is bread: a French note that says it is not a note about pain.
    let d = pensine_health::detect(
        "Courses : pain, lait, café.",
        lang_of("Courses : pain, lait, café."),
    );
    assert!(
        !d.signs.iter().any(|s| s.id == "pain"),
        "{:?}",
        d.signs.iter().map(|s| s.tag.clone()).collect::<Vec<_>>()
    );
    // An English note that says pain is a note about pain.
    let text = "Pain in my knee since Monday.";
    let d = pensine_health::detect(text, lang_of(text));
    assert!(d.signs.iter().any(|s| s.id == "pain"), "{d:?}");
}
