//! The health referential: signs and symptoms, measures, vital constants and red flags, as versioned data files embedded
//! in the program, and the deterministic detection of them in a text (`detect`). No model, no I/O, no clock.
//!
//! ```ignore
//! use pensine_health::{Lang, detect};
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

/// The tag under which a note keeps the sign its person took off: `signe-refuse/<sign>`, beside the note's other tags.
/// The tagger of the node and the janitor's night sweep leave that sign alone on that note; adding the sign again takes
/// the tag off. A note carries one mark per refused sign, never the sign itself.
pub const REFUSED_SIGN_PREFIX: &str = "signe-refuse/";

/// The mark of a refused sign, from the tag of the sign (`signe/fatigue` gives `signe-refuse/fatigue`).
pub fn refused_sign(sign_tag: &str) -> Option<String> {
    sign_tag
        .strip_prefix("signe/")
        .map(|s| format!("{REFUSED_SIGN_PREFIX}{s}"))
}

/// The sign a mark refuses (`signe-refuse/fatigue` gives `signe/fatigue`), `None` for a tag that is not a mark.
pub fn sign_of_refusal(mark: &str) -> Option<String> {
    mark.strip_prefix(REFUSED_SIGN_PREFIX)
        .map(|s| format!("signe/{s}"))
}

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

/// The language the referential reads a text in: French when the text looks French, every language together otherwise.
///
/// French is read alone so that a word of both languages in one sense only (`pain`, bread in French) is not taken for the
/// other's sign. The mark of French is an accent, or the words only French uses: one of them in a short note, two in a
/// longer one, and never when the text looks like another language (`the`, `and`, `mi`…). It never answers English: with
/// every language open the English signs are found too, and the red flags are read whatever the language.
pub fn lang_of(text: &str) -> Lang {
    /// Words that are French (and not a word an English note says all the same).
    const FRENCH: [&str; 57] = [
        "je",
        "j'ai",
        "tu",
        "nous",
        "vous",
        "ils",
        "elles",
        "avec",
        "pour",
        "dans",
        "mais",
        "pas",
        "très",
        "toujours",
        "jamais",
        "encore",
        "aujourd'hui",
        "hier",
        "demain",
        "matin",
        "soir",
        "journée",
        "ça",
        "c'est",
        "n'est",
        "qu'il",
        "qu'elle",
        "être",
        "avoir",
        "fait",
        "suis",
        "était",
        "depuis",
        "sans",
        "sous",
        "entre",
        "vers",
        "chez",
        "beaucoup",
        "trop",
        "peu",
        "rien",
        "bien",
        "oui",
        "non",
        "mon",
        "ma",
        "mes",
        "au",
        "aux",
        "du",
        "des",
        "une",
        "les",
        "et",
        "que",
        "qui",
    ];
    /// Words of another language: enough of them, and the note is not read as French.
    const ELSEWHERE: [&str; 48] = [
        "the", "and", "with", "my", "i'm", "i've", "this", "that", "was", "were", "been", "have",
        "has", "is", "are", "you", "your", "they", "their", "of", "it's", "don't", "can't", "not",
        "but", "for", "from", "still", "again", "what", "when", "where", "she", "we", "in", "at",
        "would", "could", "should", "because", "there", "here", "el", "los", "las", "pero", "muy",
        "sono",
    ];
    let lower = text.to_lowercase().replace('’', "'");
    if lower.chars().any(|c| "àâçéèêëîïôùûœ".contains(c)) {
        return Lang::Fr;
    }
    let words: Vec<&str> = lower
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .collect();
    let count = |list: &[&str]| -> usize {
        list.iter()
            .filter(|m| words.iter().any(|w| w == *m))
            .count()
    };
    let (fr, elsewhere) = (count(&FRENCH), count(&ELSEWHERE));
    // Two French words say it; one carries a short note (a few words jotted down), unless another language shows.
    let looks = fr >= 2 || (fr == 1 && words.len() <= 8 && elsewhere == 0);
    if looks && fr > elsewhere {
        Lang::Fr
    } else {
        Lang::Auto
    }
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
