//! Shared rules, and what an assistant may change by itself.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_spaces::*;

fn hlc(ms: u64) -> String {
    Hlc {
        ms,
        counter: 0,
        node: "n".into(),
    }
    .encode()
}

fn rule(id: &str, space: &str, kind: RuleKind, value: &str, ms: u64) -> Rule {
    Rule {
        id: id.into(),
        space: space.into(),
        kind,
        value: value.into(),
        by: "assistant:rangeur".into(),
        reason: "vu trois fois".into(),
        hlc: hlc(ms),
        retired: false,
    }
}

fn space(id: &str, sensitive: bool, level: AssistantLevel, delivers: bool) -> Space {
    let mut s = Space::default_notes(id);
    s.id = id.into();
    s.path = id.into();
    s.sensitive = sensitive;
    s.assistant_level = level;
    if delivers {
        s.connectors = vec![ConnectorConfig {
            id: "drive".into(),
            kind: ConnectorKind::Drive,
            mode: Mode::Export,
            target: String::new(),
            icloud: false,
        }];
    }
    s
}

fn tree() -> Tree {
    Tree::new(vec![
        space("perso", false, AssistantLevel::Add, false),
        space("pro", false, AssistantLevel::Add, true),
        space("sante", true, AssistantLevel::Full, false),
        space("dev", false, AssistantLevel::Full, false),
        space("idees", false, AssistantLevel::Propose, false),
    ])
    .unwrap()
}

#[test]
fn the_most_recent_rule_wins_whatever_the_order_received() {
    let mut t = tree();
    t.put_rule(rule("r", "perso", RuleKind::OrdinaryWord, "jardin", 2));
    t.put_rule(rule("r", "perso", RuleKind::OrdinaryWord, "potager", 1));
    assert_eq!(t.words("perso", RuleKind::OrdinaryWord), ["jardin"]);
    t.put_rule(Rule {
        retired: true,
        ..rule("r", "perso", RuleKind::OrdinaryWord, "jardin", 3)
    });
    assert!(t.words("perso", RuleKind::OrdinaryWord).is_empty());
    assert_eq!(t.rules_version(), Some(hlc(3)));
}

#[test]
fn add_applies_an_ordinary_word_or_a_tag_on_a_quiet_space_only() {
    let mut t = tree();
    assert_eq!(
        t.rule_propose(
            "c1",
            rule("a", "perso", RuleKind::OrdinaryWord, "jardin", 1)
        ),
        Ok(ChangeState::Applied)
    );
    assert_eq!(
        t.rule_propose("c2", rule("b", "perso", RuleKind::Tag, "maison", 2)),
        Ok(ChangeState::Applied)
    );
    // Waits: a unique word, a word that sends out of the node, a removal.
    assert_eq!(
        t.rule_propose("c3", rule("c", "perso", RuleKind::UniqueWord, "Bosch", 3)),
        Ok(ChangeState::Pending)
    );
    assert_eq!(
        t.rule_propose("c4", rule("d", "pro", RuleKind::OrdinaryWord, "devis", 4)),
        Ok(ChangeState::Pending)
    );
    assert_eq!(
        t.rule_propose("c5", rule("e", "pro", RuleKind::Tag, "client", 5)),
        Ok(ChangeState::Applied)
    );
    let gone = Rule {
        retired: true,
        ..rule("a", "perso", RuleKind::OrdinaryWord, "jardin", 6)
    };
    assert_eq!(t.rule_propose("c6", gone), Ok(ChangeState::Pending));
    assert_eq!(t.words("perso", RuleKind::OrdinaryWord), ["jardin"]);
}

#[test]
fn the_sensitive_space_and_propose_always_wait_full_changes_elsewhere() {
    let mut t = tree();
    assert_eq!(
        t.rule_propose(
            "c1",
            rule("a", "sante", RuleKind::OrdinaryWord, "migraine", 1)
        ),
        Ok(ChangeState::Pending)
    );
    assert_eq!(
        t.rule_propose("c2", rule("b", "idees", RuleKind::Tag, "livre", 2)),
        Ok(ChangeState::Pending)
    );
    assert_eq!(
        t.rule_propose("c3", rule("c", "dev", RuleKind::OrdinaryWord, "rust", 3)),
        Ok(ChangeState::Applied)
    );
    let gone = Rule {
        retired: true,
        ..rule("c", "dev", RuleKind::OrdinaryWord, "rust", 4)
    };
    assert_eq!(t.rule_propose("c4", gone), Ok(ChangeState::Applied));
    assert!(t.words("dev", RuleKind::OrdinaryWord).is_empty());
}

#[test]
fn the_person_decides_and_undoes_in_one_gesture() {
    let mut t = tree();
    t.rule_propose(
        "c1",
        rule("a", "sante", RuleKind::OrdinaryWord, "migraine", 1),
    )
    .unwrap();
    t.change_decide("c1", true, &hlc(2)).unwrap();
    assert_eq!(t.words("sante", RuleKind::OrdinaryWord), ["migraine"]);
    t.rule_propose(
        "c2",
        rule("b", "perso", RuleKind::OrdinaryWord, "jardin", 3),
    )
    .unwrap();
    t.rule_undo("c2", &hlc(4)).unwrap();
    assert!(t.words("perso", RuleKind::OrdinaryWord).is_empty());
    let history: Vec<ChangeState> = t.rule_history("perso").iter().map(|c| c.state).collect();
    assert_eq!(history, [ChangeState::Undone]);
    assert_eq!(t.rule_history("perso")[0].rule.by, "assistant:rangeur");
}

#[test]
fn an_assistant_is_limited_per_space_and_per_day() {
    let mut t = tree();
    let day = 1_700_000_000_000u64;
    for i in 0..DAILY_CHANGES as u64 {
        let state = t
            .rule_propose(
                &format!("c{i}"),
                rule(
                    &format!("r{i}"),
                    "perso",
                    RuleKind::Tag,
                    &format!("t{i}"),
                    day + i,
                ),
            )
            .unwrap();
        assert_eq!(state, ChangeState::Applied);
    }
    assert_eq!(
        t.rule_propose("more", rule("x", "perso", RuleKind::Tag, "x", day + 100)),
        Ok(ChangeState::Pending)
    );
}

#[test]
fn rules_and_changes_live_in_the_spaces_file() {
    let mut t = tree();
    t.rule_propose(
        "c1",
        rule("a", "perso", RuleKind::OrdinaryWord, "jardin", 1),
    )
    .unwrap();
    let back = parse_spaces(&spaces_text(&t), "Notes").unwrap();
    assert_eq!(back.words("perso", RuleKind::OrdinaryWord), ["jardin"]);
    assert_eq!(back.changes().len(), 1);
}
