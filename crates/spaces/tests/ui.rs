//! The settings screens as data: the same everywhere, in three languages.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use memcp_spaces::ui::*;

#[test]
fn every_text_exists_in_french_english_and_spanish() {
    for s in screens() {
        for key in text_keys(s) {
            for lang in LANGUAGES {
                assert_ne!(text(&key, lang), key, "{key} in {lang}");
            }
        }
    }
    for lang in LANGUAGES {
        assert_ne!(text("common.required", lang), "common.required");
    }
}

#[test]
fn ids_are_unique_choices_are_full_and_conditions_point_to_a_field() {
    for s in screens() {
        let fields: Vec<&Field> = s.sections.iter().flat_map(|x| &x.fields).collect();
        let mut ids: Vec<&str> = fields.iter().map(|f| f.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), fields.len(), "{}", s.id);
        for f in &fields {
            assert_eq!(
                f.kind == FieldKind::Choice,
                !f.options.is_empty(),
                "{}",
                f.id
            );
            if let Some(w) = &f.when {
                let target = w.split('=').next().unwrap();
                assert!(ids.contains(&target), "{} when {w}", f.id);
            }
            for p in &f.platforms {
                assert!(PLATFORMS.contains(&p.as_str()), "{p}");
            }
        }
    }
}

#[test]
fn the_outline_is_what_a_renderer_must_show_in_order() {
    let space = screen("space").unwrap();
    let o = outline(space);
    assert_eq!(
        &o[..3],
        ["section:identity", "field:name:text", "field:path:path"]
    );
    let health = space
        .sections
        .iter()
        .flat_map(|s| &s.fields)
        .find(|f| f.id == "sends_apple_health")
        .unwrap();
    assert!(unavailable(space, health, "ios", "fr").is_none());
    assert_eq!(
        unavailable(space, health, "windows", "fr").unwrap(),
        "Apple Santé n'existe que sur iPhone."
    );
    let name = &space.sections[0].fields[0];
    assert!(validate(name, " ", "en").is_some());
    assert!(
        validate(name, &"x".repeat(61), "es")
            .unwrap()
            .contains("60")
    );
    assert!(validate(name, "Pro", "fr").is_none());
}
