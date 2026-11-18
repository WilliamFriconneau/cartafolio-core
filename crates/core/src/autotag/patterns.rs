//! The health patterns of the tagger: signs and symptoms (with the level as said), measures (with the value as said) and
//! red flags, read from the health referential (`memcp-health`, see docs/HEALTH.md). Small, deterministic, no model.
//!
//! The referential does the reading: a number counts only next to its word, a date and a clock time are never a value, a
//! denied sign (`pas de fièvre`) is not tagged. Here the result becomes tags and fields:
//!
//! - `alerte/<id>` for a red flag, and the field `red_flag` with the first one (the 15 before the 3114);
//! - `signe/<tag>` for a sign (the tag is the id of the referential, or the older name some signs keep), with the field
//!   `level` for one level, or `level_<sign>` when several signs have one;
//! - `mesure/<tag>` for a measure, with the field `<tag>` (the value as said) and `<tag>_unit` when a unit was said.

use std::collections::BTreeMap;

use memcp_health::{Lang, detect};

pub(super) struct Found {
    pub tags: Vec<String>,
    pub fields: BTreeMap<String, String>,
}

/// Finds the red flags, the signs and the measures of a text. The red flags come first, so that the cap on the tags a
/// note gets never drops one.
pub(super) fn health(text: &str) -> Found {
    let found = detect(text, Lang::Auto);
    let mut tags: Vec<String> = Vec::new();
    let mut fields = BTreeMap::new();
    for flag in &found.red_flags {
        tags.push(format!("alerte/{}", flag.id));
    }
    if let Some(first) = found.red_flags.first() {
        fields.insert("red_flag".to_string(), first.id.clone());
    }
    for sign in &found.signs {
        tags.push(format!("signe/{}", sign.tag));
    }
    let levels: Vec<(String, &str)> = found
        .signs
        .iter()
        .filter_map(|s| s.level.as_deref().map(|l| (s.tag.replace('-', "_"), l)))
        .collect();
    match levels.as_slice() {
        [] => {}
        [(_, l)] => {
            fields.insert("level".to_string(), (*l).to_string());
        }
        many => {
            for (slug, l) in many {
                fields.insert(format!("level_{slug}"), (*l).to_string());
            }
        }
    }
    for m in &found.measures {
        tags.push(format!("mesure/{}", m.tag));
        fields.insert(m.tag.clone(), m.raw.clone());
        if let Some(u) = &m.said_unit {
            fields.insert(format!("{}_unit", m.tag), u.clone());
        }
    }
    Found { tags, fields }
}

/// The tags of the signs the referential knows (`signe/<tag>` is the tag each one gives), each once, in its order.
pub(super) fn sign_slugs() -> Vec<&'static str> {
    let mut out: Vec<&str> = Vec::new();
    for m in memcp_health::builtin().manifestations() {
        if !out.contains(&m.tag.as_str()) {
            out.push(m.tag.as_str());
        }
    }
    out
}

/// The tags of the measures the referential knows, each once, in its order.
pub(super) fn measure_slugs() -> Vec<&'static str> {
    let mut out: Vec<&str> = Vec::new();
    for m in memcp_health::builtin().measure_defs() {
        if !out.contains(&m.tag.as_str()) {
            out.push(m.tag.as_str());
        }
    }
    out
}

/// The French and English labels of a sign or a measure of the referential, by its tag.
pub(super) fn labels(tag: &str) -> Option<(String, String)> {
    let r = memcp_health::builtin();
    let pick = |l: &BTreeMap<String, String>| {
        let fr = l
            .get("fr")
            .cloned()
            .unwrap_or_else(|| tag.replace('-', " "));
        let en = l.get("en").cloned().unwrap_or_else(|| fr.clone());
        (fr, en)
    };
    r.manifestations()
        .iter()
        .find(|m| m.tag == tag)
        .map(|m| pick(&m.labels))
        .or_else(|| {
            r.measure_defs()
                .iter()
                .find(|m| m.tag == tag)
                .map(|m| pick(&m.labels))
        })
}
