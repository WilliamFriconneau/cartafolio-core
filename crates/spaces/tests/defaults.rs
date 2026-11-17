//! The vaults proposed at « Commencer une mémoire », and hashtags as explicit keywords.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use memcp_spaces::*;

fn stamp() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static MS: AtomicU64 = AtomicU64::new(1);
    Hlc::tick(None, MS.fetch_add(1, Ordering::Relaxed), "t").encode()
}

#[test]
fn the_default_vaults_have_clear_tags_and_health_and_money_are_sensitive() {
    let t = default_tree(stamp);
    let ids: Vec<&str> = t.spaces().iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "notes", "moi", "sante", "proches", "maison", "argent", "travail", "loisirs"
        ]
    );
    assert!(t.get("sante").unwrap().sensitive && t.get("argent").unwrap().sensitive);
    assert!(
        t.words("maison", RuleKind::Tag)
            .contains(&"domotique".to_string())
    );
    assert!(t.words("notes", RuleKind::Tag).is_empty());
    assert_eq!(
        (
            t.get("notes").unwrap().name.as_str(),
            t.get("notes").unwrap().path.as_str()
        ),
        ("Inbox", "a-trier")
    );
}

#[test]
fn a_hashtag_files_the_note_and_the_plain_word_does_not() {
    let t = default_tree(stamp);
    assert_eq!(
        route(&t, "Rappel #maison : la chaudière", false),
        Routing::Spaces(vec!["maison".into()])
    );
    // « maison » without the hash is only a word; « chaudière » suggests Maison, nothing decides.
    assert_eq!(
        route(&t, "je rentre à la maison ce soir", false),
        Routing::Undecided
    );
    // #santé = #sante = #Santé.
    assert_eq!(
        route(&t, "#Santé tension 13/8", false),
        Routing::Spaces(vec!["sante".into()])
    );
    assert_eq!(
        route(&t, "#sante tension 13/8", false),
        Routing::Spaces(vec!["sante".into()])
    );
    // Two hashtags, two vaults; an unknown one breaks nothing.
    assert_eq!(
        route(&t, "#travail #loisirs dev du soir", false),
        Routing::Spaces(vec!["travail".into(), "loisirs".into()])
    );
    assert_eq!(route(&t, "#inconnu rien", false), Routing::Undecided);
    assert_eq!(hashtags("Voir #Santé, puis #work."), ["sante", "work"]);
}
