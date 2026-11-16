//! The vaults proposed by « Commencer une mémoire », each with clear tags and words
//! (docs of the core plan, part 4). Changeable before the person confirms. Local by default;
//! Santé and Argent are sensitive. No person's name is shipped.

use crate::rules::{Rule, RuleKind};
use crate::space::{DEFAULT_SPACE, Space, Tree};

struct Preset {
    id: &'static str,
    name: &'static str,
    sensitive: bool,
    tags: &'static [&'static str],
    unique: &'static [&'static str],
    ordinary: &'static [&'static str],
}

const PRESETS: &[Preset] = &[
    Preset {
        id: "a-trier",
        name: "À trier",
        sensitive: false,
        tags: &[],
        unique: &[],
        ordinary: &[],
    },
    Preset {
        id: "moi",
        name: "Moi",
        sensitive: false,
        tags: &[
            "journal",
            "idees",
            "objectifs",
            "a-faire",
            "decisions",
            "lectures",
        ],
        unique: &["#moi", "#perso"],
        ordinary: &["idée", "envie", "objectif", "je retiens"],
    },
    Preset {
        id: "sante",
        name: "Santé",
        sensitive: true,
        tags: &[
            "journal",
            "signes",
            "mesures",
            "traitements",
            "rendez-vous",
            "sommeil",
            "activite",
            "alimentation",
        ],
        unique: &["#santé", "#sante", "#health"],
        ordinary: &[
            "douleur",
            "mal à",
            "fatigue",
            "fièvre",
            "médecin",
            "ordonnance",
            "médicament",
            "pharmacie",
            "kiné",
            "vaccin",
            "prise de sang",
            "tension",
            "migraine",
            "insomnie",
        ],
    },
    Preset {
        id: "proches",
        name: "Proches",
        sensitive: false,
        tags: &["famille", "amis", "anniversaires", "cadeaux", "nouvelles"],
        unique: &["#proches"],
        ordinary: &["anniversaire", "cadeau", "appeler", "nouvelles de"],
    },
    Preset {
        id: "maison",
        name: "Maison",
        sensitive: false,
        tags: &[
            "travaux",
            "equipements",
            "courses",
            "jardin",
            "vehicule",
            "domotique",
            "papiers",
        ],
        unique: &["#maison"],
        ordinary: &[
            "chaudière",
            "plombier",
            "électricien",
            "courses",
            "jardin",
            "voiture",
            "contrôle technique",
            "garantie",
            "home assistant",
        ],
    },
    Preset {
        id: "argent",
        name: "Argent",
        sensitive: true,
        tags: &[
            "banque",
            "factures",
            "impots",
            "abonnements",
            "budget",
            "remboursements",
        ],
        unique: &["#argent", "#finances"],
        ordinary: &[
            "facture",
            "impôts",
            "banque",
            "virement",
            "prélèvement",
            "abonnement",
            "loyer",
            "remboursement",
        ],
    },
    Preset {
        id: "travail",
        name: "Travail",
        sensitive: false,
        tags: &[
            "reunions",
            "decisions",
            "projets",
            "a-faire",
            "idees",
            "contacts",
        ],
        unique: &["#travail", "#pro", "#work"],
        ordinary: &[
            "réunion",
            "client",
            "projet",
            "compte rendu",
            "échéance",
            "ticket",
        ],
    },
    Preset {
        id: "loisirs",
        name: "Loisirs",
        sensitive: false,
        tags: &["lecture", "sport", "musique", "voyages", "bricolage", "dev"],
        unique: &["#loisirs"],
        ordinary: &[
            "livre",
            "film",
            "randonnée",
            "voyage",
            "concert",
            "bricolage",
        ],
    },
];

/// The default tree: « À trier » is the default space, then the vaults above, with their rules. `stamp`
/// gives an increasing clock; ids of rules are `<space>/<kind>/<n>`, stable across devices.
pub fn default_tree(mut stamp: impl FnMut() -> String) -> Tree {
    let mut tree = Tree::default();
    for p in PRESETS {
        let mut s = Space::default_notes(p.name);
        // « À trier » is the core's default space: what nothing decided goes there.
        let id = if p.id == "a-trier" {
            DEFAULT_SPACE
        } else {
            p.id
        };
        s.id = id.into();
        s.path = p.id.into();
        s.sensitive = p.sensitive;
        if tree.save(s).is_err() {
            continue;
        }
        for (kind, values) in [
            (RuleKind::Tag, p.tags),
            (RuleKind::UniqueWord, p.unique),
            (RuleKind::OrdinaryWord, p.ordinary),
        ] {
            for (n, v) in values.iter().enumerate() {
                let kind_name = match kind {
                    RuleKind::Tag => "tag",
                    RuleKind::UniqueWord => "unique",
                    RuleKind::OrdinaryWord => "ordinary",
                };
                tree.put_rule(Rule {
                    id: format!("{}/{kind_name}/{n}", p.id),
                    space: id.into(),
                    kind,
                    value: (*v).into(),
                    by: "person".into(),
                    reason: String::new(),
                    hlc: stamp(),
                    retired: false,
                });
            }
        }
    }
    tree
}
