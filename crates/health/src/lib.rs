//! The health referential: signs and symptoms, measures, vital constants and red flags, as versioned data files embedded
//! in the program, and the deterministic detection of them in a text (`detect`). No model, no I/O, no clock.
//!
//! ```ignore
//! use memcp_health::{Lang, detect};
//! let d = detect("Mal de tête 6/10, tension 12/8", Lang::Fr);
//! assert_eq!(d.signs[0].id, "headache");
//! assert_eq!(d.signs[0].level.as_deref(), Some("6/10"));
//! assert_eq!(d.measures[0].id, "blood_pressure");
//! assert_eq!(d.measures[0].value, 120.0);
//! ```
//!
//! What a result means is a suggestion, never a diagnosis; the data files say where each part comes from (`docs/HEALTH.md`).

mod measures;
mod model;
mod redflags;
mod signs;
pub mod text;

use std::sync::OnceLock;

use serde::Serialize;
use serde_json::{Value, json};

pub use measures::Measure;
pub use model::{
    Error, Kind, Lang, Manifestation, MeasureDef, Pair, Provenance, Range, Referential, Scale,
    Sources, UnitDef, UsualLimit, Vital,
};
pub use redflags::{Question, RedFlagHit, RedFlags, Rule, Source};
pub use signs::Sign;

use text::{fold, tokenize};

const MANIFESTATIONS: &str = include_str!("../data/manifestations.json");
const MANIFESTATIONS_FR: &str = include_str!("../data/manifestations.fr.json");
const MANIFESTATIONS_EN: &str = include_str!("../data/manifestations.en.json");
const MEASURES: &str = include_str!("../data/measures.json");
const VITALS: &str = include_str!("../data/vitals.json");
const RED_FLAGS_FR: &str = include_str!("../data/red_flags.fr.json");

/// What a text says.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct Detection {
    pub signs: Vec<Sign>,
    pub measures: Vec<Measure>,
    pub red_flags: Vec<RedFlagHit>,
}

const MEASURE_LABELS: &str = include_str!("../data/measure_labels.json");

/// The name and the unit of a number a health app sends that the referential has no measure for (an Apple Health constant
/// such as `hrv_sdnn_ms`), by its key. `None` for a key it does not list.
pub fn extra_measure(key: &str) -> Option<(String, String)> {
    static LABELS: OnceLock<std::collections::BTreeMap<String, (String, String)>> = OnceLock::new();
    let map = LABELS.get_or_init(|| {
        let v: serde_json::Value = serde_json::from_str(MEASURE_LABELS).unwrap_or_default();
        v["labels"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, e)| {
                        (
                            k.clone(),
                            (
                                e["fr"].as_str().unwrap_or("").to_string(),
                                e["unit"].as_str().unwrap_or("").to_string(),
                            ),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    });
    map.get(key).cloned()
}

/// The referential shipped with this program.
pub fn builtin() -> &'static Referential {
    static BUILTIN: OnceLock<Result<Referential, Error>> = OnceLock::new();
    match BUILTIN.get_or_init(|| {
        Referential::from_sources(&Sources {
            manifestations: MANIFESTATIONS,
            manifestations_fr: MANIFESTATIONS_FR,
            manifestations_en: MANIFESTATIONS_EN,
            measures: MEASURES,
            vitals: VITALS,
            red_flags_fr: RED_FLAGS_FR,
        })
    }) {
        Ok(r) => r,
        // The files are embedded and tested: this is a build mistake, never a runtime condition.
        Err(e) => panic!("the embedded {e}"),
    }
}

/// The signs, measures and red flags of a text, with the shipped referential.
pub fn detect(text: &str, lang: Lang) -> Detection {
    builtin().detect(text, lang)
}

impl Referential {
    /// The signs (with their levels), the measures and the red flags of a text. `Lang::Es` finds nothing: the referential has
    /// no Spanish vocabulary.
    pub fn detect(&self, text: &str, lang: Lang) -> Detection {
        let toks = tokenize(text);
        let red_flags = if matches!(lang, Lang::Auto | Lang::Fr) {
            self.red_flags.check(text)
        } else {
            Vec::new()
        };
        Detection {
            signs: self.signs_of(&toks, lang),
            measures: self.measures_of(&toks, lang),
            red_flags,
        }
    }

    pub fn manifestations(&self) -> &[Manifestation] {
        &self.manifestations
    }

    pub fn scales(&self) -> &[Scale] {
        &self.scales
    }

    pub fn pairs(&self) -> &[Pair] {
        &self.pairs
    }

    /// The opposite of a sign that belongs to a pair of opposites.
    pub fn opposite(&self, id: &str) -> Option<&str> {
        self.pairs.iter().find_map(|p| {
            if p.weighs == id {
                Some(p.helps.as_str())
            } else if p.helps == id {
                Some(p.weighs.as_str())
            } else {
                None
            }
        })
    }

    pub fn measure_defs(&self) -> &[MeasureDef] {
        &self.measures
    }

    pub fn vitals(&self) -> &[Vital] {
        &self.vitals
    }

    pub fn red_flag_rules(&self) -> &RedFlags {
        &self.red_flags
    }

    /// Version, licence and provenance of each part.
    pub fn provenance(&self) -> &[Provenance] {
        &self.provenance
    }

    /// A compact view for apps: ids, labels and keywords. `kind` is `manifestations`, `measures`, `vitals` or `red_flags`
    /// (all four when left out); `query` keeps the entries whose id, label or a keyword holds those words (folded);
    /// `lang` keeps the keywords of one language (all by default).
    pub fn compact(&self, kind: Option<&str>, query: Option<&str>, lang: Lang) -> Value {
        let q = query
            .map(fold)
            .map(|q| q.trim().to_string())
            .filter(|q| !q.is_empty());
        let hit = |parts: &[&str]| {
            q.as_ref()
                .is_none_or(|q| parts.iter().any(|p| fold(p).contains(q.as_str())))
        };
        let want = |k: &str| kind.is_none_or(|x| x == k);
        let tables: &[usize] = match lang {
            Lang::Es => &[],
            l => l.tables(),
        };
        let mut out = json!({
            "versions": self.provenance.iter().map(|p| (p.part.clone(), json!(p.version))).collect::<serde_json::Map<_, _>>(),
            "lang": lang.code(),
        });
        if want("manifestations") {
            let list: Vec<Value> = self
                .manifestations
                .iter()
                .filter_map(|m| {
                    let mut kw = serde_json::Map::new();
                    let mut all: Vec<&str> = vec![m.id.as_str()];
                    for t in tables {
                        let code = model::CODES[*t];
                        let mut v: Vec<&str> = m.keywords.get(code).into_iter().flatten().map(String::as_str).collect();
                        v.extend(m.keywords_added.get(code).into_iter().flatten().map(String::as_str));
                        all.extend(&v);
                        if let Some(l) = m.labels.get(code) {
                            all.push(l);
                        }
                        kw.insert(code.to_string(), json!(v));
                    }
                    hit(&all).then(|| {
                        json!({
                            "id": m.id, "label": m.label(lang), "scale": m.scale, "category": m.category,
                            "dimension": m.dimension, "keywords": kw,
                        })
                    })
                })
                .collect();
            out["manifestations"] = json!(list);
            if q.is_none() {
                out["pairs"] = json!(self.pairs);
            }
        }
        if want("measures") {
            let list: Vec<Value> = self
                .measures
                .iter()
                .filter_map(|d| {
                    let mut kw = serde_json::Map::new();
                    let mut all: Vec<String> = vec![d.id.clone()];
                    all.extend(d.labels.values().cloned());
                    for t in tables {
                        let v: Vec<String> = d.keywords[*t].iter().map(|w| w.join(" ")).collect();
                        all.extend(v.clone());
                        kw.insert(model::CODES[*t].to_string(), json!(v));
                    }
                    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
                    hit(&refs).then(|| {
                        json!({
                            "id": d.id, "unit": d.unit, "labels": d.labels, "keywords": kw,
                            "units": d.units.iter().map(|u| json!({ "id": u.id, "label": u.label })).collect::<Vec<_>>(),
                            "vital": d.vital,
                        })
                    })
                })
                .collect();
            out["measures"] = json!(list);
        }
        if want("vitals") {
            let list: Vec<&Vital> = self
                .vitals
                .iter()
                .filter(|v| {
                    let mut all: Vec<&str> = vec![v.id.as_str()];
                    all.extend(v.labels.values().map(String::as_str));
                    hit(&all)
                })
                .collect();
            out["vitals"] = json!(list);
        }
        if want("red_flags") {
            let list: Vec<Value> = self
                .red_flags
                .rules()
                .iter()
                .filter(|r| {
                    let mut all: Vec<&str> = vec![r.id.as_str()];
                    all.extend(r.patterns.iter().map(String::as_str));
                    hit(&all)
                })
                .map(|r| {
                    json!({ "id": r.id, "first": r.first, "message": r.message, "keywords": if matches!(lang, Lang::En | Lang::Es) { json!([]) } else { json!(r.patterns) } })
                })
                .collect();
            out["red_flags"] = json!(list);
        }
        out
    }
}
