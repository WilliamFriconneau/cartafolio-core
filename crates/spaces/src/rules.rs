//! The words and tags of the spaces are shared data: one rule, one id; on
//! two nodes, the greater clock wins. A person changes them freely; an
//! assistant only within its space's level, and what it writes stays data,
//! never an instruction: the router stays deterministic.

use serde::{Deserialize, Serialize};

use crate::space::{SpaceError, Tree};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    /// Decides alone.
    UniqueWord,
    /// Only suggests (decides for a sensitive space when nothing else does).
    OrdinaryWord,
    /// A label the space gives its notes.
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub space: String,
    pub kind: RuleKind,
    pub value: String,
    /// `person` or `assistant:<name>`.
    pub by: String,
    #[serde(default)]
    pub reason: String,
    /// Encoded [`crate::Hlc`]: the greater wins.
    pub hlc: String,
    #[serde(default)]
    pub retired: bool,
}

/// How far an assistant may go on a space.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantLevel {
    /// Everything waits for the person.
    Propose,
    /// Adds an ordinary word or a tag at once; the rest waits.
    #[default]
    Add,
    /// Adds, changes and retires; never on a sensitive space.
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeState {
    Applied,
    Pending,
    Refused,
    Undone,
}

/// One change of a rule by an assistant: kept, with its reason, undoable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub id: String,
    /// The rule as proposed (retired: true for a removal).
    pub rule: Rule,
    /// The rule before, none for an addition.
    pub before: Option<Rule>,
    pub state: ChangeState,
    pub hlc: String,
}

/// Changes an assistant may apply per space and per day.
pub const DAILY_CHANGES: usize = 20;

impl Tree {
    /// Puts a rule: a new id is added; a known id is replaced only by a
    /// greater clock (the most recent wins, whatever the order received).
    pub fn put_rule(&mut self, rule: Rule) {
        match self.rules.iter_mut().find(|r| r.id == rule.id) {
            Some(r) if r.hlc < rule.hlc => *r = rule,
            Some(_) => {}
            None => self.rules.push(rule),
        }
    }

    /// The live values of a kind for a space.
    pub fn words(&self, space: &str, kind: RuleKind) -> Vec<String> {
        self.rules
            .iter()
            .filter(|r| r.space == space && r.kind == kind && !r.retired)
            .map(|r| r.value.clone())
            .collect()
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    /// The version of the rules: their greatest clock.
    pub fn rules_version(&self) -> Option<String> {
        self.rules.iter().map(|r| r.hlc.clone()).max()
    }

    /// The person sets the values of a kind: new ones are added, missing ones
    /// retired, kept ones untouched. `next` gives an id and a clock per change.
    pub fn set_words(
        &mut self,
        space: &str,
        kind: RuleKind,
        values: &[String],
        mut next: impl FnMut() -> (String, String),
    ) -> Result<(), SpaceError> {
        self.get(space)
            .ok_or_else(|| SpaceError::NotFound(space.into()))?;
        let live: Vec<Rule> = self
            .rules
            .iter()
            .filter(|r| r.space == space && r.kind == kind && !r.retired)
            .cloned()
            .collect();
        for r in live.iter().filter(|r| !values.contains(&r.value)) {
            let (_, hlc) = next();
            self.put_rule(Rule {
                retired: true,
                by: "person".into(),
                reason: String::new(),
                hlc,
                ..r.clone()
            });
        }
        for v in values {
            let v = v.trim();
            if v.is_empty() || live.iter().any(|r| r.value == v) {
                continue;
            }
            let (id, hlc) = next();
            self.put_rule(Rule {
                id,
                space: space.into(),
                kind,
                value: v.into(),
                by: "person".into(),
                reason: String::new(),
                hlc,
                retired: false,
            });
        }
        Ok(())
    }

    /// Moves the rules of `from` to `into` (a merge), or retires them (a deletion).
    pub(crate) fn hand_rules(
        &mut self,
        from: &str,
        into: Option<&str>,
        mut next: impl FnMut() -> String,
    ) {
        let moving: Vec<Rule> = self
            .rules
            .iter()
            .filter(|r| r.space == from && !r.retired)
            .cloned()
            .collect();
        for r in moving {
            let hlc = next();
            let taken = into.is_some_and(|t| self.words(t, r.kind).contains(&r.value));
            let moved = match into {
                Some(t) if !taken => Rule {
                    space: t.into(),
                    hlc,
                    ..r
                },
                _ => Rule {
                    retired: true,
                    hlc,
                    ..r
                },
            };
            self.put_rule(moved);
        }
    }

    /// Whether this change must wait for the person.
    fn waits(&self, rule: &Rule, before: Option<&Rule>) -> bool {
        let Some(space) = self.get(&rule.space) else {
            return true;
        };
        let delivers = space.connectors.iter().any(|c| c.delivers());
        let removal = rule.retired || before.is_some();
        let today = rule
            .hlc
            .get(..13)
            .and_then(|ms| ms.parse::<u64>().ok())
            .map(|ms| ms / 86_400_000);
        let done_today = self
            .changes
            .iter()
            .filter(|c| c.rule.space == rule.space && c.state == ChangeState::Applied)
            .filter(|c| {
                c.hlc
                    .get(..13)
                    .and_then(|ms| ms.parse::<u64>().ok())
                    .map(|ms| ms / 86_400_000)
                    == today
            })
            .count();
        rule.kind == RuleKind::UniqueWord
            || space.sensitive
            // A word that sends notes out of the node, or a removal on a space that delivers.
            || (delivers && (removal || rule.kind != RuleKind::Tag))
            || done_today >= DAILY_CHANGES
            || match space.assistant_level {
                AssistantLevel::Propose => true,
                AssistantLevel::Add => removal,
                AssistantLevel::Full => false,
            }
    }

    /// An assistant proposes a rule (a new one, a changed one under a known
    /// id, or `retired: true` to remove it). Applied at once within the
    /// space's level, else kept as pending for the person.
    pub fn rule_propose(&mut self, change_id: &str, rule: Rule) -> Result<ChangeState, SpaceError> {
        self.get(&rule.space)
            .ok_or_else(|| SpaceError::NotFound(rule.space.clone()))?;
        let before = self.rules.iter().find(|r| r.id == rule.id).cloned();
        let state = if self.waits(&rule, before.as_ref()) {
            ChangeState::Pending
        } else {
            ChangeState::Applied
        };
        if state == ChangeState::Applied {
            self.put_rule(rule.clone());
        }
        let hlc = rule.hlc.clone();
        self.changes.push(Change {
            id: change_id.into(),
            rule,
            before,
            state,
            hlc,
        });
        Ok(state)
    }

    /// The person accepts or refuses a pending change.
    pub fn change_decide(
        &mut self,
        change_id: &str,
        accept: bool,
        hlc: &str,
    ) -> Result<(), SpaceError> {
        let c = self.change_mut(change_id)?;
        if c.state != ChangeState::Pending {
            return Ok(());
        }
        c.state = if accept {
            ChangeState::Applied
        } else {
            ChangeState::Refused
        };
        let rule = Rule {
            hlc: hlc.into(),
            ..c.rule.clone()
        };
        if accept {
            self.put_rule(rule);
        }
        Ok(())
    }

    /// Undoes an applied change in one gesture: the rule goes back to what it
    /// was (an addition is retired).
    pub fn rule_undo(&mut self, change_id: &str, hlc: &str) -> Result<(), SpaceError> {
        let c = self.change_mut(change_id)?;
        if c.state != ChangeState::Applied {
            return Ok(());
        }
        c.state = ChangeState::Undone;
        let back = match &c.before {
            Some(b) => Rule {
                hlc: hlc.into(),
                ..b.clone()
            },
            None => Rule {
                retired: true,
                hlc: hlc.into(),
                ..c.rule.clone()
            },
        };
        self.put_rule(back);
        Ok(())
    }

    /// The changes of a space, oldest first.
    pub fn rule_history(&self, space: &str) -> Vec<&Change> {
        self.changes
            .iter()
            .filter(|c| c.rule.space == space)
            .collect()
    }

    fn change_mut(&mut self, id: &str) -> Result<&mut Change, SpaceError> {
        self.changes
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| SpaceError::NotFound(id.into()))
    }
}
