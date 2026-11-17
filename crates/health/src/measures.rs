//! Measures: a number counts only next to its word (`tension 12/8`, `je pèse 72,5 kg`, `38,5 de fièvre`) or with a unit
//! that says what it is (`64 bpm`), inside the range a body can show. A date and a clock time are never a value; only a
//! duration (`dormi 6h30`) reads a time of the `6h30` form.

use serde::Serialize;

use crate::model::{Kind, Lang, MeasureDef, Referential, UnitDef};
use crate::text::{Tok, is_word, phrase_at, tokenize};

/// A measure found in a text.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Measure {
    pub id: String,
    /// The value in `unit` (the systolic for a pressure; a pressure said in cmHg, `12/8`, is given in mmHg).
    pub value: f64,
    /// The diastolic, for a pressure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub second: Option<f64>,
    pub unit: String,
    /// The value as it was said (`12/8`, `72.5`).
    #[serde(skip)]
    pub raw: String,
    /// The id of the unit when the text said one (`mmhg`, `kg`).
    #[serde(skip)]
    pub said_unit: Option<String>,
    /// The name of the measure's tag and field in the tagger (`tension`).
    #[serde(skip)]
    pub tag: String,
}

const FILLER: [&str; 22] = [
    "est", "de", "d", "du", "a", "au", "aux", "of", "is", "at", "was", "etait", "vaut", "autour",
    "environ", "about", "around", "ce", "level", "niveau", "avec", "with",
];

const DURATION_FILLER: [&str; 10] = [
    "environ",
    "pendant",
    "seulement",
    "juste",
    "about",
    "for",
    "only",
    "just",
    "around",
    "presque",
];

const DENIERS: [&str; 5] = ["pas", "jamais", "plus", "not", "never"];

/// The index of the first number after `from`, past a few filler words (« est », « de », « of », `:`).
fn next_value(toks: &[Tok], from: usize) -> Option<usize> {
    for (skipped, k) in (from..).enumerate() {
        match toks.get(k)? {
            Tok::Num { .. } => return Some(k),
            Tok::Word(w) if FILLER.contains(&w.as_str()) => {}
            Tok::Sym(':' | '=' | '\'' | '\u{2019}') => {}
            _ => return None,
        }
        if skipped >= 5 {
            return None;
        }
    }
    None
}

fn num(t: Option<&Tok>) -> Option<(f64, bool)> {
    match t? {
        Tok::Num {
            text,
            pair: None,
            int,
        } => text.parse().ok().map(|v| (v, *int)),
        _ => None,
    }
}

/// The unit said right after the number at `k`: a word (`kg`, `mg/dl`) or, after a degree sign, `c` or `f`.
fn unit_after<'a>(def: &'a MeasureDef, toks: &[Tok], k: usize) -> Option<&'a UnitDef> {
    let word = |i: usize| match toks.get(i) {
        Some(Tok::Word(w)) => Some(w.as_str()),
        _ => None,
    };
    let joined = match (word(k + 1), toks.get(k + 2), word(k + 3)) {
        (Some(a), Some(Tok::Sym('/')), Some(b)) => Some(format!("{a}/{b}")),
        _ => None,
    };
    def.units.iter().find(|u| {
        if u.degree {
            matches!(toks.get(k + 1), Some(Tok::Sym('\u{b0}')))
                && word(k + 2).is_some_and(|w| u.words.iter().any(|x| x == w))
        } else {
            word(k + 1).is_some_and(|w| u.words.iter().any(|x| x == w))
                || joined
                    .as_deref()
                    .is_some_and(|j| u.words.iter().any(|x| x == j))
        }
    })
}

fn in_range(v: f64, r: Option<(f64, f64)>) -> bool {
    r.is_some_and(|(a, b)| (a..=b).contains(&v))
}

fn reading(
    def: &MeasureDef,
    value: f64,
    second: Option<f64>,
    unit: &UnitDef,
    raw: String,
    said: bool,
) -> Measure {
    Measure {
        id: def.id.clone(),
        value,
        second,
        unit: if def.kind == Kind::Pair {
            def.unit.clone()
        } else {
            unit.label.clone()
        },
        raw,
        said_unit: said.then(|| unit.id.clone()),
        tag: def.tag.clone(),
    }
}

/// A single number at `k`.
fn read_number(def: &MeasureDef, toks: &[Tok], k: usize) -> Option<Measure> {
    let (v, int) = num(toks.get(k))?;
    if def.integer && !int {
        return None;
    }
    let raw = match toks.get(k)? {
        Tok::Num { text, .. } => text.clone(),
        _ => return None,
    };
    let said = unit_after(def, toks, k);
    let fallback = def.units.iter().find(|u| u.id == def.default_unit)?;
    match said {
        Some(u) => in_range(v, u.range).then(|| reading(def, v, None, u, raw, true)),
        None if in_range(v, fallback.range) => Some(reading(def, v, None, fallback, raw, false)),
        None => {
            // A figure with no unit that no other reading allows: milligrams per decilitre.
            let mg = def.units.iter().find(|u| u.id == "mgdl")?;
            (in_range(v, def.unitless_mgdl) && in_range(v, mg.range))
                .then(|| reading(def, v, None, mg, raw, false))
        }
    }
}

/// Two numbers from `k`: `12/8`, `12 sur 8`, `130 over 85`, `14-9`, `13 8`.
fn read_pair(def: &MeasureDef, toks: &[Tok], k: usize) -> Option<Measure> {
    let (s, d, last, raw) = match toks.get(k)? {
        Tok::Num {
            text,
            pair: Some(p),
            int: true,
        } => (
            text.parse::<u32>().ok()?,
            p.parse::<u32>().ok()?,
            k,
            format!("{text}/{p}"),
        ),
        Tok::Num {
            text,
            pair: None,
            int: true,
        } => {
            let s = text.parse::<u32>().ok()?;
            let mut j = k + 1;
            if is_word(toks.get(j), "sur")
                || is_word(toks.get(j), "over")
                || matches!(toks.get(j), Some(Tok::Sym('/' | '-')))
            {
                j += 1;
            }
            match toks.get(j)? {
                Tok::Num {
                    text: t2,
                    pair: None,
                    int: true,
                } => (s, t2.parse::<u32>().ok()?, j, format!("{s}/{t2}")),
                _ => return None,
            }
        }
        _ => return None,
    };
    let (s, d) = (f64::from(s), f64::from(d));
    let fits = |u: &UnitDef| in_range(s, u.systolic) && in_range(d, u.diastolic) && s > d;
    let said = unit_after(def, toks, last);
    let unit = match said {
        Some(u) => fits(u).then_some(u)?,
        None => def.units.iter().find(|u| fits(u))?,
    };
    Some(reading(
        def,
        s * unit.factor,
        Some(d * unit.factor),
        unit,
        raw,
        said.is_some(),
    ))
}

/// A time span after its keyword: `6h30`, `2 h`, `7 heures`, `6 h 30`.
fn read_duration(def: &MeasureDef, toks: &[Tok], from: usize) -> Option<Measure> {
    let mut k = from;
    let mut skipped = 0;
    while let Some(Tok::Word(w)) = toks.get(k)
        && DURATION_FILLER.contains(&w.as_str())
        && skipped < 3
    {
        k += 1;
        skipped += 1;
    }
    let unit = def.units.iter().find(|u| u.id == def.default_unit)?;
    let hours = match toks.get(k)? {
        Tok::Time(Some(h)) => *h,
        Tok::Num { .. } => {
            let (h, _) = num(toks.get(k))?;
            let Some(Tok::Word(w)) = toks.get(k + 1) else {
                return None;
            };
            if !unit.words.contains(w) {
                return None;
            }
            // Minutes right after the unit: `6 h 30`.
            match num(toks.get(k + 2)) {
                Some((m, true)) if m < 60.0 => h + m / 60.0,
                _ => h,
            }
        }
        _ => return None,
    };
    in_range(hours, unit.range).then(|| {
        reading(
            def,
            (hours * 100.0).round() / 100.0,
            None,
            unit,
            format!("{hours}"),
            true,
        )
    })
}

fn read_at(def: &MeasureDef, toks: &[Tok], k: usize) -> Option<Measure> {
    match def.kind {
        Kind::Pair => read_pair(def, toks, k),
        Kind::Number => read_number(def, toks, k),
        Kind::Duration => None,
    }
}

impl MeasureDef {
    fn read(&self, toks: &[Tok], lang: Lang) -> Option<Measure> {
        let keys = lang
            .tables()
            .iter()
            .flat_map(|t| self.keywords[*t].iter())
            .collect::<Vec<_>>();
        for i in 0..toks.len() {
            for key in &keys {
                if !phrase_at(toks, i, key) {
                    continue;
                }
                let after = i + key.len();
                let found = match self.kind {
                    Kind::Duration => {
                        let denied = i > 0
                            && matches!(&toks[i - 1], Tok::Word(w) if DENIERS.contains(&w.as_str()));
                        if denied {
                            None
                        } else {
                            read_duration(self, toks, after)
                        }
                    }
                    _ => next_value(toks, after).and_then(|k| read_at(self, toks, k)),
                };
                if found.is_some() {
                    return found;
                }
                // « 38,5 de fièvre »: the value before the word.
                if self.kind == Kind::Number && self.value_before.contains(&key[0]) {
                    let mut j = i;
                    if j > 0
                        && matches!(&toks[j - 1], Tok::Word(w) if ["de", "of", "a", "avec", "with"].contains(&w.as_str()))
                    {
                        j -= 1;
                    }
                    if j > 0
                        && let Some(m) = read_number(self, toks, j - 1)
                    {
                        return Some(m);
                    }
                }
            }
        }
        // A number with a unit that says what it is, without the word: `64 bpm`, `72 kg`, `38,5 °C`.
        if !self.anchor_units.is_empty() {
            for k in 0..toks.len() {
                if let Some(m) = read_at(self, toks, k)
                    && m.said_unit
                        .as_ref()
                        .is_some_and(|u| self.anchor_units.contains(u))
                {
                    return Some(m);
                }
            }
        }
        None
    }
}

impl Referential {
    /// The measures of a text, one per measure at most (the first said), in referential order.
    pub fn measures(&self, text: &str, lang: Lang) -> Vec<Measure> {
        self.measures_of(&tokenize(text), lang)
    }

    pub(crate) fn measures_of(&self, toks: &[Tok], lang: Lang) -> Vec<Measure> {
        self.measures
            .iter()
            .filter_map(|d| d.read(toks, lang))
            .collect()
    }
}
