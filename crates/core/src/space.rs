use std::fmt;

use crate::error::{Error, Result};

/// A memory space: an isolated SQLite file. Each installation chooses its own
/// spaces (`health`, `home`, `household`…); none is built in.
///
/// A name is 1 to 32 characters: lowercase ASCII letters, digits and dashes,
/// starting with a letter. `control` is reserved.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Space(String);

const RESERVED: [&str; 2] = ["control", "all"];

impl Space {
    pub fn new(name: &str) -> Result<Space> {
        let ok = (1..=32).contains(&name.len())
            && name.starts_with(|c: char| c.is_ascii_lowercase())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !RESERVED.contains(&name);
        if ok {
            Ok(Space(name.to_string()))
        } else {
            Err(Error::Invalid(format!(
                "invalid space name {name:?}: 1 to 32 lowercase letters, digits or dashes, \
                 starting with a letter (\"control\" and \"all\" are reserved)"
            )))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn file_name(&self) -> String {
        format!("{}.sqlite", self.0)
    }
}

impl fmt::Display for Space {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::Space;

    #[test]
    fn valid_names() {
        for n in ["health", "home", "household", "work-2", "a"] {
            assert_eq!(
                Space::new(n).map(|s| s.as_str().to_string()).ok(),
                Some(n.into())
            );
        }
    }

    #[test]
    fn invalid_names() {
        for n in [
            "",
            "Health",
            "2work",
            "-x",
            "a b",
            "santé",
            "control",
            "all",
            &"x".repeat(33),
        ] {
            assert!(Space::new(n).is_err(), "{n:?} should be refused");
        }
    }
}
