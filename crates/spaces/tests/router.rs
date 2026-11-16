//! The keyword router (design: « Le routeur de mots-clés »).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use memcp_spaces::*;

fn space(id: &str, path: &str, sensitive: bool) -> Space {
    let mut s = Space::default_notes(id);
    s.id = id.into();
    s.path = path.into();
    s.sensitive = sensitive;
    s
}

fn words(t: &mut Tree, id: &str, unique: &[&str], ordinary: &[&str]) {
    let mut n = 0u32;
    let mut next = || {
        n += 1;
        (format!("{id}-{n}"), format!("{:013}-{:06}-test", 1, n))
    };
    let u: Vec<String> = unique.iter().map(|s| s.to_string()).collect();
    let o: Vec<String> = ordinary.iter().map(|s| s.to_string()).collect();
    t.set_words(id, RuleKind::UniqueWord, &u, &mut next)
        .unwrap();
    t.set_words(id, RuleKind::OrdinaryWord, &o, &mut next)
        .unwrap();
}

fn tree() -> Tree {
    let mut t = Tree::new(vec![
        space("sante", "sante", true),
        space("travail", "travail", false),
        space("client", "travail/client-a", false),
        space("finances", "finances", false),
    ])
    .unwrap();
    words(&mut t, "sante", &["kiné"], &["mal", "fatigue", "dormi"]);
    words(&mut t, "travail", &["Acme"], &["réunion", "devis"]);
    words(&mut t, "client", &["Dupont et fils", "dupond"], &[]);
    words(&mut t, "finances", &["banque"], &["facture"]);
    t
}

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_unique_word_decides_without_case_or_accents_on_whole_words() {
    let t = tree();
    assert_eq!(
        route(&t, "Rappeler ACME demain", false),
        Routing::Spaces(ids(&["travail"]))
    );
    assert_eq!(
        route(&t, "rdv kine jeudi", false),
        Routing::Spaces(ids(&["sante"]))
    );
    // A whole word only: « acmeX » is not « Acme ».
    assert_eq!(route(&t, "acmex", false), Routing::Undecided);
}

#[test]
fn a_phrase_is_searched_as_a_phrase_and_spellings_count() {
    let t = tree();
    assert_eq!(
        route(&t, "devis pour Dupont et fils", false),
        Routing::Spaces(ids(&["client"]))
    );
    assert_eq!(
        route(&t, "appel Dupond", false),
        Routing::Spaces(ids(&["client"]))
    );
    assert_eq!(route(&t, "Dupont seul", false), Routing::Undecided);
}

#[test]
fn unique_words_of_several_spaces_send_to_each() {
    let t = tree();
    assert_eq!(
        route(&t, "Acme : virement à la banque", false),
        Routing::Spaces(ids(&["travail", "finances"]))
    );
}

#[test]
fn a_sensitive_space_wins_alone() {
    let t = tree();
    // Never health in the work folder.
    assert_eq!(
        route(&t, "kiné avant la réunion Acme", false),
        Routing::Spaces(ids(&["sante"]))
    );
    // An ordinary word of the sensitive space decides when no unique word is found.
    assert_eq!(
        route(&t, "mal au dos ce matin", false),
        Routing::Spaces(ids(&["sante"]))
    );
    // Even an ordinary health word beside a work name: health alone.
    assert_eq!(
        route(&t, "Acme m'a fatigué", false),
        Routing::Spaces(ids(&["sante"]))
    );
}

#[test]
fn ordinary_words_only_suggest() {
    let t = tree();
    assert_eq!(
        route(&t, "préparer le devis", false),
        Routing::Suggested(ids(&["travail"]))
    );
    assert_eq!(route(&t, "rien de spécial", false), Routing::Undecided);
}

#[test]
fn a_distress_note_stays_on_the_device() {
    // law: S1
    let t = tree();
    assert_eq!(route(&t, "Acme kiné banque", true), Routing::KeptLocal);
}

#[test]
fn a_parent_gets_its_childrens_notes_only_when_asked() {
    let mut t = tree();
    assert_eq!(
        route(&t, "Dupond", false),
        Routing::Spaces(ids(&["client"]))
    );
    let mut travail = t.get("travail").unwrap().clone();
    travail.includes_children = true;
    t.save(travail).unwrap();
    assert_eq!(
        route(&t, "Dupond", false),
        Routing::Spaces(ids(&["travail", "client"]))
    );
}

#[test]
fn the_tree_refuses_bad_or_repeated_paths() {
    let mut t = tree();
    assert!(t.add(space("x", "travail", false)).is_err());
    assert!(t.add(space("y", "a//b", false)).is_err());
    assert!(t.add(space("sante", "autre", false)).is_err());
    let mut empty = space("z", "z", false);
    empty.name = " ".into();
    assert!(t.add(empty).is_err());
}

#[test]
fn a_sensitive_space_shares_only_with_the_spaces_the_person_chose() {
    let mut t = tree();
    assert_eq!(
        route(&t, "kiné avant la réunion Acme", false),
        Routing::Spaces(ids(&["sante"]))
    );
    let mut sante = t.get("sante").unwrap().clone();
    sante.shares_with = vec!["travail".into()];
    t.save(sante).unwrap();
    assert_eq!(
        route(&t, "kiné avant la réunion Acme", false),
        Routing::Spaces(ids(&["sante", "travail"]))
    );
    // Not the bank: it was not chosen.
    assert_eq!(
        route(&t, "kiné, Acme et banque", false),
        Routing::Spaces(ids(&["sante", "travail"]))
    );
}

#[test]
fn a_space_and_its_sub_space_both_get_the_note_when_both_words_are_said() {
    let t = tree();
    assert_eq!(
        route(&t, "Acme : appel de Dupond", false),
        Routing::Spaces(ids(&["travail", "client"]))
    );
}

#[test]
fn two_spaces_may_share_a_word_on_purpose_and_it_is_shown() {
    let mut t = tree();
    let mut next = {
        let mut n = 100;
        move || {
            n += 1;
            (format!("x{n}"), format!("{:013}-{:06}-test", 2, n))
        }
    };
    t.set_words(
        "finances",
        RuleKind::UniqueWord,
        &["banque".into(), "Acme".into()],
        &mut next,
    )
    .unwrap();
    let r = route(&t, "facture Acme", false);
    assert_eq!(r, Routing::Spaces(ids(&["travail", "finances"])));
    assert_eq!(
        shared_word_spaces(&t, "facture Acme", &ids(&["travail", "finances"])),
        ids(&["travail", "finances"])
    );
    assert_eq!(
        shared_word_spaces(&t, "Acme et la banque", &ids(&["travail", "finances"])),
        ids(&["travail"])
    );
    assert_eq!(word_also_in(&t, "travail", "acme"), ids(&["finances"]));
}
