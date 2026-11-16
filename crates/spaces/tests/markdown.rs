//! A note as a Markdown file.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use memcp_spaces::*;

#[test]
fn the_file_name_is_dated_and_made_of_the_first_words() {
    assert_eq!(
        file_name(
            "2026-11-16T08:05:00+01:00",
            "Rappeler l'équipe d'Acme, demain matin sans faute"
        ),
        "2026-11-16-0805-rappeler-l-equipe-d-acme.md"
    );
    assert_eq!(
        file_name("2026-11-16T08:05:00+01:00", "  …  "),
        "2026-11-16-0805-note.md"
    );
}

#[test]
fn the_file_has_a_yaml_header_then_the_text_as_written() {
    let f = note_file(
        "2026-11-16T08:05:00+01:00",
        &["Travail".into(), "Client \"A\"".into()],
        "Devis à envoyer.\nAvant midi.",
    );
    assert_eq!(
        f.content,
        "---\ndate: \"2026-11-16T08:05:00+01:00\"\nspaces: [\"Travail\", \"Client \\\"A\\\"\"]\nsource: kalios\n---\n\nDevis à envoyer.\nAvant midi.\n"
    );
}
