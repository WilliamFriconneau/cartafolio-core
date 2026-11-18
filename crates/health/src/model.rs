//! The referential as data: what the JSON files say, validated, and the shape the detection reads.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::text::{fold, words};

/// The language of a text. The referential has French everywhere, English for a part of the signs and the measures, and
/// no Spanish vocabulary (see `docs/HEALTH.md`): `Es` is accepted and finds nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    /// Every language the referential has, together.
    Auto,
    Fr,
    En,
    Es,
}

impl Lang {
    /// `auto`, `fr`, `en` or `es`.
    pub fn parse(code: &str) -> Option<Lang> {
        match code.trim().to_lowercase().as_str() {
            "auto" | "" => Some(Lang::Auto),
            "fr" => Some(Lang::Fr),
            "en" => Some(Lang::En),
            "es" => Some(Lang::Es),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::Auto => "auto",
            Lang::Fr => "fr",
            Lang::En => "en",
            Lang::Es => "es",
        }
    }

    /// The vocabularies this language reads (indexes: 0 French, 1 English).
    pub(crate) fn tables(self) -> &'static [usize] {
        match self {
            Lang::Auto => &[0, 1],
            Lang::Fr => &[0],
            Lang::En => &[1],
            Lang::Es => &[],
        }
    }
}

pub(crate) const FR: usize = 0;
pub(crate) const EN: usize = 1;
pub(crate) const CODES: [&str; 2] = ["fr", "en"];

/// Error loading a referential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid health referential: {}", self.0)
    }
}

impl std::error::Error for Error {}

fn bad<T>(msg: impl Into<String>) -> Result<T, Error> {
    Err(Error(msg.into()))
}

// ---- the files --------------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawBase {
    schema: String,
    version: u32,
    #[serde(default)]
    provenance: String,
    #[serde(default)]
    license: String,
    categories: Vec<String>,
    dimensions: Vec<String>,
    scales: Vec<Scale>,
    manifestations: Vec<RawManifestation>,
    #[serde(default)]
    pairs: Vec<RawPair>,
}

#[derive(Deserialize)]
struct RawManifestation {
    id: String,
    category: String,
    dimension: String,
    scale: String,
    #[serde(default)]
    located: bool,
    #[serde(default)]
    also: Vec<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    origin: Option<String>,
}

#[derive(Deserialize)]
struct RawPair {
    weighs: String,
    helps: String,
}

/// A scale a sign is noted on: `0..=10` (`pain`, `intensity`, `wellbeing`), `-5..=5` (`mood`), or presence only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scale {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<i32>,
}

#[derive(Deserialize)]
struct RawText {
    schema: String,
    version: u32,
    language: String,
    #[serde(default)]
    provenance: String,
    #[serde(default)]
    negators: Vec<String>,
    #[serde(default)]
    level_words: Vec<String>,
    #[serde(default)]
    pairs: BTreeMap<String, [String; 2]>,
    manifestations: Vec<RawEntryText>,
}

#[derive(Deserialize)]
struct RawEntryText {
    id: String,
    label: String,
    keywords: Vec<String>,
    #[serde(default)]
    keywords_added: Vec<String>,
}

// ---- the model --------------------------------------------------------------------------------------------------------

/// One sign or symptom the person can report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifestation {
    pub id: String,
    pub category: String,
    pub dimension: String,
    /// The scale it is noted on (`pain`, `intensity`, `wellbeing`, `mood`, `presence`).
    pub scale: String,
    pub located: bool,
    /// More general signs it also is (mental fatigue is fatigue too).
    pub also: Vec<String>,
    /// The name of its tag in the tagger: the id, or the older French name some signs keep (`cephalee`).
    pub tag: String,
    /// Written for this project, not in the source referential.
    pub added: bool,
    /// Label and keywords by language code.
    pub labels: BTreeMap<String, String>,
    pub keywords: BTreeMap<String, Vec<String>>,
    /// Keywords of the tagger's first version that the source does not list, by language.
    pub keywords_added: BTreeMap<String, Vec<String>>,
}

impl Manifestation {
    /// The label in `lang` (French when that language has none).
    pub fn label(&self, lang: Lang) -> &str {
        let code = match lang {
            Lang::En => "en",
            _ => "fr",
        };
        self.labels
            .get(code)
            .or_else(|| self.labels.get("fr"))
            .map_or(self.id.as_str(), String::as_str)
    }
}

/// A ressenti that weighs and its opposite that helps (one bar from -5 to +5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pair {
    pub weighs: String,
    pub helps: String,
    /// The short words at both ends, in French.
    pub words: [String; 2],
}

/// A measure or vital constant unit, as said in a text.
#[derive(Debug, Clone)]
pub struct UnitDef {
    pub id: String,
    pub label: String,
    pub words: Vec<String>,
    /// Plausible range of a single number (`number` and `duration` measures).
    pub range: Option<(f64, f64)>,
    pub systolic: Option<(f64, f64)>,
    pub diastolic: Option<(f64, f64)>,
    /// Multiplier to the measure's own unit (cmHg to mmHg).
    pub factor: f64,
    /// Said after a degree sign (`°C`).
    pub degree: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Two numbers (blood pressure).
    Pair,
    Number,
    /// A time span (`6h30`, `7 hours`).
    Duration,
}

/// A measure the detection reads from a text.
#[derive(Debug, Clone)]
pub struct MeasureDef {
    pub id: String,
    /// The name of its tag and field in the tagger (`tension`, `poids`).
    pub tag: String,
    pub kind: Kind,
    pub integer: bool,
    /// The unit the value is given in.
    pub unit: String,
    pub labels: BTreeMap<String, String>,
    /// Keywords by vocabulary (0 French, 1 English), as folded words.
    pub keywords: [Vec<Vec<String>>; 2],
    pub value_before: Vec<String>,
    pub units: Vec<UnitDef>,
    pub default_unit: String,
    pub anchor_units: Vec<String>,
    pub unitless_mgdl: Option<(f64, f64)>,
    /// The vital constant it is, in `vitals`.
    pub vital: Option<String>,
}

/// A vital constant, with its unit and what a reading can plausibly be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vital {
    pub id: String,
    pub unit: String,
    pub plausible: Range,
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usual_limit: Option<UsualLimit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Range {
    pub min: f64,
    pub max: f64,
}

/// A limit the source gives for a vital: a reading `above` or `below` it is worth a word with a professional.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsualLimit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub above: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub below: Option<f64>,
}

/// Where a part of the referential comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Provenance {
    pub part: String,
    pub version: u32,
    pub license: String,
    pub provenance: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Pat {
    pub m: usize,
    pub words: Vec<String>,
    pub table: usize,
}

/// The loaded referential.
#[derive(Debug)]
pub struct Referential {
    pub(crate) manifestations: Vec<Manifestation>,
    pub(crate) scales: Vec<Scale>,
    pub(crate) pairs: Vec<Pair>,
    pub(crate) measures: Vec<MeasureDef>,
    pub(crate) vitals: Vec<Vital>,
    pub(crate) red_flags: crate::redflags::RedFlags,
    pub(crate) provenance: Vec<Provenance>,
    /// By vocabulary, the first word of a pattern to the patterns that start with it.
    pub(crate) index: [HashMap<String, Vec<Pat>>; 2],
    pub(crate) negators: [Vec<Vec<String>>; 2],
    pub(crate) level_words: Vec<String>,
}

#[derive(Deserialize)]
struct RawMeasures {
    schema: String,
    version: u32,
    #[serde(default)]
    provenance: String,
    #[serde(default)]
    license: String,
    measures: Vec<RawMeasure>,
}

#[derive(Deserialize)]
struct RawMeasure {
    id: String,
    tag: String,
    kind: String,
    #[serde(default)]
    integer: bool,
    unit: String,
    labels: BTreeMap<String, String>,
    keywords: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    value_before: Vec<String>,
    units: Vec<RawUnit>,
    default_unit: String,
    #[serde(default)]
    anchor_units: Vec<String>,
    #[serde(default)]
    unitless_mgdl: Option<(f64, f64)>,
    #[serde(default)]
    vital: Option<String>,
}

#[derive(Deserialize)]
struct RawUnit {
    id: String,
    label: String,
    words: Vec<String>,
    #[serde(default)]
    range: Option<(f64, f64)>,
    #[serde(default)]
    systolic: Option<(f64, f64)>,
    #[serde(default)]
    diastolic: Option<(f64, f64)>,
    #[serde(default)]
    factor: Option<f64>,
    #[serde(default)]
    degree: bool,
}

#[derive(Deserialize)]
struct RawVitals {
    schema: String,
    version: u32,
    #[serde(default)]
    provenance: String,
    #[serde(default)]
    license: String,
    vitals: Vec<Vital>,
}

fn parse<T: for<'a> Deserialize<'a>>(name: &str, json: &str) -> Result<T, Error> {
    serde_json::from_str(json).map_err(|e| Error(format!("{name}: {e}")))
}

fn check_schema(name: &str, got: &str, want: &str) -> Result<(), Error> {
    if got == want {
        Ok(())
    } else {
        bad(format!("{name}: schema {got}, expected {want}"))
    }
}

/// The JSON text of every file of the referential.
pub struct Sources<'a> {
    pub manifestations: &'a str,
    pub manifestations_fr: &'a str,
    pub manifestations_en: &'a str,
    pub measures: &'a str,
    pub vitals: &'a str,
    pub red_flags_fr: &'a str,
}

impl Referential {
    /// Loads and validates a referential from the text of its files.
    pub fn from_sources(src: &Sources) -> Result<Referential, Error> {
        let base: RawBase = parse("manifestations.json", src.manifestations)?;
        check_schema(
            "manifestations.json",
            &base.schema,
            "memcp.health.manifestations/1",
        )?;
        let texts = [
            parse::<RawText>("manifestations.fr.json", src.manifestations_fr)?,
            parse::<RawText>("manifestations.en.json", src.manifestations_en)?,
        ];
        for (t, code) in texts.iter().zip(CODES) {
            check_schema(
                "manifestations text",
                &t.schema,
                "memcp.health.manifestations.text/1",
            )?;
            if t.language != code {
                return bad(format!("{code} file says language {}", t.language));
            }
        }
        let mut manifestations: Vec<Manifestation> = Vec::new();
        for m in &base.manifestations {
            if m.id.is_empty() || manifestations.iter().any(|x| x.id == m.id) {
                return bad(format!("manifestation {:?} is empty or listed twice", m.id));
            }
            if !base.categories.contains(&m.category) || !base.dimensions.contains(&m.dimension) {
                return bad(format!("{} has an unknown category or dimension", m.id));
            }
            if !base.scales.iter().any(|s| s.id == m.scale) {
                return bad(format!("{} has an unknown scale {}", m.id, m.scale));
            }
            manifestations.push(Manifestation {
                id: m.id.clone(),
                category: m.category.clone(),
                dimension: m.dimension.clone(),
                scale: m.scale.clone(),
                located: m.located,
                also: m.also.clone(),
                tag: m.tag.clone().unwrap_or_else(|| m.id.clone()),
                added: m.origin.as_deref() == Some("added"),
                labels: BTreeMap::new(),
                keywords: BTreeMap::new(),
                keywords_added: BTreeMap::new(),
            });
        }
        for m in &base.manifestations {
            for a in &m.also {
                if a == &m.id || !manifestations.iter().any(|x| &x.id == a) {
                    return bad(format!("{} is also {a}, which is itself or unknown", m.id));
                }
            }
        }
        for (t, code) in texts.iter().zip(CODES) {
            for e in &t.manifestations {
                let Some(m) = manifestations.iter_mut().find(|m| m.id == e.id) else {
                    return bad(format!("{code} text names the unknown sign {}", e.id));
                };
                if e.label.is_empty() || e.keywords.iter().any(|k| words(k).is_empty()) {
                    return bad(format!("{code} {}: an empty label or keyword", e.id));
                }
                m.labels.insert(code.to_string(), e.label.clone());
                m.keywords.insert(code.to_string(), e.keywords.clone());
                if !e.keywords_added.is_empty() {
                    m.keywords_added
                        .insert(code.to_string(), e.keywords_added.clone());
                }
            }
        }
        if let Some(m) = manifestations.iter().find(|m| !m.labels.contains_key("fr")) {
            return bad(format!("{} has no French label", m.id));
        }
        let mut pairs = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        for p in &base.pairs {
            let words_of = texts[FR]
                .pairs
                .get(&format!("{}/{}", p.weighs, p.helps))
                .cloned()
                .unwrap_or_else(|| [String::new(), String::new()]);
            for c in [&p.weighs, &p.helps] {
                if !manifestations.iter().any(|m| &m.id == c) || seen.contains(&c.as_str()) {
                    return bad(format!("pair names {c}, unknown or in two pairs"));
                }
                seen.push(c);
            }
            pairs.push(Pair {
                weighs: p.weighs.clone(),
                helps: p.helps.clone(),
                words: words_of,
            });
        }

        // The patterns: the label and the keywords of each sign, folded, by vocabulary.
        let mut index: [HashMap<String, Vec<Pat>>; 2] = [HashMap::new(), HashMap::new()];
        for (mi, m) in manifestations.iter().enumerate() {
            for (table, code) in CODES.iter().enumerate() {
                let all = m
                    .labels
                    .get(*code)
                    .into_iter()
                    .chain(m.keywords.get(*code).into_iter().flatten())
                    .chain(m.keywords_added.get(*code).into_iter().flatten());
                for k in all {
                    let w = words(k);
                    let Some(first) = w.first().cloned() else {
                        continue;
                    };
                    let list = index[table].entry(first).or_default();
                    if !list.iter().any(|p| p.m == mi && p.words == w) {
                        list.push(Pat {
                            m: mi,
                            words: w,
                            table,
                        });
                    }
                }
            }
        }
        let negators = [
            texts[FR].negators.iter().map(|n| words(n)).collect(),
            texts[EN].negators.iter().map(|n| words(n)).collect(),
        ];
        let level_words: Vec<String> = texts
            .iter()
            .flat_map(|t| t.level_words.iter().map(|w| fold(w)))
            .collect();

        let measures_doc: RawMeasures = parse("measures.json", src.measures)?;
        check_schema(
            "measures.json",
            &measures_doc.schema,
            "memcp.health.measures/1",
        )?;
        let vitals_doc: RawVitals = parse("vitals.json", src.vitals)?;
        check_schema("vitals.json", &vitals_doc.schema, "memcp.health.vitals/1")?;
        let measures = measures_of(&measures_doc.measures, &vitals_doc.vitals)?;
        let red_flags = crate::redflags::RedFlags::from_json(src.red_flags_fr)?;
        let provenance = vec![
            Provenance {
                part: "manifestations".into(),
                version: base.version,
                license: base.license.clone(),
                provenance: base.provenance.clone(),
            },
            Provenance {
                part: "manifestations.fr".into(),
                version: texts[FR].version,
                license: base.license.clone(),
                provenance: texts[FR].provenance.clone(),
            },
            Provenance {
                part: "manifestations.en".into(),
                version: texts[EN].version,
                license: base.license.clone(),
                provenance: texts[EN].provenance.clone(),
            },
            Provenance {
                part: "measures".into(),
                version: measures_doc.version,
                license: measures_doc.license.clone(),
                provenance: measures_doc.provenance.clone(),
            },
            Provenance {
                part: "vitals".into(),
                version: vitals_doc.version,
                license: vitals_doc.license.clone(),
                provenance: vitals_doc.provenance.clone(),
            },
            Provenance {
                part: "red_flags".into(),
                version: red_flags.version,
                license: red_flags.license.clone(),
                provenance: red_flags.provenance.clone(),
            },
        ];
        Ok(Referential {
            manifestations,
            scales: base.scales,
            pairs,
            measures,
            vitals: vitals_doc.vitals,
            red_flags,
            provenance,
            index,
            negators,
            level_words,
        })
    }
}

fn measures_of(raw: &[RawMeasure], vitals: &[Vital]) -> Result<Vec<MeasureDef>, Error> {
    let mut out: Vec<MeasureDef> = Vec::new();
    for r in raw {
        if out.iter().any(|m| m.id == r.id) {
            return bad(format!("measure {} is listed twice", r.id));
        }
        let kind = match r.kind.as_str() {
            "pair" => Kind::Pair,
            "number" => Kind::Number,
            "duration" => Kind::Duration,
            k => return bad(format!("measure {}: unknown kind {k}", r.id)),
        };
        if let Some(v) = &r.vital
            && !vitals.iter().any(|x| &x.id == v)
        {
            return bad(format!("measure {} names the unknown vital {v}", r.id));
        }
        let units: Vec<UnitDef> = r
            .units
            .iter()
            .map(|u| UnitDef {
                id: u.id.clone(),
                label: u.label.clone(),
                words: u.words.iter().map(|w| fold(w)).collect(),
                range: u.range,
                systolic: u.systolic,
                diastolic: u.diastolic,
                factor: u.factor.unwrap_or(1.0),
                degree: u.degree,
            })
            .collect();
        if !units.iter().any(|u| u.id == r.default_unit)
            || r.anchor_units
                .iter()
                .any(|a| !units.iter().any(|u| &u.id == a))
        {
            return bad(format!("measure {}: unknown default or anchor unit", r.id));
        }
        let ok_ranges = units.iter().all(|u| match kind {
            Kind::Pair => u.systolic.is_some() && u.diastolic.is_some(),
            _ => u.range.is_some_and(|(a, b)| a < b),
        });
        if !ok_ranges {
            return bad(format!("measure {}: a unit has no valid range", r.id));
        }
        let table = |code: &str| -> Vec<Vec<String>> {
            r.keywords
                .get(code)
                .map(|k| {
                    k.iter()
                        .map(|w| words(w))
                        .filter(|w| !w.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };
        out.push(MeasureDef {
            id: r.id.clone(),
            tag: r.tag.clone(),
            kind,
            integer: r.integer,
            unit: r.unit.clone(),
            labels: r.labels.clone(),
            keywords: [table("fr"), table("en")],
            value_before: r.value_before.iter().map(|w| fold(w)).collect(),
            units,
            default_unit: r.default_unit.clone(),
            anchor_units: r.anchor_units.clone(),
            unitless_mgdl: r.unitless_mgdl,
            vital: r.vital.clone(),
        });
    }
    for v in vitals {
        if v.plausible.min >= v.plausible.max {
            return bad(format!("vital {} has an empty plausible range", v.id));
        }
    }
    Ok(out)
}
