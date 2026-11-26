//! pensine-finances: a person's bank statements in a memory space.
//!
//! The person exports their accounts from the bank (an `.xlsx` workbook: a summary
//! sheet named "Vos comptes" and one sheet per account, "Cpt `<number>`" or
//! "CB `<number>`"), stores it in the space (admin page or `file_put`) and has an
//! agent import it with `finances_import`. An import is idempotent: importing the
//! same export again, or one that overlaps the previous, adds only the new
//! movements.
//!
//! Every figure is a figure of the statement, never an estimate. Account numbers
//! are never returned in clear: an account shows its name and its last four digits.
//! The person's own rules (a monthly cap on a merchant, a merchant to watch, the
//! words that mark an internal transfer) are stored in the space, never in the code.

use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;

use calamine::{Data, Reader, Xlsx, open_workbook_from_rs};
use pensine_core::access::Caller;
use pensine_core::tools::{ToolDef, ToolError};
use pensine_core::util::{self, civil};
use pensine_core::{Brain, Space, files};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS fin_accounts(
  number TEXT PRIMARY KEY, name TEXT NOT NULL DEFAULT '', balance REAL, balance_on TEXT,
  currency TEXT NOT NULL DEFAULT 'EUR');
CREATE TABLE IF NOT EXISTS fin_moves(
  number TEXT NOT NULL, date TEXT NOT NULL, value_date TEXT, label TEXT NOT NULL,
  amount REAL NOT NULL, rank INTEGER NOT NULL, imported_at TEXT NOT NULL,
  PRIMARY KEY(number, date, label, amount, rank));
CREATE TABLE IF NOT EXISTS fin_imports(file TEXT NOT NULL, at TEXT NOT NULL, new_moves INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS fin_rules(id INTEGER PRIMARY KEY CHECK (id = 1), json TEXT NOT NULL);
";

/// Words that mark a bank fee, and an internal transfer, until the person sets their own.
const DEFAULT_FEES: [&str; 5] = [
    "COMMISSION",
    "FRAIS",
    "COTIS",
    "AGIOS",
    "INTERETS DEBITEURS",
];
const DEFAULT_INTERNAL: [&str; 3] = ["COMPTE", "EPARGNE", "LIVRET"];

/// The finances of one space.
pub struct Ledger<'a> {
    pub brain: &'a Brain,
    pub space: Space,
}

// ── reading a workbook ────────────────────────────────────────────────────────

/// A sheet title with every run of five digits or more cut to its last four: `Compte 12345678901` → `Compte …8901`.
fn mask_account(title: &str) -> String {
    let mut out = String::new();
    let mut digits = String::new();
    let flush = |digits: &mut String, out: &mut String| {
        if digits.len() > 4 {
            out.push('…');
            out.push_str(&digits[digits.len() - 4..]);
        } else {
            out.push_str(digits);
        }
        digits.clear();
    };
    for c in title.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            flush(&mut digits, &mut out);
            out.push(c);
        }
    }
    flush(&mut digits, &mut out);
    out
}

fn text(c: &Data) -> String {
    match c {
        Data::Empty => String::new(),
        Data::String(s) => s.trim().to_string(),
        other => other.to_string().trim().to_string(),
    }
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn number(c: &Data) -> Option<f64> {
    match c {
        Data::Float(f) => Some(*f),
        Data::Int(i) => Some(*i as f64),
        Data::String(s) => {
            let t: String = s
                .chars()
                .filter(|c| !c.is_whitespace() && *c != '\u{a0}' && *c != '\u{202f}')
                .collect();
            t.replace(',', ".").parse().ok()
        }
        _ => None,
    }
}

fn round2(x: f64) -> f64 {
    let r = (x * 100.0).round() / 100.0;
    // No negative zero in the answers.
    if r == 0.0 { 0.0 } else { r }
}

fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

/// `dd/mm/yyyy` or `yyyy-mm-dd` (a longer text is cut after the date), as `yyyy-mm-dd`.
fn date_text(s: &str) -> Option<String> {
    let t: Vec<char> = s.trim().chars().take(10).collect();
    let num = |a: usize, b: usize| t.get(a..b).filter(|x| x.iter().all(char::is_ascii_digit));
    let join = |x: &[char]| x.iter().collect::<String>();
    if t.len() == 10 && t[2] == '/' && t[5] == '/' {
        let (d, m, y) = (num(0, 2)?, num(3, 5)?, num(6, 10)?);
        return Some(format!("{}-{}-{}", join(y), join(m), join(d)));
    }
    if t.len() == 10 && t[4] == '-' && t[7] == '-' {
        let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
        return Some(format!("{}-{}-{}", join(y), join(m), join(d)));
    }
    None
}

/// A spreadsheet date (days since 1899-12-30) as `yyyy-mm-dd`.
fn date_serial(serial: f64) -> Option<String> {
    if !(1.0..2_958_466.0).contains(&serial) {
        return None;
    }
    let unix_days = serial.floor() as i64 - 25_569;
    let (y, m, d, ..) = civil(unix_days * 86_400);
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

fn cell_date(c: &Data) -> Option<String> {
    match c {
        Data::DateTime(d) => date_serial(d.as_f64()),
        Data::DateTimeIso(s) | Data::String(s) => date_text(s),
        _ => None,
    }
}

/// The account number of a sheet titled `Cpt 12345…` or `CB 12345…`.
fn sheet_account(title: &str) -> Option<String> {
    let rest = title
        .strip_prefix("Cpt")
        .or_else(|| title.strip_prefix("CB"))?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let tail: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ' ')
        .collect();
    let d = digits(&tail);
    (!d.is_empty()).then_some(d)
}

fn has_run_of_digits(s: &str, n: usize) -> bool {
    let mut run = 0;
    for c in s.chars() {
        run = if c.is_ascii_digit() { run + 1 } else { 0 };
        if run >= n {
            return true;
        }
    }
    false
}

/// `Solde au dd/mm/yyyy` in a row's text: the date.
fn closing_date(row_text: &str) -> Option<String> {
    let at = row_text.find("Solde au ")? + "Solde au ".len();
    date_text(row_text.get(at..)?)
}

struct Account {
    number: String,
    name: String,
    balance: Option<f64>,
    balance_on: Option<String>,
    moves: Vec<Move>,
}

struct Move {
    date: String,
    value_date: Option<String>,
    label: String,
    amount: f64,
}

fn read_workbook(bytes: &[u8]) -> Result<Vec<Account>, ToolError> {
    let mut wb: Xlsx<_> = open_workbook_from_rs(Cursor::new(bytes.to_vec()))
        .map_err(|e| ToolError::Invalid(format!("not a readable .xlsx workbook: {e}")))?;
    let titles = wb.sheet_names();
    // 1. the summary: account name, account identifier, balance.
    let mut summary: Vec<(String, String, Option<f64>)> = vec![];
    for t in &titles {
        if !t.to_lowercase().starts_with("vos comptes") {
            continue;
        }
        let Ok(range) = wb.worksheet_range(t) else {
            continue;
        };
        for r in range.rows() {
            if r.len() >= 3 && !text(&r[1]).is_empty() && has_run_of_digits(&text(&r[1]), 5) {
                summary.push((digits(&text(&r[1])), text(&r[0]), number(&r[2])));
            }
        }
    }
    // 2. one sheet per account: Date | Value | Label | Debit | Credit | Balance | Currency.
    let mut out = vec![];
    for t in &titles {
        let Some(number_) = sheet_account(t) else {
            continue;
        };
        let Ok(range) = wb.worksheet_range(t) else {
            continue;
        };
        let rows: Vec<&[Data]> = range.rows().collect();
        let Some(head) = rows
            .iter()
            .position(|r| r.first().is_some_and(|c| text(c) == "Date"))
        else {
            continue;
        };
        let (name, mut balance) = summary
            .iter()
            .find(|(rib, ..)| rib.contains(&number_))
            // With no summary line the sheet's title is the name: it holds the full account number, which never leaves
            // this crate whole (only its last four digits).
            .map_or_else(|| (mask_account(t), None), |(_, n, b)| (n.clone(), *b));
        let mut balance_on = None;
        let mut moves = vec![];
        for r in &rows[head + 1..] {
            let line = r
                .iter()
                .map(text)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if let Some(on) = closing_date(&line) {
                balance_on = Some(on);
                if let Some(last) = r.iter().filter_map(number).next_back() {
                    balance = Some(last);
                }
                break;
            }
            let Some(date) = r.first().and_then(cell_date) else {
                continue;
            };
            let get = |i: usize| r.get(i).and_then(number).unwrap_or(0.0);
            let (debit, credit) = (get(3), get(4));
            let amount = round2(credit + if debit < 0.0 { debit } else { -debit });
            moves.push(Move {
                date,
                value_date: r.get(1).and_then(cell_date),
                label: collapse(&r.get(2).map(text).unwrap_or_default()),
                amount,
            });
        }
        out.push(Account {
            number: number_,
            name,
            balance,
            balance_on,
            moves,
        });
    }
    Ok(out)
}

// ── rules ─────────────────────────────────────────────────────────────────────

fn default_rules() -> Value {
    json!({ "caps": [], "watch": [], "fees": DEFAULT_FEES, "internal": DEFAULT_INTERNAL })
}

fn words(v: &Value, max: usize) -> Result<Vec<String>, ToolError> {
    let bad = || ToolError::Invalid("a list of up to 30 short words is expected".into());
    let list = v.as_array().ok_or_else(bad)?;
    if list.len() > 30 {
        return Err(bad());
    }
    list.iter()
        .map(|w| {
            let s = w.as_str().ok_or_else(bad)?.trim().to_uppercase();
            (!s.is_empty() && s.chars().count() <= max)
                .then_some(s)
                .ok_or_else(bad)
        })
        .collect()
}

fn checked_rules(v: &Value) -> Result<Value, ToolError> {
    let mut rules = default_rules();
    let obj = v
        .as_object()
        .ok_or_else(|| ToolError::Invalid("rules must be an object".into()))?;
    for k in obj.keys() {
        if !["caps", "watch", "fees", "internal"].contains(&k.as_str()) {
            return Err(ToolError::Invalid(format!("unknown rule: {k}")));
        }
    }
    for k in ["fees", "internal"] {
        if let Some(x) = obj.get(k) {
            rules[k] = json!(words(x, 40)?);
        }
    }
    for k in ["caps", "watch"] {
        let Some(list) = obj.get(k) else { continue };
        let list = list
            .as_array()
            .filter(|l| l.len() <= 30)
            .ok_or_else(|| ToolError::Invalid(format!("{k}: a list of up to 30 rules")))?;
        let mut out = vec![];
        for item in list {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|n| !n.is_empty() && n.chars().count() <= 40)
                .ok_or_else(|| ToolError::Invalid(format!("{k}: each rule needs a name")))?;
            let matches = words(item.get("match").unwrap_or(&Value::Null), 40)?;
            if matches.is_empty() {
                return Err(ToolError::Invalid(format!(
                    "{k}: {name} needs words to match"
                )));
            }
            let mut rule = json!({ "name": name, "match": matches });
            if k == "caps" {
                let limit = item
                    .get("limit")
                    .and_then(Value::as_f64)
                    .filter(|l| *l > 0.0)
                    .ok_or_else(|| {
                        ToolError::Invalid(format!("caps: {name} needs a positive limit"))
                    })?;
                rule["limit"] = json!(limit);
            } else if let Some(except) = item.get("except") {
                rule["except"] = json!(words(except, 40)?);
            }
            out.push(rule);
        }
        rules[k] = json!(out);
    }
    Ok(rules)
}

fn contains_any(label_upper: &str, list: &[String]) -> bool {
    list.iter().any(|w| label_upper.contains(w.as_str()))
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

// ── days ──────────────────────────────────────────────────────────────────────

fn parse_day(s: &str) -> Option<(i64, u32, u32)> {
    let mut it = s.get(..10)?.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok()?;
    let d: u32 = it.next()?.parse().ok()?;
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y, m, d))
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn day_number(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn day_text(days: i64) -> String {
    let (y, m, d, ..) = civil(days * 86_400);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The month before the one of `today` (`yyyy-mm`).
fn previous_month(today: &str) -> String {
    let (y, m, _) = parse_day(today).unwrap_or((1970, 1, 1));
    if m == 1 {
        format!("{:04}-12", y - 1)
    } else {
        format!("{y:04}-{:02}", m - 1)
    }
}

fn valid_month(s: &str) -> bool {
    s.len() == 7 && parse_day(&format!("{s}-01")).is_some() && s.as_bytes()[4] == b'-'
}

fn mask(number: &str) -> String {
    let d = digits(number);
    format!("…{}", &d[d.len().saturating_sub(4)..])
}

// ── the ledger ────────────────────────────────────────────────────────────────

impl Ledger<'_> {
    fn conn(&self) -> Result<Connection, ToolError> {
        let c = self.brain.open_space(&self.space)?;
        c.execute_batch(SCHEMA).map_err(db)?;
        Ok(c)
    }

    fn rules(&self, c: &Connection) -> Result<Value, ToolError> {
        let stored: Option<String> = c
            .query_row("SELECT json FROM fin_rules WHERE id = 1", [], |r| r.get(0))
            .ok();
        Ok(stored
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(default_rules))
    }

    fn account_label(c: &Connection, number: &str) -> String {
        let name: String = c
            .query_row(
                "SELECT name FROM fin_accounts WHERE number = ?1",
                [number],
                |r| r.get(0),
            )
            .unwrap_or_default();
        format!(
            "{} {}",
            if name.is_empty() { "account" } else { &name },
            mask(number)
        )
    }

    /// Reads a stored `.xlsx` export and adds its new movements.
    pub fn import(&self, file_id: &str, now: i64) -> Result<Value, ToolError> {
        let (info, bytes) = files::get(self.brain, &self.space, file_id)?
            .ok_or_else(|| ToolError::Invalid("no such file in this space".into()))?;
        if !info.name.to_lowercase().ends_with(".xlsx") {
            return Err(ToolError::Invalid(
                "an .xlsx export of the accounts is expected".into(),
            ));
        }
        let accounts = read_workbook(&bytes)?;
        if accounts.is_empty() {
            return Err(ToolError::Invalid(
                "no account sheet found (\"Cpt <number>\" or \"CB <number>\")".into(),
            ));
        }
        let at = util::iso(now);
        let mut c = self.conn()?;
        let tx = c.transaction().map_err(db)?;
        let (mut report, mut total) = (vec![], 0usize);
        for a in &accounts {
            let mut seen: HashMap<(&str, &str, i64), i64> = HashMap::new();
            let mut new = 0usize;
            for m in &a.moves {
                let key = (
                    m.date.as_str(),
                    m.label.as_str(),
                    (m.amount * 100.0).round() as i64,
                );
                let rank = seen.entry(key).and_modify(|n| *n += 1).or_insert(1);
                new += tx
                    .execute(
                        "INSERT OR IGNORE INTO fin_moves VALUES (?1,?2,?3,?4,?5,?6,?7)",
                        params![a.number, m.date, m.value_date, m.label, m.amount, *rank, at],
                    )
                    .map_err(db)?;
            }
            tx.execute(
                "INSERT INTO fin_accounts VALUES (?1,?2,?3,?4,'EUR') ON CONFLICT(number) DO UPDATE SET \
                 name = excluded.name, balance = COALESCE(excluded.balance, fin_accounts.balance), \
                 balance_on = COALESCE(excluded.balance_on, fin_accounts.balance_on)",
                params![a.number, a.name, a.balance, a.balance_on],
            )
            .map_err(db)?;
            total += new;
            report.push(json!({
                "account": format!("{} {}", a.name, mask(&a.number)),
                "balance": a.balance, "as_of": a.balance_on, "new_movements": new,
            }));
        }
        tx.execute(
            "INSERT INTO fin_imports VALUES (?1,?2,?3)",
            params![info.name, at, total as i64],
        )
        .map_err(db)?;
        tx.commit().map_err(db)?;
        Ok(json!({ "file": info.name, "new_movements": total, "accounts": report }))
    }

    pub fn balances(&self) -> Result<Value, ToolError> {
        let c = self.conn()?;
        let mut st = c
            .prepare("SELECT number, name, balance, balance_on FROM fin_accounts ORDER BY name")
            .map_err(db)?;
        let accounts: Vec<Value> = st
            .query_map([], |r| {
                let number: String = r.get(0)?;
                let name: String = r.get(1)?;
                Ok(json!({
                    "account": format!("{name} {}", mask(&number)),
                    "balance": r.get::<_, Option<f64>>(2)?, "as_of": r.get::<_, Option<String>>(3)?,
                }))
            })
            .map_err(db)?
            .filter_map(Result::ok)
            .collect();
        let last: Option<(String, String)> = c
            .query_row(
                "SELECT file, at FROM fin_imports ORDER BY at DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        Ok(json!({
            "accounts": accounts,
            "last_import": last.map(|(f, a)| json!({ "file": f, "at": a })),
        }))
    }

    fn matching_accounts(c: &Connection, query: &str) -> Result<Vec<String>, ToolError> {
        let mut st = c
            .prepare("SELECT number, name FROM fin_accounts")
            .map_err(db)?;
        let all: Vec<(String, String)> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(db)?
            .filter_map(Result::ok)
            .collect();
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Ok(all.into_iter().map(|(n, _)| n).collect());
        }
        let q_digits = digits(&q);
        Ok(all
            .into_iter()
            .filter(|(n, name)| {
                name.to_lowercase().contains(&q) || (!q_digits.is_empty() && n.ends_with(&q_digits))
            })
            .map(|(n, _)| n)
            .collect())
    }

    pub fn movements(
        &self,
        account: &str,
        from: &str,
        to: &str,
        filter: &str,
        limit: i64,
    ) -> Result<Value, ToolError> {
        let c = self.conn()?;
        let numbers = Self::matching_accounts(&c, account)?;
        if numbers.is_empty() {
            return Ok(json!({ "movements": [], "note": "no account matches" }));
        }
        let mut out = vec![];
        let mut st = c
            .prepare(
                "SELECT date, amount, label FROM fin_moves WHERE number = ?1 AND date >= ?2 AND date <= ?3 \
                 AND label LIKE ?4 ORDER BY date DESC",
            )
            .map_err(db)?;
        let pattern = format!("%{}%", filter.replace(['%', '_'], ""));
        let (lo, hi) = (
            if from.is_empty() { "0000-00-00" } else { from },
            if to.is_empty() { "9999-99-99" } else { to },
        );
        for n in &numbers {
            let label = Self::account_label(&c, n);
            let rows = st
                .query_map(params![n, lo, hi, pattern], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, f64>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(db)?;
            for (date, amount, text) in rows.filter_map(Result::ok) {
                out.push(
                    json!({ "date": date, "amount": amount, "label": text, "account": label }),
                );
            }
        }
        out.sort_by(|a, b| b["date"].as_str().cmp(&a["date"].as_str()));
        out.truncate(usize::try_from(limit.clamp(1, 500)).unwrap_or(100));
        Ok(json!({ "movements": out }))
    }

    /// The money register for the space view contract (`memcp.view/1`): movements with a stable id,
    /// a cursor and their provenance. It is the projection the core cannot compute, since the bank
    /// statements live in this crate. An id comes from the whole table, so a movement keeps it
    /// whatever window, account or page is asked for.
    pub fn movements_page(
        &self,
        account: &str,
        from: &str,
        to: &str,
        filter: &str,
        limit: i64,
        cursor: &str,
    ) -> Result<Value, ToolError> {
        let c = self.conn()?;
        let numbers = Self::matching_accounts(&c, account)?;
        let limit = limit.clamp(1, 200) as usize;
        if numbers.is_empty() {
            return Ok(page(
                self,
                (from, to, account, filter),
                limit,
                vec![],
                false,
            ));
        }
        let wanted: std::collections::HashSet<&String> = numbers.iter().collect();
        let text = filter.to_lowercase();
        let (lo, hi) = (
            if from.is_empty() { "0000-00-00" } else { from },
            if to.is_empty() { "9999-99-99" } else { to },
        );
        let mut all: Vec<(String, String, f64, String, i64)> = Vec::new();
        let mut st = c
            .prepare(
                "SELECT number, date, amount, label, rowid FROM fin_moves \
                 ORDER BY number, date, label, amount, rowid",
            )
            .map_err(db)?;
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, f64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })
            .map_err(db)?;
        all.extend(rows.filter_map(Result::ok));
        let mut seen: std::collections::HashMap<String, u32> = Default::default();
        let mut ids: std::collections::HashMap<i64, String> = Default::default();
        for (number, date, amount, label, rowid) in &all {
            let key = format!("{number}|{date}|{amount}|{label}");
            let n = *seen.entry(key.clone()).or_insert(0);
            seen.insert(key.clone(), n + 1);
            ids.insert(
                *rowid,
                format!(
                    "mv_{:016x}",
                    fnv1a(format!("{}|{}|{n}", self.space.as_str(), key).as_bytes())
                ),
            );
        }
        let mut items = Vec::new();
        for (number, date, amount, label, rowid) in &all {
            if !wanted.contains(number) || date.as_str() < lo || date.as_str() > hi {
                continue;
            }
            if !text.is_empty() && !label.to_lowercase().contains(&text) {
                continue;
            }
            let Some(id) = ids.get(rowid) else {
                continue;
            };
            items.push(json!({
                "id": id,
                "type": "movement",
                "space": self.space.as_str(),
                "at": date,
                "date": date,
                "text": label,
                "title": label,
                "amount": amount,
                "currency": Value::Null,
                "account": Self::account_label(&c, number),
                "tags": [],
                "fields": {},
                "provenance": {
                    "kind": "statement",
                    "source": "bank export",
                    "account": Self::account_label(&c, number),
                    "author": Value::Null,
                },
            }));
        }
        items.sort_by(|a, b| {
            b["date"]
                .as_str()
                .cmp(&a["date"].as_str())
                .then(b["id"].as_str().cmp(&a["id"].as_str()))
        });
        if !cursor.is_empty() {
            match items.iter().position(|i| i["id"].as_str() == Some(cursor)) {
                Some(at) => {
                    items.drain(..=at);
                }
                None => {
                    return Err(ToolError::Invalid(
                        "cursor: take `page.next_cursor` from the answer".into(),
                    ));
                }
            }
        }
        let more = items.len() > limit;
        items.truncate(limit);
        Ok(page(self, (from, to, account, filter), limit, items, more))
    }

    /// Income, outflow and net of the last `count` months (the current one first), all accounts
    /// together: the bank movements the `stats` view draws beside the balances.
    pub fn monthly_totals(&self, now: i64, count: usize) -> Result<Value, ToolError> {
        let (y, m, ..) = civil(now);
        let mut month = format!("{y:04}-{m:02}");
        let mut out = vec![];
        for i in 0..count.clamp(1, 24) {
            if i > 0 {
                month = previous_month(&format!("{month}-01"));
            }
            let statement = self.monthly(&month, now)?;
            let (mut income, mut outflow) = (0.0, 0.0);
            for a in statement["accounts"].as_array().into_iter().flatten() {
                income += a["income"].as_f64().unwrap_or(0.0);
                outflow += a["outflow"].as_f64().unwrap_or(0.0);
            }
            out.push(json!({
                "month": month,
                "income": round2(income), "outflow": round2(outflow), "net": round2(income + outflow),
            }));
        }
        Ok(json!(out))
    }

    /// When each overdrawn account comes back above zero, as an estimate: the balance of the last export divided by the
    /// average monthly net of the last `months` complete months (income and outflow of that account, transfers between
    /// the person's own accounts included, as the statement counts them). An account whose average net is not positive has
    /// no exit at the current rate; an account above zero is not listed as overdrawn.
    pub fn overdraft_exit(&self, now: i64, months: usize) -> Result<Value, ToolError> {
        let (y, m, ..) = civil(now);
        let mut month = format!("{y:04}-{m:02}");
        let mut used: Vec<String> = vec![];
        let mut per_account: std::collections::BTreeMap<String, Vec<f64>> =
            std::collections::BTreeMap::new();
        for _ in 0..months.clamp(1, 12) {
            month = previous_month(&format!("{month}-01"));
            let st = self.monthly(&month, now)?;
            let accounts = st["accounts"].as_array().cloned().unwrap_or_default();
            if accounts.is_empty() {
                continue;
            }
            used.push(month.clone());
            for a in accounts {
                if let (Some(name), Some(net)) = (a["account"].as_str(), a["net"].as_f64()) {
                    per_account.entry(name.to_string()).or_default().push(net);
                }
            }
        }
        let balances = self.balances()?;
        let mut out = vec![];
        for acc in balances["accounts"].as_array().into_iter().flatten() {
            let (Some(name), Some(balance)) = (acc["account"].as_str(), acc["balance"].as_f64())
            else {
                continue;
            };
            if balance >= 0.0 {
                continue;
            }
            let nets = per_account.get(name).cloned().unwrap_or_default();
            let avg = if nets.is_empty() {
                None
            } else {
                Some(round2(nets.iter().sum::<f64>() / nets.len() as f64))
            };
            let (exit, note) = match avg {
                None => (None, "no movement in the months considered".to_string()),
                Some(a) if a <= 0.0 => (
                    None,
                    "the average month does not bring the balance up: no exit at the current rate"
                        .to_string(),
                ),
                Some(a) => {
                    let needed = (-balance / a).ceil().max(1.0);
                    let (ey, em, ..) = civil(now);
                    let total = ey * 12 + i64::from(em) - 1 + needed as i64;
                    (
                        Some(format!(
                            "{:04}-{:02}",
                            total.div_euclid(12),
                            total.rem_euclid(12) + 1
                        )),
                        format!("about {needed:.0} month(s) at the average net"),
                    )
                }
            };
            out.push(json!({
                "account": name, "balance": balance, "balance_on": acc["balance_on"],
                "average_monthly_net": avg, "months_considered": nets.len(),
                "exit_estimate": exit, "note": note,
            }));
        }
        Ok(json!({
            "estimate": true,
            "based_on": used,
            "accounts": out,
            "notice": "An estimate from past months, not a promise: one big expense moves it. Statement figures, as read in the last bank export.",
        }))
    }

    /// The statement of a month: income, outflow, the person's rules, overdrawn days, fees.
    pub fn monthly(&self, month: &str, now: i64) -> Result<Value, ToolError> {
        let (y, m, d, ..) = civil(now);
        let month = if month.is_empty() {
            previous_month(&format!("{y:04}-{m:02}-{d:02}"))
        } else if valid_month(month) {
            month.to_string()
        } else {
            return Err(ToolError::Invalid("month: yyyy-mm".into()));
        };
        let c = self.conn()?;
        let rules = self.rules(&c)?;
        let fees = strings(&rules["fees"]);
        let internal = strings(&rules["internal"]);
        let mut accounts = vec![];
        let mut st = c
            .prepare("SELECT number, balance, balance_on FROM fin_accounts ORDER BY name")
            .map_err(db)?;
        let list: Vec<(String, Option<f64>, Option<String>)> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(db)?
            .filter_map(Result::ok)
            .collect();
        for (number_, balance, balance_on) in list {
            let rows: Vec<(String, String, f64)> = c
                .prepare(
                    "SELECT date, label, amount FROM fin_moves WHERE number = ?1 AND substr(date,1,7) = ?2 \
                     ORDER BY date",
                )
                .map_err(db)?
                .query_map(params![number_, month], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(db)?
                .filter_map(Result::ok)
                .collect();
            if rows.is_empty() {
                continue;
            }
            let income: f64 = rows.iter().filter(|r| r.2 > 0.0).map(|r| r.2).sum();
            let outflow: f64 = rows.iter().filter(|r| r.2 < 0.0).map(|r| r.2).sum();
            let upper = |l: &str| l.to_uppercase();
            let caps: Vec<Value> = rules["caps"]
                .as_array()
                .map(|a| a.iter().collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
                .map(|rule| {
                    let words = strings(&rule["match"]);
                    let spent: f64 = -rows
                        .iter()
                        .filter(|r| r.2 < 0.0 && contains_any(&upper(&r.1), &words))
                        .map(|r| r.2)
                        .sum::<f64>();
                    let limit = rule["limit"].as_f64().unwrap_or(0.0);
                    json!({ "name": rule["name"], "spent": round2(spent), "limit": limit, "exceeded": spent > limit })
                })
                .collect();
            let watch: Vec<Value> = rules["watch"]
                .as_array()
                .map(|a| a.iter().collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
                .map(|rule| {
                    let (words, except) = (strings(&rule["match"]), strings(&rule["except"]));
                    let found: Vec<Value> = rows
                        .iter()
                        .filter(|r| {
                            let l = upper(&r.1);
                            contains_any(&l, &words) && !contains_any(&l, &except)
                        })
                        .map(|r| json!({ "date": r.0, "amount": r.2 }))
                        .collect();
                    json!({ "name": rule["name"], "movements": found })
                })
                .collect();
            let fee_rows: Vec<&(String, String, f64)> = rows
                .iter()
                .filter(|r| r.2 < 0.0 && contains_any(&upper(&r.1), &fees))
                .collect();
            let internal_rows: Vec<&(String, String, f64)> = rows
                .iter()
                .filter(|r| {
                    let l = upper(&r.1);
                    (l.starts_with("VIR ") || l.starts_with("VIREMENT "))
                        && contains_any(&l, &internal)
                })
                .collect();
            let mut biggest: Vec<&(String, String, f64)> =
                rows.iter().filter(|r| r.2 < 0.0).collect();
            biggest.sort_by(|a, b| a.2.total_cmp(&b.2));
            let overdrawn = match (balance, &balance_on) {
                (Some(b), Some(on)) => self.overdrawn_days(&c, &number_, &month, b, on),
                _ => None,
            };
            accounts.push(json!({
                "account": Self::account_label(&c, &number_),
                "income": round2(income), "outflow": round2(outflow), "net": round2(income + outflow),
                "caps": caps, "watch": watch,
                "overdrawn_days": overdrawn,
                "fees": { "total": round2(-fee_rows.iter().map(|r| r.2).sum::<f64>()), "count": fee_rows.len() },
                "internal_transfers": { "count": internal_rows.len(), "net": round2(internal_rows.iter().map(|r| r.2).sum()) },
                "largest_outflows": biggest.iter().take(5)
                    .map(|r| json!({ "date": r.0, "amount": r.2, "label": r.1.chars().take(40).collect::<String>() }))
                    .collect::<Vec<_>>(),
            }));
        }
        if accounts.is_empty() {
            return Ok(json!({ "month": month, "accounts": [], "note": "no movement this month" }));
        }
        Ok(json!({ "month": month, "accounts": accounts }))
    }

    /// Days of the month with a negative balance, the daily balances being rebuilt backwards
    /// from the balance of the last export.
    fn overdrawn_days(
        &self,
        c: &Connection,
        number_: &str,
        month: &str,
        balance: f64,
        on: &str,
    ) -> Option<usize> {
        let per_day = |sql: &str, arg: &str| -> BTreeMap<String, f64> {
            c.prepare(sql)
                .ok()
                .and_then(|mut st| {
                    st.query_map(params![number_, arg], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
                    })
                    .ok()
                    .map(|rows| rows.filter_map(Result::ok).collect())
                })
                .unwrap_or_default()
        };
        let after = per_day(
            "SELECT date, SUM(amount) FROM fin_moves WHERE number = ?1 AND date > ?2 GROUP BY date",
            &format!("{month}-31"),
        );
        let in_month = per_day(
            "SELECT date, SUM(amount) FROM fin_moves WHERE number = ?1 AND substr(date,1,7) = ?2 GROUP BY date",
            month,
        );
        let mut balance = balance
            - after
                .iter()
                .filter(|(d, _)| d.as_str() <= on)
                .map(|(_, v)| v)
                .sum::<f64>();
        let (y, m, _) = parse_day(&format!("{month}-01"))?;
        let first = day_number(y, m, 1);
        let next_first = if m == 12 {
            day_number(y + 1, 1, 1)
        } else {
            day_number(y, m + 1, 1)
        };
        let mut negative = 0;
        for day in (first..next_first).rev() {
            if balance < 0.0 {
                negative += 1;
            }
            balance -= in_month.get(&day_text(day)).copied().unwrap_or(0.0);
        }
        Some(negative)
    }

    pub fn definitions(&self, caller: &Caller) -> Vec<ToolDef> {
        let mut out = vec![];
        if !caller.can_read(&self.space) {
            return out;
        }
        out.push(ToolDef {
            name: "finances_balances",
            description: "Balance of each account as read in the last bank export (a statement figure, with its date), and the last import.".into(),
            schema: json!({ "type": "object", "properties": {} }),
        });
        out.push(ToolDef {
            name: "finances_movements",
            description: "Account movements from the bank exports, newest first. `account`: a part of its name or its last four digits (empty: all).".into(),
            schema: json!({ "type": "object", "properties": {
                "account": { "type": "string" },
                "from": { "type": "string", "description": "yyyy-mm-dd" },
                "to": { "type": "string", "description": "yyyy-mm-dd" },
                "filter": { "type": "string", "description": "A word of the label." },
                "limit": { "type": "integer", "minimum": 1, "maximum": 500 } } }),
        });
        out.push(ToolDef {
            name: "finances_monthly",
            description: "Statement of a month, per account: income, outflow, the person's caps and merchants to watch, days overdrawn, fees, internal transfers (to leave out of real income and spending), largest outflows. Statement figures. Default: the last complete month.".into(),
            schema: json!({ "type": "object", "properties": { "month": { "type": "string", "description": "yyyy-mm" } } }),
        });
        out.push(ToolDef {
            name: "finances_overdraft_exit",
            description: "When each overdrawn account comes back above zero, as an estimate: the balance of the last export divided by the average monthly net (income minus outflow) of the last `months` complete months (default 3). Says so when the average month does not bring the balance up. An estimate from statement figures, never a promise.".into(),
            schema: json!({ "type": "object", "properties": { "months": { "type": "integer", "minimum": 1, "maximum": 12, "description": "Complete months the average is made over; default 3." } } }),
        });
        out.push(ToolDef {
            name: "finances_rules",
            description: "The person's own rules for the monthly statement: caps (a merchant and a monthly limit), merchants to watch (with exceptions), words that mark bank fees and internal transfers. Pass `rules` to replace them (needs write access).".into(),
            schema: json!({ "type": "object", "properties": { "rules": { "type": "object", "properties": {
                "caps": { "type": "array", "items": { "type": "object", "properties": { "name": { "type": "string" }, "match": { "type": "array", "items": { "type": "string" } }, "limit": { "type": "number" } }, "required": ["name", "match", "limit"] } },
                "watch": { "type": "array", "items": { "type": "object", "properties": { "name": { "type": "string" }, "match": { "type": "array", "items": { "type": "string" } }, "except": { "type": "array", "items": { "type": "string" } } }, "required": ["name", "match"] } },
                "fees": { "type": "array", "items": { "type": "string" } },
                "internal": { "type": "array", "items": { "type": "string" } } } } } }),
        });
        if !caller.can_write(&self.space) {
            return out;
        }
        out.push(ToolDef {
            name: "finances_import",
            description: "Imports a bank export (.xlsx workbook: a summary sheet and one sheet per account) already stored in this space (see file_list). Idempotent: only new movements are added.".into(),
            schema: json!({ "type": "object", "properties": { "file_id": { "type": "string" } }, "required": ["file_id"] }),
        });
        out
    }

    pub fn call(
        &self,
        caller: &Caller,
        name: &str,
        args: &Value,
        now: i64,
    ) -> Result<Value, ToolError> {
        if !self.definitions(caller).iter().any(|d| d.name == name) {
            return Err(ToolError::Unknown);
        }
        let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("");
        match name {
            "finances_balances" => self.balances(),
            "finances_movements" => {
                let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(100);
                self.movements(s("account"), s("from"), s("to"), s("filter"), limit)
            }
            "finances_monthly" => self.monthly(s("month"), now),
            "finances_overdraft_exit" => {
                let months = args.get("months").and_then(Value::as_u64).unwrap_or(3) as usize;
                self.overdraft_exit(now, months)
            }
            "finances_rules" => {
                let c = self.conn()?;
                if let Some(new) = args.get("rules") {
                    if !caller.can_write(&self.space) {
                        return Err(ToolError::Denied(
                            "write access is needed to change the rules".into(),
                        ));
                    }
                    let rules = checked_rules(new)?;
                    c.execute(
                        "INSERT INTO fin_rules VALUES (1, ?1) ON CONFLICT(id) DO UPDATE SET json = excluded.json",
                        [rules.to_string()],
                    )
                    .map_err(db)?;
                }
                Ok(json!({ "rules": self.rules(&c)? }))
            }
            "finances_import" => self.import(s("file_id"), now),
            _ => Err(ToolError::Unknown),
        }
    }
}

fn db(e: rusqlite::Error) -> ToolError {
    ToolError::Internal(e.to_string())
}

/// A 64-bit FNV-1a hash, for a movement's stable id. No model, no dependency, the same on every node.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The contract envelope of a money register page.
fn page(
    ledger: &Ledger<'_>,
    (from, to, account, filter): (&str, &str, &str, &str),
    limit: usize,
    items: Vec<Value>,
    more: bool,
) -> Value {
    let next = more
        .then(|| {
            items
                .last()
                .and_then(|i| i["id"].as_str())
                .map(String::from)
        })
        .flatten();
    json!({
        "contract": pensine_core::view::CONTRACT,
        "contract_version": pensine_core::view::CONTRACT_VERSION,
        "space": ledger.space.as_str(),
        "kind": "money",
        "view": "register",
        "filters": { "from": from, "to": to, "type": "movement", "account": account, "filter": filter },
        "items": items,
        "page": { "limit": limit, "has_more": more, "next_cursor": next },
        "provenance": { "kind": "statement", "source": "bank export" },
        "notice": pensine_core::tools::NOTICE,
    })
}

#[cfg(test)]
mod mask_tests {
    #[test]
    fn an_account_number_keeps_only_its_last_four_digits() {
        assert_eq!(
            super::mask_account("Compte 12345678901 EUR"),
            "Compte …8901 EUR"
        );
        assert_eq!(super::mask_account("Livret A 2026"), "Livret A 2026");
        assert_eq!(
            super::mask_account("FR76 3000 4000 0312345678900"),
            "FR76 3000 4000 …8900"
        );
    }
}
