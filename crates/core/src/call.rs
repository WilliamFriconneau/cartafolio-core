//! A call recording and its participants.
//!
//! A `call` object travels with a note or a file: `{"caller": [...], "callee": [...], "app": "phone", "direction"?,
//! "started_at"?, "duration_s"?}`. The node keeps it as tags, the same way it keeps any field (see [`crate::autotag`]):
//!
//! - `field/caller/<slug>` and `field/callee/<slug>`, one per participant (a name as a slug, a phone number as its digits:
//!   the `+` of a number is not a tag character, it comes back in the fields);
//! - `field/call_app/<app>`, `field/call_direction/<dir>`, `field/call_started_at/<unix>`, `field/call_duration_s/<n>`;
//! - `personnes/<slug>` for every named participant (a number names nobody);
//! - `appel` and `appel/<app>`.
//!
//! [`people_names`] lists the names the notes of a vault carry.

use std::collections::BTreeMap;

use rusqlite::Connection;
use serde_json::Value;

use crate::autotag::FIELD_PREFIX;
use crate::error::{Error, Result};

/// Most participants of one call (both sides together).
pub const MAX_PARTICIPANTS: usize = 20;
/// Most characters of one participant.
pub const MAX_NAME_CHARS: usize = 80;
/// The apps a call goes through.
pub const APPS: [&str; 7] = [
    "phone", "facetime", "whatsapp", "teams", "meet", "zoom", "other",
];
/// The directions of a call.
pub const DIRECTIONS: [&str; 3] = ["in", "out", "missed"];
/// The prefix of the tags that name a person.
pub const PERSON_PREFIX: &str = "personnes/";
/// The fields that hold a list, even with one value.
pub const LIST_FIELDS: [&str; 2] = ["caller", "callee"];
/// Fewest digits of a phone number.
const MIN_DIGITS: usize = 3;

/// One participant, as the node keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Participant {
    /// A person by name: the slug.
    Name(String),
    /// A phone number: its digits (it is written with a leading `+`).
    Number(String),
}

impl Participant {
    /// The value of the field tag.
    pub fn slug(&self) -> &str {
        match self {
            Participant::Name(s) | Participant::Number(s) => s,
        }
    }
}

/// A call, checked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Call {
    /// Who called.
    pub caller: Vec<Participant>,
    /// Who was called.
    pub callee: Vec<Participant>,
    /// The app.
    pub app: String,
    /// `in`, `out` or `missed`.
    pub direction: Option<String>,
    /// When it started (Unix seconds).
    pub started_at: Option<i64>,
    /// How long it lasted.
    pub duration_s: Option<u64>,
}

fn invalid(what: &str) -> Error {
    Error::Invalid(format!("call: {what}"))
}

/// A participant as the node keeps it: a number (digits, from 3) or a name (a slug of letters and digits, `/` a level).
pub fn participant(raw: &str) -> Result<Participant> {
    if raw.chars().any(char::is_control) {
        return Err(invalid("a participant holds no control character"));
    }
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(invalid("a participant is not empty"));
    }
    if raw.chars().count() > MAX_NAME_CHARS {
        return Err(invalid(&format!(
            "a participant is at most {MAX_NAME_CHARS} characters"
        )));
    }
    let phone_chars = raw
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '+' | ' ' | '(' | ')' | '-' | '.'));
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    if phone_chars && digits.len() >= MIN_DIGITS {
        // `00` is the international prefix: `0033…` is `+33…`.
        let digits = if raw.starts_with('+') {
            digits
        } else {
            digits.strip_prefix("00").unwrap_or(&digits).to_string()
        };
        return Ok(Participant::Number(digits));
    }
    let slug: Vec<String> = raw
        .split('/')
        .map(|seg| {
            let mut out = String::new();
            for c in crate::text::fold(seg).chars() {
                if c.is_ascii_alphanumeric() {
                    out.push(c);
                } else if !out.ends_with('-') && !out.is_empty() {
                    out.push('-');
                }
            }
            out.trim_end_matches('-').to_string()
        })
        .filter(|s| !s.is_empty())
        .collect();
    if slug.is_empty() {
        return Err(invalid(
            "a participant holds letters or digits (a name or a number)",
        ));
    }
    Ok(Participant::Name(slug.join("/")))
}

fn side(v: Option<&Value>, key: &str) -> Result<Vec<Participant>> {
    let list = v
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(&format!("{key}: a list of names or numbers")))?;
    let mut out: Vec<Participant> = vec![];
    for item in list {
        let raw = item
            .as_str()
            .ok_or_else(|| invalid(&format!("{key}: a list of names or numbers (text)")))?;
        let p = participant(raw)?;
        if !out.contains(&p) {
            out.push(p);
        }
    }
    Ok(out)
}

impl Call {
    /// Reads and checks the `call` argument.
    pub fn parse(v: &Value) -> Result<Call> {
        let o = v
            .as_object()
            .ok_or_else(|| invalid("an object {caller, callee, app}"))?;
        let caller = side(o.get("caller"), "caller")?;
        let callee = side(o.get("callee"), "callee")?;
        // The same slug on both sides counts once: it is one person.
        let mut all: Vec<&str> = caller
            .iter()
            .chain(&callee)
            .map(Participant::slug)
            .collect();
        all.sort_unstable();
        all.dedup();
        if all.is_empty() {
            return Err(invalid("at least one participant"));
        }
        if all.len() > MAX_PARTICIPANTS {
            return Err(invalid(&format!("at most {MAX_PARTICIPANTS} participants")));
        }
        let app = o
            .get("app")
            .and_then(Value::as_str)
            .map(str::to_lowercase)
            .filter(|a| APPS.contains(&a.as_str()))
            .ok_or_else(|| invalid(&format!("app: one of {}", APPS.join(", "))))?;
        let direction = match o.get("direction") {
            None | Some(Value::Null) => None,
            Some(d) => Some(
                d.as_str()
                    .map(str::to_lowercase)
                    .filter(|d| DIRECTIONS.contains(&d.as_str()))
                    .ok_or_else(|| {
                        invalid(&format!("direction: one of {}", DIRECTIONS.join(", ")))
                    })?,
            ),
        };
        let started_at = match o.get("started_at") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(
                crate::tools::parse_time(s)
                    .or_else(|| crate::tools::parse_date(&Value::String(s.clone())).ok())
                    .ok_or_else(|| invalid("started_at: an ISO time or Unix seconds"))?,
            ),
            Some(Value::Number(n)) => Some(
                n.as_i64()
                    .filter(|n| *n >= 0)
                    .ok_or_else(|| invalid("started_at: an ISO time or Unix seconds"))?,
            ),
            Some(_) => return Err(invalid("started_at: an ISO time or Unix seconds")),
        };
        let duration_s = match o.get("duration_s") {
            None | Some(Value::Null) => None,
            Some(d) => Some(
                d.as_u64()
                    .ok_or_else(|| invalid("duration_s: a whole number of seconds"))?,
            ),
        };
        Ok(Call {
            caller,
            callee,
            app,
            direction,
            started_at,
            duration_s,
        })
    }

    /// The tags the node keeps for it, each once.
    pub fn tags(&self) -> Vec<String> {
        let mut raw: Vec<String> = vec![];
        for (role, list) in [("caller", &self.caller), ("callee", &self.callee)] {
            for p in list {
                raw.push(format!("{FIELD_PREFIX}{role}/{}", p.slug()));
                if let Participant::Name(n) = p {
                    raw.push(format!("{PERSON_PREFIX}{n}"));
                }
            }
        }
        raw.push(format!("{FIELD_PREFIX}call_app/{}", self.app));
        if let Some(d) = &self.direction {
            raw.push(format!("{FIELD_PREFIX}call_direction/{d}"));
        }
        if let Some(t) = self.started_at {
            raw.push(format!("{FIELD_PREFIX}call_started_at/{t}"));
        }
        if let Some(n) = self.duration_s {
            raw.push(format!("{FIELD_PREFIX}call_duration_s/{n}"));
        }
        raw.push("appel".into());
        raw.push(format!("appel/{}", self.app));
        let mut out: Vec<String> = vec![];
        for t in raw {
            if !out.contains(&t) {
                out.push(t);
            }
        }
        out
    }
}

/// Adds the tags of the `call` argument of a write (if it has one) to `tags`. Checks it first.
pub fn add_to_tags(args: &Value, tags: &mut Vec<String>) -> Result<()> {
    match args.get("call") {
        None | Some(Value::Null) => Ok(()),
        Some(v) => {
            for t in Call::parse(v)?.tags() {
                if !tags.contains(&t) {
                    tags.push(t);
                }
            }
            Ok(())
        }
    }
}

/// Whether a tag comes from a call (it is what a transcription inherits from its recording).
pub fn is_call_tag(tag: &str) -> bool {
    tag == "appel"
        || tag.starts_with("appel/")
        || tag.starts_with(PERSON_PREFIX)
        || [
            "caller/",
            "callee/",
            "call_app/",
            "call_direction/",
            "call_started_at/",
            "call_duration_s/",
        ]
        .iter()
        .any(|f| {
            tag.strip_prefix(FIELD_PREFIX)
                .is_some_and(|r| r.starts_with(f))
        })
}

/// The names of the `personnes/` tags of a list of tags.
pub fn people_of(tags: &[String]) -> Vec<String> {
    tags.iter()
        .filter_map(|t| t.strip_prefix(PERSON_PREFIX).map(String::from))
        .collect()
}

/// The value of a list field as it is read back: a number gets its `+` again.
pub fn value_back(name: &str, value: &str) -> String {
    if LIST_FIELDS.contains(&name)
        && value.len() >= MIN_DIGITS
        && value.chars().all(|c| c.is_ascii_digit())
    {
        format!("+{value}")
    } else {
        value.to_string()
    }
}

/// The names the notes of one vault carry as `personnes/` tags (active notes), with how many notes each is on.
pub fn people_in(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut st = conn.prepare(
        "SELECT t.tag, COUNT(DISTINCT t.fact_id) FROM fact_tags t JOIN facts f ON f.id = t.fact_id
         WHERE f.status = 'active' AND t.tag LIKE 'personnes/%' GROUP BY t.tag",
    )?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    let mut out = vec![];
    for r in rows {
        let (tag, n) = r?;
        if let Some(name) = tag.strip_prefix(PERSON_PREFIX) {
            out.push((name.to_string(), n));
        }
    }
    Ok(out)
}

/// The distinct names of the `personnes/` tags, with their counts, most first then by name: of one space, or of every
/// space of the node when `space` is `None`.
pub fn people_names(
    brain: &crate::Brain,
    space: Option<&crate::Space>,
) -> Result<Vec<(String, i64)>> {
    let spaces = match space {
        Some(s) => vec![s.clone()],
        None => brain.spaces()?,
    };
    let mut total: BTreeMap<String, i64> = BTreeMap::new();
    for s in spaces {
        for (name, n) in people_in(&brain.open_space(&s)?)? {
            *total.entry(name).or_insert(0) += n;
        }
    }
    let mut out: Vec<(String, i64)> = total.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(out)
}
