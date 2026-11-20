#![allow(clippy::expect_used, clippy::unwrap_used)]

//! The shape of the shipped referential: versions, provenance, unique ids, and the compact view for apps.

use std::collections::BTreeSet;

use memcp_health::{Lang, builtin};

#[test]
fn every_part_has_a_version_a_licence_and_a_provenance_line() {
    let parts: Vec<&str> = builtin()
        .provenance()
        .iter()
        .map(|p| p.part.as_str())
        .collect();
    assert_eq!(
        parts,
        [
            "manifestations",
            "manifestations.fr",
            "manifestations.en",
            "measures",
            "vitals",
            "red_flags"
        ]
    );
    for p in builtin().provenance() {
        assert!(p.version >= 1, "{}", p.part);
        assert!(!p.provenance.trim().is_empty(), "{}", p.part);
    }
    assert!(
        builtin()
            .provenance()
            .iter()
            .filter(|p| p.part != "manifestations.en")
            .all(|p| p.license == "AGPL-3.0-or-later"
                || p.part.ends_with(".fr")
                || p.part == "manifestations"),
    );
}

#[test]
fn the_source_has_eighty_three_signs_and_the_additions_are_marked() {
    let all = builtin().manifestations();
    assert_eq!(all.iter().filter(|m| !m.added).count(), 83);
    assert_eq!(all.iter().filter(|m| m.added).count(), 3);
    let ids: BTreeSet<&str> = all.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids.len(), all.len(), "ids are unique");
    assert!(all.iter().all(|m| m.labels.contains_key("fr")));
}

#[test]
fn measures_and_vitals_have_units_and_plausible_ranges() {
    let ids: Vec<&str> = builtin()
        .measure_defs()
        .iter()
        .map(|d| d.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "blood_pressure",
            "weight",
            "heart_rate",
            "glucose",
            "temperature",
            "saturation",
            "sleep"
        ]
    );
    assert_eq!(builtin().vitals().len(), 9);
    for v in builtin().vitals() {
        assert!(
            !v.unit.is_empty() && v.plausible.min < v.plausible.max,
            "{}",
            v.id
        );
        for l in ["fr", "en", "es"] {
            assert!(v.labels.contains_key(l), "{} has no {l} label", v.id);
        }
    }
    let sat = builtin()
        .vitals()
        .iter()
        .find(|v| v.id == "oxygen_saturation")
        .expect("vital");
    assert_eq!(sat.usual_limit.as_ref().and_then(|l| l.below), Some(94.0));
}

#[test]
fn the_compact_view_gives_ids_labels_and_keywords() {
    let v = builtin().compact(Some("manifestations"), Some("migraine"), Lang::Fr);
    let list = v["manifestations"].as_array().expect("list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"], "headache");
    assert_eq!(list[0]["label"], "Mal de tête");
    assert!(
        list[0]["keywords"]["fr"]
            .as_array()
            .expect("kw")
            .iter()
            .any(|k| k == "migraine")
    );
    assert!(v.get("measures").is_none());
    // Accents and case are folded in the query.
    let v = builtin().compact(Some("measures"), Some("GLYCEMIE"), Lang::Auto);
    assert_eq!(v["measures"][0]["id"], "glucose");
    let v = builtin().compact(Some("vitals"), Some("oxygen"), Lang::En);
    assert_eq!(v["vitals"][0]["id"], "oxygen_saturation");
    let v = builtin().compact(Some("red_flags"), None, Lang::Fr);
    assert_eq!(v["red_flags"].as_array().expect("list").len(), 15);
    // Left out: every kind.
    let v = builtin().compact(None, None, Lang::Auto);
    for k in [
        "manifestations",
        "measures",
        "vitals",
        "red_flags",
        "versions",
    ] {
        assert!(v.get(k).is_some(), "{k}");
    }
    assert_eq!(v["manifestations"].as_array().expect("list").len(), 86);
}

#[test]
fn a_health_app_constant_the_referential_has_no_measure_for_has_a_name_and_a_unit() {
    let (label, unit) = memcp_health::extra_measure("hrv_sdnn_ms").expect("listed");
    assert_eq!(
        (label.as_str(), unit.as_str()),
        ("Variabilité cardiaque (VFC)", "ms")
    );
    assert!(memcp_health::extra_measure("not_a_constant").is_none());
}
