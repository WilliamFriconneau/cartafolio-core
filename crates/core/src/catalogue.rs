//! The catalogue: the kinds of vault a person can have.
//!
//! A vault is created from a kind, never freely and never by an assistant. The catalogue is
//! data: a default for a person's life, replaceable by a `catalogue.txt` in the data
//! directory, one kind per line, `key | label | description` and optionally `| money` to say that
//! the kind carries the bank-statement tools. Lines
//! starting with `#` are comments.
//!
//! The default follows what a person needs to keep track of. It is a synthesis, not a
//! standard (see docs/APPROACH.md).

use crate::error::{Error, Result};
use crate::space::Space;

/// The kind of a vault that was created before kinds existed, or by an operator with a name
/// of their own. It carries no built-in tools.
pub const CUSTOM: &str = "custom";

/// An explicitly created folder, including when it has no children yet.
pub const FOLDER: &str = "folder";

/// The kind of the person's Inbox: the one box every capturing device writes to, which belongs to no vault. The
/// janitor sends what lands there to the inbox of a vault, or leaves it for the person to decide. Never in a catalogue.
pub const TRIAGE: &str = "inbox";

/// The name the kind of the Inbox had before it was in English. A memory made then keeps it until its box is renamed; it
/// is read everywhere as the Inbox (see [`is_inbox_kind`]).
pub const TRIAGE_LEGACY: &str = "a-trier";

/// Whether a kind is the Inbox, by its name now or the one it had.
pub fn is_inbox_kind(kind: &str) -> bool {
    kind == TRIAGE || kind == TRIAGE_LEGACY
}

/// A built-in set of tools a kind can carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    /// The bank statement tools.
    Money,
}

impl Feature {
    fn parse(s: &str) -> Option<Feature> {
        match s {
            "money" => Some(Feature::Money),
            _ => None,
        }
    }

    /// The word used in `catalogue.txt` and in the page for this feature (`"money"`).
    pub fn word(self) -> &'static str {
        match self {
            Feature::Money => "money",
        }
    }
}

/// One kind of vault an installation offers when a person adds a vault.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kind {
    /// Lowercase letters, digits and dashes, starting with a letter: it becomes part of a
    /// vault name.
    pub key: String,
    /// Name shown to the person.
    pub label: String,
    /// One sentence saying what the kind is for.
    pub description: String,
    /// The extra tools the vault gets (the bank statement tools for `money`), if any.
    pub feature: Option<Feature>,
}

fn kind(key: &str, label: &str, description: &str, feature: Option<Feature>) -> Kind {
    Kind {
        key: key.into(),
        label: label.into(),
        description: description.into(),
        feature,
    }
}

/// What a person needs to keep track of: themselves, their health, the people around them,
/// their home and daily life, their money, their work, their passions.
pub fn default_catalogue() -> Vec<Kind> {
    vec![
        kind(
            "me",
            "Me",
            "Who I am, what I value, how I want assistants to work with me",
            None,
        ),
        kind(
            "health",
            "Health",
            "Care, treatments, measures, how I feel",
            None,
        ),
        kind(
            "people",
            "People",
            "Family, friends, birthdays, the people around me",
            None,
        ),
        kind(
            "home",
            "Home",
            "The house, daily life, equipment, errands, paperwork",
            None,
        ),
        kind(
            "money",
            "Finances",
            "Accounts, bank statements, budgets",
            Some(Feature::Money),
        ),
        kind("work", "Work", "Job, projects, colleagues", None),
        kind(
            "hobbies",
            "Hobbies",
            "Passions and projects for pleasure",
            None,
        ),
    ]
}

/// A catalogue from text. An error names the line.
pub fn parse(text: &str) -> Result<Vec<Kind>> {
    let mut out: Vec<Kind> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = |why: &str| Error::Invalid(format!("catalogue line {}: {why}", i + 1));
        let parts: Vec<&str> = line.split('|').map(str::trim).collect();
        if !(3..=4).contains(&parts.len()) {
            return Err(at(
                "expected `key | label | description` and optionally `| money`",
            ));
        }
        let key = parts[0];
        Space::new(key).map_err(|_| {
            at("the key is lowercase letters, digits and dashes, starting with a letter")
        })?;
        if matches!(key, CUSTOM | FOLDER) || is_inbox_kind(key) {
            return Err(at("`custom`, `folder` and `inbox` are reserved"));
        }
        if out.iter().any(|k| k.key == key) {
            return Err(at("this key is already defined"));
        }
        if parts[1].is_empty() {
            return Err(at("the label is empty"));
        }
        let feature = match parts.get(3) {
            Some(f) => Some(Feature::parse(f).ok_or_else(|| at("the last field is `money`"))?),
            None => None,
        };
        out.push(kind(key, parts[1], parts[2], feature));
    }
    if out.is_empty() {
        return Err(Error::Invalid("the catalogue has no kind".into()));
    }
    Ok(out)
}

/// The name of the vault of `kind` that a person owns: `<person>-<kind>`, the person's name
/// shortened to fit the 32 characters of a space name.
pub fn vault_name(person: &str, kind: &str) -> Result<Space> {
    let room = 32usize.saturating_sub(kind.len() + 1);
    let mut who: String = person
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .take(room)
        .collect();
    who = who.trim_matches('-').to_string();
    if who.is_empty() || !who.starts_with(|c: char| c.is_ascii_lowercase()) {
        who = format!("p{who}");
        who.truncate(room);
    }
    Space::new(&format!("{who}-{kind}"))
}

/// The kind of a space a fresh memory makes under one of the usual names (`init --defaults`, `setup`): `health` is a
/// health space, `finances` a money space, and so on, so that the toolkit of its profession works at once. Any other
/// name is custom.
pub fn default_kind(name: &str) -> &'static str {
    match name {
        "health" => "health",
        "home" => "home",
        "finances" | "money" => "money",
        "hobbies" => "hobbies",
        "work" => "work",
        "personal" => "me",
        _ => CUSTOM,
    }
}
