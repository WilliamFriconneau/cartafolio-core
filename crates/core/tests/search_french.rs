//! Search in French (its own test binary: the language is set once per process).

mod common;

use common::{Res, Temp, sp};
use memcp_core::Brain;
use memcp_core::facts::{self, NewFact, RecallOptions, Remembered};
use memcp_core::text::{self, Language};

#[test]
fn french_accents_plurals_and_stopwords() -> Res {
    assert!(text::set_language(Language::French));
    let t = Temp::new("fr");
    let b = Brain::init(&t.path().join("data"), &[sp("notes")], Language::French)?;
    let space = sp("notes");
    let conn = b.open_space(&space)?;
    for s in [
        "L'énergie solaire alimente les chevaux de la Boulangerie de la Lune.",
        "Le Dr Martin reçoit le mardi matin.",
    ] {
        let r = facts::remember(&conn, &space, "owner", NewFact::agent(s, "agent:test"), &[])?;
        assert!(matches!(r, Remembered::Created(_)));
    }
    let n = |q: &str| -> Result<usize, Box<dyn std::error::Error>> {
        let opts = RecallOptions {
            limit: 10,
            ..Default::default()
        };
        Ok(facts::recall(&conn, q, &opts, 1_900_000_000)?.len())
    };
    assert_eq!(n("energie")?, 1, "accents");
    assert_eq!(n("cheval")?, 1, "irregular plural");
    assert_eq!(n("martin mardi")?, 1);
    assert_eq!(n("le la les")?, 0, "stopwords only");
    Ok(())
}
